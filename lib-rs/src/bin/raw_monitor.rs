//! Raw serial monitor — bypasses the entire library.
//! Run with: cargo run --bin raw_monitor -- /dev/cu.usbmodemXXXX

use std::{env, io::{self, Read, Write}, thread, time::Duration};

fn main() {
    let port_name = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: raw_monitor <port>  e.g. /dev/cu.usbmodem...");
        std::process::exit(1);
    });

    println!("Opening {port_name} at 115200...");

    let mut port = serialport::new(&port_name, 115_200)
        .timeout(Duration::from_millis(50))
        .open()
        .unwrap_or_else(|e| { eprintln!("Failed: {e}"); std::process::exit(1); });

    println!("Opened OK.");

    // DTR HIGH = "host connected" for CDC-ACM devices; firmware won't respond with it low.
    // RTS HIGH = required by some chips to enable TX.
    match port.write_data_terminal_ready(true) {
        Ok(_)  => println!("  DTR: set HIGH (ok)"),
        Err(e) => println!("  DTR: set HIGH (err: {e})"),
    }
    match port.write_request_to_send(true) {
        Ok(_)  => println!("  RTS: set HIGH (ok)"),
        Err(e) => println!("  RTS: set HIGH (err: {e})"),
    }

    // Short settle then drain — show any boot/startup bytes the device emits.
    println!("\n--- Settle window (1 s) — printing everything received ---");
    io::stdout().flush().ok();
    let settle_bytes = drain_print(&mut *port, 1000);
    if settle_bytes == 0 {
        println!("  (nothing received during settle)");
    }
    println!("--- Settle done ---\n");

    // Step 1: kill any active streaming immediately.
    // The device may have been left streaming from a previous session.
    println!("=== Step 1: stopping all streaming ===");
    for cmd in &[
        b"km.buttons(0,0)\r\n" as &[u8],
        b"km.axis(0,0)\r\n",
        b"km.mouse(0,0)\r\n",
        b"km.keyboard(0)\r\n",
    ] {
        port.write_all(cmd).ok();
        port.flush().ok();
        thread::sleep(Duration::from_millis(30));
    }
    let drained = drain(&mut *port, 500);
    if drained > 0 {
        println!("  Drained {drained} stale bytes from streaming.");
    } else {
        println!("  Nothing to drain.");
    }
    println!();

    // Step 2: basic checks.
    println!("=== Step 2: basic comms check ===");
    send_and_read(&mut *port, b"km.version()\r\n", 800);
    send_and_read(&mut *port, b"km.device()\r\n",  800);

    // Step 3: enable streaming and observe.
    println!("=== Step 3: enabling streaming ===");
    send_and_read(&mut *port, b"km.buttons(1,10)\r\n", 500);
    send_and_read(&mut *port, b"km.axis(1,10)\r\n",    500);
    send_and_read(&mut *port, b"km.keyboard(1,1)\r\n", 500);

    println!("\n=== Step 4: listening — move mouse / press buttons. Ctrl+C to stop ===\n");
    io::stdout().flush().ok();

    let mut buf = [0u8; 512];
    loop {
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => { print_raw(&buf[..n]); io::stdout().flush().ok(); }
            Err(ref e) if e.kind() == io::ErrorKind::TimedOut  => {}
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => { eprintln!("Read error: {e}"); break; }
        }
    }
}

fn send_and_read(port: &mut dyn serialport::SerialPort, cmd: &[u8], window_ms: u64) {
    let text = std::str::from_utf8(cmd).unwrap_or("(binary)").trim();
    print!(">>> TX: {text}  ");
    io::stdout().flush().ok();

    match port.write_all(cmd) {
        Ok(_)  => { port.flush().unwrap_or_else(|e| eprintln!("  flush error: {e}")); println!("[write OK]"); }
        Err(e) => { println!("[WRITE FAILED: {e}]"); return; }
    }

    let deadline = std::time::Instant::now() + Duration::from_millis(window_ms);
    let mut got_any = false;
    let mut buf = [0u8; 512];
    while std::time::Instant::now() < deadline {
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => { print_raw(&buf[..n]); got_any = true; }
            Err(ref e) if e.kind() == io::ErrorKind::TimedOut  => {}
            Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => { eprintln!("  read error: {e}"); break; }
        }
    }
    if !got_any { println!("  (no response)\n"); }
}

/// Drain and print all pending bytes for `ms` milliseconds; return byte count.
fn drain_print(port: &mut dyn serialport::SerialPort, ms: u64) -> usize {
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    let mut buf = [0u8; 512];
    let mut total = 0usize;
    while std::time::Instant::now() < deadline {
        match port.read(&mut buf) {
            Ok(n) if n > 0 => { print_raw(&buf[..n]); io::stdout().flush().ok(); total += n; }
            _ => {}
        }
    }
    total
}

/// Drain all pending bytes silently, return count.
fn drain(port: &mut dyn serialport::SerialPort, ms: u64) -> usize {
    let deadline = std::time::Instant::now() + Duration::from_millis(ms);
    let mut buf = [0u8; 512];
    let mut total = 0usize;
    while std::time::Instant::now() < deadline {
        match port.read(&mut buf) {
            Ok(n) if n > 0 => total += n,
            _ => {}
        }
    }
    total
}

fn print_raw(data: &[u8]) {
    print!("  HEX [{:3}b]: ", data.len());
    for b in data { print!("{b:02X} "); }
    println!();
    print!("  TXT [{:3}b]: ", data.len());
    for b in data {
        match b {
            32..=126 => print!("{}", *b as char),
            b'\r' => print!("\\r"),
            b'\n' => print!("\\n"),
            _ => print!("\\x{b:02X}"),
        }
    }
    println!("\n");
}
