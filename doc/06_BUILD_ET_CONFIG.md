# 06 — Build, configuration, pinout

## 1. Outils

Les deux projets utilisent **PlatformIO**, plateforme `espressif32 @ 6.7.0`.

- **LEFT** : `framework = espidf` (pur ESP-IDF). TinyUSB via composant
  `espressif/esp_tinyusb ~1.4.4` (déclaré dans `src/idf_component.yml`).
- **RIGHT** : `framework = arduino` (+ client `usb_host` IDF du core).

## 2. Commandes de build

```bash
# LEFT — build par défaut (voir note ci-dessous sur log vs quiet) :
pio run -d MAKCM_ESP32s3_Pass_Left_IDF -e LEFT_IDF

# LEFT — build quiet explicite (gameplay) :
PLATFORMIO_BUILD_FLAGS="-DCOM3_LOG=0 -DKM_DIAG=0 -DKM_RING=0 -DLAT_DIAG=0" \
  pio run -d MAKCM_ESP32s3_Pass_Left_IDF -e LEFT_IDF

# RIGHT :
pio run -d MAKCM_ESP32s3_Pass_Right -e RIGHT
```

> Flash : `pio run -t upload -e <env> -d <projet>`. Vitesse upload 921600.

## 3. Drapeaux de build LEFT (`platformio.ini`)

```ini
[env:LEFT_IDF]
platform = espressif32 @ 6.7.0
board    = MAKCM
framework = espidf
board_build.partitions = partitions/partition_MAKCM.csv
build_flags =
  -DFIRMWARE_VERSION=\"V1_2_Pass_IDF\"
  -DCOM3_LOG=0
```

### ⚠️ Incohérence log/quiet (À CONNAÎTRE)
Le `README.md` racine dit « Left, default = log build ». **C'est faux pour ce
checkout** : `platformio.ini` force `-DCOM3_LOG=0`, donc le build par défaut a
le **gate de log coupé** (`km_uart_write` no-op). En revanche `KM_DIAG`,
`KM_RING`, `LAT_DIAG` restent à leur défaut source **1** (non surchargés).

Résultat concret du build par défaut actuel :
- `COM3_LOG=0` ⇒ `km_uart_write` ne sort rien sur UART0…
- …mais `KM_RING=1` ⇒ la drain task **appelle `km_uart_write`** pour drainer le
  ring → ces appels sont eux aussi no-op (gate COM3_LOG). Idem KM_DIAG/LAT_DIAG.
- Donc en pratique : **aucune sortie diag** sur COM3 sauf les réponses
  `km.version()` (qui passent par `km_uart_write_raw`, non gaté).

Le « vrai » build log nécessiterait `-DCOM3_LOG=1` (ou retirer la surcharge).
Voir piège #2 dans `08_PIEGES_ET_TODO.md`.

### Gates et leur défaut
| Gate | `#ifndef` défaut | platformio.ini | Effet si 1 |
|------|------------------|----------------|------------|
| `COM3_LOG` | 1 | **0** | `km_uart_write` écrit sur UART0 |
| `KM_DIAG` | 1 | (non surchargé → 1) | snapshot 5 Hz |
| `KM_RING` | 1 | (non surchargé → 1) | ring trace 64 KB |
| `LAT_DIAG` | 1 | (non surchargé → 1) | stats latence IN |

> `KM_GAIN_C` / `KM_GAIN_P` vivent dans `km_inject.c`. Surcharge possible par
> `-DKM_GAIN_C=` / `-DKM_GAIN_P=` pour re-tuner la courbe.

## 4. Drapeaux de build RIGHT (`platformio.ini`)

```ini
[env:RIGHT]
platform = espressif32 @ 6.7.0
board = MAKCM
framework = arduino
board_build.partitions = partitions/partition_MAKCM.csv
lib_deps =
  ArduinoJson
  locoduino/RingBuffer@^1.0.5
build_flags =
  -DUSB_IS_DEBUG=false
  -DFIRMWARE_VERSION="V1_2_Pass"
  -DCONTROLLER_TYPE_PASS=1
  -I include
```

> ⚠️ `ArduinoJson` et `RingBuffer` sont déclarés en `lib_deps` mais **ne sont
> pas utilisés** par le code source actuel (`PassUsbHost.cpp` envoie du binaire,
> pas du JSON). Résidu d'un ancien firmware. Voir piège #6.

## 5. `sdkconfig.defaults` (LEFT) — points critiques

```
# Désactive tous les class drivers TinyUSB builtin (sinon ils pré-réservent
# des adresses d'EP qui collisionneraient avec les descripteurs du vrai
# contrôleur). Seul notre driver custom (usbd_app_driver_get_cb) est actif.
CONFIG_TINYUSB_CDC_ENABLED=n
CONFIG_TINYUSB_MSC_ENABLED=n
CONFIG_TINYUSB_HID_ENABLED=n
CONFIG_TINYUSB_MIDI_ENABLED=n
CONFIG_TINYUSB_VENDOR_ENABLED=n
CONFIG_TINYUSB_DFU_RT_ENABLED=n
CONFIG_TINYUSB_DFU_ENABLED=n

CONFIG_TINYUSB_RHPORT_HS=n          # full-speed only

CONFIG_BOOTLOADER_LOG_LEVEL_WARN=y
CONFIG_ESP_CONSOLE_NONE=y           # console silencieuse → LED = seul signal

CONFIG_ESPTOOLPY_FLASHSIZE_4MB=y

CONFIG_TINYUSB_TASK_AFFINITY_CPU1=y # TinyUSB sur CPU1 (n'affame pas esp_timer/CPU0)

CONFIG_FREERTOS_HZ=1000             # tick 1 kHz → latence km.move→stick ~12ms→~3ms
```

Ces réglages sont **structurants** :
- Désactiver les class drivers builtin est **obligatoire** : sinon collision
  d'adresses d'endpoints.
- `CPU1` pour TinyUSB + tick 1 kHz = budget temporel du pipeline d'injection.
- `ESP_CONSOLE_NONE` ⇒ pas de log console ; la diag passe par UART0 (gaté) et
  la LED.

## 6. Partitions

Les deux projets utilisent `partitions/partition_MAKCM.csv` (layout
« huge_app » 4 MB, **pas d'OTA**) :

| Partition | Type | Offset | Taille |
|-----------|------|--------|--------|
| nvs | data/nvs | 0x9000 | 0x5000 |
| otadata | data/ota | 0xe000 | 0x2000 |
| app0 | app/factory | 0x10000 | 0x300000 (3 MB) |
| spiffs | data/spiffs | 0x310000 | 0xE0000 |
| coredump | data/coredump | 0x3F0000 | 0x10000 |

> `coredump` est utile en embarqué : en cas de panic, le core dump est
> sauvegardé là et lisible via `pio run -t ... ` / `espcoredump`.

## 7. Boards

| Board JSON | Projet | Flash | PSRAM | USB_MODE | CDC_ON_BOOT |
|------------|--------|-------|-------|----------|-------------|
| `Left_IDF/boards/MAKCM.json` | LEFT | 4 MB | qio_qspi (HAS_PSRAM) | 0 | 0 |
| `Right/boards/MAKCM.json` | RIGHT | (4MB) | — | — | — |
| `Right/boards/Devkit.json` | RIGHT (alt) | 16 MB | opi | 1 | 1 |

- LEFT `MAKCM.json` : `ARDUINO_USB_MODE=0` (le port USB-OTG est piloté par
  TinyUSB device, pas le CDC).
- RIGHT `Devkit.json` : variante 16 MB Devkit, `USB_MODE=1 + CDC_ON_BOOT=1`
  (route le `Serial` vers USB-Serial-JTAG — d'où le mutisme en host).
- MCU : esp32s3, f_cpu 240 MHz, f_flash 80 MHz, mode QIO.
- HWID `303A:1001` (Espressif).

## 8. Pinout complet

| Signal | LEFT (ESP32-S3) | RIGHT (ESP32-S3) |
|--------|-----------------|------------------|
| IPC UART1 TX | GPIO 2 | GPIO 1 |
| IPC UART1 RX | GPIO 1 | GPIO 2 |
| KM UART0 TX (→ COM3) | GPIO 43 | — |
| KM UART0 RX (← COM3) | GPIO 44 | — |
| Diag LED | GPIO 48 (simple) | GPIO 48 (RGB néopixel) |
| USB-OTG (D+/D−) | → PC cible | → vrai contrôleur |

- IPC : 5 Mbps 8N1, pas de flow control, buffer RX 8192.
- KM : 4 Mbaud 8N1 (= 80 MHz APB / 20).

## 9. Ordre de connexion (rappel opérationnel)

1. Alimenter les deux MCU (USB du 2e PC via CH343 alimente Left ; Right
   alimenté par le PC cible ou son alim).
2. Brancher LEFT au PC cible.
3. Brancher le vrai contrôleur sur RIGHT.
4. Suivre les LEDs (Right : IDLE→…→DEVICE_READY blanc ; Left :
   lent→1.3→2→4→10 Hz).
5. PC cible énumère contre les vrais VID/PID/strings.
6. Ouvrir km-sender sur le 2e PC, pointer sur le port COM du CH343 (4 Mbaud 8N1).

Hot-swap contrôleur : débrancher du Right suffit (Left drop le D+, le PC cible
voit la déconnexion). Rebrancher relance le cycle **sans** replug de Left.

## 10. Versions / dépendances figées

| Composant | Version |
|-----------|---------|
| PlatformIO platform | espressif32 @ 6.7.0 |
| esp_tinyusb (Left) | ~1.4.4 |
| IDF (Left, requis) | >= 4.4 |
| FIRMWARE_VERSION Left | V1_2_Pass_IDF |
| FIRMWARE_VERSION Right | V1_2_Pass |
