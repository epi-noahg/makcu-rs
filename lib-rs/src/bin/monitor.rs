//! Live monitor — prints all keyboard, button, and axis events received from a Makcu device.
//! Run with: cargo run --bin monitor -- /dev/cu.usbmodemXXXX

use makcu_rs::{Device, MouseButtonStates};
use std::{env, thread, time::Duration};

fn main() -> anyhow::Result<()> {
    let port = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: monitor <port>  (e.g. /dev/cu.usbmodem... or COM3)");
        eprintln!("Tip:   run `cargo run --bin find_makcu` first to find your port.");
        std::process::exit(1);
    });

    println!("Connecting to {port} ...");
    let dev = Device::new(&port, 4_000_000, Duration::from_millis(100));
    dev.connect()?;
    println!("Connected. Listening for events — press Ctrl+C to quit.\n");

    // Print every unsolicited streaming line (keyboard, axis, mouse text frames).
    dev.set_stream_callback(Some(|line: String| {
        // Skip echo lines and prompts.
        if line.starts_with(">>>") || line.starts_with("km.buttons") || line.starts_with("km.keyboard") || line.starts_with("km.axis") || line.starts_with("km.mouse") {
            // Only print actual data, not the enable-command echoes.
            if !line.ends_with("(1)") && !line.ends_with("(2)") {
                println!("[STREAM] {line}");
            }
            return;
        }
        println!("[STREAM] {line}");
    }));

    // Print every button state change (binary streaming byte).
    dev.set_button_callback(Some(|st: MouseButtonStates| {
        println!(
            "[BUTTONS] L={} R={} M={} S1={} S2={}",
            st.left as u8, st.right as u8, st.middle as u8,
            st.side1 as u8, st.side2 as u8,
        );
    }));

    // Enable keyboard streaming (raw physical input, every frame).
    dev.keyboard_stream(1, Some(1))?;
    // Enable button streaming (raw, every 10ms).
    dev.buttons_stream(1, Some(10))?;

    println!("Streaming active. Type on the connected keyboard or click its mouse.\n");

    // Block forever — callbacks fire from the reader thread.
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}
