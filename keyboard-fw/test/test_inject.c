// Phase 3 — tests for core/kbd_inject (injection overlay).
#include "test_framework.h"
#include "kbd_inject.h"
#include "keymap.h"
#include <string.h>

#define MODS 0
#define K0   2  // first key slot in an 8-byte boot report

static void clear_report(uint8_t r[8]) { memset(r, 0, 8); }

static void set_phys_key(KbdState *st, uint8_t code) {
    uint8_t r[8]; clear_report(r);
    r[K0] = code;
    kbd_set_physical(st, r, 8);
}

// Build the overlaid report for an empty physical report.
static void overlay_empty(KbdState *st, uint8_t out[8]) {
    clear_report(out);
    kbd_overlay(st, out, 8);
}

static bool report_has_key(const uint8_t r[8], uint8_t code) {
    for (int i = K0; i < 8; ++i) if (r[i] == code) return true;
    return false;
}

static void test_down_up_letter(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];

    kbd_down(&st, 4, false);            // 'a'
    overlay_empty(&st, r);
    ASSERT_TRUE(report_has_key(r, 4));
    ASSERT_EQ_INT(0, r[MODS]);

    kbd_up(&st, 4, false);
    overlay_empty(&st, r);
    ASSERT_FALSE(report_has_key(r, 4));
}

static void test_down_modifier(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];

    kbd_down(&st, 224, false);          // left ctrl
    overlay_empty(&st, r);
    ASSERT_EQ_INT(0x01, r[MODS]);       // bit 0

    kbd_up(&st, 224, false);
    overlay_empty(&st, r);
    ASSERT_EQ_INT(0x00, r[MODS]);
}

static void test_down_uppercase(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];

    kbd_down(&st, 4, true);             // 'A' = a + shift
    overlay_empty(&st, r);
    ASSERT_TRUE(report_has_key(r, 4));
    ASSERT_EQ_INT(0x02, r[MODS]);       // left shift bit

    kbd_up(&st, 4, true);
    overlay_empty(&st, r);
    ASSERT_EQ_INT(0x00, r[MODS]);
    ASSERT_FALSE(report_has_key(r, 4));
}

static void test_or_physical(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8]; clear_report(r);
    r[K0] = 5;                          // physical 'b'
    kbd_down(&st, 4, false);            // injected 'a'
    kbd_overlay(&st, r, 8);
    ASSERT_TRUE(report_has_key(r, 4));
    ASSERT_TRUE(report_has_key(r, 5));
}

static void test_disable_physical_only(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];

    // physical 'a' is dropped when disabled
    kbd_set_disabled(&st, 4, true);
    clear_report(r); r[K0] = 4;
    kbd_overlay(&st, r, 8);
    ASSERT_FALSE(report_has_key(r, 4));

    // but injected 'a' still passes (injection is not filtered)
    kbd_down(&st, 4, false);
    overlay_empty(&st, r);
    ASSERT_TRUE(report_has_key(r, 4));
}

static void test_mask_physical_only(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];
    kbd_set_masked(&st, 6, true);       // 'c'
    clear_report(r); r[K0] = 6;
    kbd_overlay(&st, r, 8);
    ASSERT_FALSE(report_has_key(r, 6));
}

static void test_remap_physical(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];

    kbd_set_remap(&st, 4, 5);           // physical 'a' -> 'b'
    clear_report(r); r[K0] = 4;
    kbd_overlay(&st, r, 8);
    ASSERT_FALSE(report_has_key(r, 4));
    ASSERT_TRUE(report_has_key(r, 5));

    // injected 'a' is NOT remapped
    kbd_down(&st, 4, false);
    overlay_empty(&st, r);
    ASSERT_TRUE(report_has_key(r, 4));

    // clear remap -> passthrough
    kbd_set_remap(&st, 4, 0);
    kbd_up(&st, 4, false);
    clear_report(r); r[K0] = 4;
    kbd_overlay(&st, r, 8);
    ASSERT_TRUE(report_has_key(r, 4));
}

static void test_6kro_overflow(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];
    for (uint8_t c = 4; c < 11; ++c) kbd_down(&st, c, false); // 7 keys
    overlay_empty(&st, r);
    for (int i = K0; i < 8; ++i) ASSERT_EQ_INT(0x01, r[i]);   // ErrorRollOver
}

static void test_init_releases(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];
    kbd_down(&st, 4, false);
    kbd_down(&st, 224, false);
    ASSERT_TRUE(kbd_is_active(&st));
    kbd_release_all(&st);
    overlay_empty(&st, r);
    ASSERT_EQ_INT(0, r[MODS]);
    ASSERT_FALSE(report_has_key(r, 4));
    ASSERT_FALSE(kbd_is_active(&st));
}

static void test_press_timed(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];
    kbd_tick(&st, 0);
    kbd_press(&st, 4, false, 50);
    ASSERT_TRUE(kbd_is_active(&st));
    overlay_empty(&st, r);
    ASSERT_TRUE(report_has_key(r, 4));

    kbd_tick(&st, 49);
    overlay_empty(&st, r);
    ASSERT_TRUE(report_has_key(r, 4));

    kbd_tick(&st, 50);                  // hold elapsed -> released
    overlay_empty(&st, r);
    ASSERT_FALSE(report_has_key(r, 4));
    ASSERT_FALSE(kbd_is_active(&st));
}

static void test_type_string(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];
    kbd_tick(&st, 0);
    kbd_type(&st, "ab");                // 'a'=4, 'b'=5

    kbd_tick(&st, 0);                   // press 'a'
    ASSERT_TRUE(kbd_isdown(&st, 4));

    kbd_tick(&st, KBD_STR_HOLD_MS);     // release 'a'
    ASSERT_FALSE(kbd_isdown(&st, 4));

    kbd_tick(&st, KBD_STR_HOLD_MS + KBD_STR_GAP_MS); // press 'b'
    ASSERT_TRUE(kbd_isdown(&st, 5));

    kbd_tick(&st, 2 * KBD_STR_HOLD_MS + KBD_STR_GAP_MS); // release 'b'
    ASSERT_FALSE(kbd_isdown(&st, 5));
    ASSERT_FALSE(kbd_is_active(&st));
    (void)r;
}

static void test_string_shift(void) {
    KbdState st; kbd_init(&st, false, 8);
    uint8_t r[8];
    kbd_tick(&st, 0);
    kbd_type(&st, "A");                 // needs shift
    kbd_tick(&st, 0);
    overlay_empty(&st, r);
    ASSERT_TRUE(report_has_key(r, 4));
    ASSERT_EQ_INT(0x02, r[MODS]);       // shift asserted during the press
}

static void test_isdown(void) {
    KbdState st; kbd_init(&st, false, 8);
    kbd_down(&st, 4, false);
    ASSERT_TRUE(kbd_isdown(&st, 4));
    ASSERT_FALSE(kbd_isdown(&st, 5));
    set_phys_key(&st, 5);
    ASSERT_TRUE(kbd_isdown(&st, 5));    // physical counts too
    kbd_down(&st, 224, false);
    ASSERT_TRUE(kbd_isdown(&st, 224));  // modifier
}

static void test_report_id_offset(void) {
    KbdState st; kbd_init(&st, true, 9); // 9-byte report w/ leading id
    uint8_t r[9]; memset(r, 0, 9);
    r[0] = 0x01;                         // report id
    kbd_down(&st, 4, true);              // 'A'
    kbd_overlay(&st, r, 9);
    ASSERT_EQ_INT(0x01, r[0]);           // id preserved
    ASSERT_EQ_INT(0x02, r[1]);           // modifiers at offset 1
    bool found = false;
    for (int i = 3; i < 9; ++i) if (r[i] == 4) found = true; // keys at off+2=3..
    ASSERT_TRUE(found);
}

int main(void) {
    TF_BEGIN();
    TF_RUN(test_down_up_letter);
    TF_RUN(test_down_modifier);
    TF_RUN(test_down_uppercase);
    TF_RUN(test_or_physical);
    TF_RUN(test_disable_physical_only);
    TF_RUN(test_mask_physical_only);
    TF_RUN(test_remap_physical);
    TF_RUN(test_6kro_overflow);
    TF_RUN(test_init_releases);
    TF_RUN(test_press_timed);
    TF_RUN(test_type_string);
    TF_RUN(test_string_shift);
    TF_RUN(test_isdown);
    TF_RUN(test_report_id_offset);
    return TF_END();
}
