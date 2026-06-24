//! Scans all available serial ports and prints the ones that respond as a Makcu device.
//! Run with: cargo run --bin find_makcu

use makcu_rs::Device;
use std::time::Duration;

fn main() {
    let ports = match serialport::available_ports() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to list serial ports: {e}");
            return;
        }
    };

    if ports.is_empty() {
        println!("No serial ports found.");
        return;
    }

    println!("Scanning {} port(s)...\n", ports.len());

    let mut makcu_ports: Vec<String> = Vec::new();

    // The MAKCM gamepad/keyboard passthrough firmware runs its KM command UART at
    // 4,000,000 baud; the original kmbox B/B+ used 115200. Probe both.
    const BAUDS: [u32; 2] = [4_000_000, 115_200];

    for port_info in &ports {
        let name = &port_info.port_name;
        let mut found = false;
        for &baud in &BAUDS {
            let dev = Device::new(name, baud, Duration::from_millis(300));
            if dev.connect().is_err() {
                break; // port unusable at all → no point trying other bauds
            }
            let ver = dev.version();
            dev.disconnect();
            if let Ok(ver) = ver {
                println!("  {name:<20} — MAKCU @ {baud:>7} baud, firmware: {}", ver.trim());
                makcu_ports.push(name.clone());
                found = true;
                break;
            }
        }
        if !found {
            println!("  {name:<20} — not a Makcu");
        }
    }

    println!();
    if makcu_ports.is_empty() {
        println!("No Makcu devices found.");
    } else {
        println!("{} Makcu device(s) found:", makcu_ports.len());
        for port in &makcu_ports {
            println!("  -> {port}");
        }
    }
}
