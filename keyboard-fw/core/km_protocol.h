// km_protocol.h — portable parser + reply framer for the km.* host protocol.
//
// Wire contract (authority: lib-rs/src/lib.rs, must NOT change lib-rs):
//   RX  one command per line: (km.|.)?name(args)[#<cid>]
//   TX  reply ONLY when the line carried a #<cid> (tracked GET). The reply
//       line must contain "#<cid>:<value>" so the host reader matches it
//       (lib.rs:449-462). Fire-and-forget setters emit nothing.
// No platform dependencies — host-testable.
#ifndef KM_PROTOCOL_H
#define KM_PROTOCOL_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef enum {
    KM_UNKNOWN = 0, // unparseable (no parentheses / empty)
    KM_IGNORE,      // parsed but not a keyboard command (mouse cmds, buttons…)
    KM_DOWN, KM_UP, KM_PRESS, KM_STRING, KM_INIT, KM_ISDOWN,
    KM_DISABLE, KM_MASK, KM_REMAP, KM_KEYBOARD,
    KM_VERSION, KM_INFO, KM_DEVICE, KM_SERIAL, KM_REBOOT, KM_ECHO, KM_BAUD,
} KmKind;

#define KM_MAX_ARGS 18
#define KM_NAME_LEN 16
#define KM_LINE_MAX 320  // covers km.string up to 256 chars + quotes/escapes

typedef struct {
    KmKind   kind;
    char     name[KM_NAME_LEN];
    int      argc;
    char     buf[KM_LINE_MAX];        // arg region, split into NUL-terminated tokens
    const char *args[KM_MAX_ARGS];    // point into buf (quotes preserved)
    bool     has_cid;
    uint32_t cid;
} KmCmd;

// Parse one line (CR/LF already stripped is fine; trailing CR tolerated).
void km_parse(const char *line, size_t len, KmCmd *out);

// Strip one layer of surrounding quotes and unescape \" and \\ (US km.string
// semantics). Always NUL-terminates. Returns the resulting length.
size_t km_arg_unquote(const char *arg, char *out, size_t outsz);

// Format a tracked reply into out. Returns bytes written, or 0 when the
// command has no cid (fire-and-forget => no output). `value` may be NULL.
size_t km_format_reply(const KmCmd *cmd, const char *value, char *out, size_t outsz);

#endif // KM_PROTOCOL_H
