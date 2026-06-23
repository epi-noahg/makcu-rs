# AGENT.md — Carte intention → code

Carte de navigation rapide pour faire avancer le projet **sans relire toute la
doc**. Chaque entrée mappe une *idée* / *intention* vers la(les) fonction(s)
réelle(s), avec `fichier:ligne` cliquable et les pièges à connaître.

Doc détaillée : `./doc/`. Invariants à ne jamais casser : `./doc/08_PIEGES_ET_TODO.md`.

Abréviations chemins :
- **L/** = `MAKCM_ESP32s3_Pass_Left_IDF/src/` (device USB, ESP-IDF)
- **R/** = `MAKCM_ESP32s3_Pass_Right/src/` (host USB, Arduino+IDF)
- **H/** = headers (`*/include/`)

---

## 1. Les 12 fonctions qu'on touche le plus

| # | Intention | Fonction | Emplacement |
|---|-----------|----------|-------------|
| 1 | Parser une commande KM | `parse_km_text` | L/km_inject.c:472 |
| 2 | Appliquer l'injection à un rapport USB | `km_apply` | L/km_inject.c:693 |
| 3 | Mélanger stick réel + injecté | `blend_stick` | L/km_inject.c:272 |
| 4 | Courbe vitesse souris→stick | `xim_curve` | L/km_inject.c:355 |
| 5 | Drain accumulateur 8 ms + latches | `km_housekeep_cb` | L/km_inject.c:363 |
| 6 | Soumettre un rapport IN au PC cible | `submit_in_core` | L/pass_usb_device.c:422 |
| 7 | Relayer un transfert de contrôle | `pass_driver_control_xfer` | L/pass_usb_device.c:260 |
| 8 | Re-jouer un rapport (synthèse) | `synth_cb` | L/pass_usb_device.c:551 |
| 9 | Démarrer / reconnecter l'USB device | `start_usb` | L/main.c:117 |
| 10 | Snapshot des descripteurs du vrai contrôleur | `fetch_and_relay_descriptors` | R/PassUsbHost.cpp:150 |
| 11 | Soumettre un contrôle au vrai contrôleur | `submit_control` | R/PassUsbHost.cpp:344 |
| 12 | Assembler/émettre une trame IPC | `ipc_send` | L/ipc.c:119 · R/ipc.cpp:25 |

---

## 2. « Je veux modifier l'injection / le ressenti en jeu »

| Tâche concrète | Où | Notes |
|----------------|-----|-------|
| Changer la courbe de vitesse (sensi/accel) | `xim_curve` L/km_inject.c:355 ; constantes `KM_GAIN_C`/`KM_GAIN_P` L/km_inject.c:198-203 | surchargeable par `-DKM_GAIN_C=` / `-DKM_GAIN_P=` |
| Changer la fenêtre de drain (8 ms) | `KM_HOUSEKEEP_TICK_MS` L/km_inject.c:205 (+ timer `km_init` L/km_inject.c:782) | re-mesurer la latence (invariant I7) |
| Changer la logique de blend utilisateur/cheat | `blend_stick` L/km_inject.c:272 | **garder ±32767**, jamais −32768 (I4) |
| Changer la deadzone du stick physique | `PHYSICAL_IDLE_DEADZONE` L/km_inject.c:218 ; `physical_deadzone_clean` L/km_inject.c:667 | |
| Durée du pulse km.click | `CLICK_HOLD_MS` L/km_inject.c:214 ; `btn_pulse` L/km_inject.c:446 ; auto-release dans `km_housekeep_cb` L/km_inject.c:413 | |
| Ajouter/remapper un bouton | bits `BTN_*` L/km_inject.c:242-250 ; `map_btn_gip`/`map_btn_xinput` L/km_inject.c:580/615 ; `apply_ds5` L/km_inject.c:645 | mapper dans **les 3** adaptateurs |
| Accumuler le delta souris | `applyMouseDelta` L/km_inject.c:342 | écrit `g_vel_accum_*` sous `km_state_lock` |
| Filet anti-stick-collé | `STALE_RELEASE_MS` L/km_inject.c:210 ; logique dans `km_apply` L/km_inject.c:696 | |

---

## 3. « Je veux ajouter / changer un format de contrôleur »

Pour supporter un nouveau format de rapport, il faut **3 + 1 endroits** :

| Étape | Fonction | Emplacement |
|-------|----------|-------------|
| 1. Détecter le format + router | `km_apply` (les `if` de détection) | L/km_inject.c:693 (GIP:723, XInput:742, DS5:762) |
| 2. Extraire le stick physique | `extract_physical_*` | L/km_inject.c:672/677/684 |
| 3. Réécrire stick + boutons | `apply_*` + `map_btn_*` | L/km_inject.c:580-662 |
| 4. Autoriser le cache de synthèse | test `is_input` dans `submit_in_core` | L/pass_usb_device.c:448-460 |

**Pièges** : convention +Y=down (négation par protocole, voir
`doc/00_INDEX.md`), saturation ±32767 (I4), early-return idle conservé (I5).

---

## 4. « Je veux toucher au protocole / au transport USB »

### Canal IPC (Left ↔ Right, UART1 5 Mbps)
| Tâche | Où |
|-------|-----|
| Format de trame, types, CRC | **H/ : `pass_ipc.h` (DUPLIQUÉ des 2 côtés !)** — Left:include, Right:include |
| Émettre une trame | `ipc_send` L/ipc.c:119 · R/ipc.cpp:25 |
| Recevoir/déframer (machine à états) | `ipc_feed` L/ipc.c:52 · R/ipc.cpp:71 |
| Dispatch d'une trame reçue | `ipc_handle_frame` L/main.c:167 · R/main.cpp:28 |
| Ajouter un type de frame | enum `pass_ipc_type` dans `pass_ipc.h` + branche dans les 2 `ipc_handle_frame` |

> ⚠️ `pass_ipc.h` existe en **deux copies identiques**. Toute modif → les deux
> (invariant I8). Le CRC couvre `type..dernier octet payload` (pas les magic).

### USB device (Left ↔ PC cible)
| Tâche | Où |
|-------|-----|
| Ouvrir les endpoints à SET_CONFIG | `pass_driver_open` L/pass_usb_device.c:196 |
| Rapport IN → PC cible (+ injection) | `pass_usb_submit_in` L/pass_usb_device.c:534 → `submit_in_core` :422 |
| Rapport OUT PC cible → contrôleur | `pass_driver_xfer_cb` (branche OUT) L/pass_usb_device.c:383 |
| Transferts de contrôle | `pass_driver_control_xfer` L/pass_usb_device.c:260 ; completion `pass_usb_control_in_complete` :657 / `pass_usb_control_status` :673 |
| Vendor requests (MS OS, PowerA…) | `tinyusb_vendor_control_request_cb` / `tud_vendor_control_xfer_cb` L/pass_usb_device.c:97/108 |
| Enregistrer le driver de classe | `usbd_app_driver_get_cb` L/pass_usb_device.c:406 |

### USB host (Right ↔ vrai contrôleur)
| Tâche | Où |
|-------|-----|
| Installer le host + tâches | `begin` R/PassUsbHost.cpp:82 |
| Détecter branché/débranché | `client_event_cb` R/PassUsbHost.cpp:99 → `on_new_device`:113 / `on_device_gone`:142 |
| Snapshot descripteurs | `fetch_and_relay_descriptors` R/PassUsbHost.cpp:150 |
| Ouvrir les EP IN | `open_all_endpoints_for_interface` R/PassUsbHost.cpp:248 |
| Rapport IN du contrôleur → Left | `in_xfer_complete` R/PassUsbHost.cpp:285 |
| OUT / contrôle vers le contrôleur | `submit_out`:327 / `submit_control`:344 ; completion `control_xfer_complete`:307 |
| Libérer à la déconnexion | `release_all` R/PassUsbHost.cpp:398 |

---

## 5. « Je veux changer le cycle de vie (branchement / hot-swap) »

| Étape du cycle | Fonction | Emplacement |
|----------------|----------|-------------|
| Réception des descripteurs + réveil | `ipc_handle_frame` (DESC_*/READY/GONE) | L/main.c:167 |
| Décision install vs reconnect | `start_usb` | L/main.c:117 |
| Thread unique du cycle de vie | `main_task` | L/main.c:259 |
| Construire la config TinyUSB | `build_tusb_cfg` / `build_string_array` | L/main.c:104 / :76 |
| Hot-disconnect (drop D+) | `pass_usb_disconnect` | L/pass_usb_device.c:642 |
| Hot-reconnect | `pass_usb_reconnect` | L/pass_usb_device.c:651 |
| Reset injection sur GONE | `km_reset_injection` | L/km_inject.c:311 |

**Pièges** : tout passe par `main_task` (I3) ; **jamais** réinstaller TinyUSB,
seulement `tud_disconnect`/`tud_connect` + `tinyusb_set_descriptors` (I2).

---

## 6. « Je veux la synthèse / le comblement de trous GIP »

| Tâche | Où |
|-------|-----|
| Timer 4 ms qui rejoue le dernier rapport | `synth_cb` L/pass_usb_device.c:551 |
| Cadence / gap | `SYNTH_TICK_MS`=4 / `SYNTH_GAP_MS`=3 L/pass_usb_device.c:547-548 |
| « Injection active ? » | `km_has_active_injection` L/km_inject.c:331 |
| Cache du template à rejouer | dans `submit_in_core` L/pass_usb_device.c:448 |
| Drop du cache après 10 s idle | `pass_usb_idle_housekeep` L/pass_usb_device.c:612 (appelé par `km_housekeep_cb` L/km_inject.c:431) |

---

## 7. « Je veux du logging / debug »

| Besoin | Où | Gate |
|--------|-----|------|
| Activer la sortie diag sur COM3 | `-DCOM3_LOG=1` (⚠️ platformio.ini le met à **0**) | `COM3_LOG` |
| Snapshot 5 Hz du pipeline | `km_housekeep_cb` bloc KM_DIAG L/km_inject.c:378 | `KM_DIAG` |
| Trace ring 64 KB | `km_ring_*` L/km_inject.c:104-190 | `KM_RING` |
| Stats latence IN | `submit_in_core` bloc LAT_DIAG L/pass_usb_device.c:461 | `LAT_DIAG` |
| Écrire sur UART0 (gaté) | `km_uart_write` L/ipc.c:159 | COM3_LOG |
| Écrire sur UART0 (jamais gaté) | `km_uart_write_raw` L/ipc.c:155 | — |
| Log Right (tunnel IPC) | `R_LOG`/`R_LOG_fmt` R/PassUsbHost.cpp:21 → `FRAME_LOG` |
| LED d'état Right | `diag.cpp` (machine à états `current_state` R/diag.cpp:30, `pattern_step`:47) |
| LED heartbeat Left | `led_task` L/main.c:242 |

> ⚠️ Avec `COM3_LOG=0` (défaut actuel), **toute** sortie via `km_uart_write` est
> supprimée, y compris KM_DIAG/KM_RING/LAT_DIAG. Seul `km.version()` répond.
> Détail : `doc/06_BUILD_ET_CONFIG.md` §3 et `doc/08` P1.

---

## 8. « Je veux toucher au build / config »

| Besoin | Fichier |
|--------|---------|
| Flags de build Left (log/quiet, version) | `MAKCM_ESP32s3_Pass_Left_IDF/platformio.ini` |
| Flags de build Right | `MAKCM_ESP32s3_Pass_Right/platformio.ini` |
| Désactiver class drivers TinyUSB, affinité CPU, tick | `MAKCM_ESP32s3_Pass_Left_IDF/sdkconfig.defaults` |
| Partitions (4 MB, no OTA) | `*/partitions/partition_MAKCM.csv` |
| Board (pins flash/psram/usb mode) | `*/boards/MAKCM.json`, `Right/boards/Devkit.json` |
| Dépendance TinyUSB | `MAKCM_ESP32s3_Pass_Left_IDF/src/idf_component.yml` |
| Sources compilées Left | `MAKCM_ESP32s3_Pass_Left_IDF/src/CMakeLists.txt` |

Commandes : `pio run -d <projet> -e <LEFT_IDF|RIGHT>` (+`-t upload`).

---

## 9. Cycle de vie d'un octet (chemins de données de bout en bout)

**km.move(dx,dy) → caméra bouge :**
```
COM3 → km_uart_task (L/ipc.c:169) → km_ingest_raw (L/km_inject.c:546)
  → parse_km_text (:472) → applyMouseDelta (:342) → g_vel_accum
  → [8ms] km_housekeep_cb (:363) → xim_curve (:355) → rx_injected
  → [rapport IN] km_apply (:693) → blend_stick (:272) → apply_gip/xinput/ds5
  → submit_in_core (L/pass_usb_device.c:422) → usbd_edpt_xfer → PC cible
```

**Rapport du vrai contrôleur → PC cible :**
```
contrôleur → in_xfer_complete (R/PassUsbHost.cpp:285) → ipc_send FRAME_EP_IN
  → [UART1] ipc_feed (L/ipc.c:52) → ipc_handle_frame (L/main.c:167)
  → pass_usb_submit_in (L/pass_usb_device.c:534) → submit_in_core → PC cible
```

**Contrôle PC cible → vrai contrôleur → réponse :**
```
PC cible SETUP → pass_driver_control_xfer (L/pass_usb_device.c:260)
  → ipc_send FRAME_CTRL_SETUP → ipc_handle_frame (R/main.cpp:28)
  → submit_control (R/PassUsbHost.cpp:344) → contrôleur
  → control_xfer_complete (:307) → FRAME_CTRL_IN_DATA + FRAME_CTRL_STATUS
  → pass_usb_control_in_complete (L/pass_usb_device.c:657) / _status (:673)
  → tud_control_xfer → PC cible
```

---

## 10. Garde-fous (lire avant de committer)

- **`pass_ipc.h` : 2 copies** → modifier les deux (I8).
- **Stick : ±32767 only**, jamais −32768 (I4).
- **Cycle de vie USB : main_task only**, pas de réinstall TinyUSB (I2/I3).
- **Tout échec de contrôle côté Right → `FRAME_CTRL_STATUS`** sinon freeze (I1).
- **64 B partout** (full-speed) — agrandir si HS ou MPS>64 (F6).
- Mapper un bouton/format → **les 3 adaptateurs** GIP/XInput/DS5.

Détails et raisons : `doc/08_PIEGES_ET_TODO.md`.
