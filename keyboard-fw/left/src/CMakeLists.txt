idf_component_register(
    SRCS "main.c" "ipc.c" "pass_usb_device.c" "km_platform.c"
         "../../core/keymap.c"
         "../../core/km_protocol.c"
         "../../core/kbd_inject.c"
         "../../core/cfgdesc.c"
         "../../core/km_glue.c"
    INCLUDE_DIRS "." "../include" "../../core"
    REQUIRES driver esp_tinyusb esp_timer
)
