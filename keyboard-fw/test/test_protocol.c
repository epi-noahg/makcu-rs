// Phase 2 — tests for core/km_protocol (line parser + reply framer).
#include "test_framework.h"
#include "km_protocol.h"
#include <string.h>

static void parse(const char *s, KmCmd *c) { km_parse(s, strlen(s), c); }

// Mimic lib-rs handle_line_bytes cid extraction (lib.rs:449-462):
// find '#', read cid digits until ':' , value = rest after ':'.
// Returns true if a cid line was matched; fills *cid and value buffer.
static bool lib_extract(const char *line, uint32_t *cid, char *val, size_t vsz) {
    const char *h = strchr(line, '#');
    if (!h) return false;
    const char *colon = strchr(h + 1, ':');
    if (!colon) return false;
    uint32_t v = 0;
    for (const char *p = h + 1; p < colon; ++p) {
        if (*p < '0' || *p > '9') return false;
        v = v * 10 + (uint32_t)(*p - '0');
    }
    *cid = v;
    const char *payload = colon + 1;
    size_t n = 0;
    while (payload[n] && payload[n] != '\r' && payload[n] != '\n' && n + 1 < vsz) {
        val[n] = payload[n]; n++;
    }
    val[n] = '\0';
    return true;
}

// ---- prefixes & kinds --------------------------------------------------------
static void test_prefixes(void) {
    KmCmd c;
    parse("km.down('a')", &c);
    ASSERT_EQ_INT(KM_DOWN, c.kind);
    ASSERT_EQ_STR("down", c.name);
    ASSERT_EQ_INT(1, c.argc);
    ASSERT_EQ_STR("'a'", c.args[0]);

    parse(".down('a')", &c);   ASSERT_EQ_INT(KM_DOWN, c.kind);
    parse("down('a')", &c);    ASSERT_EQ_INT(KM_DOWN, c.kind);
}

static void test_kinds(void) {
    KmCmd c;
    parse("km.up('a')", &c);        ASSERT_EQ_INT(KM_UP, c.kind);
    parse("km.press('a')", &c);     ASSERT_EQ_INT(KM_PRESS, c.kind);
    parse("km.string(\"x\")", &c);  ASSERT_EQ_INT(KM_STRING, c.kind);
    parse("km.init()", &c);         ASSERT_EQ_INT(KM_INIT, c.kind);
    parse("km.isdown('a')", &c);    ASSERT_EQ_INT(KM_ISDOWN, c.kind);
    parse("km.disable('a','b')", &c); ASSERT_EQ_INT(KM_DISABLE, c.kind);
    parse("km.mask('a',1)", &c);    ASSERT_EQ_INT(KM_MASK, c.kind);
    parse("km.remap('a','b')", &c); ASSERT_EQ_INT(KM_REMAP, c.kind);
    parse("km.keyboard(1,100)", &c); ASSERT_EQ_INT(KM_KEYBOARD, c.kind);
    parse("km.version()", &c);      ASSERT_EQ_INT(KM_VERSION, c.kind);

    // recognized-but-ignored (mouse / connect noise)
    parse("km.buttons(1,10)", &c);  ASSERT_EQ_INT(KM_IGNORE, c.kind);
    parse(".move(1,1,)", &c);       ASSERT_EQ_INT(KM_IGNORE, c.kind);
}

// ---- argument splitting ------------------------------------------------------
static void test_args(void) {
    KmCmd c;
    parse("km.press('d', 50, 10)", &c);
    ASSERT_EQ_INT(3, c.argc);
    ASSERT_EQ_STR("'d'", c.args[0]);
    ASSERT_EQ_STR("50", c.args[1]);
    ASSERT_EQ_STR("10", c.args[2]);

    parse(".move(1,1,)", &c);  // trailing empty arg
    ASSERT_EQ_INT(3, c.argc);
    ASSERT_EQ_STR("1", c.args[0]);
    ASSERT_EQ_STR("", c.args[2]);

    parse("km.init()", &c);    // no args
    ASSERT_EQ_INT(0, c.argc);
}

// ---- quotes & escapes (km.string) -------------------------------------------
static void test_quotes_escapes(void) {
    KmCmd c;
    char out[64];

    parse("km.string(\"a,b\")", &c);    // comma inside quotes is NOT a separator
    ASSERT_EQ_INT(1, c.argc);
    ASSERT_EQ_STR("\"a,b\"", c.args[0]);
    km_arg_unquote(c.args[0], out, sizeof out);
    ASSERT_EQ_STR("a,b", out);

    parse("km.string(\"a\\\"b\")", &c); // escaped quote: payload "a\"b"
    ASSERT_EQ_INT(1, c.argc);
    km_arg_unquote(c.args[0], out, sizeof out);
    ASSERT_EQ_STR("a\"b", out);

    km_arg_unquote("'a'", out, sizeof out);
    ASSERT_EQ_STR("a", out);
}

// ---- cid extraction ----------------------------------------------------------
static void test_cid(void) {
    KmCmd c;
    parse("km.isdown('a')#7", &c);
    ASSERT_EQ_INT(KM_ISDOWN, c.kind);
    ASSERT_TRUE(c.has_cid);
    ASSERT_EQ_INT(7, c.cid);
    ASSERT_EQ_INT(1, c.argc);
    ASSERT_EQ_STR("'a'", c.args[0]);

    parse("km.version()#42", &c);
    ASSERT_TRUE(c.has_cid);
    ASSERT_EQ_INT(42, c.cid);

    parse("km.down('a')", &c);
    ASSERT_FALSE(c.has_cid);
}

// ---- malformed ---------------------------------------------------------------
static void test_malformed(void) {
    KmCmd c;
    parse("km.down", &c);   ASSERT_EQ_INT(KM_UNKNOWN, c.kind);  // no parens
    parse("", &c);          ASSERT_EQ_INT(KM_UNKNOWN, c.kind);
    parse("garbage", &c);   ASSERT_EQ_INT(KM_UNKNOWN, c.kind);
}

// ---- reply framer (tracked GET only) ----------------------------------------
static void test_framer(void) {
    KmCmd c;
    char out[128];
    uint32_t cid; char val[64];

    parse("km.isdown('a')#7", &c);
    size_t n = km_format_reply(&c, "1", out, sizeof out);
    ASSERT_TRUE(n > 0);
    ASSERT_TRUE(lib_extract(out, &cid, val, sizeof val));
    ASSERT_EQ_INT(7, cid);
    ASSERT_EQ_STR("1", val);
    // must terminate with CRLF
    ASSERT_TRUE(n >= 2 && out[n - 2] == '\r' && out[n - 1] == '\n');

    parse("km.version()#5", &c);
    n = km_format_reply(&c, "3.9.0", out, sizeof out);
    ASSERT_TRUE(lib_extract(out, &cid, val, sizeof val));
    ASSERT_EQ_INT(5, cid);
    ASSERT_EQ_STR("3.9.0", val);

    // fire-and-forget setter: no cid => NO output (must not pollute stream_cb)
    parse("km.down('a')", &c);
    n = km_format_reply(&c, NULL, out, sizeof out);
    ASSERT_EQ_INT(0, n);
}

int main(void) {
    TF_BEGIN();
    TF_RUN(test_prefixes);
    TF_RUN(test_kinds);
    TF_RUN(test_args);
    TF_RUN(test_quotes_escapes);
    TF_RUN(test_cid);
    TF_RUN(test_malformed);
    TF_RUN(test_framer);
    return TF_END();
}
