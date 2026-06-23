# 03 — LEFT MCU (device USB)

Projet : `MAKCM_ESP32s3_Pass_Left_IDF` — **pur ESP-IDF** (framework=espidf),
TinyUSB via `espressif/esp_tinyusb ~1.4.4`.

Fichiers : `main.c`, `ipc.c`, `pass_usb_device.c`, `km_inject.c`
(km_inject documenté à part dans `04_KM_INJECT.md`).

---

## A. `main.c` — cycle de vie USB + cache descripteurs

### Variables d'état globales (statique fichier)
| Variable | Type | Rôle |
|----------|------|------|
| `desc_device` | `tusb_desc_device_t` | descripteur device (18 B) reçu de Right |
| `desc_device_valid` | bool | descripteur device reçu |
| `desc_config[1024]` | u8[] | config descriptor complet |
| `desc_config_len` | u16 | longueur réelle du config |
| `desc_config_valid` | bool | config reçu et cohérent |
| `device_ready` | bool | `FRAME_DEVICE_READY` reçu |
| `usb_started` | bool | `tinyusb_driver_install` a réussi **au moins une fois** |
| `host_visible` | bool | D+ pull-up asserté ⇒ Windows nous voit |
| `strings[8]` | `StringCache[]` | cache des string descriptors (idx, byte_len, utf16[128]) |
| `strings_count` | u8 | nb de strings en cache |
| `teardown_pending` | volatile bool | `FRAME_DEVICE_GONE` reçu, à traiter par main_task |
| `str_bufs[10]`, `str_storage[10][130]` | | tableaux ASCII passés à esp_tinyusb |

> **Important** : `tinyusb_config_t` garde des **pointeurs** vers
> `desc_device`, `desc_config` et `str_bufs` au moment de l'install. Ces
> buffers doivent rester valides à vie. C'est pourquoi ils sont statiques.

### `build_string_array(count_out)`
- esp_tinyusb veut des `char*` (ASCII), pas de l'UTF-16. Conversion best-effort :
  - `str_storage[0]` = LangID `0x0409` (`{0x09,0x04,0}`) en index 0.
  - Pour `want_idx` de 1 à 7 : cherche le string en cache d'index correspondant,
    convertit chaque paire UTF-16LE → ASCII si `hi==0 && 0x20<=lo<0x7F`, sinon
    `'?'`. Max 128 caractères. Non trouvé ⇒ chaîne vide.
- ⚠️ Conséquence : caractères non-ASCII remplacés par `?`. Les strings ne sont
  donc **pas byte-exact** (voir limitation `08`).

### `build_tusb_cfg(out, str_n_out)`
Remplit `tinyusb_config_t` : pointeurs device/config/strings, `external_phy=false`.
Si `TUD_OPT_HIGH_SPEED` (non actif ici, full-speed), duplique aussi en HS.

### `start_usb()` — le cœur du cycle de vie
Garde : ne fait rien si `!desc_device_valid || !desc_config_valid`.

- **Premier appel** (`!usb_started`) :
  - logue VID/PID/cfgLen/strCnt sur UART0,
  - `tinyusb_driver_install(&tusb_cfg)`,
  - si OK : `usb_started = true`, `host_visible = true`.
- **Appels suivants** (stack déjà installé, descripteurs peut-être changés) :
  - `tinyusb_set_descriptors(&tusb_cfg)` (rafraîchit les pointeurs internes
    d'esp_tinyusb **sans** réinstaller le PHY),
  - `pass_usb_reconnect()` (= `tud_connect`),
  - `host_visible = true`.

> `tinyusb_set_descriptors` est déclaré `extern` ici car c'est une fonction
> **privée** d'esp_tinyusb. Elle évite de re-run `tinyusb_driver_install`
> (qui réinitialiserait le PHY et déclencherait le bug INVALID_STATE).

### `ipc_handle_frame(type, ep, seq, payload, len)`
Dispatch des trames IPC reçues de Right. `poke_main` est mis à true quand il
faut réveiller `main_task` :

| type | action |
|------|--------|
| `FRAME_DESC_DEVICE` | si len==18 : copie desc_device, valid=true, poke |
| `FRAME_DESC_CONFIG` | valide len + wTotal==len, copie, poke |
| `FRAME_DESC_STRING` | stocke (idx, corps tronqué 128B) si <8 strings |
| `FRAME_DEVICE_READY` | `device_ready=true`, poke |
| `FRAME_DEVICE_GONE` | `teardown_pending=true`, poke |
| `FRAME_EP_IN` | `pass_usb_submit_in(ep, payload, len)` |
| `FRAME_CTRL_IN_DATA` | `pass_usb_control_in_complete(seq, payload, len)` |
| `FRAME_CTRL_STATUS` | `pass_usb_control_status(seq, status)` |
| `FRAME_KM_INJECT` | `km_ingest_raw` (chemin réservé, non utilisé en pratique) |
| `FRAME_LOG` | réémet `[R] ` + payload sur UART0 (+ `\n` si manquant) |
| `FRAME_PING` | echo `FRAME_PING` avec même seq |

> ⚠️ `FRAME_EP_IN`, `FRAME_CTRL_*` sont traités **directement dans le contexte
> de `ipc_rx_task`** (core 0, prio 5), pas sur main_task. Seules les opérations
> de cycle de vie (descripteurs, ready, gone) réveillent main_task.

### `led_task` (heartbeat)
Période selon l'état le plus avancé atteint :
| Condition | Période | Fréq |
|-----------|---------|------|
| `host_visible` | 100 ms | 10 Hz |
| `device_ready` | 250 ms | 4 Hz |
| `desc_config_valid` | 500 ms | 2 Hz |
| `desc_device_valid` | 750 ms | ~1.3 Hz |
| (rien) | 1500 ms | ~0.33 Hz |
LED sur GPIO 48.

### `main_task`
Boucle bloquée sur `ulTaskNotifyTake` :
- Si `teardown_pending` :
  - si `host_visible` : `pass_usb_disconnect()` + `km_reset_injection()` +
    `host_visible=false`. Sinon warning.
  - **ré-arme le staging** : invalide desc_device/config, reset strings_count,
    device_ready=false. `usb_started` **reste true** (seul le D+ a été lâché).
- Sinon, si `!host_visible && device_ready && desc_device_valid &&
  desc_config_valid` ⇒ `start_usb()`.

### `app_main`
`ipc_init()` → `km_init()` → crée `led` (core1 prio1) et `main` (core1 prio3).

---

## B. `ipc.c` — UART1 (IPC) + UART0 (KM)

(Le framing IPC est documenté dans `02_PROTOCOLE_IPC.md`.)

### Configuration UART
| Port | Usage | Baud | TX | RX | Buffer |
|------|-------|------|----|----|--------|
| UART_NUM_1 | IPC vers Right | 5 000 000 | GPIO 2 | GPIO 1 | 8192 |
| UART_NUM_0 | KM via CH343 | 4 000 000 | GPIO 43 | GPIO 44 | 8192 |

> Note baud KM : `4 Mbaud = 80 MHz APB / 20`, diviseur propre. Le CH343
> supporte le multi-Mbaud.

### Tâches
- `ipc_rx_task` (core0, prio5) : `uart_read_bytes(UART1, chunk[256], timeout=1
  tick)` puis `ipc_feed` octet par octet. Le **timeout littéral de 1 tick** (à
  1 kHz = 1 ms) est volontaire : `pdMS_TO_TICKS(1)` arrondirait à 0 tick au
  tick legacy 100 Hz → spin non-bloquant qui affamait TinyUSB.
- `km_uart_task` (core0, prio4) : lit UART0, accumule jusqu'à `\n`/`\r`, passe
  chaque ligne (non vide) à `km_ingest_raw`. Buffer ligne 256, chunk 128.

### Sorties KM (gate COM3_LOG)
- `km_uart_write_raw(data, len)` : écrit **toujours** sur UART0 (réponses
  protocole qui doivent passer quel que soit le gate). Utilisé par
  `km.version()`.
- `km_uart_write(data, len)` :
  - si `COM3_LOG=1` : écrit sur UART0.
  - si `COM3_LOG=0` : **no-op** (retourne len). Build « quiet » gameplay.
- `COM3_LOG` est `#ifndef`-gardé à 1 par défaut, mais **surchargé à 0 dans
  `platformio.ini`** (voir piège build dans `06` et `08`).

---

## C. `pass_usb_device.c` — driver de classe TinyUSB custom

C'est le composant le plus dense. Il implémente un `usbd_class_driver_t`
custom qui **avale tout le config descriptor**, ouvre chaque endpoint, et
relaie le trafic.

### Enregistrement du driver
`usbd_app_driver_get_cb` (weak-override TinyUSB) retourne un seul driver
`pass_driver` avec callbacks : `init`, `reset`, `open`, `control_xfer_cb`,
`xfer_cb`. Pas de `sof`.

### Structure `OpenEp` (par endpoint ouvert, max 8)
| Champ | Rôle |
|-------|------|
| `addr` | adresse EP (avec bit direction) |
| `mps` | max packet size |
| `is_in` | direction IN ? |
| `in_flight` | un xfer IN est en vol |
| `rx_buf[64]` | OUT : USB→nous ; IN : rapport courant soumis |
| `in_pending_buf[64]` / `in_pending_len` | IN : dernier rapport coalescé en attente |
| `tpl_buf[64]` / `tpl_len` / `tpl_have` | **template de synthèse** : dernier rapport réel cacheable |
| `last_real_us` | timestamp du dernier rapport réel (pour le gap de synthèse) |

### Callbacks de cycle de vie
- `pass_driver_init` : reset compteurs, démarre le timer de synthèse une fois.
- `pass_driver_reset(rhport)` : reset tous les EP (in_flight, pending, tpl),
  `open_ep_count=0`, `pass_mounted=false`, `ctl_pending=false`.
- `tud_mount_cb` / `umount` / `suspend` / `resume` : logs seulement.

### `pass_driver_open(rhport, itf_desc, max_len)` — ouverture des endpoints
Appelé par TinyUSB pendant SET_CONFIGURATION, **par interface**. Parcourt les
descripteurs de l'interface cible (`bInterfaceNumber`), pour son **alt 0
uniquement** :
- chaque `TUSB_DESC_ENDPOINT` ⇒ `usbd_edpt_open` ⇒ slot `OpenEp`.
- pour les EP **OUT**, amorce immédiatement un `usbd_edpt_xfer` (lecture).
- pour les EP IN, rien d'amorcé ici (les IN arrivent via `pass_usb_submit_in`).
- s'arrête si une autre `bInterfaceNumber` apparaît (fin de l'interface).
- met `pass_mounted = true`, retourne le nombre d'octets consommés.

### `pass_driver_control_xfer(rhport, stage, request)` — transferts de contrôle
Trois étapes TinyUSB : SETUP, DATA, ACK.

**SETUP** :
- **Garde stale-pending** : si `ctl_pending` déjà true :
  - si le pending a **> 2 s** (`age_us > 2000000`), on force-clear (le STATUS
    a été perdu, sinon EP0 stallerait à vie) et on accepte.
  - sinon, on **rejette** (`return false` ⇒ BUSY/STALL).
- Enregistre la requête, `ctl_pending_seq = ++ctl_seq_counter`, `ctl_pending =
  true`, `ctl_setup_us = now`.
- Cas **OUT avec wLength>0** : `tud_control_xfer` pour recevoir la data dans
  `ctl_out_buf` (rejette si `wLength > 256` ⇒ STALL). L'envoi IPC se fait à
  l'étape DATA.
- Cas **IN** ou **OUT wLength==0** : envoie `FRAME_CTRL_SETUP` (8 B) tout de
  suite.

**DATA** (uniquement utile pour OUT avec data) : concatène SETUP(8) + data et
envoie `FRAME_CTRL_SETUP` de `8+dlen` octets.

**ACK** : `ctl_pending = false`.

> Ce mécanisme « single-pending » est l'invariant #1 : **un seul transfert de
> contrôle en vol**. Le stale-clear 2 s est le filet anti-freeze si un
> `FRAME_CTRL_STATUS` est perdu (CRC drop).

### `pass_driver_xfer_cb(rhport, ep, result, xferred)` — completion d'endpoint
- **EP IN** : un IN vient de finir. Si un rapport coalescé attend
  (`in_pending_len>0`), on le promeut dans `rx_buf` et on le resoumet
  (garde le snapshot le plus frais en vol). Sinon `in_flight=false`.
- **EP OUT** : si succès et data reçue, `FRAME_EP_OUT` vers Right + log. Puis
  ré-amorce un `usbd_edpt_xfer` (relit).

### `submit_in_core(ep, data, len, is_synth)` — chemin IN principal
Appelé par `pass_usb_submit_in` (réel) et par `synth_cb` (synthèse).
1. Garde : EP existe, est IN, `pass_mounted`, `tud_ready()`.
2. Log des 200 premiers paquets réels par EP (pas les synth).
3. **Cache du template de synthèse** (si réel et format reconnu) : copie dans
   `tpl_buf`, `tpl_have=true`, `last_real_us=now`. Formats cacheables :
   - `data[0]==0x20` (GIP)
   - `data[0]==0x00 && len>=14 && data[1]==0x14` (XInput)
   - `data[0]==0x01 && len>=64` (DS5)
4. **Injection** : copie dans un scratch pile (64 B max), appelle
   `km_apply(ep, scratch, len)`.
5. **Soumission / coalescing** (sous `io_lock`) :
   - si `!in_flight` : copie scratch→rx_buf, `in_flight=true`, va soumettre.
   - sinon : copie dans `in_pending_buf` (écrase l'ancien pending → `coalesced`).
6. Si à soumettre : `usbd_edpt_claim` puis `usbd_edpt_xfer`. En cas d'échec,
   release + `in_flight=false`.
7. Diag latence (LAT_DIAG) : min/avg/max du coût de submit et du gap
   inter-arrivée, émis tous les 100 IN.

### Timer de synthèse (`synth_cb`, 4 ms)
But : les contrôleurs GIP n'émettent **que sur changement**. Si une injection
est active mais le contrôleur silencieux, on rejoue le dernier rapport réel
avec l'injection overlaid, pour que Windows voie l'override.

- `SYNTH_TICK_MS = 4`, `SYNTH_GAP_MS = 3`.
- Garde : `pass_mounted && tud_ready()`.
- `now_active = km_has_active_injection()`.
- **Falling edge** : `prev_active && !now_active` ⇒ on émet **une dernière**
  trame de synthèse pour un release propre (ex : RT repasse de plein à la
  vraie valeur quand le pulse km.click finit).
- Si ni actif ni falling edge ⇒ return.
- Pour chaque EP IN avec `tpl_have` et `now - last_real_us >= 3 ms` ⇒
  `submit_in_core(..., is_synth=true)`.

### `pass_usb_idle_housekeep()` (appelé depuis km_inject après 10 s d'inactivité)
Drop `tpl_have`/`tpl_len` sur chaque EP IN (défense contre un `tpl_have`
bloqué qui rejouerait de vieux rapports si un DEVICE_GONE a été manqué).
**Ne touche pas** `ctl_pending` (géré par TinyUSB) ni `in_flight`/`in_pending`
(toucher en plein vol désynchroniserait l'EP).

### `pass_usb_disconnect()` / `pass_usb_reconnect()`
- disconnect : stop synth (+ clear templates/in_flight), `ctl_pending=false`,
  `tud_disconnect()` (drop D+).
- reconnect : resume synth, `tud_connect()`. L'appelant doit avoir fait
  `tinyusb_set_descriptors` avant.

### `pass_usb_control_in_complete(seq, data, len)` / `pass_usb_control_status(seq, status)`
- in_complete : si `ctl_pending && seq==ctl_pending_seq`, `tud_control_xfer`
  pousse la data IN au PC cible.
- status : si seq apparié, `ctl_pending=false` ; si status != OK, stall EP0.

### Vendor control requests
`tinyusb_vendor_control_request_cb` et `tud_vendor_control_xfer_cb`
redirigent tous deux vers `pass_driver_control_xfer` — couvre les deux chemins
de routage TinyUSB pour les requêtes vendor device-recipient (MS OS 1.0,
PowerA register-access sur 20D6:4001, etc.).

---

## D. Constantes / limites côté LEFT

| Constante | Valeur | Où | Sens |
|-----------|--------|----|----|
| `PASS_MAX_EPS` | 8 | pass_usb_device.c | max endpoints ouverts |
| `CTL_OUT_BUF_SZ` | 256 | pass_usb_device.c | buffer data OUT de contrôle |
| `SYNTH_TICK_MS` | 4 | pass_usb_device.c | période timer synthèse |
| `SYNTH_GAP_MS` | 3 | pass_usb_device.c | silence réel mini avant synth |
| `LAT_DIAG_EMIT_N` | 100 | pass_usb_device.c | fréquence d'émission stats latence |
| stale-clear contrôle | 2 s | pass_usb_device.c | force-clear ctl_pending |
| rx_buf / scratch / tpl_buf | 64 | pass_usb_device.c | MPS full-speed |
| strings max | 8 | main.c | cache string |
| desc_config max | 1024 | main.c | taille config descriptor |
