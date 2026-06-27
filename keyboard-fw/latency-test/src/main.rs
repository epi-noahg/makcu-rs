//! kbd-latency — end-to-end latency & jitter measurement for the MAKCU keyboard
//! firmware.
//!
//! Topology (single measurement computer = this Mac):
//!
//!   real keyboard ─USB─▶ RIGHT ─IPC─▶ LEFT ─┬─USB──▶ this Mac (HID keyboard)
//!                                            └─UART0─▶ CH343 ─USB─▶ this Mac (serial)
//!
//! LEFT (built `-DSIGTAP=1`) timestamps each keyboard report at the lowest level
//! it can see it (the instant the raw IPC frame is deframed) and again at USB
//! submit, then streams those stamps + the raw report bytes as framed binary
//! records on the CH343 serial. This tool reads that serial AND the emulated
//! keyboard's HID reports (kernel mach-time stamps), aligns the two clocks via
//! the SYNC beacons, correlates report-for-report, and reports the firmware's
//! internal latency (exact, single-clock) and the full end-to-end latency +
//! jitter.

mod clock;
mod hid;
mod macos_time;
mod proto;
mod serial;
mod stats;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use clock::ClockModel;
use hid::HidEvent;
use proto::{Record, Tap};
use serial::SerialEvent;
use stats::Samples;

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

const QUEUE_CAP: usize = 16384;
const LOOKAHEAD: usize = 32;

struct Args {
    serial: Option<String>,
    vid: Option<u16>,
    pid: Option<u16>,
    seconds: u64,
    seize: bool,
    csv: Option<String>,
    baud: u32,
    list: bool,
    list_serial: bool,
    debug: bool,
}

fn parse_u16(s: &str) -> Option<u16> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u16>().ok()
    }
}

fn print_help() {
    println!(
        "kbd-latency — MAKCU keyboard firmware latency & jitter meter (macOS)\n\
\n\
USAGE:\n\
    kbd-latency --vid <id> --pid <id> [options]\n\
    kbd-latency --list            # list connected HID keyboards (find VID/PID)\n\
    kbd-latency --list-serial     # list serial ports\n\
\n\
OPTIONS:\n\
    --vid <id>      Emulated keyboard USB vendor id  (hex 0x.. or decimal)\n\
    --pid <id>      Emulated keyboard USB product id (hex 0x.. or decimal)\n\
    --serial <dev>  CH343 serial device (default: autodetect QinHeng 0x1A86)\n\
    --baud <n>      Serial baud (default 4000000, must match firmware)\n\
    --seconds <n>   Measurement duration; 0 = until Ctrl-C (default 30)\n\
    --seize         Exclusively grab the keyboard so keystrokes do NOT reach\n\
                    macOS (needs sudo). Otherwise they also type into the\n\
                    focused app.\n\
    --csv <path>    Write a per-report CSV (seq,t_cap_us,t_sub_us,fw_internal_us,\n\
                    hid_host_ns,host_cap_ns,e2e_us,report_hex)\n\
    --debug         Print every tap/HID report (hex) live; on 0 matches also\n\
                    dumps sample reports so you can compare the bytes\n\
    -h, --help      This help\n\
\n\
Grant the terminal 'Input Monitoring' (System Settings → Privacy & Security).\n"
    );
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        serial: None,
        vid: None,
        pid: None,
        seconds: 30,
        seize: false,
        csv: None,
        baud: 4_000_000,
        list: false,
        list_serial: false,
        debug: false,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let mut next = || {
            i += 1;
            argv.get(i).cloned()
        };
        match arg {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--list" => a.list = true,
            "--list-serial" => a.list_serial = true,
            "--seize" => a.seize = true,
            "--debug" => a.debug = true,
            "--serial" => a.serial = Some(next().ok_or("--serial needs a value")?),
            "--csv" => a.csv = Some(next().ok_or("--csv needs a value")?),
            "--vid" => {
                let v = next().ok_or("--vid needs a value")?;
                a.vid = Some(parse_u16(&v).ok_or(format!("bad --vid {v}"))?);
            }
            "--pid" => {
                let v = next().ok_or("--pid needs a value")?;
                a.pid = Some(parse_u16(&v).ok_or(format!("bad --pid {v}"))?);
            }
            "--seconds" => {
                let v = next().ok_or("--seconds needs a value")?;
                a.seconds = v.parse().map_err(|_| format!("bad --seconds {v}"))?;
            }
            "--baud" => {
                let v = next().ok_or("--baud needs a value")?;
                a.baud = v.parse().map_err(|_| format!("bad --baud {v}"))?;
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }
    Ok(a)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Two reports pair up if equal, or equal ignoring a single report-ID byte
/// present on only one side (some HID stacks prepend it, others strip it).
fn reports_match(a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    if a.len() == b.len() + 1 && &a[1..] == b {
        return true;
    }
    if b.len() == a.len() + 1 && &b[1..] == a {
        return true;
    }
    false
}

struct Session {
    clock: ClockModel,
    taps: VecDeque<Tap>,
    hids: VecDeque<HidEvent>,
    fw_internal: Samples, // t_sub - t_cap (firmware clock, exact)
    e2e: Samples,         // hid_arrival - mapped(t_cap)
    transport: Samples,   // e2e - fw_internal (submit→host, incl. const serial bias)
    matched: u64,
    tap_unmatched: u64,
    hid_unmatched: u64,
    unmapped: u64,
    first_seq: Option<u32>,
    last_seq: u32,
    recs: u64,
    csv: Option<BufWriter<File>>,
    sample_taps: Vec<Vec<u8>>,
    sample_hids: Vec<Vec<u8>>,
    debug: bool,
}

impl Session {
    fn new(csv: Option<BufWriter<File>>, debug: bool) -> Self {
        Session {
            clock: ClockModel::new(),
            taps: VecDeque::new(),
            hids: VecDeque::new(),
            fw_internal: Samples::new(),
            e2e: Samples::new(),
            transport: Samples::new(),
            matched: 0,
            tap_unmatched: 0,
            hid_unmatched: 0,
            unmapped: 0,
            first_seq: None,
            last_seq: 0,
            recs: 0,
            csv,
            sample_taps: Vec::new(),
            sample_hids: Vec::new(),
            debug,
        }
    }

    fn track_seq(&mut self, seq: u32) {
        if self.first_seq.is_none() {
            self.first_seq = Some(seq);
        }
        self.last_seq = seq;
        self.recs += 1;
    }

    fn on_serial(&mut self, ev: SerialEvent) {
        match ev.rec {
            Record::Sync(s) => {
                self.track_seq(s.seq);
                self.clock.add(s.t_emit_us, ev.host_recv_ns);
            }
            Record::Tap(t) => {
                self.track_seq(t.seq);
                if self.sample_taps.len() < 12 {
                    self.sample_taps.push(t.report.clone());
                }
                if self.debug {
                    eprintln!("TAP  len={:<2} {}", t.report.len(), hex(&t.report));
                }
                if self.taps.len() >= QUEUE_CAP {
                    self.taps.pop_front();
                    self.tap_unmatched += 1;
                }
                self.taps.push_back(t);
            }
        }
    }

    fn on_hid(&mut self, ev: HidEvent) {
        if self.sample_hids.len() < 12 {
            self.sample_hids.push(ev.report.clone());
        }
        if self.debug {
            eprintln!("HID  len={:<2} {}", ev.report.len(), hex(&ev.report));
        }
        if self.hids.len() >= QUEUE_CAP {
            self.hids.pop_front();
            self.hid_unmatched += 1;
        }
        self.hids.push_back(ev);
    }

    fn on_match(&mut self, t: &Tap, h: &HidEvent) {
        self.matched += 1;
        let fw_internal_us = (t.t_sub_us - t.t_cap_us) as f64;
        self.fw_internal.push(fw_internal_us);

        let (host_cap_ns, e2e_us) = match self.clock.map_fw_us_to_host_ns(t.t_cap_us) {
            Some(host_cap) => {
                let e2e = (h.host_ts_ns as f64 - host_cap) / 1000.0;
                self.e2e.push(e2e);
                self.transport.push(e2e - fw_internal_us);
                (host_cap, e2e)
            }
            None => {
                self.unmapped += 1;
                (f64::NAN, f64::NAN)
            }
        };

        if let Some(w) = &mut self.csv {
            let _ = writeln!(
                w,
                "{},{},{},{:.0},{},{:.0},{:.3},{}",
                t.seq,
                t.t_cap_us,
                t.t_sub_us,
                fw_internal_us,
                h.host_ts_ns,
                host_cap_ns,
                e2e_us,
                hex(&t.report)
            );
        }
    }

    fn correlate(&mut self) {
        loop {
            if self.taps.is_empty() || self.hids.is_empty() {
                break;
            }
            if reports_match(&self.taps[0].report, &self.hids[0].report) {
                let t = self.taps.pop_front().unwrap();
                let h = self.hids.pop_front().unwrap();
                self.on_match(&t, &h);
                continue;
            }
            // Fronts disagree → a report was lost on one side. Resync by the
            // nearest forward match, dropping the skipped entries.
            let hk = self.hids.len().min(LOOKAHEAD);
            let tk = self.taps.len().min(LOOKAHEAD);
            let hj = (1..hk).find(|&j| reports_match(&self.hids[j].report, &self.taps[0].report));
            let ti = (1..tk).find(|&i| reports_match(&self.taps[i].report, &self.hids[0].report));
            match (hj, ti) {
                (Some(j), Some(i)) if j <= i => self.drop_hids(j),
                (Some(_), Some(i)) => self.drop_taps(i),
                (Some(j), None) => self.drop_hids(j),
                (None, Some(i)) => self.drop_taps(i),
                (None, None) => {
                    if self.taps.len() >= LOOKAHEAD && self.hids.len() >= LOOKAHEAD {
                        self.drop_taps(1);
                        self.drop_hids(1);
                    } else {
                        break; // wait for more context
                    }
                }
            }
        }
    }

    fn drop_taps(&mut self, n: usize) {
        for _ in 0..n {
            if self.taps.pop_front().is_some() {
                self.tap_unmatched += 1;
            }
        }
    }
    fn drop_hids(&mut self, n: usize) {
        for _ in 0..n {
            if self.hids.pop_front().is_some() {
                self.hid_unmatched += 1;
            }
        }
    }

    fn lost_records(&self) -> u64 {
        match self.first_seq {
            Some(first) => {
                let span = self.last_seq.wrapping_sub(first) as u64 + 1;
                span.saturating_sub(self.recs)
            }
            None => 0,
        }
    }

    fn print_running(&self, elapsed: f64) {
        let fw = self.fw_internal.summary();
        eprint!(
            "\r[{:6.1}s] matched={:<6} fw_internal med={:6.1}µs jitter={:6.1}µs | e2e ",
            elapsed, self.matched, fw.median, fw.stddev
        );
        if self.e2e.len() >= 2 {
            let e = self.e2e.summary();
            eprint!("med={:7.1}µs jitter={:6.1}µs", e.median, e.stddev);
        } else {
            eprint!("(aligning clock…)");
        }
        let _ = std::io::stderr().flush();
    }

    fn print_final(&self) {
        println!("\n══════════════════════════ RESULTS ══════════════════════════");
        println!("matched reports : {}", self.matched);
        println!(
            "unmatched       : {} tap-only, {} hid-only (report drops on one path)",
            self.tap_unmatched, self.hid_unmatched
        );
        println!(
            "serial health   : {} records received, {} lost (seq gaps / CRC drops)",
            self.recs,
            self.lost_records()
        );
        if let Some(s) = self.clock.slope() {
            println!(
                "clock alignment : {} sync points, rate {:.3} ns/µs (drift {:+.1} ppm)",
                self.clock.points(),
                s,
                (s / 1000.0 - 1.0) * 1e6
            );
        } else {
            println!("clock alignment : insufficient (no end-to-end numbers)");
        }
        println!();
        println!("FIRMWARE INTERNAL  (deframe → USB submit, single LEFT clock — exact):");
        println!("  {}", self.fw_internal.summary());
        if self.e2e.len() >= 2 {
            println!();
            println!("END-TO-END  (capture → host HID delivery; absolute has a constant");
            println!("serial-path offset, but JITTER is exact):");
            println!("  {}", self.e2e.summary());
            println!();
            println!("TRANSPORT  (USB submit → host delivery = e2e − firmware internal):");
            println!("  {}", self.transport.summary());
        } else if self.matched == 0 {
            println!("\n(no end-to-end stats — 0 reports matched; see DIAGNOSTIC below)");
        } else if self.clock.slope().is_none() {
            println!("\n(no end-to-end stats — clock never aligned; check SYNC beacons)");
        } else {
            println!("\n(no end-to-end stats — too few matched reports)");
        }
        if self.matched == 0 && (!self.sample_taps.is_empty() || !self.sample_hids.is_empty()) {
            println!();
            println!("DIAGNOSTIC — 0 matched. Firmware tap bytes and macOS HID bytes for the");
            println!("same keystrokes must be identical to pair up. Samples (len: hex):");
            println!("  firmware taps:");
            for r in &self.sample_taps {
                println!("    {:>2}: {}", r.len(), hex(r));
            }
            println!("  macOS HID:");
            for r in &self.sample_hids {
                println!("    {:>2}: {}", r.len(), hex(r));
            }
            println!("  → lengths differ by 1  ⇒ report-ID offset (auto-tolerated; just rerun)");
            println!(
                "  → firmware capped at {} B but HID longer ⇒ NKRO report exceeds the tap cap:",
                proto::MAX_REPORT
            );
            println!("     raise SIGTAP_MAX_REPORT in keyboard-fw/left/src/sigtap.h and reflash");
            println!("  → unrelated bytes ⇒ --vid/--pid matched the wrong HID collection");
        }
        if self.unmapped > 0 {
            println!(
                "\nnote: {} early reports had no clock alignment yet (firmware-internal only)",
                self.unmapped
            );
        }
        println!("══════════════════════════════════════════════════════════════");
    }
}

fn run(args: Args) -> Result<(), String> {
    let vid = args.vid.ok_or("missing --vid (use --list to find it)")?;
    let pid = args.pid.ok_or("missing --pid (use --list to find it)")?;

    let port = match args.serial {
        Some(p) => p,
        None => serial::autodetect_ch34x()
            .ok_or("no CH34x serial found; pass --serial <dev> (see --list-serial)")?,
    };
    println!("serial : {port} @ {} baud", args.baud);
    println!("hid    : {vid:04x}:{pid:04x}{}", if args.seize { "  (seize)" } else { "" });

    let (serial_tx, serial_rx) = mpsc::channel::<SerialEvent>();
    let (hid_tx, hid_rx) = mpsc::channel::<HidEvent>();
    let (ready_tx, ready_rx) = mpsc::channel::<hid::Startup>();

    serial::spawn_reader(&port, args.baud, serial_tx).map_err(|e| format!("open serial: {e}"))?;
    hid::spawn_capture(vid, pid, args.seize, hid_tx, ready_tx);

    match ready_rx.recv_timeout(Duration::from_secs(3)) {
        Ok(hid::Startup::Ready(0)) => {
            return Err(format!(
                "no HID device matched {vid:04x}:{pid:04x} — is LEFT plugged in? (try --list)"
            ))
        }
        Ok(hid::Startup::Ready(n)) => println!("hid    : capturing {n} matching device(s)"),
        Ok(hid::Startup::Error(e)) => return Err(e),
        Err(_) => return Err("HID startup timed out".into()),
    }

    unsafe {
        libc::signal(libc::SIGINT, on_sigint as *const () as usize);
    }

    let csv = match &args.csv {
        Some(path) => {
            let f = File::create(path).map_err(|e| format!("create csv: {e}"))?;
            let mut w = BufWriter::new(f);
            let _ = writeln!(
                w,
                "seq,t_cap_us,t_sub_us,fw_internal_us,hid_host_ns,host_cap_ns,e2e_us,report_hex"
            );
            Some(w)
        }
        None => None,
    };

    let mut sess = Session::new(csv, args.debug);

    if args.seconds == 0 {
        println!("measuring until Ctrl-C…\n");
    } else {
        println!("measuring for {}s (Ctrl-C to stop early)…\n", args.seconds);
    }

    let start = Instant::now();
    let mut last_print = Instant::now();

    loop {
        if STOP.load(Ordering::SeqCst) {
            break;
        }
        if args.seconds != 0 && start.elapsed().as_secs() >= args.seconds {
            break;
        }
        let mut got = false;
        while let Ok(ev) = serial_rx.try_recv() {
            sess.on_serial(ev);
            got = true;
        }
        while let Ok(ev) = hid_rx.try_recv() {
            sess.on_hid(ev);
            got = true;
        }
        if got {
            sess.correlate();
        }
        if last_print.elapsed() >= Duration::from_millis(500) {
            sess.print_running(start.elapsed().as_secs_f64());
            last_print = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    if let Some(w) = &mut sess.csv {
        let _ = w.flush();
    }
    sess.print_final();
    Ok(())
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n");
            print_help();
            std::process::exit(2);
        }
    };

    if args.list_serial {
        println!("serial ports:");
        for p in serial::list_ports() {
            println!("  {p}");
        }
        return;
    }
    if args.list {
        println!("HID keyboards (VID:PID  Product):");
        for (vid, pid, name) in hid::list_keyboards() {
            println!("  {vid:04x}:{pid:04x}  {name}");
        }
        return;
    }

    if let Err(e) = run(args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::Sync as SyncRec;

    // Ground-truth host clock: host_ns = A*fw_us + B (two crystals, ~40 ppm
    // apart) plus a little serial jitter on the SYNC arrivals.
    const A: f64 = 1000.04;
    const B: f64 = 7_000_000.0;

    fn report_for(k: u32) -> Vec<u8> {
        vec![((k >> 8) & 0xff) as u8, (k & 0xff) as u8, 0x04, 0, 0, 0, 0, 0]
    }

    fn host_of(fw_us: i64) -> i64 {
        (A * fw_us as f64 + B) as i64
    }

    /// Full pipeline: SYNC alignment + report correlation recovers the injected
    /// firmware-internal and end-to-end latencies.
    #[test]
    fn end_to_end_recovers_injected_latencies() {
        let mut sess = Session::new(None, false);
        let internal_us: i64 = 250; // deframe → submit
        let e2e_true_us: i64 = 1800; // capture → host HID delivery
        let mut seq: u32 = 1;
        let mut jit: u64 = 99; // deterministic tiny sync jitter
        let mut sync_jitter = || {
            jit = jit.wrapping_mul(2862933555777941757).wrapping_add(3037000493);
            ((jit >> 40) as i64 % 4001) - 2000 // ±2000 ns
        };

        let n = 200u32;
        for k in 0..n {
            let fw_cap = 1_000_000 + (k as i64) * 8_000; // a report every 8 ms
            if k % 6 == 0 {
                // SYNC beacon at this instant.
                sess.on_serial(SerialEvent {
                    rec: Record::Sync(SyncRec { seq, t_emit_us: fw_cap }),
                    host_recv_ns: host_of(fw_cap) + sync_jitter(),
                });
                seq += 1;
            }
            // HID delivery (kernel mach-time) and the serial TAP.
            sess.on_hid(HidEvent {
                report: report_for(k),
                host_ts_ns: host_of(fw_cap) + e2e_true_us * 1000,
            });
            sess.on_serial(SerialEvent {
                rec: Record::Tap(Tap {
                    seq,
                    t_cap_us: fw_cap,
                    t_sub_us: fw_cap + internal_us,
                    t_emit_us: fw_cap + 900,
                    report: report_for(k),
                }),
                host_recv_ns: host_of(fw_cap + 900) + 30_000,
            });
            seq += 1;
            sess.correlate();
        }

        assert_eq!(sess.matched, n as u64, "every report matched");
        assert_eq!(sess.tap_unmatched, 0);
        assert_eq!(sess.hid_unmatched, 0);

        // Firmware-internal is single-clock and exact.
        let fw = sess.fw_internal.summary();
        assert!((fw.mean - 250.0).abs() < 1e-6, "fw mean {}", fw.mean);
        assert!(fw.stddev < 1e-6, "fw jitter {}", fw.stddev);

        // End-to-end recovered within a few µs (sync jitter is ±2 µs); jitter low.
        let e = sess.e2e.summary();
        assert!(e.count >= (n as usize) - 6, "mapped {} of {}", e.count, n);
        assert!((e.mean - 1800.0).abs() < 20.0, "e2e mean {}", e.mean);
        assert!(e.stddev < 20.0, "e2e jitter {}", e.stddev);

        // Transport = e2e − internal ≈ 1550 µs.
        let t = sess.transport.summary();
        assert!((t.mean - 1550.0).abs() < 20.0, "transport mean {}", t.mean);

        assert_eq!(sess.lost_records(), 0, "no seq gaps");
    }

    /// A dropped HID report (USB loss) is resynced, not fatal: the matcher skips
    /// the orphaned tap and keeps correlating.
    #[test]
    fn recovers_from_a_dropped_hid_report() {
        let mut sess = Session::new(None, false);
        let n = 30u32;
        let mut seq = 1u32;
        for k in 0..n {
            let fw_cap = 1_000_000 + (k as i64) * 8_000;
            if k % 4 == 0 {
                sess.on_serial(SerialEvent {
                    rec: Record::Sync(SyncRec { seq, t_emit_us: fw_cap }),
                    host_recv_ns: host_of(fw_cap),
                });
                seq += 1;
            }
            if k != 15 {
                // report 15's HID is "lost" on the USB path.
                sess.on_hid(HidEvent {
                    report: report_for(k),
                    host_ts_ns: host_of(fw_cap) + 1_800_000,
                });
            }
            sess.on_serial(SerialEvent {
                rec: Record::Tap(Tap {
                    seq,
                    t_cap_us: fw_cap,
                    t_sub_us: fw_cap + 250,
                    t_emit_us: fw_cap + 900,
                    report: report_for(k),
                }),
                host_recv_ns: host_of(fw_cap + 900),
            });
            seq += 1;
            sess.correlate();
        }
        assert_eq!(sess.matched, (n - 1) as u64, "all but the dropped one match");
        assert_eq!(sess.tap_unmatched, 1, "the orphaned tap is dropped");
    }
}
