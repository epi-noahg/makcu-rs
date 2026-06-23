# 01 — Architecture système

## 1. Vue d'ensemble

Le but : un vrai contrôleur de jeu (Scuf / Xbox One / DualSense / Xbox 360…)
est branché derrière un passthrough USB **transparent** à deux ESP32-S3. Le
PC cible voit **exactement** le VID/PID, les descripteurs et le protocole du
vrai contrôleur. Un **second PC** envoie des commandes « km » (style kmbox,
souris→stick) qui sont **mélangées** dans le flux de rapports USB pendant que
l'utilisateur tient toujours physiquement le contrôleur.

```
[ PC CIBLE ]
     |  USB full-speed (le contrôleur "réel" tel que vu par Windows)
     v
+------------+   UART1 IPC    +------------+   USB-OTG host  +-------------------+
|  LEFT MCU  |<-------------->|  RIGHT MCU |<--------------->|  vrai contrôleur  |
| (USB device)|   5 Mbps 8N1  | (USB host) |                 |  (Scuf/DS5/360...) |
+------------+                +------------+                 +-------------------+
     |
     | UART0 "KM" 4 Mbaud 8N1
     v
  [ CH343 ]  (pont USB-UART)
     |  USB
     v
[ 2e PC, COM3 ]
  logiciel km-sender  (envoie km.move / km.click / km.btnX ...)
```

**Idée clé** : LEFT n'a aucune connaissance native du contrôleur. RIGHT
énumère le vrai appareil, prend un *snapshot* de ses descripteurs, et les
**expédie à LEFT par IPC**. LEFT démarre alors TinyUSB avec ces descripteurs,
si bien que Windows énumère contre les vrais VID/PID/strings. Ensuite tout le
trafic USB (IN, OUT, contrôle) est **relayé en direct** entre les deux côtés.
L'injection KM ne touche que les octets de stick droit et certains boutons
dans les rapports IN dont le **format** est reconnu.

## 2. Rôles détaillés des deux MCU

### LEFT = device USB (`MAKCM_ESP32s3_Pass_Left_IDF`, pur ESP-IDF)
Possède :
- Le **cache de descripteurs** (rempli par les trames IPC de Right) —
  `main.c`.
- La **pile TinyUSB** + un **driver de classe custom** (`pass_usb_device.c`)
  qui ouvre tous les endpoints déclarés et relaie le trafic.
- Le **pipeline km_inject** (`km_inject.c`) : accumulateur 8 ms, blend,
  courbe XIM, adaptateurs de protocole.
- Le **canal de commandes KM** (UART0, parser ASCII ligne-à-ligne).
- Le **timer de synthèse** (4 ms) qui rejoue le dernier rapport réel + injection.

### RIGHT = host USB (`MAKCM_ESP32s3_Pass_Right`, Arduino + client IDF usb_host)
Possède :
- Le client **`usb_host`** ESP-IDF (énumère / claim / submit).
- Le **snapshot de descripteurs** et leur expédition IPC.
- Le **forwarding d'URB** (IN/OUT/contrôle) de/vers Left.
- La **LED de diagnostic RGB** (machine à états du pipeline host).
- **Right ne touche jamais au KM.** C'est purement USB host + IPC.

## 3. Les trois canaux de communication

| Canal | Support | Débit | Format | Sens |
|-------|---------|-------|--------|------|
| **USB device** | LEFT ↔ PC cible | Full-speed (12 Mbps) | USB natif | bidir |
| **IPC** | LEFT ↔ RIGHT | UART1 @ 5 Mbps 8N1 | trame binaire (voir `02`) | bidir |
| **KM** | LEFT ↔ 2e PC | UART0 @ 4 Mbaud 8N1 (via CH343) | ASCII ligne | entrant surtout |
| **USB host** | RIGHT ↔ vrai contrôleur | Full-speed | USB natif | bidir |

Le canal KM sert aussi de **sortie de log** : les logs de Right sont
tunnellisés par IPC (`FRAME_LOG`) jusqu'à Left, qui les réémet sur UART0 →
COM3, préfixés `[R] `. Les logs de Left sortent directement sur UART0,
préfixés `[L] `.

## 4. Flux de bout en bout (séquence nominale)

### 4.1 Branchement et énumération
1. RIGHT démarre, installe `usb_host`, enregistre un client (`PassUsbHost::begin`).
2. Vrai contrôleur branché → événement `USB_HOST_CLIENT_EVENT_NEW_DEV`.
3. `on_new_device` ouvre l'appareil, appelle `fetch_and_relay_descriptors` :
   - lit le descripteur device (18 B) → `FRAME_DESC_DEVICE`
   - lit le config descriptor complet → `FRAME_DESC_CONFIG`
   - expédie les strings (fabricant/produit/série) → `FRAME_DESC_STRING`
   - **claim** chaque interface alt-0 + soumet une transaction IN sur chaque
     endpoint IN.
4. RIGHT envoie `FRAME_DEVICE_READY`.
5. LEFT (`ipc_handle_frame`) remplit son cache, met `device_ready=true`, réveille
   `main_task`.
6. `main_task` appelle `start_usb()` → `tinyusb_driver_install` la première
   fois (ensuite `tinyusb_set_descriptors` + `tud_connect`). Le D+ pull-up est
   asserté → Windows énumère contre les vrais descripteurs.

### 4.2 Trafic de rapports (gameplay)
- **IN (contrôleur → PC cible)** :
  - RIGHT : `in_xfer_complete` → `FRAME_EP_IN` vers Left.
  - LEFT : `ipc_handle_frame` → `pass_usb_submit_in` → `km_apply` (injection)
    → `usbd_edpt_xfer` vers le PC cible.
- **OUT (PC cible → contrôleur)** :
  - LEFT : `pass_driver_xfer_cb` (OUT) → `FRAME_EP_OUT` vers Right.
  - RIGHT : `submit_out` → URB vers le vrai contrôleur.
- **Contrôle (PC cible ↔ contrôleur)** :
  - LEFT capture le SETUP → `FRAME_CTRL_SETUP`.
  - RIGHT `submit_control` → vrai appareil → `FRAME_CTRL_IN_DATA` +
    `FRAME_CTRL_STATUS` retour.
  - LEFT termine le transfert de contrôle (`tud_control_xfer`).

### 4.3 Injection KM
- 2e PC envoie `km.move(dx,dy)` sur COM3 → CH343 → UART0 de Left.
- `km_uart_task` lit la ligne → `km_ingest_raw` → `parse_km_text` →
  `applyMouseDelta` accumule dans `g_vel_accum_{x,y}`.
- Toutes les **8 ms**, `km_housekeep_cb` (esp_timer) draine l'accumulateur via
  `xim_curve` → `rx_injected/ry_injected`, puis remet à zéro.
- À chaque rapport IN sortant, `km_apply` extrait le stick physique, `blend`
  avec l'injecté, et réécrit les octets.
- Le **timer de synthèse** (4 ms) rejoue le dernier rapport réel avec
  injection si le contrôleur GIP est silencieux et qu'une injection est active.

### 4.4 Débranchement / hot-swap
- Vrai contrôleur débranché → `USB_HOST_CLIENT_EVENT_DEV_GONE` sur Right →
  `release_all` + `FRAME_DEVICE_GONE`.
- LEFT : `teardown_pending=true`, `main_task` exécute `pass_usb_disconnect`
  (drop du D+ via `tud_disconnect`) + `km_reset_injection`. La pile TinyUSB
  **reste installée à vie** (voir piège esp_tinyusb 1.4.x).
- Nouveau contrôleur branché → tout le cycle FRAME_DESC_* recommence, Left
  fait `tinyusb_set_descriptors` + `tud_connect` (pas de réinstall PHY).

## 5. Placement cœurs / tâches / priorités

### LEFT (ESP-IDF, dual-core)
| Tâche | Cœur | Priorité | Rôle | Créée dans |
|-------|------|----------|------|-----------|
| `ipc_rx` | 0 | 5 | Lit UART1, déframe IPC | `ipc_init` |
| `km_uart` | 0 | 4 | Lit UART0, parse lignes KM | `ipc_init` |
| `km_drain` | 0 | 2 | Drain du ring-buffer trace (build KM_RING) | `km_ring_init` |
| `main` | 1 | 3 | Cycle de vie USB (install/connect/disconnect) | `app_main` |
| `led` | 1 | 1 | Heartbeat LED | `app_main` |
| TinyUSB | **1** | (interne) | Pile device + callbacks driver | sdkconfig `CPU1` |
| `km_hk` (esp_timer) | (tâche esp_timer) | haute | `km_housekeep_cb` toutes 8 ms | `km_init` |
| `km_synth` (esp_timer) | (tâche esp_timer) | haute | `synth_cb` toutes 4 ms | `synth_timer_start_once` |

> **Choix d'affinité crucial** : TinyUSB est épinglé sur CPU1
> (`CONFIG_TINYUSB_TASK_AFFINITY_CPU1=y`) pour ne pas affamer la tâche
> esp_timer (sur CPU0) qui cadence `km_housekeep_cb`. Le tick FreeRTOS est à
> **1 kHz** (`CONFIG_FREERTOS_HZ=1000`) pour que les `vTaskDelay(1ms)` et les
> timeouts UART d'1 tick valent réellement 1 ms (au lieu de 10 ms au tick
> 100 Hz par défaut). C'est ce qui a fait passer la latence km.move→stick de
> ~12 ms à ~3 ms.

### RIGHT (Arduino + IDF)
| Tâche | Cœur | Priorité | Rôle | Créée dans |
|-------|------|----------|------|-----------|
| `usb_lib` | 1 | 5 | `usb_host_lib_handle_events` + enregistrement client | `begin` |
| `usb_client` | 1 | 5 | `usb_host_client_handle_events` (callbacks NEW_DEV/DEV_GONE/xfer) | `begin` |
| `ipc_rx` | 0 | 5 | Pompe `IpcSerial` + déframe | `setup` |
| `diag_led` | 0 | 1 | Machine à états LED RGB | `diag_setup` |
| `loop()` (Arduino) | (par défaut) | — | idle (`delay(10)`) | core Arduino |

> Right sépare volontairement les tâches USB host (core 1) de l'IPC et de la
> LED (core 0).

## 6. Synchronisation / concurrence

- **LEFT** :
  - `km_state_lock` (portMUX) protège `rx/ry_injected`, `rx/ry_physical`,
    `g_vel_accum_*`. Collisionné par `km_uart_task`, `km_housekeep_cb`,
    `km_apply` (TinyUSB).
  - `io_lock` (portMUX) protège `in_flight`, `in_pending_*` par endpoint.
    Collisionné par `ipc_rx_task` (submitter) et la tâche TinyUSB (xfer_cb).
  - `ipc_tx_mutex` (sémaphore) sérialise les écritures UART1.
  - `km_ring_lock` (portMUX) protège le ring-buffer trace.
  - Compteurs diag : `_Atomic` (pas de lock).
- **RIGHT** :
  - `ipc_tx_mutex` (sémaphore) sérialise `ipc_send` (appelé depuis 3 sources
    sur 2 cores : callbacks USB completion, ipc_rx_task, callbacks lib).
  - Flags diag : `volatile`.

## 7. Invariants système à respecter avant toute modif

1. **Un seul transfert de contrôle en vol** (`ctl_pending`). Right sérialise ;
   Left rejette un 2e SETUP tant qu'un est pending (avec un stale-clear à 2 s).
2. **TinyUSB reste installé à vie** sur Left ; hot-swap = `tud_disconnect`/
   `tud_connect` seulement.
3. **Le cache de descripteurs doit être complet** (`desc_device_valid` &&
   `desc_config_valid` && `device_ready`) avant `start_usb`.
4. **Toute opération de cycle de vie USB se fait sur `main_task`** (même thread
   que `start_usb`), via `teardown_pending` + `xTaskNotifyGive`.
5. **Tout chemin d'échec côté Right DOIT émettre `FRAME_CTRL_STATUS`** sinon
   `ctl_pending` reste bloqué sur Left → EP0 STALL → freeze apparent.
6. **Saturation symétrique ±32767** partout dans le pipeline d'injection.

Voir `08_PIEGES_ET_TODO.md` pour le détail et les conséquences.
