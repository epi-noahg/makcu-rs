// km_protocol.c — see km_protocol.h.
#include "km_protocol.h"

#include <ctype.h>
#include <stdio.h>
#include <string.h>

struct name_kind { const char *name; KmKind kind; };

static const struct name_kind KINDS[] = {
    {"down", KM_DOWN}, {"up", KM_UP}, {"press", KM_PRESS}, {"string", KM_STRING},
    {"init", KM_INIT}, {"isdown", KM_ISDOWN}, {"disable", KM_DISABLE},
    {"mask", KM_MASK}, {"remap", KM_REMAP}, {"keyboard", KM_KEYBOARD},
    {"version", KM_VERSION}, {"info", KM_INFO}, {"device", KM_DEVICE},
    {"serial", KM_SERIAL}, {"reboot", KM_REBOOT}, {"echo", KM_ECHO},
    {"baud", KM_BAUD},
};

static KmKind kind_of(const char *name) {
    for (size_t i = 0; i < sizeof(KINDS) / sizeof(KINDS[0]); ++i) {
        // case-insensitive compare
        const char *a = name, *b = KINDS[i].name;
        while (*a && *b && tolower((unsigned char)*a) == *b) { ++a; ++b; }
        if (*a == '\0' && *b == '\0') return KINDS[i].kind;
    }
    return KM_IGNORE;
}

void km_parse(const char *line, size_t len, KmCmd *out) {
    memset(out, 0, sizeof(*out));
    out->kind = KM_UNKNOWN;
    if (!line || len == 0) return;

    // Trim leading whitespace.
    size_t i = 0;
    while (i < len && isspace((unsigned char)line[i])) ++i;

    // Optional "km." or "." prefix.
    if (i + 3 <= len && line[i] == 'k' && line[i + 1] == 'm' && line[i + 2] == '.') {
        i += 3;
    } else if (i < len && line[i] == '.') {
        i += 1;
    }

    // Command name: up to '('.
    size_t name_start = i;
    while (i < len && line[i] != '(') ++i;
    if (i >= len) return;            // no '(' => malformed
    size_t name_len = i - name_start;
    if (name_len == 0 || name_len >= KM_NAME_LEN) return;
    memcpy(out->name, line + name_start, name_len);
    out->name[name_len] = '\0';

    // Args between '(' and the matching top-level ')'.
    size_t args_start = i + 1;       // after '('
    char quote = 0;
    bool esc = false;
    size_t j = args_start;
    size_t args_end = (size_t)-1;
    for (; j < len; ++j) {
        char ch = line[j];
        if (esc) { esc = false; continue; }
        if (quote) {
            if (ch == '\\') esc = true;
            else if (ch == quote) quote = 0;
        } else if (ch == '\'' || ch == '"') {
            quote = ch;
        } else if (ch == ')') {
            args_end = j;
            break;
        }
    }
    if (args_end == (size_t)-1) return;   // no closing ')'

    // Optional #<cid> after ')'.
    size_t k = args_end + 1;
    if (k < len && line[k] == '#') {
        ++k;
        uint32_t cid = 0;
        bool any = false;
        while (k < len && isdigit((unsigned char)line[k])) {
            cid = cid * 10 + (uint32_t)(line[k] - '0');
            any = true;
            ++k;
        }
        if (any) { out->has_cid = true; out->cid = cid; }
    }

    out->kind = kind_of(out->name);

    // Split the args region into tokens on top-level commas.
    size_t alen = args_end - args_start;
    if (alen >= KM_LINE_MAX) alen = KM_LINE_MAX - 1;
    memcpy(out->buf, line + args_start, alen);
    out->buf[alen] = '\0';

    if (alen == 0) { out->argc = 0; return; } // empty parens => no args

    quote = 0; esc = false;
    size_t tok_start = 0;
    int argc = 0;
    for (size_t p = 0; p <= alen; ++p) {
        char ch = (p < alen) ? out->buf[p] : ',';   // sentinel terminator
        bool split = false;
        if (p == alen) {
            split = true;
        } else if (esc) {
            esc = false;
        } else if (quote) {
            if (ch == '\\') esc = true;
            else if (ch == quote) quote = 0;
        } else if (ch == '\'' || ch == '"') {
            quote = ch;
        } else if (ch == ',') {
            split = true;
        }

        if (split) {
            if (argc < KM_MAX_ARGS) {
                out->buf[p] = '\0';
                // trim leading/trailing whitespace of the token
                size_t s = tok_start;
                while (s < p && isspace((unsigned char)out->buf[s])) ++s;
                size_t e = p;
                while (e > s && isspace((unsigned char)out->buf[e - 1])) --e;
                out->buf[e] = '\0';
                out->args[argc++] = &out->buf[s];
            }
            tok_start = p + 1;
        }
    }
    out->argc = argc;
}

size_t km_arg_unquote(const char *arg, char *out, size_t outsz) {
    size_t n = 0;
    if (!arg || outsz == 0) { if (outsz) out[0] = '\0'; return 0; }

    size_t len = strlen(arg);
    const char *s = arg;
    char quote = 0;
    if (len >= 2 && ((arg[0] == '\'' && arg[len - 1] == '\'') ||
                     (arg[0] == '"'  && arg[len - 1] == '"'))) {
        quote = arg[0];
        s = arg + 1;
        len -= 2;
    }

    for (size_t i = 0; i < len && n + 1 < outsz; ++i) {
        char c = s[i];
        if (quote && c == '\\' && i + 1 < len) {
            char nx = s[i + 1];
            if (nx == '\\' || nx == '"' || nx == '\'') { out[n++] = nx; ++i; continue; }
        }
        out[n++] = c;
    }
    out[n] = '\0';
    return n;
}

size_t km_format_reply(const KmCmd *cmd, const char *value, char *out, size_t outsz) {
    if (!cmd->has_cid) return 0;        // fire-and-forget => silent
    const char *v = value ? value : "";
    // "<echo>#<cid>:<value>\r\n" — host reads cid between '#' and ':',
    // value is everything after ':' (lib.rs:449-462).
    int n = snprintf(out, outsz, "km.%s(%s)#%u:%s\r\n",
                     cmd->name, v, (unsigned)cmd->cid, v);
    if (n < 0 || (size_t)n >= outsz) return 0;
    return (size_t)n;
}
