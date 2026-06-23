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

    for port_info in &ports {
        let name = &port_info.port_name;
        let dev = Device::new(name, 115_200, Duration::from_millis(300));

        match dev.connect() {
            Err(_) => {}
            Ok(()) => {
                match dev.version() {
                    Ok(ver) => {
                        println!("  {name:<20} — MAKCU  firmware: {ver}");
                        makcu_ports.push(name.clone());
                    }
                    Err(_) => {
                        println!("  {name:<20} — not a Makcu");
                    }
                }
                dev.disconnect();
            }
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
