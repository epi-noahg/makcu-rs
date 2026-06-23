//! Demonstrates keyboard usage of the `makcu-rs` crate.

use makcu_rs::Device;
use std::time::Duration;

fn main() -> anyhow::Result<()> {
    Device::set_debug(true);

    // -> /dev/cu.usbmodem5B906937721
    // -> /dev/tty.usbmodem5B906937721
    let dev = Device::new("/dev/cu.usbmodem5B906937721", 115_200, Duration::from_millis(10));
    dev.connect()?;

    // --- Type a string automatically (handles shift for uppercase/symbols) ---
    dev.type_string("Hello, World!")?;

    // // --- Individual key press with default random timing (35-75ms) ---
    // dev.key_press("'enter'", None, None)?;

    // // --- Key press with explicit hold duration ---
    dev.key_press("'a'", Some(50), None)?;

    // // --- Key press with base duration + randomization ---
    // dev.key_press("'d'", Some(50), Some(10))?;

    // // --- Hold modifier + press key (Ctrl+C) ---
    // dev.key_down("'ctrl'")?;
    // dev.key_press("'c'", None, None)?;
    // dev.key_up("'ctrl'")?;

    // // --- Ctrl+Shift+Esc via batch ---
    // dev.batch()
    //     .key_down("'ctrl'")
    //     .key_down("'shift'")
    //     .key_press("'esc'")
    //     .key_up("'shift'")
    //     .key_up("'ctrl'")
    //     .run()?;

    // // --- Check if a key is currently held ---
    // let state = dev.is_key_down("'shift'")?;
    // println!("Shift held: {state}");

    // // --- Disable a key (block it from reaching host) ---
    // dev.disable_keys(&["'f1'", "'alt'", "'win'"])?;
    // println!("F1, Alt, Win disabled");

    // // --- Re-enable a single key ---
    // dev.set_key_disabled("'f1'", false)?;
    // println!("F1 re-enabled");

    // // --- Query which keys are disabled ---
    // let disabled = dev.get_disabled_keys()?;
    // println!("Currently disabled: {disabled}");

    // // --- Remap 'a' -> 'b' (physical input only) ---
    // dev.remap_key("'a'", "'b'")?;
    // println!("'a' remapped to 'b'");

    // // --- Clear the remap ---
    // dev.remap_key("'a'", "0")?;

    // // --- Mask a key ---
    // dev.mask_key("'capslock'", true)?;

    // // --- Clear all keyboard state ---
    // dev.keyboard_init()?;
    // println!("Keyboard state cleared");

    // // --- Stream raw keyboard input (mode 1, every 100 frames) ---
    dev.keyboard_stream(1, Some(100))?;
    std::thread::sleep(Duration::from_secs(2));

    // // --- Stop streaming ---
    dev.keyboard_stream(0, None)?;

    dev.disconnect();
    Ok(())
}
