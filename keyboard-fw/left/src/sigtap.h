// sigtap.h — "signal tap": low-overhead, raw timestamping of every keyboard
// report as it travels through LEFT, for latency / jitter measurement.
//
// Gated entirely by the SIGTAP build flag (default 0 → every call below is an
// inline no-op, so the gameplay build pays NOTHING — no extra clock reads, no
// branches, no RAM). Enable with `-DSIGTAP=1` (see env:LEFT_KBD_SIGTAP).
//
// When enabled, LEFT emits framed binary records on UART0 (the CH343 bridge,
// the "middle" serial the measurement PC also sees). Two record kinds:
//
//   TAP  — one per keyboard report ACTUALLY submitted to the target PC,
//          carrying the lowest-level capture time (the instant the raw IPC
//          frame from RIGHT was deframed), the USB-submit time, and the raw
//          report bytes (post-overlay = exactly what the host receives).
//   SYNC — a 50 ms beacon carrying only a fresh firmware timestamp, so the
//          host can align LEFT's esp_timer clock to its own clock (slope +
//          offset) without any cross-MCU clock sync.
//
// Wire format (little-endian), framed so the host can resync past stray bytes
// (e.g. km.version() ASCII replies) and reject corruption:
//
//   off field      size  meaning
//   0   magic0     1     0xAA
//   1   magic1     1     0x55
//   2   version    1     SIGTAP_VERSION
//   3   type       1     SIGTAP_TYPE_TAP | SIGTAP_TYPE_SYNC
//   4   seq        4     u32 LE, monotonic across all records (drop detection)
//   8   t_cap_us   8     i64 LE, esp_timer_get_time() at deframe (TAP); 0 (SYNC)
//   16  t_sub_us   8     i64 LE, esp_timer_get_time() at usbd_edpt_xfer (TAP); 0
//   24  t_emit_us  8     i64 LE, esp_timer_get_time() just before UART write
//   32  rlen       1     report length (TAP); 0 (SYNC)
//   33  report     rlen  raw report bytes
//   ..  crc16      2     u16 LE, CCITT-FALSE over bytes [2 .. 32+rlen]
//
// The hot path only stamps a clock value and copies the record into a ring;
// framing + UART output happen on a low-priority drain task, so measurement
// never perturbs the path it measures.

#pragma once
#include <stdint.h>
#include <stddef.h>

#ifndef SIGTAP
#define SIGTAP 0
#endif

#define SIGTAP_MAGIC0      0xAAu
#define SIGTAP_MAGIC1      0x55u
#define SIGTAP_VERSION     0x01u
#define SIGTAP_TYPE_TAP    1u
#define SIGTAP_TYPE_SYNC   2u
#define SIGTAP_MAX_REPORT  64u   // full-speed max packet size — captures the whole
                                 // keyboard report (boot=8B, NKRO/extended up to 64B)

#if SIGTAP

// Start the drain task + SYNC beacon timer. Call once from app_main.
void    sigtap_init(void);

// Lowest-level hook: stamp "now" the instant a raw keyboard frame is in hand.
// Reads esp_timer internally so the off build evaluates no clock at all.
void    sigtap_mark_capture(void);

// Read back the most recent capture stamp (same task as mark_capture).
int64_t sigtap_last_capture(void);

// Record one report that was actually submitted to the host.
void    sigtap_report(int64_t t_cap_us, int64_t t_sub_us,
                      const uint8_t *report, uint16_t len);

// Note a report dropped by IN coalescing before it ever went out.
void    sigtap_note_drop(void);

#else  // SIGTAP disabled — every call compiles to nothing.

static inline void    sigtap_init(void) {}
static inline void    sigtap_mark_capture(void) {}
static inline int64_t sigtap_last_capture(void) { return 0; }
static inline void    sigtap_report(int64_t t_cap_us, int64_t t_sub_us,
                                    const uint8_t *report, uint16_t len) {
    (void)t_cap_us; (void)t_sub_us; (void)report; (void)len;
}
static inline void    sigtap_note_drop(void) {}

#endif
