//! A library for interacting with a serial-controlled device that supports mouse button simulation and movement.
use bytes::BytesMut;
use crossbeam_channel::{bounded, Sender, Receiver};
use memchr::{memchr, memchr2};
use parking_lot::{Mutex, RwLock};
use once_cell::sync::Lazy;
use rustc_hash::FxHashMap;
use serialport::SerialPort; 
use std::time::Instant;

use std::{
    io::{Write},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

const CRLF: &str = "\r\n";
static DEBUG_IO: AtomicBool = AtomicBool::new(false);
#[inline] fn dbg() -> bool { DEBUG_IO.load(Ordering::Relaxed) }
use std::collections::VecDeque;
static SENT_LOG: Lazy<Mutex<VecDeque<Vec<u8>>>> =
    Lazy::new(|| Mutex::new(VecDeque::with_capacity(32)));
const LOG_MAX: usize = 32;
// ============================= Errors ======================================

#[derive(Debug, thiserror::Error)]
/// Represents the different types of errors that may occur while using Makcu.
pub enum MakcuError {
    #[error("Connection error: {0}")]
    /// A connection error occurred.
    Connection(String),
    #[error("Command error: {0}")]
    /// A command failed or returned an error.
    Command(String),
    #[error("Timeout for command {0}")]
    /// A command timed out after a specified duration.
    Timeout(u32),
}

pub type MakcuResult<T> = Result<T, MakcuError>;

// ========================= Helper structures ===============================

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseButtonStates {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub side1: bool,
    pub side2: bool,
}
impl MouseButtonStates {
    #[inline]
    pub fn from_mask(mask: u8) -> Self {
        Self {
            left: mask & 0x01 != 0,
            right: mask & 0x02 != 0,
            middle: mask & 0x04 != 0,
            side1: mask & 0x08 != 0,
            side2: mask & 0x10 != 0,
        }
    }
    #[allow(dead_code)]
    #[inline]
    pub fn to_mask(self) -> u8 {
        (self.left as u8)
            | ((self.right as u8) << 1)
            | ((self.middle as u8) << 2)
            | ((self.side1 as u8) << 3)
            | ((self.side2 as u8) << 4)
    }
}



struct PendingCommand {
     tx: Sender<String>,
     tag: &'static str,
     started: Instant,
}

// ========================= Performance profiler (optional) =================

#[cfg(feature = "profile")]
mod profiler {
    use super::*;
    use once_cell::sync::Lazy;
    use std::time::Instant;

    static PROFILER: Lazy<Mutex<FxHashMap<&'static str, (u64, f64)>>> =
        Lazy::new(|| Mutex::new(FxHashMap::default()));

    pub struct Perf;
    impl Perf {
        pub fn measure<F: FnOnce()>(name: &'static str, f: F) {
            let t0 = Instant::now();
            f();
            let dt = t0.elapsed().as_secs_f64() * 1e6;
            let mut map = PROFILER.lock();
            let e = map.entry(name).or_insert((0, 0.0));
            e.0 += 1;
            e.1 += dt;
        }
        pub fn stats() -> FxHashMap<&'static str, FxHashMap<&'static str, f64>> {
            PROFILER
                .lock()
                .iter()
                .map(|(k, (c, t))| {
                    let mut inner = FxHashMap::default();
                    inner.insert("count", *c as f64);
                    inner.insert("total_us", *t);
                    inner.insert("avg_us", if *c == 0 { 0.0 } else { *t / *c as f64 });
                    (*k, inner)
                })
                .collect()
        }
        pub fn prof(tag: &'static str, started: Instant) {
            let dt = started.elapsed().as_secs_f64() * 1e6;
            let mut map = PROFILER.lock();
            let e = map.entry(tag).or_insert((0, 0.0));
            e.0 += 1;
            e.1 += dt;
        }
    }
}
#[cfg(feature = "profile")]
use profiler::Perf as PerformanceProfiler;
#[cfg(not(feature = "profile"))]
struct PerformanceProfiler;
#[cfg(not(feature = "profile"))]
impl PerformanceProfiler {
     #[inline(always)]
    fn measure<F: FnOnce()>(_name: &'static str, f: F) {
         f()
     }
     fn stats() -> FxHashMap<&'static str, FxHashMap<&'static str, f64>> {
         FxHashMap::default()
     }
     pub fn prof(_tag: &'static str, _started: Instant) {}
}

// ================================ SerialPort ===============================

#[derive(Debug)]
struct Packet {
    data: Vec<u8>,
    tag: &'static str,
    started: Instant,
    tracked_id: Option<u32>,
}

#[derive(Clone)]
struct TxHandle {
    queue: Sender<Packet>,
}
impl TxHandle {
    #[inline]
    fn send(&self, packet: Packet) {
        let _ = self.queue.try_send(packet);
    }
}

/// Reader/Writer wrapper
pub struct SerialPortWrap {
    port_name: String,
    baudrate: u32,
    timeout: Duration,

    ser: Option<Box<dyn SerialPort>>,
    stop: Arc<AtomicBool>,
    reader: Option<thread::JoinHandle<()>>,
    writer: Option<thread::JoinHandle<()>>,
    tx: Option<TxHandle>,

    pending: Arc<Mutex<FxHashMap<u32, PendingCommand>>>,
    button_cb: Arc<RwLock<Option<Box<dyn Fn(MouseButtonStates) + Send + Sync>>>>,
    stream_cb: Arc<RwLock<Option<Box<dyn Fn(String) + Send + Sync>>>>,
    cmd_id: AtomicU32,
}

impl SerialPortWrap {
    pub fn new(port: impl Into<String>, baud: u32, timeout: Duration) -> Self {
        Self {
            port_name: port.into(),
            baudrate: baud,
            timeout,
            ser: None,
            stop: Arc::new(AtomicBool::new(false)),
            reader: None,
            writer: None,
            tx: None,
            pending: Arc::new(Mutex::new(FxHashMap::default())),
            button_cb: Arc::new(RwLock::new(None)),
            stream_cb: Arc::new(RwLock::new(None)),
            cmd_id: AtomicU32::new(0),
        }
    }

    pub fn open(&mut self) -> MakcuResult<()> {
        if self.is_open() {
            return Ok(());
        }
        let ser = serialport::new(&self.port_name, self.baudrate)
            .timeout(self.timeout)
            .open()
            .map_err(|e| MakcuError::Connection(e.to_string()))?;
        self.ser = Some(ser.into());
        self.stop.store(false, Ordering::Relaxed);

        // ---------- writer ----------
        let (tx, rx): (Sender<Packet>, Receiver<Packet>) = bounded(1024);
        let mut wport = self.ser.as_mut().unwrap().try_clone().unwrap();
        let stop_w = self.stop.clone();
        self.writer = Some(thread::spawn(move || {
            let mut buf = Vec::<u8>::with_capacity(4096);
            // Loop until the TX channel is closed (sender dropped) AND all pending
            // packets have been written. `rx.recv()` returns Err only when the channel
            // is empty AND every sender has been dropped — that is our clean-shutdown signal.
            loop {
                match rx.recv() {
                    Ok(first) => {
                        let current_tag = first.tag;
                        let started     = first.started;
                        buf.clear();
                        buf.extend_from_slice(&first.data);
                        // Drain any additional packets that arrived in the meantime.
                        while let Ok(pk) = rx.try_recv() {
                            buf.extend_from_slice(&pk.data);
                        }
                        if buf.iter().any(|&b| b != 0) {
                            if let Err(e) = wport.write_all(&buf) {
                                eprintln!("writer error: {e}");
                                break;
                            }
                            // Flush so the OS actually pushes bytes to the serial hardware.
                            let _ = wport.flush();
                        }
                        PerformanceProfiler::prof(current_tag, started);
                    }
                    // Channel closed and empty — all data has been sent, safe to exit.
                    Err(_) => break,
                }
            }
        }));
        self.tx = Some(TxHandle { queue: tx });

        // ---------- reader ----------
        let mut rport = self.ser.as_mut().unwrap().try_clone().unwrap();
        let stop_r = self.stop.clone();
        let pending_r = self.pending.clone();
        let button_cb_r = self.button_cb.clone();
        let stream_cb_r = self.stream_cb.clone();
        self.reader = Some(thread::spawn(move || {
            reader_loop(&mut *rport, stop_r, pending_r, button_cb_r, stream_cb_r);
        }));

        Ok(())
    }

    pub fn close(&mut self) {
        // Drop the TX handle first — this closes the channel so the writer thread
        // drains all queued packets and then exits cleanly via Err(_) on recv().
        self.tx = None;

        // Shorten the reader's serial timeout so it unblocks quickly.
        self.stop.store(true, Ordering::Relaxed);
        if let Some(ser) = &mut self.ser {
            let _ = ser.set_timeout(Duration::from_millis(20));
        }

        // Wait for both threads to finish — writer won't exit until all bytes are sent.
        if let Some(h) = self.writer.take() { let _ = h.join(); }
        if let Some(h) = self.reader.take() { let _ = h.join(); }

        self.ser = None;

        
        for (_, pc) in self.pending.lock().drain() {
            let _ = pc.tx.send("Port closed".into());
        }
    }


    #[inline] pub fn is_open(&self) -> bool { self.ser.is_some() }

    #[inline] fn next_id(&self) -> u32 {
        self.cmd_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
    }

    fn enqueue_ff(&self, packet: Packet) -> MakcuResult<()> {
         self.tx
             .as_ref()
             .ok_or_else(|| MakcuError::Connection("Port not open".into()))
             .map(|h| h.send(packet))
    }

    pub fn send_ff(&self, payload: &str) -> MakcuResult<()> {
        if dbg() { println!("[TX] {payload}"); }
        let mut v = Vec::with_capacity(payload.len() + 2);
        v.extend_from_slice(payload.as_bytes());
        v.extend_from_slice(CRLF.as_bytes());
        {
            let mut log = SENT_LOG.lock();
            if log.len() == LOG_MAX { log.pop_front(); }
            log.push_back(payload.as_bytes().to_vec());
            
        }
        self.enqueue_ff(Packet{
             data: v,
             tag: "move",           
             started: Instant::now(),
             tracked_id: None,
        })
    }

    pub fn send_tracked(&self, payload: &str, timeout_s: f32) -> MakcuResult<String> {
        if dbg() { println!("[TX tracked] {payload}"); }
        let started = Instant::now();
        let tag = "serial";
        {
            let mut log = SENT_LOG.lock();
            if log.len()==LOG_MAX { log.pop_front(); }
            log.push_back(payload.as_bytes().to_vec());
        }
        let cid = self.next_id();
        let (tx, rx) = bounded::<String>(1);
        self.pending.lock().insert(cid, PendingCommand { tx, tag, started });

        let mut v = Vec::with_capacity(payload.len() + 16);
        v.extend_from_slice(payload.as_bytes());
        v.extend_from_slice(format!("#{cid}{CRLF}").as_bytes());
        self.enqueue_ff(Packet { data: v, tag, started, tracked_id: Some(cid) })?;

        match rx.recv_timeout(Duration::from_secs_f32(timeout_s)) {
            Ok(resp) => Ok(resp),
            Err(_) => {
                self.pending.lock().remove(&cid);
                Err(MakcuError::Timeout(cid))
            }
        }
    }

    pub fn set_button_callback<F>(&self, cb: Option<F>)
    where
        F: Fn(MouseButtonStates) + Send + Sync + 'static,
    {
        *self.button_cb.write() = cb.map(|f| Box::new(f) as _);
    }

    pub fn set_stream_callback<F>(&self, cb: Option<F>)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        *self.stream_cb.write() = cb.map(|f| Box::new(f) as _);
    }
}

// --------------------------- Reader loop -----------------------------------

fn reader_loop(
    ser: &mut dyn SerialPort,
    stop: Arc<AtomicBool>,
    pending: Arc<Mutex<FxHashMap<u32, PendingCommand>>>,
    button_cb: Arc<RwLock<Option<Box<dyn Fn(MouseButtonStates) + Send + Sync>>>>,
    stream_cb: Arc<RwLock<Option<Box<dyn Fn(String) + Send + Sync>>>>,
) {
    const BUF_TMP: usize = 512;
    let mut buf = BytesMut::with_capacity(2048);
    let mut tmp = [0u8; BUF_TMP];

    while !stop.load(Ordering::Relaxed) {
        match ser.read(&mut tmp) {
            Ok(0) => continue,
            Ok(n) => {
                if dbg() {
                    // True raw dump — every byte before any parsing.
                    print!("[RAW {n:3}B]: ");
                    for b in &tmp[..n] {
                        match b {
                            32..=126 => print!("{}", *b as char),
                            b'\r'    => print!("\\r"),
                            b'\n'    => print!("\\n"),
                            _        => print!("\\x{b:02X}"),
                        }
                    }
                    println!();
                }
                // Single control byte = binary button state from streaming.
                if n == 1 && tmp[0] < 32 && tmp[0] != b'\r' && tmp[0] != b'\n' {
                    if dbg() {
                        println!("[RX binary] button mask = 0x{:02X} ({:08b})", tmp[0], tmp[0]);
                    }
                    if let Some(cb) = &*button_cb.read() {
                        cb(MouseButtonStates::from_mask(tmp[0]));
                    }
                    continue;
                }
                buf.extend_from_slice(&tmp[..n]);
                while let Some(pos) = memchr(b'\n', &buf) {
                    let mut line = buf.split_to(pos + 1);
                    if line.ends_with(b"\n") { line.truncate(line.len() - 1); }
                    if line.ends_with(b"\r") { line.truncate(line.len() - 1); }
                    if line.is_empty() { continue; }

                    // Binary button streaming: "km.buttons" (10 bytes) + 1 mask byte.
                    if line.starts_with(b"km.buttons") && line.len() == 11 {
                        let mask = line[10];
                        if dbg() { println!("[RX binary] km.buttons mask=0x{mask:02X} ({mask:08b})"); }
                        if let Some(cb) = &*button_cb.read() {
                            cb(MouseButtonStates::from_mask(mask));
                        }
                        continue;
                    }

                    // Binary mouse streaming: "km.mouse" (8 bytes) + 8 data bytes.
                    if line.starts_with(b"km.mouse") && line.len() == 16 {
                        if dbg() { println!("[RX binary] km.mouse {:02X?}", &line[8..]); }
                        if let Some(cb) = &*stream_cb.read() {
                            cb(format!("mouse_raw:{:02X?}", &line[8..]));
                        }
                        continue;
                    }

                    handle_line_bytes(&line, &pending, &stream_cb);
                }
            }
            Err(_) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[inline]
fn handle_line_bytes(
    line: &[u8],
    pending: &Mutex<FxHashMap<u32, PendingCommand>>,
    stream_cb: &RwLock<Option<Box<dyn Fn(String) + Send + Sync>>>,
) {
    if dbg() {
        match std::str::from_utf8(line) {
            Ok(s)  => println!("[RX] {s}"),
            Err(_) => println!("[RX binary] {:02X?}", line),
        }
    }

    if let Some(hash) = memchr(b'#', line) {
        if let Some(colon) = memchr2(b':', b'\r', &line[hash + 1..]) {
            if let Some(cid) = std::str::from_utf8(&line[hash + 1..hash + 1 + colon])
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
            {
                if let Some(pc) = pending.lock().remove(&cid) {
                    let dt = pc.started.elapsed();
                    PerformanceProfiler::prof(pc.tag, pc.started); 
                    let payload =
                        String::from_utf8_lossy(&line[hash + 1 + colon + 1..]).into_owned();
                    let _ = pc.tx.send(payload);
                }
                return;
            }
        }
    }

    if pending.lock().is_empty() {
        // No command is waiting for a reply — this is unsolicited streaming data.
        if let Some(cb) = &*stream_cb.read() {
            cb(String::from_utf8_lossy(line).into_owned());
        }
        return;
    }

    
    let payload = if line.starts_with(b">>>") {
        let mut p = &line[3..];
        if p.first() == Some(&b' ') { p = &p[1..]; }
        p
    } else {
        line
    };

    // Drop exact echoes of sent commands (device may normalize params, e.g. "1" → "raw").
    // Also drop any line that starts with the same "km.cmd(" prefix as a recently sent command,
    // since the device rewrites the args in its echo (e.g. km.buttons(1,10) → km.buttons(raw,10)).
    {
        let log = SENT_LOG.lock();
        if log.iter().any(|cmd| {
            if cmd.as_slice() == payload { return true; }
            // Match by command name prefix: "km.foo(" prefix of both sides.
            if let Some(op) = memchr(b'(', cmd) {
                if payload.starts_with(&cmd[..=op]) { return true; }
            }
            false
        }) { return; }
    }

    let mut pend = pending.lock();
    if let Some((&cid, _)) = pend.iter().next() {
        if let Some(pc) = pend.remove(&cid) {
            let _ = pc.tx.send(String::from_utf8_lossy(payload).into_owned());
        }
    }
}

pub struct Batch<'a> {
    dev: &'a Device,
    buf: String,
}
impl<'a> Batch<'a> {
    #[inline(always)]
    fn mark(op: &'static str) {
        PerformanceProfiler::measure(op, || {})
    }

    #[inline(always)]
   fn btn_name(btn: MouseButton) -> &'static str {
       match btn {
           MouseButton::Left   => "left",
           MouseButton::Right  => "right",
           MouseButton::Middle => "middle",
           MouseButton::Side1  => "side1",
           MouseButton::Side2  => "side2",
       }
   }

    pub fn move_rel(mut self, dx:i32, dy:i32) -> Self {
        Self::mark("move");
        use std::fmt::Write; let _ = write!(self.buf,"km.move({dx},{dy}){CRLF}");
        self
    }
    pub fn click(mut self, btn:MouseButton) -> Self {
        Self::mark("click");
        use std::fmt::Write;
       let _ = write!(self.buf, "km.{}(){CRLF}", Self::btn_name(btn));
       self
    }
    pub fn press(mut self, btn: MouseButton) -> Self {
       Self::mark("press");
       use std::fmt::Write;
       let _ = write!(self.buf, "km.{}(1){CRLF}", Self::btn_name(btn));
       self
   }

    pub fn release(mut self, btn: MouseButton) -> Self {
       Self::mark("release");
       use std::fmt::Write;
       let _ = write!(self.buf, "km.{}(0){CRLF}", Self::btn_name(btn));
        self
    }
    pub fn wheel(mut self, d:i32)->Self {
        Self::mark("wheel");
        use std::fmt::Write;  let _ = write!(self.buf, "km.wheel({d}){CRLF}");
        self
    }
    pub fn move_abs(mut self, x:i32, y:i32)->Self {
        use std::fmt::Write; let _ = write!(self.buf, "km.moveto({x},{y}){CRLF}");
        self
    }
    pub fn pan(mut self, steps:i32)->Self {
        use std::fmt::Write; let _ = write!(self.buf, "km.pan({steps}){CRLF}");
        self
    }
    pub fn tilt(mut self, steps:i32)->Self {
        use std::fmt::Write; let _ = write!(self.buf, "km.tilt({steps}){CRLF}");
        self
    }
    pub fn mouse_raw(mut self, buttons:u8, x:i32, y:i32, whl:i32, pan:i32, tilt:i32)->Self {
        use std::fmt::Write;
        let _ = write!(self.buf, "km.mo({buttons},{x},{y},{whl},{pan},{tilt}){CRLF}");
        self
    }
    pub fn key_down(mut self, key:&str)->Self {
        use std::fmt::Write; let _ = write!(self.buf, "km.down({key}){CRLF}");
        self
    }
    pub fn key_up(mut self, key:&str)->Self {
        use std::fmt::Write; let _ = write!(self.buf, "km.up({key}){CRLF}");
        self
    }
    pub fn key_press(mut self, key:&str)->Self {
        use std::fmt::Write; let _ = write!(self.buf, "km.press({key}){CRLF}");
        self
    }
    pub fn run(self) -> MakcuResult<()> {
        Ok(PerformanceProfiler::measure("batch", || {

            let _ = self.dev.send_ff(&self.buf);
        }))
    }
}


#[cfg(not(feature = "async"))]
pub struct DeviceAsync; 

#[cfg(not(feature = "async"))]
impl DeviceAsync {
    pub async fn new(_: &str, _: u32) -> std::io::Result<Self> {
        Err(std::io::Error::new(std::io::ErrorKind::Other, "async not enabled"))
    }
}

#[cfg(feature = "async")]
pub struct DeviceAsync {
    inner: tokio::sync::Mutex<tokio_serial::SerialStream>,
}

#[cfg(feature = "async")]
impl DeviceAsync {
    /* ---------- конструктор ---------- */
    pub async fn new(port: &str, baud: u32) -> std::io::Result<Self> {
        use tokio_serial::SerialPortBuilderExt;
        let stream = tokio_serial::new(port, baud).open_native_async()?;
        Ok(Self { inner: tokio::sync::Mutex::new(stream) })
    }

    /* ---------- низкоуровневый raw‑write ---------- */
    #[inline(always)]
    async fn send_raw(&self, txt: &str) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut guard = self.inner.lock().await;
        AsyncWriteExt::write_all(&mut *guard, txt.as_bytes()).await
    }

    /* --- общие вспомогательные штуки --- */
    #[inline(always)]
    fn btn_name(btn: MouseButton) -> &'static str {
        match btn {
            MouseButton::Left  => "left",
            MouseButton::Right => "right",
            MouseButton::Middle=> "middle",
            MouseButton::Side1 => "side1",
            MouseButton::Side2 => "side2",
        }
    }

    /* ---------- отдельные операции ---------- */
    pub async fn move_rel(&self, dx: i32, dy: i32) -> std::io::Result<()> {
        PerformanceProfiler::measure("move_async", || {});
        self.send_raw(&format!("km.move({dx},{dy}){CRLF}")).await
    }

    pub async fn wheel(&self, delta: i32) -> std::io::Result<()> {
        PerformanceProfiler::measure("wheel_async", || {});
        self.send_raw(&format!("km.wheel({delta}){CRLF}")).await
    }

    pub async fn press(&self, btn: MouseButton) -> std::io::Result<()> {
        PerformanceProfiler::measure("press_async", || {});
        self.send_raw(&format!("km.{}(1){CRLF}", Self::btn_name(btn))).await
    }

    pub async fn release(&self, btn: MouseButton) -> std::io::Result<()> {
        PerformanceProfiler::measure("release_async", || {});
        self.send_raw(&format!("km.{}(0){CRLF}", Self::btn_name(btn))).await
    }

    pub async fn click(&self, btn: MouseButton) -> std::io::Result<()> {
        PerformanceProfiler::measure("click_async", || {});
        self.press(btn).await?;
        self.release(btn).await
    }

    /* ---------- пакетная отправка ---------- */
    pub fn batch(&self) -> AsyncBatch<'_> {
        AsyncBatch { dev: self, buf: String::new() }
    }
}

/* ---------------- builder для async batch ---------------- */

#[cfg(feature = "async")]
pub struct AsyncBatch<'a> {
    dev: &'a DeviceAsync,
    buf: String,
}

#[cfg(feature = "async")]
impl<'a> AsyncBatch<'a> {
    #[inline(always)] fn mark(op: &'static str){ PerformanceProfiler::measure(op, ||{}) }
    #[inline(always)] fn btn(btn: MouseButton)->&'static str{ DeviceAsync::btn_name(btn) }

    pub fn move_rel(mut self, dx:i32, dy:i32)->Self{
        Self::mark("move_async");
        use core::fmt::Write; let _=write!(self.buf,"km.move({dx},{dy}){CRLF}");
        self
    }
    pub fn wheel  (mut self, d:i32)->Self{
        Self::mark("wheel_async");
        use core::fmt::Write; let _=write!(self.buf,"km.wheel({d}){CRLF}");
        self
    }
    pub fn press  (mut self, b:MouseButton)->Self{
        Self::mark("press_async");
        self.buf.push_str(&format!("km.{}(1){CRLF}", Self::btn(b))); self
    }
    pub fn release(mut self, b:MouseButton)->Self{
        Self::mark("release_async");
        self.buf.push_str(&format!("km.{}(0){CRLF}", Self::btn(b))); self
    }
    pub fn click  (mut self, b:MouseButton)->Self{
        Self::mark("click_async");
        self.buf.push_str(&format!("km.{}(){CRLF}",   Self::btn(b))); self
    }
    pub fn move_abs(mut self, x:i32, y:i32)->Self{
        use core::fmt::Write; let _=write!(self.buf,"km.moveto({x},{y}){CRLF}");
        self
    }
    pub fn pan(mut self, steps:i32)->Self{
        use core::fmt::Write; let _=write!(self.buf,"km.pan({steps}){CRLF}");
        self
    }
    pub fn tilt(mut self, steps:i32)->Self{
        use core::fmt::Write; let _=write!(self.buf,"km.tilt({steps}){CRLF}");
        self
    }
    pub fn mouse_raw(mut self, buttons:u8, x:i32, y:i32, whl:i32, pan:i32, tilt:i32)->Self{
        use core::fmt::Write;
        let _=write!(self.buf,"km.mo({buttons},{x},{y},{whl},{pan},{tilt}){CRLF}");
        self
    }
    pub fn key_down(mut self, key:&str)->Self{
        use core::fmt::Write; let _=write!(self.buf,"km.down({key}){CRLF}");
        self
    }
    pub fn key_up(mut self, key:&str)->Self{
        use core::fmt::Write; let _=write!(self.buf,"km.up({key}){CRLF}");
        self
    }
    pub fn key_press(mut self, key:&str)->Self{
        use core::fmt::Write; let _=write!(self.buf,"km.press({key}){CRLF}");
        self
    }

    pub async fn run(self) -> std::io::Result<()> {
        PerformanceProfiler::measure("batch_async", ||{});
        self.dev.send_raw(&self.buf).await
    }
}


// ================================ Device ===================================

#[derive(Clone)]
pub struct Device {
    sp: Arc<Mutex<SerialPortWrap>>,
    connected: Arc<AtomicBool>,
    lock_valid: Arc<AtomicBool>,
    btn_cache: Arc<Mutex<MouseButtonStates>>,
}

#[derive(Clone, Copy)]
pub enum MouseButton { Left, Right, Middle, Side1, Side2 }

impl Device {
    pub fn batch(&self)->Batch { Batch { dev:self, buf:String::new() } }
    pub fn new(port: impl Into<String>, baud: u32, timeout: Duration) -> Self {
        Self {
            
            sp: Arc::new(Mutex::new(SerialPortWrap::new(port, baud, timeout))),
            connected: Arc::new(AtomicBool::new(false)),
            lock_valid: Arc::new(AtomicBool::new(false)),
            btn_cache: Arc::new(Mutex::new(MouseButtonStates::default())),
        }
    }

    pub fn connect(&self) -> MakcuResult<()> {
        self.sp.lock().open()?;
        PerformanceProfiler::measure("connect", || {});
        self.connected.store(true, Ordering::Release);

        let cache = self.btn_cache.clone();
        self.sp.lock().set_button_callback(Some(move |st| *cache.lock() = st));
        // mode=1 (raw), period=10ms — device streams a 1-byte button mask every 10ms.
        self.send_ff("km.buttons(1,10)")
    }

    pub fn disconnect(&self) {
        PerformanceProfiler::measure("disconnect", || self.sp.lock().close());
        self.connected.store(false, Ordering::Release);
        self.lock_valid.store(false, Ordering::Release);
    }

    /* ---- построение команд для кнопок ---- */
    fn btn_cmd(&self, btn: MouseButton, down: bool) -> MakcuResult<()> {
        PerformanceProfiler::measure("click", || {
            let suffix = if down { "(1)" } else { "(0)" };
            let cmd = match btn {
                MouseButton::Left   => "km.left",
                MouseButton::Right  => "km.right",
                MouseButton::Middle => "km.middle",
                MouseButton::Side1  => "km.side1",
                MouseButton::Side2  => "km.side2",
            };
            let _ = self.send_ff(&format!("{cmd}{suffix}"));
            });
        Ok(())
    }
    

    /* ---- lock‑helpers ---- */
    fn send_lock(&self, short: &str, lock: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.lock_{short}({})", if lock {1} else {0}))
    }


    pub fn set_button_callback<F>(&self, cb: Option<F>)
    where
        F: Fn(MouseButtonStates) + Send + Sync + 'static,
    {
        let cache = self.btn_cache.clone();
        let sp = self.sp.lock();
        if let Some(user_cb) = cb {
            sp.set_button_callback(Some(move |st| {
                *cache.lock() = st;     
                user_cb(st);            
            }));
        } else {
            sp.set_button_callback(None::<fn(MouseButtonStates)>);
        }
    }

    /// Register a callback for unsolicited streaming lines (keyboard, axis, mouse stream text).
    /// Called from the reader thread for every line that arrives with no pending command.
    pub fn set_stream_callback<F>(&self, cb: Option<F>)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        self.sp.lock().set_stream_callback(cb);
    }

    /// Enable or disable debug logging of all serial I/O.
    /// When enabled, every sent command and every received byte/line is printed to stdout.
    pub fn set_debug(enabled: bool) {
        DEBUG_IO.store(enabled, Ordering::Relaxed);
    }

    #[inline] fn ensure(&self) -> MakcuResult<()> {
        if self.connected.load(Ordering::Acquire) && self.sp.lock().is_open() {
            Ok(())
        } else {
            Err(MakcuError::Connection("Not connected".into()))
        }
    }

    #[inline] fn send_ff(&self, cmd: &str) -> MakcuResult<()> {
        self.ensure()?;
        self.sp.lock().send_ff(cmd)
    }
    #[inline] fn send_tr(&self, cmd: &str, t: f32) -> MakcuResult<String> {
        self.ensure()?;
        self.sp.lock().send_tracked(cmd, t)
    }

    


    // ====== API ======

    // ---- Mouse Movement ----

    pub fn move_rel(&self, dx: i32, dy: i32) -> MakcuResult<()> {
        PerformanceProfiler::measure("move", || {
            let _ = self.send_ff(&format!("km.move({dx},{dy})"));
        });
        Ok(())
    }

    /// Move relative with segments and optional cubic Bézier control points.
    /// `bezier`: optional `(cx1, cy1, cx2, cy2)` control points.
    pub fn move_rel_advanced(&self, dx: i32, dy: i32, segments: u32, bezier: Option<(i32,i32,i32,i32)>) -> MakcuResult<()> {
        let cmd = match bezier {
            Some((cx1,cy1,cx2,cy2)) => format!("km.move({dx},{dy},{segments},{cx1},{cy1},{cx2},{cy2})"),
            None => format!("km.move({dx},{dy},{segments})"),
        };
        self.send_ff(&cmd)
    }

    /// Move to absolute screen position (device calculates delta internally).
    pub fn move_abs(&self, x: i32, y: i32) -> MakcuResult<()> {
        self.send_ff(&format!("km.moveto({x},{y})"))
    }

    /// Move to absolute position with segments and optional Bézier control points.
    pub fn move_abs_advanced(&self, x: i32, y: i32, segments: u32, bezier: Option<(i32,i32,i32,i32)>) -> MakcuResult<()> {
        let cmd = match bezier {
            Some((cx1,cy1,cx2,cy2)) => format!("km.moveto({x},{y},{segments},{cx1},{cy1},{cx2},{cy2})"),
            None => format!("km.moveto({x},{y},{segments})"),
        };
        self.send_ff(&cmd)
    }

    /// Get current pointer position as (x, y).
    pub fn get_pos(&self) -> MakcuResult<String> {
        self.send_tr("km.getpos()", 1.0)
    }

    /// Move to (x,y) then perform a silent left-click (no frame sent for click).
    pub fn silent_click(&self, x: i32, y: i32) -> MakcuResult<()> {
        self.send_ff(&format!("km.silent({x},{y})"))
    }

    // ---- Mouse Buttons ----

    #[inline] fn btn(&self, name: &str, down: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.{name}({})", if down { 1 } else { 0 }))
    }

    pub fn press_left(&self) -> MakcuResult<()> { self.btn("left", true) }
    pub fn release_left(&self) -> MakcuResult<()> { self.btn("left", false) }
    pub fn press_right(&self) -> MakcuResult<()> { self.btn("right", true) }
    pub fn release_right(&self) -> MakcuResult<()> { self.btn("right", false) }
    pub fn press_middle(&self) -> MakcuResult<()> { self.btn("middle", true) }
    pub fn release_middle(&self) -> MakcuResult<()> { self.btn("middle", false) }

    pub fn click_left(&self) -> MakcuResult<()> { self.press_left()?; self.release_left() }
    pub fn click_right(&self) -> MakcuResult<()> { self.press_right()?; self.release_right() }

    pub fn press(&self, btn: MouseButton)  -> MakcuResult<()> { self.btn_cmd(btn, true)  }
    pub fn release(&self, btn: MouseButton)-> MakcuResult<()> { self.btn_cmd(btn, false) }
    pub fn click(&self, btn: MouseButton)  -> MakcuResult<()> { self.press(btn)?; self.release(btn) }

    /// Schedule `count` clicks on `button` with optional `delay_ms` between each.
    /// `button`: 1=left, 2=right, 3=middle, 4=side1, 5=side2.
    pub fn click_scheduled(&self, button: u8, count: Option<u32>, delay_ms: Option<u32>) -> MakcuResult<()> {
        let cmd = match (count, delay_ms) {
            (Some(c), Some(d)) => format!("km.click({button},{c},{d})"),
            (Some(c), None)    => format!("km.click({button},{c})"),
            _                  => format!("km.click({button})"),
        };
        self.send_ff(&cmd)
    }

    /// Enable turbo (rapid-fire) for a mouse button.
    /// `button`: 1-5; `delay_ms`: None = random 35-75ms; 0 = disable for that button.
    /// Call `turbo_disable_all()` or `turbo(0, Some(0))` to disable all.
    pub fn turbo(&self, button: u8, delay_ms: Option<u32>) -> MakcuResult<()> {
        let cmd = match delay_ms {
            Some(d) => format!("km.turbo({button},{d})"),
            None    => format!("km.turbo({button})"),
        };
        self.send_ff(&cmd)
    }

    /// Disable turbo on all buttons.
    pub fn turbo_disable_all(&self) -> MakcuResult<()> {
        self.send_ff("km.turbo(0)")
    }

    /// Query active turbo settings; returns the device response string.
    pub fn turbo_get(&self) -> MakcuResult<String> {
        self.send_tr("km.turbo()", 1.0)
    }

    // ---- Wheel / Pan / Tilt ----

    pub fn wheel(&self, delta: i32) -> MakcuResult<()> {
        PerformanceProfiler::measure("wheel", ||
            { self.send_ff(&format!("km.wheel({delta})")).ok(); }
        );
        Ok(())
    }

    /// Horizontal scroll (pan). Query pending pan with `pan_get()`.
    pub fn pan(&self, steps: i32) -> MakcuResult<()> {
        self.send_ff(&format!("km.pan({steps})"))
    }

    pub fn pan_get(&self) -> MakcuResult<String> {
        self.send_tr("km.pan()", 1.0)
    }

    /// Z-axis / tilt scroll. Query with `tilt_get()`.
    pub fn tilt(&self, steps: i32) -> MakcuResult<()> {
        self.send_ff(&format!("km.tilt({steps})"))
    }

    pub fn tilt_get(&self) -> MakcuResult<String> {
        self.send_tr("km.tilt()", 1.0)
    }

    // ---- Raw Mouse Frame ----

    /// Send a complete raw mouse frame.
    /// `buttons`: bitmask; `x`,`y`: deltas; `wheel`,`pan`,`tilt`: scroll.
    pub fn mouse_raw(&self, buttons: u8, x: i32, y: i32, wheel: i32, pan: i32, tilt: i32) -> MakcuResult<()> {
        self.send_ff(&format!("km.mo({buttons},{x},{y},{wheel},{pan},{tilt})"))
    }

    // ---- Locks ----

    fn get_lock(&self, short: &str) -> MakcuResult<String> {
        self.send_tr(&format!("km.lock_{short}()"), 1.0)
    }

    pub fn lock_mouse_x(&self, lock: bool) -> MakcuResult<()> { self.send_lock("mx", lock) }
    pub fn lock_mouse_y(&self, lock: bool) -> MakcuResult<()> { self.send_lock("my", lock) }
    pub fn lock_mouse_wheel(&self, lock: bool) -> MakcuResult<()> { self.send_lock("mw", lock) }
    pub fn lock_mouse_x_pos(&self, lock: bool) -> MakcuResult<()> { self.send_lock("mx+", lock) }
    pub fn lock_mouse_x_neg(&self, lock: bool) -> MakcuResult<()> { self.send_lock("mx-", lock) }
    pub fn lock_mouse_y_pos(&self, lock: bool) -> MakcuResult<()> { self.send_lock("my+", lock) }
    pub fn lock_mouse_y_neg(&self, lock: bool) -> MakcuResult<()> { self.send_lock("my-", lock) }
    pub fn lock_mouse_wheel_pos(&self, lock: bool) -> MakcuResult<()> { self.send_lock("mw+", lock) }
    pub fn lock_mouse_wheel_neg(&self, lock: bool) -> MakcuResult<()> { self.send_lock("mw-", lock) }
    pub fn lock_left     (&self, lock: bool) -> MakcuResult<()> { self.send_lock("ml", lock) }
    pub fn lock_right    (&self, lock: bool) -> MakcuResult<()> { self.send_lock("mr", lock) }
    pub fn lock_middle   (&self, lock: bool) -> MakcuResult<()> { self.send_lock("mm", lock) }
    pub fn lock_side1    (&self, lock: bool) -> MakcuResult<()> { self.send_lock("ms1", lock) }
    pub fn lock_side2    (&self, lock: bool) -> MakcuResult<()> { self.send_lock("ms2", lock) }

    pub fn get_lock_mouse_x(&self) -> MakcuResult<String> { self.get_lock("mx") }
    pub fn get_lock_mouse_y(&self) -> MakcuResult<String> { self.get_lock("my") }
    pub fn get_lock_mouse_wheel(&self) -> MakcuResult<String> { self.get_lock("mw") }
    pub fn get_lock_left   (&self) -> MakcuResult<String> { self.get_lock("ml") }
    pub fn get_lock_right  (&self) -> MakcuResult<String> { self.get_lock("mr") }
    pub fn get_lock_middle (&self) -> MakcuResult<String> { self.get_lock("mm") }
    pub fn get_lock_side1  (&self) -> MakcuResult<String> { self.get_lock("ms1") }
    pub fn get_lock_side2  (&self) -> MakcuResult<String> { self.get_lock("ms2") }

    // ---- Catch ----

    /// Enable catch on a locked button. `mode`: 0=auto, 1=manual.
    /// Requires the corresponding lock to be set first.
    pub fn catch_button(&self, btn: MouseButton, mode: u8) -> MakcuResult<()> {
        let name = Self::mouse_btn_lock_name(btn);
        self.send_ff(&format!("km.catch_{name}({mode})"))
    }

    pub fn get_catch_button(&self, btn: MouseButton) -> MakcuResult<String> {
        let name = Self::mouse_btn_lock_name(btn);
        self.send_tr(&format!("km.catch_{name}()"), 1.0)
    }

    fn mouse_btn_lock_name(btn: MouseButton) -> &'static str {
        match btn {
            MouseButton::Left   => "ml",
            MouseButton::Right  => "mr",
            MouseButton::Middle => "mm",
            MouseButton::Side1  => "ms1",
            MouseButton::Side2  => "ms2",
        }
    }

    // ---- Mouse Remap ----

    /// Remap a physical mouse button. `src`,`dst`: 1-5. `dst=0` clears remap for `src`.
    /// `src=0` resets all remaps.
    pub fn remap_button(&self, src: u8, dst: u8) -> MakcuResult<()> {
        self.send_ff(&format!("km.remap_button({src},{dst})"))
    }

    /// Reset all button remaps.
    pub fn remap_button_reset(&self) -> MakcuResult<()> {
        self.send_ff("km.remap_button(0)")
    }

    /// Get current button remaps.
    pub fn get_remap_button(&self) -> MakcuResult<String> {
        self.send_tr("km.remap_button()", 1.0)
    }

    /// Remap physical mouse axes. All three flags must be set at once.
    pub fn remap_axis(&self, invert_x: bool, invert_y: bool, swap_xy: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.remap_axis({},{},{})",
            invert_x as u8, invert_y as u8, swap_xy as u8))
    }

    /// Reset all axis remaps.
    pub fn remap_axis_reset(&self) -> MakcuResult<()> {
        self.send_ff("km.remap_axis(0)")
    }

    /// Get current axis remap settings.
    pub fn get_remap_axis(&self) -> MakcuResult<String> {
        self.send_tr("km.remap_axis()", 1.0)
    }

    pub fn invert_x(&self, enable: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.invert_x({})", enable as u8))
    }
    pub fn get_invert_x(&self) -> MakcuResult<String> { self.send_tr("km.invert_x()", 1.0) }

    pub fn invert_y(&self, enable: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.invert_y({})", enable as u8))
    }
    pub fn get_invert_y(&self) -> MakcuResult<String> { self.send_tr("km.invert_y()", 1.0) }

    pub fn swap_xy(&self, enable: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.swap_xy({})", enable as u8))
    }
    pub fn get_swap_xy(&self) -> MakcuResult<String> { self.send_tr("km.swap_xy()", 1.0) }

    // ---- Streaming ----

    /// Configure mouse button streaming. `mode`: 1=raw, 2=constructed; `period_ms`: 1-1000.
    /// Pass `(0, None)` or `(0, Some(0))` to disable.
    pub fn buttons_stream(&self, mode: u8, period_ms: Option<u32>) -> MakcuResult<()> {
        let cmd = match period_ms {
            Some(p) => format!("km.buttons({mode},{p})"),
            None    => format!("km.buttons({mode})"),
        };
        self.send_ff(&cmd)
    }

    /// Configure axis streaming. `mode`: 1=raw, 2=constructed; `period_ms`: 1-1000.
    pub fn axis_stream(&self, mode: u8, period_ms: Option<u32>) -> MakcuResult<()> {
        let cmd = match period_ms {
            Some(p) => format!("km.axis({mode},{p})"),
            None    => format!("km.axis({mode})"),
        };
        self.send_ff(&cmd)
    }

    /// Configure full mouse frame streaming (8-byte binary). `mode`: 1=raw, 2=constructed.
    pub fn mouse_stream(&self, mode: u8, period_ms: Option<u32>) -> MakcuResult<()> {
        let cmd = match period_ms {
            Some(p) => format!("km.mouse({mode},{p})"),
            None    => format!("km.mouse({mode})"),
        };
        self.send_ff(&cmd)
    }

    // ---- Keyboard ----

    /// Press a key down. `key`: HID code (e.g. `"'a'"`) or numeric string (e.g. `"4"`).
    pub fn key_down(&self, key: &str) -> MakcuResult<()> {
        self.send_ff(&format!("km.down({key})"))
    }

    /// Release a key. `key`: HID code or quoted name.
    pub fn key_up(&self, key: &str) -> MakcuResult<()> {
        self.send_ff(&format!("km.up({key})"))
    }

    /// Press and release a key with optional timing.
    /// `hold_ms`: None = random 35-75ms; `rand_ms`: additional randomization range.
    pub fn key_press(&self, key: &str, hold_ms: Option<u32>, rand_ms: Option<u32>) -> MakcuResult<()> {
        let cmd = match (hold_ms, rand_ms) {
            (Some(h), Some(r)) => format!("km.press({key},{h},{r})"),
            (Some(h), None)    => format!("km.press({key},{h})"),
            _                  => format!("km.press({key})"),
        };
        self.send_ff(&cmd)
    }

    /// Type an ASCII string (max 256 chars). Handles Shift for uppercase/symbols automatically.
    pub fn type_string(&self, text: &str) -> MakcuResult<()> {
        let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
        self.send_ff(&format!("km.string(\"{escaped}\")"))
    }

    /// Clear keyboard state and release all pressed keys.
    pub fn keyboard_init(&self) -> MakcuResult<()> {
        self.send_ff("km.init()")
    }

    /// Query whether a key is currently held down. Returns "1" or "0".
    pub fn is_key_down(&self, key: &str) -> MakcuResult<String> {
        self.send_tr(&format!("km.isdown({key})"), 1.0)
    }

    /// Disable one or more keys (block them from reaching host).
    /// `keys`: slice of key strings like `["'a'", "'f1'", "'ctrl'"]` or HID codes `["4", "6"]`.
    pub fn disable_keys(&self, keys: &[&str]) -> MakcuResult<()> {
        if keys.is_empty() { return Ok(()); }
        let args = keys.join(",");
        self.send_ff(&format!("km.disable({args})"))
    }

    /// Enable (un-disable) a single key. mode: 0=enable, 1=disable.
    pub fn set_key_disabled(&self, key: &str, disabled: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.disable({key},{})", disabled as u8))
    }

    /// Query all currently disabled keys.
    pub fn get_disabled_keys(&self) -> MakcuResult<String> {
        self.send_tr("km.disable()", 1.0)
    }

    /// Mask a key. `mode`: 0=off, 1=on.
    pub fn mask_key(&self, key: &str, enable: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.mask({key},{})", enable as u8))
    }

    /// Remap a key. `target=0` (or `"0"`) clears the remap (passthrough).
    pub fn remap_key(&self, source: &str, target: &str) -> MakcuResult<()> {
        self.send_ff(&format!("km.remap({source},{target})"))
    }

    /// Stream keyboard input. `mode`: 1=raw, 2=constructed; `period`: 1-1000 frames.
    /// `(0)` disables.
    pub fn keyboard_stream(&self, mode: u8, period: Option<u32>) -> MakcuResult<()> {
        let cmd = match period {
            Some(p) => format!("km.keyboard({mode},{p})"),
            None    => format!("km.keyboard({mode})"),
        };
        self.send_ff(&cmd)
    }

    // ---- System ----

    pub fn help(&self) -> MakcuResult<String> { self.send_tr("km.help()", 2.0) }
    pub fn info(&self) -> MakcuResult<String> { self.send_tr("km.info()", 2.0) }
    pub fn version(&self) -> MakcuResult<String> { self.send_tr("km.version()", 1.0) }
    pub fn device_type(&self) -> MakcuResult<String> { self.send_tr("km.device()", 1.0) }
    pub fn fault(&self) -> MakcuResult<String> { self.send_tr("km.fault()", 1.0) }

    /// Reboot the device (takes effect after response).
    pub fn reboot(&self) -> MakcuResult<()> { self.send_ff("km.reboot()") }

    pub fn get_serial(&self) -> MakcuResult<String> { self.send_tr("km.serial()", 1.0) }

    pub fn set_serial(&self, v: &str) -> MakcuResult<String> {
        let arg = if v.is_empty() { "0".into() }
                  else { format!("'{}'", v.replace('\'', "\\'")) };
        self.send_tr(&format!("km.serial({arg})"), 1.0)
    }

    /// Reset serial to default.
    pub fn reset_serial(&self) -> MakcuResult<String> { self.send_tr("km.serial(0)", 1.0) }

    /// Set log level (0-5). Persists for 3 power cycles then auto-disables.
    pub fn log_level(&self, level: u8) -> MakcuResult<()> {
        self.send_ff(&format!("km.log({level})"))
    }

    pub fn get_log_level(&self) -> MakcuResult<String> { self.send_tr("km.log()", 1.0) }

    /// Enable/disable command echo. `enable`: true=on, false=off.
    pub fn echo(&self, enable: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.echo({})", enable as u8))
    }

    pub fn get_echo(&self) -> MakcuResult<String> { self.send_tr("km.echo()", 1.0) }

    /// Change serial baud rate (115200–4000000). `rate=0` resets to 115200.
    /// Note: host must re-open the port at the new speed after calling this.
    pub fn set_baud(&self, rate: u32) -> MakcuResult<()> {
        self.send_ff(&format!("km.baud({rate})"))
    }

    pub fn get_baud(&self) -> MakcuResult<String> { self.send_tr("km.baud()", 1.0) }

    /// Set bypass mode: 0=off, 1=mouse bypass, 2=keyboard bypass.
    pub fn bypass(&self, mode: u8) -> MakcuResult<()> {
        self.send_ff(&format!("km.bypass({mode})"))
    }

    pub fn get_bypass(&self) -> MakcuResult<String> { self.send_tr("km.bypass()", 1.0) }

    /// USB high-speed compatibility (persistent).
    pub fn high_speed(&self, enable: bool) -> MakcuResult<()> {
        self.send_ff(&format!("km.hs({})", enable as u8))
    }

    pub fn get_high_speed(&self) -> MakcuResult<String> { self.send_tr("km.hs()", 1.0) }

    /// Control LED. `target`: 1=device, 2=host. `mode`: 0=off, 1=on.
    pub fn led(&self, target: u8, mode: u8) -> MakcuResult<()> {
        self.send_ff(&format!("km.led({target},{mode})"))
    }

    /// Flash LED. `target`: 1=device, 2=host. `times`: flash count. `delay_ms`: interval.
    pub fn led_flash(&self, target: u8, times: u32, delay_ms: u32) -> MakcuResult<()> {
        self.send_ff(&format!("km.led({target},{times},{delay_ms})"))
    }

    /// Query LED state.
    pub fn get_led(&self) -> MakcuResult<String> { self.send_tr("km.led()", 1.0) }

    /// Set auto-release timer (500–300000 ms). `timer_ms=0` disables. Persists across reboots.
    pub fn auto_release(&self, timer_ms: u32) -> MakcuResult<()> {
        self.send_ff(&format!("km.release({timer_ms})"))
    }

    pub fn get_auto_release(&self) -> MakcuResult<String> { self.send_tr("km.release()", 1.0) }

    /// Set virtual screen dimensions for `moveto` absolute coordinates.
    pub fn screen(&self, width: u32, height: u32) -> MakcuResult<()> {
        self.send_ff(&format!("km.screen({width},{height})"))
    }

    pub fn get_screen(&self) -> MakcuResult<String> { self.send_tr("km.screen()", 1.0) }

    pub fn profiler_stats() -> FxHashMap<&'static str, FxHashMap<&'static str, f64>> {
        PerformanceProfiler::stats()
    }

}
// ============================ MockSerial (optional) ========================
#[cfg(feature = "mockserial")]
pub mod mockserial {
    use super::*;
    use std::io::{Read, Write};

    pub struct MockSerial {
        timeout: Duration,
        in_q: Mutex<std::collections::VecDeque<Vec<u8>>>,
        out_q: Mutex<std::collections::VecDeque<Vec<u8>>>,
        last_btn: Mutex<Instant>,
    }
    impl MockSerial {
        pub fn new(timeout: Duration) -> Self {
            Self {
                timeout,
                in_q: Mutex::default(),
                out_q: Mutex::default(),
                last_btn: Mutex::new(Instant::now()),
            }
        }
    }
    impl Read for MockSerial {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let now = Instant::now();
            if now.duration_since(*self.last_btn.lock()) > Duration::from_millis(500) {
                *self.last_btn.lock() = now;
                self.in_q.lock().push_back(vec![0x01]);
            }
            if let Some(chunk) = self.in_q.lock().pop_front() {
                let n = chunk.len().min(buf.len());
                buf[..n].copy_from_slice(&chunk[..n]);
                return Ok(n);
            }
            thread::sleep(self.timeout);
            Ok(0)
        }
    }
    impl Write for MockSerial {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.out_q.lock().push_back(buf.to_vec());
            let txt = String::from_utf8_lossy(buf);
            if let Some(idx) = txt.rfind('#') {
                let sid = &txt[idx + 1..].trim_end();
                let resp = format!(">>> OK#{sid}:OK{CRLF}").into_bytes();
                self.in_q.lock().push_back(resp);
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
}
