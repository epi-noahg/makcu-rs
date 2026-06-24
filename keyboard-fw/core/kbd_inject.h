// kbd_inject.h — portable keyboard injection overlay.
//
// Holds the injected key state driven by km.* commands and overlays it onto the
// raw HID boot-keyboard report relayed from the physical keyboard. Mirrors the
// gamepad km_apply() model: physical filters (remap/disable/mask) apply to the
// PHYSICAL report only; injected keys are OR-ed in and never filtered.
//
// Boot report layout (8 bytes, or 9 with a leading report-ID):
//   [report-id?] [modifiers] [reserved] [k0] [k1] [k2] [k3] [k4] [k5]
//
// No platform dependencies — host-testable with an injected clock.
#ifndef KBD_INJECT_H
#define KBD_INJECT_H

#include <stdbool.h>
#include <stdint.h>

#define KBD_DEFAULT_HOLD_MS 50   // km.press() with no explicit hold
#define KBD_STR_HOLD_MS     10   // km.string per-char hold
#define KBD_STR_GAP_MS      10   // km.string inter-char gap
#define KBD_MAX_HELD        12   // persistent injected keys (km.down)
#define KBD_MAX_TIMED       12   // timed injected presses (km.press / string)
#define KBD_STR_MAX         256  // km.string max chars

typedef struct { uint8_t code; bool shift; bool used; } KbdHeld;
typedef struct { uint8_t code; bool shift; uint32_t release_at; bool used; } KbdTimed;

typedef struct {
    // report layout
    bool     has_report_id;
    uint16_t report_len;

    // injected state
    uint8_t  inj_mods;                 // persistent modifier bits (km.down on a modifier)
    KbdHeld  held[KBD_MAX_HELD];       // persistent non-modifier keys
    KbdTimed timed[KBD_MAX_TIMED];     // timed presses

    // km.string state machine
    char     str_buf[KBD_STR_MAX + 1];
    int      str_len;
    int      str_idx;
    uint32_t str_next_due;
    bool     str_active;

    // physical-only filters (code space)
    uint8_t  disabled[32];             // bitmap
    uint8_t  masked[32];               // bitmap
    uint8_t  remap_to[256];            // 0 = passthrough

    // last physical report (for isdown)
    uint8_t  phys_codes[32];           // bitmap of physical-active codes

    uint32_t now;
} KbdState;

void kbd_init(KbdState *st, bool has_report_id, uint16_t report_len);
void kbd_set_layout(KbdState *st, bool has_report_id, uint16_t report_len);

// km.down / km.up — persistent. `shift` asserts Left-Shift alongside the key
// (used for uppercase letters / shifted symbols).
void kbd_down(KbdState *st, uint8_t code, bool shift);
void kbd_up(KbdState *st, uint8_t code, bool shift);

// km.press — timed press; auto-releases after hold_ms (0 => KBD_DEFAULT_HOLD_MS).
void kbd_press(KbdState *st, uint8_t code, bool shift, uint32_t hold_ms);

// km.string — type an ASCII string with internal timing.
void kbd_type(KbdState *st, const char *text);

// km.init — release all injected keys (filters/remaps untouched).
void kbd_release_all(KbdState *st);

// Reset everything injection-related; called on DEVICE_GONE.
void kbd_reset_injection(KbdState *st);

// km.disable / km.mask / km.remap (physical-only). remap dst==0 clears.
void kbd_set_disabled(KbdState *st, uint8_t code, bool on);
void kbd_set_masked(KbdState *st, uint8_t code, bool on);
void kbd_set_remap(KbdState *st, uint8_t src, uint8_t dst);

// Query whether a code is currently disabled (for km.disable() listing).
bool kbd_is_disabled(KbdState *st, uint8_t code);

// Advance the injected clock; releases expired presses, drives string typing.
void kbd_tick(KbdState *st, uint32_t now_ms);

// Record the latest physical report (for km.isdown). Does not mutate it.
void kbd_set_physical(KbdState *st, const uint8_t *report, uint16_t len);

// Overlay injection onto a live boot-keyboard report in place.
void kbd_overlay(KbdState *st, uint8_t *report, uint16_t len);

// km.isdown — true if the key is currently down (injected OR physical).
bool kbd_isdown(KbdState *st, uint8_t code);

// True if any injection is currently asserted (drives the synth timer).
bool kbd_is_active(KbdState *st);

#endif // KBD_INJECT_H
