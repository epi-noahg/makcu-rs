// Phase 4 — tests for core/cfgdesc (USB config-descriptor walking).
#include "test_framework.h"
#include "cfgdesc.h"
#include <string.h>

// Descriptor builders (USB little-endian).
static int put_config(uint8_t *b, uint16_t total, uint8_t nifaces) {
    b[0]=9; b[1]=0x02; b[2]=(uint8_t)total; b[3]=(uint8_t)(total>>8);
    b[4]=nifaces; b[5]=1; b[6]=0; b[7]=0xA0; b[8]=50; return 9;
}
static int put_iface(uint8_t *b, uint8_t num, uint8_t alt, uint8_t neps,
                     uint8_t cls, uint8_t sub, uint8_t proto) {
    b[0]=9; b[1]=0x04; b[2]=num; b[3]=alt; b[4]=neps;
    b[5]=cls; b[6]=sub; b[7]=proto; b[8]=0; return 9;
}
static int put_hid(uint8_t *b, uint16_t report_len) {
    b[0]=9; b[1]=0x21; b[2]=0x11; b[3]=0x01; b[4]=0; b[5]=1;
    b[6]=0x22; b[7]=(uint8_t)report_len; b[8]=(uint8_t)(report_len>>8); return 9;
}
static int put_ep(uint8_t *b, uint8_t addr, uint16_t mps, uint8_t interval) {
    b[0]=7; b[1]=0x05; b[2]=addr; b[3]=0x03;
    b[4]=(uint8_t)mps; b[5]=(uint8_t)(mps>>8); b[6]=interval; return 7;
}

static void test_single_keyboard(void) {
    uint8_t b[64]; int n = 0;
    n += put_config(b+n, 0, 1);
    n += put_iface(b+n, 0, 0, 1, 3, 1, 1);   // HID boot keyboard
    n += put_hid(b+n, 65);
    n += put_ep(b+n, 0x81, 8, 1);
    // fix wTotalLength
    b[2]=(uint8_t)n; b[3]=(uint8_t)(n>>8);

    uint8_t ep; uint16_t mps;
    ASSERT_TRUE(cfgdesc_find_kbd_in_ep(b, (uint16_t)n, &ep, &mps));
    ASSERT_EQ_INT(0x81, ep);
    ASSERT_EQ_INT(8, mps);
}

static void test_combo_mouse_then_keyboard(void) {
    uint8_t b[128]; int n = 0;
    n += put_config(b+n, 0, 2);
    // iface 0: mouse (proto 2) with its own IN ep 0x81
    n += put_iface(b+n, 0, 0, 1, 3, 1, 2);
    n += put_hid(b+n, 50);
    n += put_ep(b+n, 0x81, 4, 10);
    // iface 1: keyboard (proto 1) with IN ep 0x82
    n += put_iface(b+n, 1, 0, 1, 3, 1, 1);
    n += put_hid(b+n, 65);
    n += put_ep(b+n, 0x82, 8, 1);
    b[2]=(uint8_t)n; b[3]=(uint8_t)(n>>8);

    uint8_t ep; uint16_t mps;
    ASSERT_TRUE(cfgdesc_find_kbd_in_ep(b, (uint16_t)n, &ep, &mps));
    ASSERT_EQ_INT(0x82, ep);   // picked the keyboard's endpoint, not the mouse's
    ASSERT_EQ_INT(8, mps);
}

static void test_no_keyboard(void) {
    uint8_t b[64]; int n = 0;
    n += put_config(b+n, 0, 1);
    n += put_iface(b+n, 0, 0, 1, 3, 1, 2);   // mouse only
    n += put_hid(b+n, 50);
    n += put_ep(b+n, 0x81, 4, 10);
    b[2]=(uint8_t)n; b[3]=(uint8_t)(n>>8);

    uint8_t ep; uint16_t mps;
    ASSERT_FALSE(cfgdesc_find_kbd_in_ep(b, (uint16_t)n, &ep, &mps));
}

static void test_skips_alt_settings(void) {
    uint8_t b[96]; int n = 0;
    n += put_config(b+n, 0, 1);
    n += put_iface(b+n, 0, 0, 1, 3, 1, 1);   // keyboard alt 0
    n += put_hid(b+n, 65);
    n += put_ep(b+n, 0x81, 8, 1);
    n += put_iface(b+n, 0, 1, 1, 3, 1, 1);   // alt 1 — must be ignored
    n += put_ep(b+n, 0x83, 16, 1);
    b[2]=(uint8_t)n; b[3]=(uint8_t)(n>>8);

    uint8_t ep; uint16_t mps;
    ASSERT_TRUE(cfgdesc_find_kbd_in_ep(b, (uint16_t)n, &ep, &mps));
    ASSERT_EQ_INT(0x81, ep);   // alt-0 endpoint
}

static void test_truncated_safe(void) {
    uint8_t b[64]; int n = 0;
    n += put_config(b+n, 0, 1);
    n += put_iface(b+n, 0, 0, 1, 3, 1, 1);
    // claim there's more but cut the buffer short — must not over-read
    uint8_t ep; uint16_t mps;
    ASSERT_FALSE(cfgdesc_find_kbd_in_ep(b, (uint16_t)n, &ep, &mps));
}

int main(void) {
    TF_BEGIN();
    TF_RUN(test_single_keyboard);
    TF_RUN(test_combo_mouse_then_keyboard);
    TF_RUN(test_no_keyboard);
    TF_RUN(test_skips_alt_settings);
    TF_RUN(test_truncated_safe);
    return TF_END();
}
