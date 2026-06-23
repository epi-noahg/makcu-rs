# 05 — RIGHT MCU (host USB)

Projet : `MAKCM_ESP32s3_Pass_Right` — **Arduino-on-ESP32** + client ESP-IDF
`usb_host` (inclus via le core bundled). Right est purement USB host + IPC ;
il ne touche **jamais** au KM.

Fichiers : `main.cpp`, `ipc.cpp` (voir `02`), `PassUsbHost.cpp`,
`PassUsbHost.h`, `diag.cpp`, `diag.h`.

---

## A. `main.cpp` — entrée + dispatch IPC

### Allocation des périphériques
| Périphérique | Usage |
|--------------|-------|
| USB-OTG | **HOST** → vrai contrôleur |
| `Serial` | USB-Serial-JTAG, **silencieux en mode host** (partage les pins de l'OTG) |
| `Serial1` (`IpcSerial`) | UART1 @ 5 Mbps → Left. RX=GPIO2, TX=GPIO1 |

### `setup()`
1. `Serial.begin(115200)` (peu utile en host).
2. `IpcSerial.begin(5_000_000, SERIAL_8N1, RX=2, TX=1)`.
3. `diag_setup()` — **LED en premier** pour voir la liveness même si
   `usb_host_install` se bloque.
4. `pass_host.begin()` — installe `usb_host`, enregistre le client, spawne
   `usb_lib` + `usb_client` sur core 1.
5. crée `ipc_rx` (core 0, prio 5).
6. envoie `FRAME_LOG "Pass_Right boot\n"`.

### `loop()`
Idle : `delay(10)`. Tout le travail est dans les tâches.

### `ipc_handle_frame(type, ep, seq, payload, len)` (dispatch des trames de Left)
| type | action |
|------|--------|
| `FRAME_CTRL_SETUP` | si len>=8 : `submit_control(payload, data?, data_len, seq)` (data = payload+8 si len>8) |
| `FRAME_EP_OUT` | `submit_out(ep, payload, len)` |
| `FRAME_PING` | echo |
| `FRAME_LOG` | `Serial.write` (peu visible en host) |

### `ipc_rx_task`
Boucle : `ipc_pump_serial()` puis `vTaskDelay(1ms)`. (Le déframer + watchdog
10 ms sont dans `ipc.cpp`, voir `02`.)

---

## B. `PassUsbHost.cpp` — wrapper usb_host

Instance globale `PassUsbHost pass_host`.

### État privé (PassUsbHost.h)
| Membre | Rôle |
|--------|------|
| `client_handle_` | handle client usb_host |
| `device_handle_` | handle de l'appareil ouvert |
| `device_connected_` | appareil présent (flag de garde anti-double-free) |
| `ready_` | DEVICE_READY émis |
| `in_transfers_[8]` / `in_transfer_count_` | URB IN persistants (re-soumis) |
| `claimed_ifs_[4]` / `claimed_if_count_` | interfaces claimées |

`PASS_MAX_ENDPOINTS = 8`, `PASS_MAX_INTERFACES = 4`.

### `begin()`
- `usb_host_install({skip_phy_setup=false, intr_flags=LEVEL1})`.
- `diag_on_host_install(ok)`.
- crée `usb_lib` (core1 prio5) et `usb_client` (core1 prio5).

### `lib_task`
Boucle `usb_host_lib_handle_events(portMAX_DELAY)`. **Enregistre le client
seulement après le premier step** de la state machine (l'enregistrer trop tôt
peut rater les NEW_DEV d'appareils déjà branchés). À l'enregistrement :
`diag_on_client_registered()`.

### `client_task`
Boucle `usb_host_client_handle_events(portMAX_DELAY)` (dispatche les callbacks
client : NEW_DEV, DEV_GONE, et les completions de transfert).

### `client_event_cb`
- `NEW_DEV` ⇒ `on_new_device(addr)`.
- `DEV_GONE` ⇒ `on_device_gone()`.

### `on_new_device(address)`
1. `device_connected_ = true`, `diag_on_new_dev`.
2. `usb_host_device_open` → `device_handle_`.
3. `fetch_and_relay_descriptors()`.
4. Si échec : envoie `FRAME_DEVICE_GONE` (sinon Left attend un READY qui ne
   vient jamais), `release_all`, return.
5. Sinon : `FRAME_DEVICE_READY`, `ready_=true`, `diag_on_device_ready`.

### `fetch_and_relay_descriptors()` — le snapshot
1. `usb_host_get_device_descriptor` → `FRAME_DESC_DEVICE` (18 B).
2. `usb_host_get_active_config_descriptor` → `FRAME_DESC_CONFIG`
   (`wTotalLength`).
3. Collecte les **index de strings** référencés : iManufacturer, iProduct,
   iSerialNumber, iConfiguration, + chaque `iInterface` (parcours du config).
   `idx_set[16]`, déduplication via lambda `push_idx`.
4. `usb_host_device_info(&dinfo)` : expédie les strings **résolues par
   ESP-IDF** (`str_desc_manufacturer/product/serial_num`) via `ship_one` →
   `FRAME_DESC_STRING`. `ship_one` saute l'entête 2 B (`sd->val + 2`) pour
   n'envoyer que le corps UTF-16LE.
   - ⚠️ Limitation : seules manufacturer/product/serial sont expédiées, **pas**
     les strings d'interface ni la 0xEE (le code collecte les index mais
     n'expédie via dinfo que ces 3). Voir piège dans `08`.
5. `diag_on_descriptors_sent`.
6. **Claim + ouverture** : parcourt le config, pour chaque interface
   `bAlternateSetting==0` (et `claimed_if_count_ < 4`) :
   `usb_host_interface_claim(iface, 0)` puis
   `open_all_endpoints_for_interface(cfg, iface, 0)`.

### `open_all_endpoints_for_interface(cfg, iface, alt)`
Parcourt les descripteurs de (iface, alt). Pour chaque endpoint **IN** (et
`in_transfer_count_ < 8`) :
- `usb_host_transfer_alloc(mps, 0, &t)`,
- configure `t` (device_handle, bEndpointAddress, num_bytes=mps,
  callback=`in_xfer_complete`, context=this),
- `usb_host_transfer_submit(t)`.

Les EP **OUT** ne sont pas pré-ouverts ici ; ils sont alloués à la demande
dans `submit_out`.

### `in_xfer_complete(t)` (completion IN)
- **Garde anti-double-free** : si status `NO_DEVICE` / `CANCELED` ou
  `!device_connected_` ⇒ **return** (pas de re-submit). ESP-IDF met chaque URB
  en vol à NO_DEVICE avant de tirer DEV_GONE.
- Si `COMPLETED && actual_num_bytes > 0` ⇒ `FRAME_EP_IN` (ep, data, nbytes).
- Re-soumet l'URB (`usb_host_transfer_submit`).

### `out_xfer_complete(t)`
`usb_host_transfer_free(t)` (les URB OUT sont jetables, alloués par submit).

### `submit_out(ep, data, len)`
Alloue un URB de `len`, copie data, callback=`out_xfer_complete`, submit. Free
en cas d'échec. Garde : `device_connected_ && device_handle_`.

### `submit_control(setup[8], data_out, len, seq)`
- **Tout chemin d'échec émet `FRAME_CTRL_STATUS`** (lambda `fail`) — sinon
  `ctl_pending` de Left reste bloqué et tout le contrôle se fige. **C'est
  l'invariant le plus important côté Right.**
- Lit `wLength = setup[6]|setup[7]<<8`, `is_in = setup[0]&0x80`.
- `data_stage = is_in ? wLength : len`.
- OUT : rejette si `len != wLength` (`OUT-LEN-MISMATCH`).
- Alloue `8 + data_stage`, copie setup (+ data si OUT), `bEndpointAddress=0`,
  `context = seq`, `usb_host_transfer_submit_control`.
- Échec submit ⇒ free + `FRAME_CTRL_STATUS(XFER_ERROR)`.

### `control_xfer_complete(t)` (completion contrôle)
- `seq = (uint16_t)t->context`.
- Si `COMPLETED` :
  - si `actual_num_bytes > 8` ⇒ `FRAME_CTRL_IN_DATA(seq, data+8, n-8)`.
  - `FRAME_CTRL_STATUS(seq, XFER_OK)`.
- Sinon : map status (STALL/TIMED_OUT/ERROR) → `FRAME_CTRL_STATUS`.
- `usb_host_transfer_free(t)`.

### `on_device_gone()` / `release_all()`
- `ready_=false`, `device_connected_=false`, `release_all`, `FRAME_DEVICE_GONE`.
- `release_all` :
  1. `device_connected_ = false` **en premier** (les completions in-flight
     bailent proprement).
  2. pour chaque URB IN : `endpoint_halt` + `endpoint_flush` (ceinture +
     bretelles anti-double-free) puis `transfer_free`.
  3. release chaque interface claimée.
  4. `usb_host_device_close`.

---

## C. `diag.cpp` / `diag.h` — LED RGB d'état

Right `Serial` est muet en host ⇒ la LED RGB (GPIO 48) est la **seule
visibilité directe** sur la progression du pipeline.

### Flags d'état (mis par les hooks `diag_on_*`)
`diag_on_host_install`, `_client_registered`, `_new_dev`, `_descriptors_sent`,
`_device_ready`, `_ipc_tx_bytes`.

### Machine à états (`current_state`, ordre décroissant de priorité)
| État | Valeur | Condition | Pattern LED |
|------|--------|-----------|-------------|
| `DS_DEVICE_READY` | 5 | device_ready | blanc fixe (180,180,180) |
| `DS_DESC_SENT` | 4 | descriptors_sent | double-flash cyan/bleu (période 1250 ms) |
| `DS_NEW_DEV` | 3 | new_dev_seen | double-flash vert (période 1000 ms) |
| `DS_CLIENT` | 2 | client_registered | jaune 0.5 Hz |
| `DS_HOST_INSTALL` | 1 | host_install_ok | orange ~2.5 Hz |
| `DS_IDLE` | 0 | (rien) | rouge rapide (crash/loop) |

- `led_rgb` : `rgbLedWrite` (Arduino core ≥ 3) ou `neopixelWrite` (sinon).
- `diag_led_task` (core0 prio1) : `pattern_step(state, millis())` toutes les
  10 ms.
- `PASS_RIGHT_DIAG_PLAIN_PIN = -1` (pas de LED simple ; serait gérée si ≥ 0).

> Séquence nominale au branchement : IDLE → HOST_INSTALL → CLIENT → NEW_DEV →
> DESC_SENT → DEVICE_READY (blanc fixe).

---

## D. Différences Arduino vs ESP-IDF (à garder en tête pour modifier Right)

- Right est **Arduino-framework** : `setup()`/`loop()`, `HardwareSerial`,
  `millis()`, `pinMode`, mais utilise directement l'API C `usb/usb_host.h`
  d'ESP-IDF (incluse via le core).
- Pas de `app_main` ; le point d'entrée est `setup()`.
- Les logs passent par IPC (`FRAME_LOG`) car `Serial` est muet en host.
- `diag.cpp`/`.h` exposent les hooks en `extern "C"` pour être appelables
  depuis du C si besoin.
