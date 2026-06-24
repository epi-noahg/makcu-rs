// keymap.h — portable USB HID key resolution (US layout).
//
// Maps km.* key tokens (numeric HID codes or quoted/unquoted names) to HID
// usage codes, and ASCII characters to (code, shift) for km.string typing.
// No platform dependencies — host-testable.
#ifndef KEYMAP_H
#define KEYMAP_H

#include <stdbool.h>
#include <stdint.h>

// Resolve a key token to a HID usage code.
//
// Accepted forms:
//   - bare digits ("4", "224")      -> that HID code (0..255)
//   - quoted/unquoted single letter -> letter key; 'A' sets *shift
//   - quoted digit ("'1'")          -> that digit key (1->30 .. 0->39)
//   - named key ("'enter'", "f1")   -> case-insensitive table lookup
//
// Generic modifier names ('ctrl','shift','alt','gui'/'win') resolve to the
// LEFT variant (224..227). Returns false if unrecognized or out of range.
bool key_lookup(const char *tok, uint8_t *code_out, bool *shift_out);

// Map a printable/whitespace ASCII char to a HID code (+ shift). Covers the
// US layout: letters, digits, space, tab, newline(->Enter), and the shifted
// symbol set. Returns false for chars with no single-key representation.
bool ascii_to_hid(char c, uint8_t *code_out, bool *shift_out);

// True for the 8 modifier usage codes (Left/Right Ctrl/Shift/Alt/GUI).
static inline bool key_is_modifier(uint8_t code) {
    return code >= 224 && code <= 231;
}

#endif // KEYMAP_H
