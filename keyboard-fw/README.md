# keyboard-fw — MAKCU keyboard firmware (passthrough + injection)

Dual-ESP32-S3 firmware that relays a **real USB keyboard** transparently to a
target PC and overlays `km.*` injection from a control PC — the keyboard analog
of the existing controller passthrough. Compatible with the `lib-rs` client
**without modifying it**.

```
[real keyboard] -> (RIGHT: USB host) --IPC--> (LEFT: USB device) -> [target PC]
                                                     ^
                                       km.* commands (control PC, lib-rs)
```

The model is identical to the gamepad firmware: RIGHT snapshots the real
device's descriptors and relays raw HID reports; LEFT presents the real
descriptors and overlays injection on the raw report. Only the injection
overlay (`km_apply`) and the command parser changed — gamepad stick/button
logic was replaced by keyboard logic.

## Layout

```
core/        Portable C, ZERO platform deps — unit-tested on the host:
  keymap        key names / ASCII <-> HID usage codes
  km_protocol   km.* line parser + tracked-reply framer
  kbd_inject    injection overlay: remap/disable/mask (physical-only),
                OR of physical+injected, 6KRO, press/string timing
  cfgdesc       finds the boot-keyboard IN endpoint in a config descriptor
  km_glue       wires the above to the firmware seam (km_init / km_ingest_raw /
                km_apply / km_reset_injection / km_has_active_injection)
test/        Host unit + integration tests (no hardware, no PlatformIO needed)
left/        ESP-IDF USB-device firmware (fork of MAKCM_ESP32s3_Pass_Left_IDF)
             - src/km_platform.c : esp_timer clock + periodic tick (platform shim)
             - edits: pass_usb_device.c (cache the keyboard EP),
                      main.c (parse cfgdesc on DEVICE_READY)
right/       Arduino+IDF USB-host firmware (fork of MAKCM_ESP32s3_Pass_Right),
             reused verbatim — PassUsbHost is device-agnostic
```

## Host unit tests (run anywhere with a C compiler)

```sh
make test        # builds + runs every test/test_*.c
```

Status: **all green** — keymap, km_protocol, kbd_inject, cfgdesc, and the
full protocol→engine→overlay integration (`test_glue`). These cover the
genuinely tricky logic and are the TDD heart of the project. Also run in CI
(`.github/workflows/rust.yml`, job `keyboard-fw-core`).

## Firmware build & on-hardware verification (needs the ESP-IDF toolchain + hardware)

Not buildable in a plain host environment — requires PlatformIO + ESP-IDF and
two ESP32-S3 boards.

```sh
pio run -d keyboard-fw/left   -e LEFT_KBD     # USB-device side
pio run -d keyboard-fw/right  -e RIGHT_KBD    # USB-host side
# flash both: pio run -t upload -e <env> -d <dir>
```

Integration checklist (Phase 7):
1. Flash LEFT and RIGHT. LEFT -> target PC, a real USB keyboard -> RIGHT.
2. Target PC sees the **real keyboard** (same VID/PID); physical typing passes
   through.
3. Control PC (Rust): `cargo run --bin find_makcu` detects it (via `version()`);
   `cargo run --bin demo_keyboard` → `type_string` / `key_press` appear on the
   target PC; `cargo run --bin monitor` shows the `km.keyboard` stream.
4. `disable_keys(["'a'"])` blocks physical `a`; `remap_key('a','b')` maps a→b;
   injected `key_down('a')` still passes (injection is never filtered).
5. Hot-plug the keyboard on RIGHT — no freeze.

## Deferred (documented, out of scope for v1)

Full HID **report-descriptor** parsing (NKRO / arbitrary layouts) via a new
`FRAME_HID_REPORT_DESC`, consumer-control (media keys), host→keyboard LEDs, and
the two-PC KVM switch. The current overlay targets the standard **boot
keyboard** layout (8-byte report), which covers the vast majority of keyboards.
