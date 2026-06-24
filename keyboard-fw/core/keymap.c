// keymap.c — US-layout HID key resolution. See keymap.h.
#include "keymap.h"

#include <ctype.h>
#include <stddef.h>
#include <string.h>

// Named keys (lowercase). Generic modifier aliases map to the left variant.
struct named_key { const char *name; uint8_t code; };

static const struct named_key NAMED[] = {
    // control
    {"enter", 40}, {"return", 40}, {"esc", 41}, {"escape", 41},
    {"backspace", 42}, {"bksp", 42}, {"tab", 43}, {"space", 44}, {"spacebar", 44},
    // symbol keys (by name)
    {"minus", 45}, {"equal", 46}, {"leftbracket", 47}, {"rightbracket", 48},
    {"backslash", 49}, {"semicolon", 51}, {"apostrophe", 52}, {"grave", 53},
    {"comma", 54}, {"period", 55}, {"slash", 56}, {"capslock", 57},
    // function keys
    {"f1", 58}, {"f2", 59}, {"f3", 60}, {"f4", 61}, {"f5", 62}, {"f6", 63},
    {"f7", 64}, {"f8", 65}, {"f9", 66}, {"f10", 67}, {"f11", 68}, {"f12", 69},
    // system / navigation
    {"printscreen", 70}, {"scrolllock", 71}, {"pause", 72},
    {"insert", 73}, {"ins", 73}, {"home", 74}, {"pageup", 75}, {"pgup", 75},
    {"delete", 76}, {"del", 76}, {"end", 77}, {"pagedown", 78}, {"pgdn", 78},
    {"right", 79}, {"left", 80}, {"down", 81}, {"up", 82}, {"numlock", 83},
    // modifiers — generic names default to the LEFT variant
    {"ctrl", 224}, {"control", 224}, {"lctrl", 224},
    {"shift", 225}, {"lshift", 225},
    {"alt", 226}, {"lalt", 226},
    {"gui", 227}, {"lgui", 227}, {"win", 227}, {"lwin", 227}, {"cmd", 227}, {"meta", 227},
    {"rctrl", 228}, {"rshift", 229}, {"ralt", 230}, {"altgr", 230},
    {"rgui", 231}, {"rwin", 231},
};

static bool name_lookup_ci(const char *s, size_t len, uint8_t *code_out) {
    for (size_t i = 0; i < sizeof(NAMED) / sizeof(NAMED[0]); ++i) {
        if (strlen(NAMED[i].name) == len) {
            size_t k = 0;
            for (; k < len; ++k) {
                if (tolower((unsigned char)s[k]) != NAMED[i].name[k]) break;
            }
            if (k == len) { *code_out = NAMED[i].code; return true; }
        }
    }
    return false;
}

bool ascii_to_hid(char c, uint8_t *code_out, bool *shift_out) {
    bool shift = false;
    uint8_t code = 0;

    if (c >= 'a' && c <= 'z')      { code = (uint8_t)(4 + (c - 'a')); }
    else if (c >= 'A' && c <= 'Z') { code = (uint8_t)(4 + (c - 'A')); shift = true; }
    else if (c >= '1' && c <= '9') { code = (uint8_t)(30 + (c - '1')); }
    else {
        switch (c) {
            case '0': code = 39; break;
            case '\n': code = 40; break;       // Enter
            case '\t': code = 43; break;       // Tab
            case ' ': code = 44; break;
            // shifted number row
            case '!': code = 30; shift = true; break;
            case '@': code = 31; shift = true; break;
            case '#': code = 32; shift = true; break;
            case '$': code = 33; shift = true; break;
            case '%': code = 34; shift = true; break;
            case '^': code = 35; shift = true; break;
            case '&': code = 36; shift = true; break;
            case '*': code = 37; shift = true; break;
            case '(': code = 38; shift = true; break;
            case ')': code = 39; shift = true; break;
            // symbol keys + shifted variants
            case '-': code = 45; break;        case '_': code = 45; shift = true; break;
            case '=': code = 46; break;        case '+': code = 46; shift = true; break;
            case '[': code = 47; break;        case '{': code = 47; shift = true; break;
            case ']': code = 48; break;        case '}': code = 48; shift = true; break;
            case '\\': code = 49; break;       case '|': code = 49; shift = true; break;
            case ';': code = 51; break;        case ':': code = 51; shift = true; break;
            case '\'': code = 52; break;       case '"': code = 52; shift = true; break;
            case '`': code = 53; break;        case '~': code = 53; shift = true; break;
            case ',': code = 54; break;        case '<': code = 54; shift = true; break;
            case '.': code = 55; break;        case '>': code = 55; shift = true; break;
            case '/': code = 56; break;        case '?': code = 56; shift = true; break;
            default: return false;
        }
    }
    *code_out = code;
    *shift_out = shift;
    return true;
}

// Resolve a single character used as a *key name* (quoted single char):
// letters keep case (uppercase => shift), other chars defer to ascii_to_hid.
static bool single_char_key(char c, uint8_t *code_out, bool *shift_out) {
    return ascii_to_hid(c, code_out, shift_out);
}

bool key_lookup(const char *tok, uint8_t *code_out, bool *shift_out) {
    *shift_out = false;
    if (!tok) return false;

    size_t len = strlen(tok);

    // Strip one layer of matching surrounding quotes.
    bool quoted = false;
    const char *s = tok;
    if (len >= 2 && ((tok[0] == '\'' && tok[len - 1] == '\'') ||
                     (tok[0] == '"'  && tok[len - 1] == '"'))) {
        quoted = true;
        s = tok + 1;
        len -= 2;
    }

    if (len == 0) return false;

    // Unquoted all-digits => raw HID code.
    if (!quoted) {
        bool all_digits = true;
        for (size_t i = 0; i < len; ++i) {
            if (!isdigit((unsigned char)s[i])) { all_digits = false; break; }
        }
        if (all_digits) {
            unsigned v = 0;
            for (size_t i = 0; i < len; ++i) v = v * 10 + (unsigned)(s[i] - '0');
            if (v > 255) return false;
            *code_out = (uint8_t)v;
            return true;
        }
    }

    // Single character: letter/digit/symbol key.
    if (len == 1) {
        return single_char_key(s[0], code_out, shift_out);
    }

    // Multi-character named key (case-insensitive).
    return name_lookup_ci(s, len, code_out);
}
