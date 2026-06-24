// km_platform.c — ESP-IDF platform hooks for the portable km_glue core.
//
// The injection logic lives in ../../core (keymap, km_protocol, kbd_inject,
// cfgdesc, km_glue) and is unit-tested on the host. This shim supplies the
// three platform hooks km_glue declares: a monotonic millisecond clock and a
// periodic tick that drives km.press / km.string timing. The reply channel
// (km_uart_write_raw) is provided by ipc.c.
#include "km_glue.h"
#include "esp_timer.h"

uint32_t km_now_ms(void) {
    return (uint32_t)(esp_timer_get_time() / 1000);
}

static void km_tick_cb(void *arg) {
    (void)arg;
    km_periodic();
}

void km_platform_start(void) {
    static esp_timer_handle_t h;
    const esp_timer_create_args_t args = {
        .callback = km_tick_cb,
        .name     = "km_tick",
    };
    if (esp_timer_create(&args, &h) == ESP_OK) {
        esp_timer_start_periodic(h, 4000);  // 4 ms — matches the synth cadence
    }
}
