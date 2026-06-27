// sigtap.c — implementation of the SIGTAP signal-tap (see sigtap.h).
//
// The whole translation unit is empty unless SIGTAP is set, so it is safe to
// keep in the source list for every build.

#include "sigtap.h"

#if SIGTAP

#include <string.h>
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "esp_timer.h"

// Always-on UART0 writer from ipc.c (ungated by COM3_LOG).
extern int km_uart_write_raw(const void *data, size_t len);

// ---- record ring -----------------------------------------------------------
// Single fixed-size ring. Producers (ipc_rx_task on core0, TinyUSB xfer_cb on
// core1, the SYNC esp_timer task) push under a portMUX spinlock; the drain task
// pops + serialises + writes outside the lock. On overrun the newest record is
// dropped and counted (never block the measured path).

typedef struct {
    uint8_t  type;
    uint8_t  rlen;
    uint32_t seq;
    int64_t  t_cap_us;
    int64_t  t_sub_us;
    uint8_t  report[SIGTAP_MAX_REPORT];
} rec_t;

#define RING_N 512u   // power of two
static rec_t   ring[RING_N];
static volatile uint32_t ring_head;        // producer write index
static volatile uint32_t ring_tail;        // consumer read index
static volatile uint32_t drops_overrun;    // ring-full drops
static volatile uint32_t drops_coalesce;   // reports coalesced before TX
static uint32_t seq_ctr;

static portMUX_TYPE ring_lock = portMUX_INITIALIZER_UNLOCKED;

// Capture stash — written and read only on ipc_rx_task, so no lock needed.
static volatile int64_t cap_us_stash;

void sigtap_mark_capture(void) {
    cap_us_stash = esp_timer_get_time();
}

int64_t sigtap_last_capture(void) {
    return cap_us_stash;
}

static void ring_push(uint8_t type, int64_t t_cap, int64_t t_sub,
                      const uint8_t *report, uint16_t len) {
    portENTER_CRITICAL(&ring_lock);
    uint32_t head = ring_head;
    uint32_t next = (head + 1u) & (RING_N - 1u);
    if (next == ring_tail) {        // full
        drops_overrun++;
        portEXIT_CRITICAL(&ring_lock);
        return;
    }
    rec_t *r   = &ring[head];
    r->type    = type;
    r->seq     = ++seq_ctr;
    r->t_cap_us = t_cap;
    r->t_sub_us = t_sub;
    uint16_t n = len > SIGTAP_MAX_REPORT ? SIGTAP_MAX_REPORT : len;
    r->rlen    = (uint8_t)n;
    if (n) memcpy(r->report, report, n);
    ring_head  = next;
    portEXIT_CRITICAL(&ring_lock);
}

void sigtap_report(int64_t t_cap_us, int64_t t_sub_us,
                   const uint8_t *report, uint16_t len) {
    ring_push(SIGTAP_TYPE_TAP, t_cap_us, t_sub_us, report, len);
}

void sigtap_note_drop(void) {
    drops_coalesce++;
}

// ---- serialisation ---------------------------------------------------------

static uint16_t crc16_ccitt(const uint8_t *d, size_t n) {
    uint16_t crc = 0xFFFF;
    for (size_t i = 0; i < n; ++i) {
        crc ^= ((uint16_t)d[i]) << 8;
        for (int b = 0; b < 8; ++b)
            crc = (crc & 0x8000) ? (uint16_t)((crc << 1) ^ 0x1021)
                                 : (uint16_t)(crc << 1);
    }
    return crc;
}

static inline void put_u32(uint8_t *b, int *o, uint32_t v) {
    b[(*o)++] = (uint8_t)(v);
    b[(*o)++] = (uint8_t)(v >> 8);
    b[(*o)++] = (uint8_t)(v >> 16);
    b[(*o)++] = (uint8_t)(v >> 24);
}

static inline void put_i64(uint8_t *b, int *o, int64_t s) {
    uint64_t v = (uint64_t)s;
    for (int i = 0; i < 8; ++i) b[(*o)++] = (uint8_t)(v >> (8 * i));
}

static void emit(const rec_t *r) {
    uint8_t b[2 + 1 + 1 + 4 + 8 + 8 + 8 + 1 + SIGTAP_MAX_REPORT + 2];
    int o = 0;
    b[o++] = SIGTAP_MAGIC0;
    b[o++] = SIGTAP_MAGIC1;
    int crc_start = o;                       // CRC covers version..last report
    b[o++] = SIGTAP_VERSION;
    b[o++] = r->type;
    put_u32(b, &o, r->seq);
    put_i64(b, &o, r->t_cap_us);
    put_i64(b, &o, r->t_sub_us);
    put_i64(b, &o, esp_timer_get_time());    // t_emit: stamped at TX time
    b[o++] = r->rlen;
    for (uint8_t i = 0; i < r->rlen; ++i) b[o++] = r->report[i];
    uint16_t crc = crc16_ccitt(&b[crc_start], (size_t)(o - crc_start));
    b[o++] = (uint8_t)(crc);
    b[o++] = (uint8_t)(crc >> 8);
    km_uart_write_raw(b, (size_t)o);
}

// ---- drain task + SYNC beacon ---------------------------------------------

static void drain_task(void *arg) {
    (void)arg;
    for (;;) {
        bool any = false;
        for (;;) {
            rec_t local;
            portENTER_CRITICAL(&ring_lock);
            if (ring_tail == ring_head) { portEXIT_CRITICAL(&ring_lock); break; }
            local     = ring[ring_tail];
            ring_tail = (ring_tail + 1u) & (RING_N - 1u);
            portEXIT_CRITICAL(&ring_lock);
            emit(&local);
            any = true;
        }
        if (!any) vTaskDelay(1);   // 1 kHz tick → ~1 ms idle poll
    }
}

static void sync_cb(void *arg) {
    (void)arg;
    ring_push(SIGTAP_TYPE_SYNC, 0, 0, NULL, 0);
}

void sigtap_init(void) {
    xTaskCreatePinnedToCore(drain_task, "sigtap", 4096, NULL, 1, NULL, 0);
    static esp_timer_handle_t h;
    const esp_timer_create_args_t args = {
        .callback = &sync_cb,
        .name     = "sigtap_sync",
    };
    if (esp_timer_create(&args, &h) == ESP_OK)
        esp_timer_start_periodic(h, 50000);   // 50 ms clock-sync beacons
}

#endif  // SIGTAP
