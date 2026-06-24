// Phase 5 — integration tests for core/km_glue (protocol -> engine -> overlay).
// Exercises the exact seam the LEFT firmware calls, with stubbed platform hooks.
#include "test_framework.h"
#include "km_glue.h"
#include <string.h>

// ---- stubbed platform hooks --------------------------------------------------
static uint32_t g_now;
static char     g_reply[512];
static size_t   g_reply_len;

uint32_t km_now_ms(void) { return g_now; }
void     km_platform_start(void) { /* no timer in tests */ }
int      km_uart_write_raw(const void *data, size_t len) {
    if (g_reply_len + len < sizeof g_reply) {
        memcpy(g_reply + g_reply_len, data, len);
        g_reply_len += len;
        g_reply[g_reply_len] = '\0';
    }
    return (int)len;
}

static void feed(const char *line) { km_ingest_raw((const uint8_t *)line, (uint16_t)strlen(line)); }
static void reset_reply(void) { g_reply_len = 0; g_reply[0] = '\0'; }

static void tick(uint32_t now) { g_now = now; km_periodic(); }

// extract value after "#<cid>:" like lib-rs
static bool reply_value(char *out, size_t osz) {
    const char *h = strchr(g_reply, '#');
    if (!h) return false;
    const char *c = strchr(h, ':');
    if (!c) return false;
    size_t n = 0; const char *p = c + 1;
    while (p[n] && p[n] != '\r' && n + 1 < osz) { out[n] = p[n]; n++; }
    out[n] = '\0';
    return true;
}

#define K0 2
static bool has_key(const uint8_t r[8], uint8_t code) {
    for (int i = K0; i < 8; ++i) if (r[i] == code) return true;
    return false;
}

static void setup_kbd(void) {
    km_init();
    km_set_kbd_endpoint(0x81, 8);
    tick(0);
    reset_reply();
}

static void test_version_reply(void) {
    setup_kbd();
    feed("km.version()#5");
    char v[64];
    ASSERT_TRUE(reply_value(v, sizeof v));
    ASSERT_TRUE(strlen(v) > 0);
}

static void test_down_apply(void) {
    setup_kbd();
    feed("km.down('a')");
    uint8_t r[8]; memset(r, 0, 8);
    km_apply(0x81, r, 8);
    ASSERT_TRUE(has_key(r, 4));

    // non-keyboard endpoint must be left untouched
    uint8_t r2[8] = {0,0,9,0,0,0,0,0};
    km_apply(0x82, r2, 8);
    ASSERT_EQ_INT(9, r2[K0]);

    feed("km.up('a')");
    memset(r, 0, 8); km_apply(0x81, r, 8);
    ASSERT_FALSE(has_key(r, 4));
}

// Regression: a keyboard whose IN endpoint advertises a large wMaxPacketSize
// (e.g. 64) must STILL be treated as an 8-byte boot report with NO report-ID.
// Previously km_set_kbd_endpoint inferred a report-ID from MPS>=9, shifting the
// layout by one byte and dropping the first physical key slot — a single
// keypress was lost and a two-key press emitted only the second key.
static void test_large_mps_boot_passthrough(void) {
    km_init();
    km_set_kbd_endpoint(0x81, 64);   // real keyboard: 8-byte report on 64-byte EP
    tick(0);
    reset_reply();

    // physical 'a' (HID 4) in the FIRST key slot, no injection active.
    uint8_t r[8] = {0, 0, 4, 0, 0, 0, 0, 0};
    km_apply(0x81, r, 8);
    ASSERT_TRUE(has_key(r, 4));       // slot-0 key must pass through

    // two keys held: BOTH pass through, not just the last.
    uint8_t r2[8] = {0, 0, 4, 5, 0, 0, 0, 0};
    km_apply(0x81, r2, 8);
    ASSERT_TRUE(has_key(r2, 4));
    ASSERT_TRUE(has_key(r2, 5));
}

static void test_press_timed(void) {
    setup_kbd();
    feed("km.press('a',50)");
    uint8_t r[8]; memset(r, 0, 8); km_apply(0x81, r, 8);
    ASSERT_TRUE(has_key(r, 4));
    ASSERT_TRUE(km_has_active_injection());

    tick(50);
    memset(r, 0, 8); km_apply(0x81, r, 8);
    ASSERT_FALSE(has_key(r, 4));
    ASSERT_FALSE(km_has_active_injection());
}

static void test_string(void) {
    setup_kbd();
    feed("km.string(\"ab\")");
    uint8_t r[8];

    tick(0);                                   // press 'a'
    memset(r, 0, 8); km_apply(0x81, r, 8);
    ASSERT_TRUE(has_key(r, 4));

    tick(10);                                  // release 'a'
    memset(r, 0, 8); km_apply(0x81, r, 8);
    ASSERT_FALSE(has_key(r, 4));

    tick(20);                                  // press 'b'
    memset(r, 0, 8); km_apply(0x81, r, 8);
    ASSERT_TRUE(has_key(r, 5));
}

static void test_disable_and_inject(void) {
    setup_kbd();
    feed("km.disable('a')");
    // physical 'a' dropped
    uint8_t r[8] = {0,0,4,0,0,0,0,0};
    km_apply(0x81, r, 8);
    ASSERT_FALSE(has_key(r, 4));
    // injected 'a' still passes
    feed("km.down('a')");
    uint8_t r2[8]; memset(r2, 0, 8);
    km_apply(0x81, r2, 8);
    ASSERT_TRUE(has_key(r2, 4));
}

static void test_remap(void) {
    setup_kbd();
    feed("km.remap('a','b')");
    uint8_t r[8] = {0,0,4,0,0,0,0,0};
    km_apply(0x81, r, 8);
    ASSERT_FALSE(has_key(r, 4));
    ASSERT_TRUE(has_key(r, 5));

    feed("km.remap('a',0)");                   // clear
    uint8_t r2[8] = {0,0,4,0,0,0,0,0};
    km_apply(0x81, r2, 8);
    ASSERT_TRUE(has_key(r2, 4));
}

static void test_isdown_reply(void) {
    setup_kbd();
    feed("km.down('a')");
    reset_reply();
    feed("km.isdown('a')#7");
    char v[8]; ASSERT_TRUE(reply_value(v, sizeof v));
    ASSERT_EQ_STR("1", v);

    reset_reply();
    feed("km.isdown('b')#8");
    ASSERT_TRUE(reply_value(v, sizeof v));
    ASSERT_EQ_STR("0", v);
}

static void test_setter_silent(void) {
    setup_kbd();
    reset_reply();
    feed("km.down('a')");          // fire-and-forget, no cid
    ASSERT_EQ_INT(0, (int)g_reply_len);
    feed("km.buttons(1,10)");      // connect noise: ignored, silent
    ASSERT_EQ_INT(0, (int)g_reply_len);
}

int main(void) {
    TF_BEGIN();
    TF_RUN(test_version_reply);
    TF_RUN(test_down_apply);
    TF_RUN(test_large_mps_boot_passthrough);
    TF_RUN(test_press_timed);
    TF_RUN(test_string);
    TF_RUN(test_disable_and_inject);
    TF_RUN(test_remap);
    TF_RUN(test_isdown_reply);
    TF_RUN(test_setter_silent);
    return TF_END();
}
