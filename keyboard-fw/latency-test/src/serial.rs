//! CH343 serial reader. Opens the "middle" debug serial, parses SIGTAP records,
//! and forwards each one tagged with its host arrival time.

use std::io::Read;
use std::sync::mpsc::Sender;
use std::time::Duration;

use crate::macos_time::now_ns;
use crate::proto::{Parser, Record};

pub struct SerialEvent {
    pub rec: Record,
    /// Host nanoseconds when the chunk carrying this record was read.
    pub host_recv_ns: i64,
}

/// Auto-detect a CH34x USB-serial bridge (QinHeng VID 0x1A86).
pub fn autodetect_ch34x() -> Option<String> {
    let ports = serialport::available_ports().ok()?;
    for p in ports {
        if let serialport::SerialPortType::UsbPort(info) = &p.port_type {
            if info.vid == 0x1A86 {
                return Some(p.port_name);
            }
        }
    }
    None
}

pub fn list_ports() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(ports) = serialport::available_ports() {
        for p in ports {
            match &p.port_type {
                serialport::SerialPortType::UsbPort(info) => out.push(format!(
                    "{}  (USB {:04x}:{:04x}{})",
                    p.port_name,
                    info.vid,
                    info.pid,
                    info.product
                        .as_deref()
                        .map(|s| format!(" {s}"))
                        .unwrap_or_default()
                )),
                _ => out.push(p.port_name.clone()),
            }
        }
    }
    out
}

/// Spawn the reader thread. Errors from opening the port surface synchronously.
pub fn spawn_reader(
    port_name: &str,
    baud: u32,
    tx: Sender<SerialEvent>,
) -> serialport::Result<()> {
    let mut port = serialport::new(port_name, baud)
        .timeout(Duration::from_millis(50))
        .open()?;
    std::thread::Builder::new()
        .name("sigtap-serial".into())
        .spawn(move || {
            let mut parser = Parser::new();
            let mut chunk = [0u8; 8192];
            let mut recs: Vec<Record> = Vec::new();
            loop {
                match port.read(&mut chunk) {
                    Ok(0) => {}
                    Ok(n) => {
                        let ts = now_ns();
                        recs.clear();
                        parser.push(&chunk[..n], &mut recs);
                        for rec in recs.drain(..) {
                            if tx.send(SerialEvent { rec, host_recv_ns: ts }).is_err() {
                                return; // receiver gone — measurement over
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(_) => return,
                }
            }
        })
        .expect("spawn serial thread");
    Ok(())
}
