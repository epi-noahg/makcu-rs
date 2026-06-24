// km_glue.h — portable wiring between the km.* protocol and the injection core.
//
// Implements the exact seam the existing LEFT firmware expects (km_init,
// km_ingest_raw, km_apply, km_reset_injection, km_has_active_injection), built
// on keymap + km_protocol + kbd_inject + cfgdesc. Fully host-testable: it
// depends only on three platform hooks (provided by the device shim, stubbed
// in tests):
//
//   int      km_uart_write_raw(const void *data, size_t len); // reply channel
//   uint32_t km_now_ms(void);                                 // monotonic ms
//   void     km_platform_start(void);                         // start periodic tick
//
// The device shim arranges for km_periodic() to be called on a timer (~4 ms).
#ifndef KM_GLUE_H
#define KM_GLUE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// ---- seam consumed by the existing firmware ----------------------------------
void km_init(void);                                    // main.c app_main
void km_ingest_raw(const uint8_t *payload, uint16_t len); // ipc.c UART0 line
void km_apply(uint8_t ep_addr, uint8_t *buf, uint16_t len); // pass_usb_device.c
void km_reset_injection(void);                         // main.c DEVICE_GONE
bool km_has_active_injection(void);                    // synth timer gate

// ---- keyboard-specific wiring ------------------------------------------------
// Called once the config descriptor is known (main.c on FRAME_DEVICE_READY):
// records which IN endpoint carries keyboard reports and the report length.
void km_set_kbd_endpoint(uint8_t ep_addr, uint16_t report_len);

// True if ep_addr is the keyboard IN endpoint (pass_usb_device.c cache gate).
bool km_is_kbd_ep(uint8_t ep_addr);

// Periodic tick — drives press/string timing. Called by the device timer
// (or directly in tests).
void km_periodic(void);

// ---- platform hooks (provided elsewhere) -------------------------------------
extern int      km_uart_write_raw(const void *data, size_t len);
extern uint32_t km_now_ms(void);
extern void     km_platform_start(void);

#endif // KM_GLUE_H
