// cfgdesc.h — minimal USB configuration-descriptor walker.
//
// Locates the boot keyboard interface (HID class 0x03, protocol 0x01 Keyboard,
// alternate setting 0) and returns its interrupt IN endpoint address and max
// packet size. Bounds-checked against `len` — safe on truncated buffers.
// No platform dependencies — host-testable.
#ifndef CFGDESC_H
#define CFGDESC_H

#include <stdbool.h>
#include <stdint.h>

// Find the keyboard IN endpoint in a full config descriptor.
// Returns true and fills *ep_out (e.g. 0x81) and *mps_out (e.g. 8) on success.
bool cfgdesc_find_kbd_in_ep(const uint8_t *cfg, uint16_t len,
                            uint8_t *ep_out, uint16_t *mps_out);

#endif // CFGDESC_H
