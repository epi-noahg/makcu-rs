# kbd-latency — keyboard firmware latency & jitter meter

Measures how fast and how *consistently* the MAKCU keyboard firmware moves a
keystroke from the keyboard to the target PC — so you can tell whether a
firmware change made it better or worse. macOS host tool, paired with a special
firmware build that timestamps every report at the lowest level.

```
 real keyboard ──USB──▶ RIGHT ──IPC──▶ LEFT ─┬─ USB ───▶  this Mac  (the emulated keyboard)
                                             │
                                             └─ UART0 ─▶ CH343 ─USB─▶  this Mac  (the "middle" serial)
```

Both cables land on **one** computer (this Mac). The tool reads the emulated
keyboard's HID reports *and* the firmware's timestamp stream, lines them up, and
prints the latency distribution + jitter.

## What it measures

Every keyboard report produces two numbers:

| Metric | Span | Clock | Precision |
|---|---|---|---|
| **Firmware internal** | deframe of the report from RIGHT → USB submit to the PC | one LEFT clock (`esp_timer`, µs) | **exact** — no cross-clock, this is the number you optimise |
| **End-to-end** | lowest-level capture in LEFT → macOS HID delivery | LEFT clock aligned to the host clock | absolute carries a *constant* serial-path offset; **jitter is exact** |

Plus **transport** = end-to-end − firmware-internal (USB submit → host delivery,
i.e. the part the firmware does not control).

Jitter is reported as the sample standard deviation. A constant offset (such as
the fixed serial-channel delay used for clock alignment) cancels out of a
standard deviation, so **the jitter figure is trustworthy even though the
absolute end-to-end number has a fixed bias**. That is exactly what you want:
"the smaller the jitter, the better the firmware."

## How the precision is achieved

* **Lowest-level capture.** LEFT stamps `esp_timer_get_time()` the instant a raw
  keyboard IPC frame is CRC-validated in `ipc_feed`, before any dispatch or
  injection work — see `sigtap_mark_capture()` wired in `left/src/ipc.c`.
* **Exact submit time.** A second stamp is taken at the real `usbd_edpt_xfer`
  call, at *both* places a report can be submitted (immediate, and the
  coalesce-promote in `pass_driver_xfer_cb`), so the firmware-internal latency is
  a single-clock subtraction with no measurement apparatus in the path.
* **No perturbation.** The hot path only stamps + copies into a ring; framing and
  UART output happen on a separate low-priority drain task.
* **Two-clock alignment without cross-MCU sync.** LEFT emits a `SYNC` beacon
  every 50 ms. The host fits LEFT's clock to its own with a global least-squares
  *rate* (handles the few-ppm crystal difference) and a recent-median *offset*
  (tracks slow drift, rejects serial spikes). Within a 50 ms beacon interval the
  residual error is a couple of nanoseconds.
* **Kernel timestamps on the host.** Reports are captured with
  `IOHIDDeviceRegisterInputReportWithTimeStampCallback`, whose timestamp is the
  kernel's mach-time stamp for the report — the most precise "the computer got
  it" instant available without a kernel extension.
* **Content + order correlation.** Each tap carries the raw report bytes, matched
  1:1 against the HID stream; a lost report on either path is resynced, not
  fatal.

## 1. Build the instrumented firmware

The signal tap is gated by `-DSIGTAP=1` (default off → zero cost in the gameplay
build). A ready-made PlatformIO env turns it on and silences the ASCII logs so
the serial carries only binary tap records:

```sh
pio run   -d keyboard-fw/left -e LEFT_KBD_SIGTAP
pio run -t upload -d keyboard-fw/left -e LEFT_KBD_SIGTAP
# RIGHT is unchanged:
pio run -t upload -d keyboard-fw/right -e RIGHT_KBD
```

> Run latency tests with **no `km.*` injection active** — injection rewrites
> report bytes and the synth timer emits extra reports, which is not what you are
> trying to measure. Physical typing only.

## 2. Build the host tool

```sh
cd keyboard-fw/latency-test
cargo build --release          # target/release/kbd-latency
cargo test                     # 15 unit + integration tests, no hardware needed
```

Grant the terminal **Input Monitoring** (System Settings → Privacy & Security →
Input Monitoring) so it can read HID reports.

## 3. Run a measurement

Find the emulated keyboard's USB VID/PID (it mirrors the real keyboard's):

```sh
./target/release/kbd-latency --list          # HID keyboards
./target/release/kbd-latency --list-serial   # serial ports (CH343 is QinHeng 0x1A86)
```

Then measure (CH343 serial is autodetected; override with `--serial`):

```sh
./target/release/kbd-latency --vid 0x046d --pid 0xc31c --seconds 60 --csv run.csv
```

Type on the keyboard during the run. For the cleanest numbers, prevent the
keystrokes from also reaching macOS by seizing the device (needs sudo):

```sh
sudo ./target/release/kbd-latency --vid 0x046d --pid 0xc31c --seize --seconds 60
```

Without `--seize`, focus a throwaway text field so the typing goes somewhere
harmless.

### Reading the output

```
FIRMWARE INTERNAL  (deframe → USB submit, single LEFT clock — exact):
  n=4123   min=   180.0 med=   240.0 mean=   244.1 p95=   310.0 p99=   470.0 max=  900.0 jitter(sd)=   45.2  [µs]

END-TO-END  (capture → host HID delivery; absolute has a constant
serial-path offset, but JITTER is exact):
  n=4101   min=  ...    med=  ...    ...                                           jitter(sd)=  ...    [µs]
```

* Drive the **firmware-internal median and jitter down** — that is the lever the
  firmware controls. Compare runs across firmware changes.
* The **end-to-end jitter** is the real-world consistency the game sees (USB +
  OS included). Its absolute value includes a fixed serial offset, so compare
  *jitter* across runs, not the absolute mean, unless you calibrate the offset.
* `serial health` / `unmatched` flag data loss; `clock alignment` shows the
  measured crystal drift between the two boards.

## Wire format (firmware ↔ host contract)

Records on UART0, little-endian, framed `AA 55` + body + CRC16 (CCITT-FALSE):

```
magic0 magic1 | version type seq(u32) t_cap(i64) t_sub(i64) t_emit(i64) rlen | report[rlen] | crc16
```

Defined once in `left/src/sigtap.{h,c}` and parsed in `src/proto.rs`; the CRC and
field offsets are covered by unit tests (`proto::tests`).

## Limitations / honest caveats

* End-to-end **absolute** latency carries a constant serial-path delay bias
  (jitter is unaffected). Calibrate it once if you need the true absolute number.
* Targets the standard **boot keyboard** (8-byte report, no report ID), matching
  the firmware. NKRO / report-ID layouts would need report-descriptor parsing.
* Firmware ring overruns (only under pathological rates) are dropped silently;
  CRC drops and serial byte loss show up as `lost` seq gaps.
* RIGHT-side capture (the true first electrical edge on the keyboard) is not
  instrumented — RIGHT's USB-serial is dead in host mode, so the lowest host-
  visible point is LEFT's deframe. The RIGHT→LEFT IPC leg is therefore part of
  the "before capture" path, not measured here.
