// kbd_inject.c — see kbd_inject.h.
#include "kbd_inject.h"
#include "keymap.h"

#include <string.h>

#define LSHIFT_BIT 0x02   // 1 << (225 - 224)

// ---- bitmap helpers (256-bit, 32 bytes) -------------------------------------
static inline void bm_set(uint8_t *bm, uint8_t code)   { bm[code >> 3] |= (uint8_t)(1u << (code & 7)); }
static inline void bm_clr(uint8_t *bm, uint8_t code)   { bm[code >> 3] &= (uint8_t)~(1u << (code & 7)); }
static inline bool bm_test(const uint8_t *bm, uint8_t code) { return (bm[code >> 3] >> (code & 7)) & 1; }

static inline uint8_t mod_bit(uint8_t code) { return (uint8_t)(1u << (code - 224)); }

void kbd_set_layout(KbdState *st, bool has_report_id, uint16_t report_len) {
    st->has_report_id = has_report_id;
    st->report_len = report_len;
}

void kbd_init(KbdState *st, bool has_report_id, uint16_t report_len) {
    memset(st, 0, sizeof(*st));
    kbd_set_layout(st, has_report_id, report_len);
}

// ---- injected state ----------------------------------------------------------
void kbd_down(KbdState *st, uint8_t code, bool shift) {
    if (code == 0) return;
    if (key_is_modifier(code)) { st->inj_mods |= mod_bit(code); return; }
    // already held? update shift, else take a free slot
    for (int i = 0; i < KBD_MAX_HELD; ++i)
        if (st->held[i].used && st->held[i].code == code) { st->held[i].shift = shift; return; }
    for (int i = 0; i < KBD_MAX_HELD; ++i)
        if (!st->held[i].used) { st->held[i].used = true; st->held[i].code = code; st->held[i].shift = shift; return; }
}

void kbd_up(KbdState *st, uint8_t code, bool shift) {
    (void)shift;
    if (code == 0) return;
    if (key_is_modifier(code)) { st->inj_mods &= (uint8_t)~mod_bit(code); return; }
    for (int i = 0; i < KBD_MAX_HELD; ++i)
        if (st->held[i].used && st->held[i].code == code) st->held[i].used = false;
}

void kbd_press(KbdState *st, uint8_t code, bool shift, uint32_t hold_ms) {
    if (code == 0) return;
    if (hold_ms == 0) hold_ms = KBD_DEFAULT_HOLD_MS;
    for (int i = 0; i < KBD_MAX_TIMED; ++i) {
        if (!st->timed[i].used) {
            st->timed[i].used = true;
            st->timed[i].code = code;
            st->timed[i].shift = shift;
            st->timed[i].release_at = st->now + hold_ms;
            return;
        }
    }
}

void kbd_type(KbdState *st, const char *text) {
    if (!text) return;
    size_t n = strlen(text);
    if (n > KBD_STR_MAX) n = KBD_STR_MAX;
    memcpy(st->str_buf, text, n);
    st->str_buf[n] = '\0';
    st->str_len = (int)n;
    st->str_idx = 0;
    st->str_next_due = st->now;
    st->str_active = (n > 0);
}

void kbd_release_all(KbdState *st) {
    st->inj_mods = 0;
    memset(st->held, 0, sizeof(st->held));
    memset(st->timed, 0, sizeof(st->timed));
    st->str_active = false;
    st->str_len = st->str_idx = 0;
}

void kbd_reset_injection(KbdState *st) {
    kbd_release_all(st);
    memset(st->phys_codes, 0, sizeof(st->phys_codes));
    // filters/remaps intentionally preserved (config persists across hot-swap)
}

void kbd_set_disabled(KbdState *st, uint8_t code, bool on) {
    if (on) bm_set(st->disabled, code); else bm_clr(st->disabled, code);
}
void kbd_set_masked(KbdState *st, uint8_t code, bool on) {
    if (on) bm_set(st->masked, code); else bm_clr(st->masked, code);
}
void kbd_set_remap(KbdState *st, uint8_t src, uint8_t dst) {
    st->remap_to[src] = dst;  // dst==0 clears (passthrough)
}

bool kbd_is_disabled(KbdState *st, uint8_t code) {
    return bm_test(st->disabled, code);
}

// Whether a string char is currently mid-press (so we don't advance early).
static bool str_char_pressed(const KbdState *st) {
    for (int i = 0; i < KBD_MAX_TIMED; ++i)
        if (st->timed[i].used && st->timed[i].release_at > st->now) {
            // any active timed press blocks string advance only if it is "ours";
            // we tag string presses by being the sole driver — simplest: treat
            // all active timed presses as blockers for ordered typing.
            return true;
        }
    return false;
}

void kbd_tick(KbdState *st, uint32_t now_ms) {
    st->now = now_ms;
    // release expired timed presses
    for (int i = 0; i < KBD_MAX_TIMED; ++i)
        if (st->timed[i].used && now_ms >= st->timed[i].release_at)
            st->timed[i].used = false;

    // drive string typing: press next char when due and nothing held
    if (st->str_active) {
        if (st->str_idx >= st->str_len) {
            if (!str_char_pressed(st)) st->str_active = false;
        } else if (now_ms >= st->str_next_due && !str_char_pressed(st)) {
            uint8_t code; bool shift;
            char c = st->str_buf[st->str_idx];
            if (ascii_to_hid(c, &code, &shift)) {
                kbd_press(st, code, shift, KBD_STR_HOLD_MS);
            }
            st->str_idx++;
            st->str_next_due = now_ms + KBD_STR_HOLD_MS + KBD_STR_GAP_MS;
        }
    }
}

void kbd_set_physical(KbdState *st, const uint8_t *report, uint16_t len) {
    memset(st->phys_codes, 0, sizeof(st->phys_codes));
    int off = st->has_report_id ? 1 : 0;
    if (len < (uint16_t)(off + 1)) return;
    uint8_t mods = report[off];
    for (int b = 0; b < 8; ++b)
        if (mods & (1u << b)) bm_set(st->phys_codes, (uint8_t)(224 + b));
    for (int i = off + 2; i < len && i < off + 8; ++i)
        if (report[i]) bm_set(st->phys_codes, report[i]);
}

// Compute the injected contribution (mods byte + non-modifier key bitmap).
static void injected_set(const KbdState *st, uint8_t *mods_out, uint8_t *keys_bm) {
    uint8_t mods = st->inj_mods;
    memset(keys_bm, 0, 32);
    for (int i = 0; i < KBD_MAX_HELD; ++i) {
        if (!st->held[i].used) continue;
        bm_set(keys_bm, st->held[i].code);
        if (st->held[i].shift) mods |= LSHIFT_BIT;
    }
    for (int i = 0; i < KBD_MAX_TIMED; ++i) {
        if (!st->timed[i].used) continue;
        uint8_t c = st->timed[i].code;
        if (key_is_modifier(c)) mods |= mod_bit(c);
        else bm_set(keys_bm, c);
        if (st->timed[i].shift) mods |= LSHIFT_BIT;
    }
    *mods_out = mods;
}

void kbd_overlay(KbdState *st, uint8_t *report, uint16_t len) {
    int off = st->has_report_id ? 1 : 0;
    if (len < (uint16_t)(off + 1)) return;

    uint8_t merged_keys[32];
    uint8_t merged_mods;
    injected_set(st, &merged_mods, merged_keys);

    // physical modifiers (filtered), then physical key slots (filtered + remapped)
    uint8_t phys_mods = report[off];
    for (int b = 0; b < 8; ++b) {
        if (!(phys_mods & (1u << b))) continue;
        uint8_t code = (uint8_t)(224 + b);
        if (bm_test(st->disabled, code) || bm_test(st->masked, code)) continue;
        merged_mods |= (uint8_t)(1u << b);
    }
    for (int i = off + 2; i < len && i < off + 8; ++i) {
        uint8_t c = report[i];
        if (c == 0) continue;
        if (bm_test(st->disabled, c) || bm_test(st->masked, c)) continue;
        uint8_t rc = st->remap_to[c] ? st->remap_to[c] : c;
        if (key_is_modifier(rc)) merged_mods |= mod_bit(rc);
        else bm_set(merged_keys, rc);
    }

    // rebuild report
    report[off] = merged_mods;
    if (off + 1 < len) report[off + 1] = 0;        // reserved byte

    // count + collect non-modifier keys
    uint8_t keys[8]; int n = 0;
    for (int code = 1; code < 256; ++code) {
        if (code >= 224 && code <= 231) continue;  // modifiers not in slots
        if (bm_test(merged_keys, (uint8_t)code)) {
            if (n < 6) keys[n] = (uint8_t)code;
            n++;
        }
    }
    int slot0 = off + 2;
    for (int i = 0; i < 6; ++i) {
        int idx = slot0 + i;
        if (idx >= len) break;
        if (n > 6)      report[idx] = 0x01;          // ErrorRollOver (6KRO)
        else            report[idx] = (i < n) ? keys[i] : 0;
    }
}

bool kbd_isdown(KbdState *st, uint8_t code) {
    if (key_is_modifier(code)) {
        if (st->inj_mods & mod_bit(code)) return true;
    } else {
        for (int i = 0; i < KBD_MAX_HELD; ++i)
            if (st->held[i].used && st->held[i].code == code) return true;
        for (int i = 0; i < KBD_MAX_TIMED; ++i)
            if (st->timed[i].used && st->timed[i].code == code) return true;
    }
    return bm_test(st->phys_codes, code);
}

bool kbd_is_active(KbdState *st) {
    if (st->inj_mods) return true;
    for (int i = 0; i < KBD_MAX_HELD; ++i) if (st->held[i].used) return true;
    for (int i = 0; i < KBD_MAX_TIMED; ++i) if (st->timed[i].used) return true;
    if (st->str_active) return true;
    return false;
}
