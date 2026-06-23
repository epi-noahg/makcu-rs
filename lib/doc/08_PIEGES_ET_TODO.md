# 08 — Pièges, invariants, incohérences & TODO

> **À lire avant toute modification.** Ce projet est embarqué et temps-réel ;
> beaucoup de comportements « bizarres » sont des correctifs délibérés. Casser
> un invariant ici se traduit par un freeze USB ou un stick collé en jeu.

---

## A. Invariants à NE PAS casser

### I1 — Un seul transfert de contrôle en vol (`ctl_pending`)
Right sérialise les contrôles ; Left rejette un 2e SETUP tant qu'un est
pending. Si tu modifies `pass_driver_control_xfer` ou `submit_control`,
**garantis que tout chemin émet un `FRAME_CTRL_STATUS`** (côté Right) qui
clear `ctl_pending` (côté Left). Sinon : EP0 STALL → Windows arrête de poller
→ freeze apparent. Le filet est le stale-clear 2 s dans le SETUP handler.

### I2 — TinyUSB reste installé à vie (Left)
esp_tinyusb 1.4.x ne peut pas faire install→uninstall→install (2e install
retourne `ESP_ERR_INVALID_STATE 0x103`). Le hot-swap **doit** rester sur
`tud_disconnect`/`tud_connect` + `tinyusb_set_descriptors`. Ne **jamais**
appeler un uninstall.

### I3 — Toute opération de cycle de vie USB sur `main_task` (Left)
`start_usb`, `pass_usb_disconnect`, install/connect doivent rester sur le même
thread (`main_task`). Le passage par `teardown_pending` + `xTaskNotifyGive`
n'est pas cosmétique. Ne pas appeler ces fonctions depuis `ipc_rx_task`.

### I4 — Saturation symétrique ±32767
`clamp_s16` et `blend_stick` ne renvoient **jamais** `-32768` car les writers
XInput/GIP négativent Y ; `-(-32768)` wrappe en int16 et inverse la direction
au rail. Le `clamp_s16(real_*)` dans `compute_merged_stick` est obligatoire.
Si tu ajoutes un nouvel adaptateur protocole avec négation, applique la même
règle.

### I5 — Ne pas gater l'écriture de stick dans `apply_gip`/`apply_xinput`
Le stick mélangé est écrit **inconditionnellement** ; le pur-idle est géré par
l'early-return `if (mrx==0 && mry==0 && gen_btn==0) return;` dans `km_apply`.
Gater sur `m*!=0` réintroduit la « lutte » contre le drift de repos.

### I6 — Le template de synthèse ne cache que les formats à stick
`submit_in_core` ne met `tpl_have` que pour 0x20/00 14/0x01. Les heartbeats /
announces ne sont pas cachés (ils n'ont pas d'octets de stick). Garder cette
condition, sinon la synthèse rejouerait « rien ne s'est passé ».

### I7 — Affinité CPU + tick 1 kHz (Left)
`CONFIG_TINYUSB_TASK_AFFINITY_CPU1` + `CONFIG_FREERTOS_HZ=1000` sont le budget
temporel du pipeline (drain 8 ms, synth 4 ms, latence ~3 ms). Ne pas changer
sans re-mesurer la latence km.move→stick.

### I8 — `pass_ipc.h` dupliqué
Le header est copié à l'identique dans les deux projets. **Toute modif du
format de trame, des enums de type, ou du CRC doit être faite dans LES DEUX
copies** sinon désynchronisation silencieuse (CRC fail / mauvais dispatch).

---

## B. Incohérences code ↔ README (constatées)

### P1 — « default = log build » est FAUX pour ce checkout
Le README dit que le build Left par défaut est verbeux. En réalité
`platformio.ini` force `-DCOM3_LOG=0`, ce qui rend `km_uart_write` no-op. Donc
**aucun** diag ne sort sur COM3 par défaut, sauf `km.version()`
(`km_uart_write_raw`). Les autres gates (KM_DIAG/KM_RING/LAT_DIAG) restent à 1
mais leur sortie passe par `km_uart_write` (gaté) → invisible.
**Pour un vrai build log : ajouter `-DCOM3_LOG=1`.** Voir `06` §3.

### P2 — Le « fast-path binaire KM » n'existe pas
README : « the binary fast-path in `km_ingest_raw` is unused ». En réalité il
n'y a **pas** de fast-path binaire du tout : `km_ingest_raw` route vers
`parse_km_text` uniquement si `payload[0]` est ASCII imprimable, sinon ignore.
`FRAME_KM_INJECT` (0x30) est aussi un chemin mort (KM arrive sur UART0).

### P3 — String descriptors : index collectés mais non tous expédiés
`fetch_and_relay_descriptors` collecte les index iManufacturer/iProduct/
iSerial/iConfiguration/iInterface, mais n'expédie via `usb_device_info` que
**manufacturer/product/serial**. iConfiguration et iInterface sont collectés
en pure perte. De plus, ces strings viennent de la **résolution ESP-IDF**, pas
d'un `GET_DESCRIPTOR(STRING)` explicite → pas byte-exact, et la 0xEE (MS OS
1.0) n'est jamais répliquée. Conséquence : certains appareils à strings
non-standard peuvent ne pas matcher au bit près.

### P4 — `lib_deps` Right inutilisés
`ArduinoJson` et `locoduino/RingBuffer` sont déclarés mais jamais inclus par
le code actuel. Résidu d'un ancien firmware. Peuvent être retirés (gain de
build) après vérif qu'aucune autre TU ne les inclut.

---

## C. Bugs latents / points fragiles potentiels

### F1 — Pas de watchdog de déframe côté LEFT
Right a un watchdog (reset à `S_WAIT_MAGIC0` si mid-frame > 10 ms sans octet).
**Left n'en a pas.** Si un octet est perdu en plein milieu d'une grosse frame
(ex. config descriptor) côté Left, le déframer peut rester coincé jusqu'à ce
qu'un `len` corrompu > 1024 le resynchronise (ou une coïncidence de magic).
À l'usage c'est rare (les grosses frames Right→Left arrivent au boot, pas sous
charge), mais c'est une asymétrie à connaître si on voit des stalls
d'énumération. **Piste d'amélioration** : porter le watchdog 10 ms dans
`ipc_rx_task` de Left.

### F2 — `desc_config[1024]` vs frames > 1024
`PASS_IPC_MAX_PAYLOAD = 1024` et `desc_config[1024]`. Un config descriptor
> 1024 octets serait rejeté à la fois par le déframer (garde de longueur) et
par la validation Left. Les contrôleurs visés tiennent largement sous 1024,
mais un composite USB exotique pourrait dépasser. À garder en tête.

### F3 — Coalescing IN peut « sauter » des rapports intermédiaires
Sous forte charge IN, `submit_in_core` n'en garde qu'un en pending
(`in_pending_buf` écrasé). C'est **voulu** (on veut le plus frais), mais si un
rapport porte un événement transitoire (ex. un bouton pressé 1 frame) il peut
être écrasé avant émission. Acceptable pour du stick ; à savoir pour des
events one-shot. Compteur `coal` dans LAT_DIAG.

### F4 — `SET_INTERFACE(alt>0)` ne réouvre pas les endpoints (Right)
Forwardé comme transfert de contrôle, mais Right n'ouvre que les endpoints de
**alt 0** (au SET_CONFIG). Un appareil qui bascule sur un alt>0 avec de
nouveaux endpoints ne verra pas ses IN ouverts. L'injection continue sur ce
qui était ouvert à SET_CONFIG.

### F5 — Wrap u32 des timestamps ms
Les timestamps `esp_timer_get_time()/1000` en u32 wrappent à ~49.7 jours
(et le click_release à ~24.85 j pour la comparaison signée). L'idle housekeep
(10 s) est explicitement conçu pour défendre contre le wrap du
`click_release_ms`. Les autres comparaisons (`now - last`) en u32 non signé
restent correctes au wrap tant que l'intervalle < 49.7 j. OK en pratique.

### F6 — Buffer de scratch 64 B = MPS full-speed dur
`rx_buf`, `scratch`, `tpl_buf`, `in_pending_buf` sont tous **64 octets**. Si on
passait en high-speed (MPS 512) ou si un endpoint déclarait un MPS > 64, les
données seraient **tronquées**. Le firmware est full-speed only
(`CONFIG_TINYUSB_RHPORT_HS=n`) ; ne pas changer sans agrandir ces buffers.

### F7 — `submit_in_core` `len` réduit silencieusement
`if (len > sizeof(scratch)) len = sizeof(scratch);` tronque à 64 sans erreur.
Idem partout. Cohérent avec F6 mais silencieux.

---

## D. Limitations connues (par design, depuis le README — vérifiées)

- Pas d'état positionnel absolu (`km.moveto` dropé).
- Pas de duration / bezier (collapse vers move).
- Un seul contrôle en vol.
- Synth fallback seulement pour les 3 formats connus.
- `SET_INTERFACE(alt>0)` ne réouvre pas les EP (F4).
- Strings via `usb_device_info` (P3).
- TinyUSB installé à vie (I2).
- Max **8 endpoints, 4 interfaces, 8 strings** (Left) / 8 EP, 4 if (Right).
- **64 B MPS** sur chaque endpoint (full-speed only).
- Build log verbeux flood COM3 — utiliser le quiet pour le jeu.

---

## E. TODO / pistes d'évolution (depuis le README + analyse)

1. `km.moveto` absolu avec état positionnel.
2. Vrais `km.move_auto` / `km.move_bezier` (duration + bezier sur plusieurs
   fenêtres de drain).
3. `km.aim_mode` multi-mode (swap de courbe à l'exécution).
4. Transport KM binaire (le parser est ASCII-only ; cf P2).
5. Replay byte-exact des strings via `GET_DESCRIPTOR(STRING, idx, 0x0409)`
   explicite côté Right (cf P3).
6. Réouverture d'endpoints sur `SET_INTERFACE(alt>0)` (cf F4).
7. Backpressure / coalescing des transferts OUT côté Right.
8. Commandes binaires par contrôleur (rumble, lightbar).
9. **(propre à cette analyse)** Watchdog de déframe côté Left (cf F1).
10. **(propre)** Corriger/clarifier la doc log vs quiet build (cf P1).
11. **(propre)** Retirer les `lib_deps` morts de Right (cf P4).

---

## F. Checklist avant de modifier...

### …le format IPC
- [ ] Modifier `pass_ipc.h` dans **les deux** projets (I8).
- [ ] Vérifier la couverture CRC (type..payload).
- [ ] Vérifier le déframer des deux côtés.

### …le pipeline d'injection
- [ ] Respecter la convention +Y=down et les négations par protocole.
- [ ] Respecter la saturation ±32767 (I4).
- [ ] Ne pas gater l'écriture de stick (I5).
- [ ] Re-mesurer la latence si on touche aux timers (8 ms / 4 ms).

### …le cycle de vie USB (Left)
- [ ] Rester sur main_task (I3).
- [ ] Ne pas réinstaller TinyUSB (I2).
- [ ] Garder le cache de descripteurs complet avant start_usb.

### …le host (Right)
- [ ] Tout chemin d'échec contrôle → `FRAME_CTRL_STATUS` (I1).
- [ ] Garde `device_connected_` avant tout re-submit (anti double-free).
