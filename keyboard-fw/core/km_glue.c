// km_glue.c — see km_glue.h.
#include "km_glue.h"
#include "km_protocol.h"
#include "keymap.h"
#include "kbd_inject.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef KBD_FW_VERSION
#define KBD_FW_VERSION "kbd-1.0.0"
#endif

static KbdState g_kbd;
static uint8_t  g_kbd_ep;       // 0 = unknown yet

void km_init(void) {
    kbd_init(&g_kbd, false, 8);
    g_kbd_ep = 0;
    km_platform_start();
}

void km_periodic(void) {
    kbd_tick(&g_kbd, km_now_ms());
}

void km_reset_injection(void) {
    kbd_reset_injection(&g_kbd);
}

bool km_has_active_injection(void) {
    return kbd_is_active(&g_kbd);
}

void km_set_kbd_endpoint(uint8_t ep_addr, uint16_t report_len) {
    g_kbd_ep = ep_addr;
    // Boot keyboard report is 8 bytes with no report-ID. If a device advertises
    // a 9-byte report we treat byte 0 as a report-ID.
    bool has_id = (report_len >= 9);
    kbd_set_layout(&g_kbd, has_id, report_len ? report_len : 8);
}

bool km_is_kbd_ep(uint8_t ep_addr) {
    return g_kbd_ep != 0 && ep_addr == g_kbd_ep;
}

void km_apply(uint8_t ep_addr, uint8_t *buf, uint16_t len) {
    if (!km_is_kbd_ep(ep_addr)) return;   // only overlay the keyboard endpoint
    kbd_set_physical(&g_kbd, buf, len);
    kbd_overlay(&g_kbd, buf, len);
}

// ---- helpers -----------------------------------------------------------------
static bool arg_is_int01(const char *s) {
    return s && (strcmp(s, "0") == 0 || strcmp(s, "1") == 0);
}

static void reply(const KmCmd *c, const char *value) {
    if (!c->has_cid) return;
    char out[KBD_STR_MAX + 64];
    size_t n = km_format_reply(c, value, out, sizeof out);
    if (n) km_uart_write_raw(out, n);
}

static void do_disable(const KmCmd *c) {
    if (c->argc == 0) {              // query: list disabled codes
        char list[KBD_STR_MAX]; size_t p = 0;
        for (int code = 1; code < 256; ++code) {
            if (kbd_is_disabled(&g_kbd, (uint8_t)code)) {
                int w = snprintf(list + p, sizeof list - p, "%d,", code);
                if (w < 0 || (size_t)w >= sizeof list - p) break;
                p += (size_t)w;
            }
        }
        list[p] = '\0';
        reply(c, list);
        return;
    }
    // (key,mode) form
    if (c->argc == 2 && arg_is_int01(c->args[1])) {
        uint8_t code; bool sh;
        if (key_lookup(c->args[0], &code, &sh))
            kbd_set_disabled(&g_kbd, code, c->args[1][0] == '1');
        return;
    }
    // disable each listed key
    for (int i = 0; i < c->argc; ++i) {
        uint8_t code; bool sh;
        if (key_lookup(c->args[i], &code, &sh))
            kbd_set_disabled(&g_kbd, code, true);
    }
}

void km_ingest_raw(const uint8_t *payload, uint16_t len) {
    KmCmd c;
    km_parse((const char *)payload, len, &c);

    uint8_t code; bool shift;
    switch (c.kind) {
    case KM_DOWN:
        if (c.argc >= 1 && key_lookup(c.args[0], &code, &shift))
            kbd_down(&g_kbd, code, shift);
        break;
    case KM_UP:
        if (c.argc >= 1 && key_lookup(c.args[0], &code, &shift))
            kbd_up(&g_kbd, code, shift);
        break;
    case KM_PRESS:
        if (c.argc >= 1 && key_lookup(c.args[0], &code, &shift)) {
            uint32_t hold = (c.argc >= 2) ? (uint32_t)strtoul(c.args[1], NULL, 10) : 0;
            kbd_press(&g_kbd, code, shift, hold);
        }
        break;
    case KM_STRING:
        if (c.argc >= 1) {
            char text[KBD_STR_MAX + 1];
            km_arg_unquote(c.args[0], text, sizeof text);
            kbd_type(&g_kbd, text);
        }
        break;
    case KM_INIT:
        kbd_release_all(&g_kbd);
        break;
    case KM_ISDOWN:
        if (c.argc >= 1 && key_lookup(c.args[0], &code, &shift))
            reply(&c, kbd_isdown(&g_kbd, code) ? "1" : "0");
        else
            reply(&c, "0");
        break;
    case KM_DISABLE:
        do_disable(&c);
        break;
    case KM_MASK:
        if (c.argc >= 1 && key_lookup(c.args[0], &code, &shift)) {
            bool on = (c.argc >= 2) ? (strtol(c.args[1], NULL, 10) != 0) : true;
            kbd_set_masked(&g_kbd, code, on);
        }
        break;
    case KM_REMAP:
        if (c.argc >= 2 && key_lookup(c.args[0], &code, &shift)) {
            uint8_t dst = 0;
            if (strcmp(c.args[1], "0") != 0) {
                bool ds; uint8_t dc;
                if (key_lookup(c.args[1], &dc, &ds)) dst = dc;
            }
            kbd_set_remap(&g_kbd, code, dst);
        }
        break;
    case KM_KEYBOARD:
        // streaming setup (mode/period) — emission handled by the device layer;
        // fire-and-forget, no reply.
        break;
    case KM_VERSION: reply(&c, KBD_FW_VERSION); break;
    case KM_DEVICE:  reply(&c, "keyboard");     break;
    case KM_INFO:    reply(&c, "MAKCU keyboard firmware"); break;
    case KM_SERIAL:  reply(&c, "");             break;
    case KM_ECHO:    reply(&c, "1");            break;
    case KM_BAUD:    reply(&c, "115200");       break;
    case KM_REBOOT:  /* device shim may hook reboot */ break;
    case KM_IGNORE:
    case KM_UNKNOWN:
    default:
        break;
    }
}
