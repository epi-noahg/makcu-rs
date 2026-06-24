// cfgdesc.c — see cfgdesc.h.
#include "cfgdesc.h"

#define DESC_INTERFACE 0x04
#define DESC_ENDPOINT  0x05
#define HID_CLASS      0x03
#define HID_PROTO_KBD  0x01
#define EP_DIR_IN      0x80

bool cfgdesc_find_kbd_in_ep(const uint8_t *cfg, uint16_t len,
                            uint8_t *ep_out, uint16_t *mps_out) {
    if (!cfg || len < 2) return false;

    bool in_kbd = false;
    uint16_t i = 0;
    while ((uint16_t)(i + 2) <= len) {
        uint8_t blen  = cfg[i];
        uint8_t btype = cfg[i + 1];
        if (blen < 2) break;                 // malformed / guard against stall
        if ((uint16_t)(i + blen) > len) break; // would over-read

        if (btype == DESC_INTERFACE && blen >= 9) {
            uint8_t alt   = cfg[i + 3];
            uint8_t cls   = cfg[i + 5];
            uint8_t proto = cfg[i + 7];
            in_kbd = (alt == 0 && cls == HID_CLASS && proto == HID_PROTO_KBD);
        } else if (btype == DESC_ENDPOINT && blen >= 7 && in_kbd) {
            uint8_t addr = cfg[i + 2];
            if (addr & EP_DIR_IN) {
                *ep_out  = addr;
                *mps_out = (uint16_t)(cfg[i + 4] | (cfg[i + 5] << 8));
                return true;
            }
        }
        i = (uint16_t)(i + blen);
    }
    return false;
}
