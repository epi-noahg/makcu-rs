# 04 — Pipeline d'injection KM (`km_inject.c`)

Le cœur « cheat ». Transforme des deltas souris (`km.move`) et des commandes
de boutons en un mélange appliqué au stick droit / boutons des rapports USB
sortants, en respectant l'entrée physique de l'utilisateur.

Fichier : `MAKCM_ESP32s3_Pass_Left_IDF/src/km_inject.c` (796 lignes).

---

## 1. Vue d'ensemble du flux

```
UART0 (CH343)         esp_timer 8ms            chaque rapport IN
  km.move(dx,dy)  →   km_housekeep_cb     →    km_apply()
       |                  drain                    |
       v                  via xim_curve            v
  applyMouseDelta()  →  rx_injected/ry_injected →  blend_stick(physique, injecté)
  g_vel_accum_{x,y}      (puis accum = 0)          → réécriture octets stick/boutons
```

1. `km.move(dx,dy)` somme dx/dy dans `g_vel_accum_{x,y}` (accumulateur).
2. Toutes les **8 ms**, `km_housekeep_cb` draine : `rx_injected =
   xim_curve(accum_x)`, puis `accum = 0`.
3. À chaque rapport IN, `km_apply` extrait le stick physique, `blend_stick`
   avec l'injecté, réécrit les octets selon le protocole (GIP/XInput/DS5).

Souris arrêtée ⇒ pas d'event ⇒ prochain drain produit 0 ⇒ stick revient au
neutre **sans overshoot**. Pas de decay, pas de carryover, pas de rate-limit.

---

## 2. État global

| Variable | Type | Protégé par | Rôle |
|----------|------|-------------|------|
| `g_vel_accum_x/y` | volatile int32 | `km_state_lock` | accumulateur de vélocité (somme des deltas du fenêtre 8 ms) |
| `rx_injected/ry_injected` | int32 | `km_state_lock` | sortie de la courbe XIM (stick injecté courant) |
| `rx_physical/ry_physical` | int32 | `km_state_lock` | dernier stick physique extrait (diag) |
| `btn_held` | _Atomic u16 | atomic | boutons maintenus (set/release) |
| `btn_click` | _Atomic u16 | atomic | boutons en pulse one-shot |
| `click_release_ms` | _Atomic u32 | atomic | échéance d'auto-release du click |
| `km_last_cmd_ms` | _Atomic u32 | atomic | timestamp dernière commande KM |

Convention : **+ry = vers le bas** (souris). Voir `00_INDEX.md` § conventions.

### Bits boutons génériques
| Bit | Valeur | Signification |
|-----|--------|---------------|
| `BTN_A` | 0x0001 | face bas (A / Cross / South) |
| `BTN_B` | 0x0002 | face droite (B / Circle / East) |
| `BTN_X` | 0x0004 | face gauche (X / Square / West) |
| `BTN_Y` | 0x0008 | face haut (Y / Triangle / North) |
| `BTN_LB` | 0x0010 | bumper gauche |
| `BTN_RB` | 0x0020 | bumper droit |
| `BTN_FIRE` | 0x0040 | RT analog (souris gauche / « fire ») |
| `BTN_ADS` | 0x0080 | LT analog (souris droite / « ADS ») |

Ces bits génériques sont **traduits** vers les positions natives par
`map_btn_gip` / `map_btn_xinput` / les bits DS5.

---

## 3. La courbe XIM (`xim_curve`)

```c
rx_stick = clamp( KM_GAIN_C × |accum|^KM_GAIN_P , ±32767 )   (signe préservé)
```

- `KM_GAIN_C = 5046.0f`, `KM_GAIN_P = 0.40f` (défauts, ajustés pour XIM Matrix
  15 cm/360 @ 1200 DPI). Surchargables par `-DKM_GAIN_C=` / `-DKM_GAIN_P=`.
- Repères : `accum=8 → ~12k` (tracking), `accum=80 → ~29k` (mid), `accum=240 →
  rail` (flick).
- `accum==0 → 0`. Utilise `powf` (FPU matériel ESP32-S3, donc bon marché).
- **Symétrique** et saturante à `±32767`.

> La sensibilité, les courbes balistiques et le pacing sont gérés par le
> logiciel km-sender **en amont**. Le firmware est strictement un *combineur*.

---

## 4. Le blend utilisateur-prioritaire (`blend_stick`)

C'est l'algorithme « XIM-style asymmetric ». Pour chaque axe :

```c
int16_t blend_stick(int16_t real, int16_t inject) {
    int32_t gain;
    if (inject==0 || real==0 || ((inject ^ real) >= 0)) {  // signes alignés (ou un nul)
        int32_t abs_real = |real|;
        gain = 32768 - abs_real;       // headroom restant
        if (gain < 0) gain = 0;
    } else {                            // signes opposés
        gain = 32768;                   // gain plein
    }
    int32_t sum = real + ((inject * gain) >> 15);
    return clamp ±32767;
}
```

- **Signes alignés** (ou inject/real nul) : l'injection est mise à l'échelle
  par le **headroom restant** de l'utilisateur `(1 − |real|/32768)`. Plus
  l'utilisateur pousse, moins l'injection ajoute (évite de dépasser le rail).
- **Signes opposés** : l'injection passe **à plein gain** pour que le cheat
  puisse ramener le stick à travers la déflexion de l'utilisateur.
- **La déflexion utilisateur est toujours intégralement présente** dans la
  sortie (`sum = real + ...`).
- Test de signe : `(inject ^ real) >= 0` est vrai ssi les signes coïncident
  (ou l'un est nul).

> ⚠️ `>> 15` divise par 32768 — cohérent avec `gain ∈ [0, 32768]`. Le terme
> `(inject*gain)` peut atteindre `32767*32768 ≈ 1.07e9`, qui tient dans
> int32 (max ~2.15e9). OK.

### `compute_merged_stick(real_x, real_y, *mrx, *mry, *inj_x, *inj_y)`
- Sous `km_state_lock` : enregistre `rx/ry_physical`, lit `rx/ry_injected`.
- Calcule `mrx = blend_stick(clamp_s16(real_x), clamp_s16(inj_x))` (idem y).
- **Le `clamp_s16` sur `real_*` est critique** : les extracteurs XInput/GIP
  négativent Y (flip +Y=up → +Y=down). Quand le fil donne `y = -32768` (pull
  plein bas), `-y = +32768` ; un cast `int16` brut wraperait à `-32768`, le
  blend verrait « plein haut », et `apply_xinput`/`apply_gip` re-négativent →
  pull-down deviendrait un flick caméra **vers le haut**. `clamp_s16` sature
  `+32768 → +32767` avant le cast.

### `clamp_s16` et `s16_to_u8`
- `clamp_s16` : sature à `[-32767, +32767]` (jamais `-32768`, qui ne se
  négative pas sans wrap).
- `s16_to_u8` : `(v + 32768) >> 8`, clampé `[0,255]` (pour DS5, sticks u8,
  centre 0x80).

---

## 5. Boutons : hold, pulse, latch

| Fonction | Effet |
|----------|-------|
| `btn_hold(mask, on)` | set/clear le bit dans `btn_held` |
| `btn_pulse(mask)` | `btn_click = mask`, `click_release_ms = now + 120 ms` |
| `current_buttons()` | `btn_held | btn_click` |

- **`CLICK_HOLD_MS = 120`** : un km.click maintient ~7 frames @60 fps / ~15
  @120 fps, assez pour que les fenêtres de poll UI captent le pulse.
- L'auto-release du pulse se fait dans `km_housekeep_cb` : si `click_release_ms
  && (int32_t)(now - rel) >= 0` ⇒ clear. Le cast signé gère le wrap u32 sur
  ~24.85 jours d'uptime.

### `km_has_active_injection()`
Renvoie true si stick injecté non-nul **OU** un bouton held/click actif. Utilisé
par le timer de synthèse (pass_usb_device.c) pour savoir s'il faut rejouer des
trames.

---

## 6. Les timers et filets de sécurité

### `km_housekeep_cb` (esp_timer périodique, **8 ms**)
À chaque tick :
1. Sous lock : `ax/ay = accum`, `accum = 0`, `rx/ry_injected = xim_curve(ax/ay)`.
2. (KM_DIAG) snapshot toutes les 25 ticks (= 200 ms) : counters de commandes,
   min/max stick, somme dx/dy → ligne `[L] KM ...`.
3. **Auto-release click** (voir §5).
4. **Idle housekeep** : après `KM_IDLE_HOUSEKEEP_MS = 10 000 ms` sans commande
   KM, appelle `pass_usb_idle_housekeep()` (drop templates synth), zéro le
   click latch. One-shot par période d'inactivité (`housekept_idle`). Mis ici
   (et pas dans la drain task KM_RING) pour rester actif en build quiet.

### Filet « stale-release » dans `km_apply` (`STALE_RELEASE_MS = 500`)
Si `km_housekeep_cb` n'a pas tourné depuis > 500 ms (famine esp_timer),
`km_apply` zéro lui-même `rx/ry_injected` et l'accumulateur, pour que le stick
ne reste **jamais** collé jusqu'au power-cycle.

### `km_init`
Crée et démarre le timer `km_hk` (8 ms, `ESP_TIMER_TASK`). Si KM_RING, init la
drain task.

### `km_reset_injection`
Zéro tout (injecté, physique, accum, boutons, latch, last_cmd). Appelé sur
`FRAME_DEVICE_GONE` (depuis main_task).

---

## 7. Le parser KM (`parse_km_text`, `km_ingest_raw`)

`km_ingest_raw(payload, len)` :
- ignore len==0,
- met `km_last_cmd_ms = now`,
- (KM_RING) trace,
- si `payload[0]` est ASCII imprimable (`0x20..0x7E`), appelle `parse_km_text`.

`parse_km_text(line, len)` copie dans un buffer 96 et matche par préfixe
(`str_starts`). Voir `07_API_KM.md` pour la table complète. Points notables :
- `km.move(` / `km.move_auto(` / `km.move_bezier(` : parse `dx,dy` (via
  `strtol`, séparateur `' '`/`,`/`\t`), appelle `applyMouseDelta`. **duration
  et path ignorés.**
- `km.click(btn)` : `sscanf` du btn, `btn_pulse` :
  - `0 → BTN_FIRE` (RT), `1 → BTN_ADS` (LT), `2 → BTN_X`.
- `km.left/right/middle(0|1)` → FIRE / ADS / X.
- `km.btnA/B/X/Y(0|1)`, `km.lb/rb(0|1)` → bits correspondants.
- `km.version(` → réponse handshake kmbox (via `km_uart_write_raw`).

> Le parser est **ASCII-only**. Le « fast-path binaire » mentionné dans le
> README (via `km_ingest_raw`) est en réalité **non implémenté** — seul le
> chemin ASCII existe.

---

## 8. Adaptateurs de protocole (le détail bas-niveau des octets)

`km_apply(ep_addr, buf, len)` détecte le format et délègue. **Ordre** des
détections :

### 8.1 GIP (Xbox One / Scuf / PowerA / Elite / GameSir)
Détection : `ep_addr == 0x82 && len >= 20 && buf[0] == 0x20`.
La structure GIP a un **header de 4 octets** ; on travaille sur `buf + 4`.

**Extraction** (`extract_physical_gip`, sur `gp = buf+4`) :
- `rx = (int16)(gp[10] | gp[11]<<8)` (déjà bon signe X)
- `ry = -(int16)(gp[12] | gp[13]<<8)` (fil +Y=up → négation)
- deadzone `±4000` (`PHYSICAL_IDLE_DEADZONE`) appliquée.

**Écriture** (`apply_gip`, `len>=16`) :
- boutons : `gp[0] | gp[1]<<8`, OR avec `map_btn_gip(gen)`.
- stick : `gp[10..11] = mrx`, `gp[12..13] = -mry` (re-négation +Y=up).
- `BTN_FIRE` ⇒ `gp[4]=0xFF, gp[5]=0x03` (RT = 1023).
- `BTN_ADS` ⇒ `gp[2]=0xFF, gp[3]=0x03` (LT = 1023).

Bits GIP : A=0x0010, B=0x0020, X=0x0040, Y=0x0080, LB=0x0100, RB=0x0200
(layout `xone gamepad.c`).

### 8.2 XInput (Xbox 360, rapport 20 B)
Détection : `(ep 0x81|0x82) && len>=14 && buf[0]==0x00 && buf[1]==0x14`.

**Extraction** (`extract_physical_xinput`, sur `buf`) :
- `rx = (int16)(buf[10] | buf[11]<<8)`
- `ry = -(int16)(buf[12] | buf[13]<<8)` (fil +Y=up)
- deadzone ±4000.

**Écriture** (`apply_xinput`, re-valide `buf[0]==0x00 && buf[1]==0x14`) :
- boutons : `buf[2] | buf[3]<<8`, OR `map_btn_xinput(gen)`.
- stick : `buf[10..11]=mrx`, `buf[12..13]=-mry`.
- `BTN_FIRE` ⇒ `buf[5]=0xFF` (RT). `BTN_ADS` ⇒ `buf[4]=0xFF` (LT).

Bits XInput : A=0x1000, B=0x2000, X=0x4000, Y=0x8000, LB=0x0100, RB=0x0200.

### 8.3 DS4 / DS5 (rapport HID 64 B)
Détection : `len >= 64 && buf[0] == 0x01`.

**Extraction** (`extract_physical_ds5`) :
- `rx = (buf[3] - 128) << 8`, `ry = (buf[4] - 128) << 8` (déjà +Y=down ⇒ pas
  de négation).
- deadzone ±4000.

**Écriture** (`apply_ds5`, `len>=11 && buf[0]==0x01`) :
- stick : `buf[3] = s16_to_u8(mrx)`, `buf[4] = s16_to_u8(mry)` (sans négation).
- boutons face (byte 8) : Square=0x10, Cross=0x20, Circle=0x40, Triangle=0x80.
- shoulders (byte 9) : L1=0x01, R1=0x02.
- `BTN_FIRE` ⇒ `buf[6]=0xFF` (R2). `BTN_ADS` ⇒ `buf[5]=0xFF` (L2).

### Early-return d'idle (commun aux 3)
Après calcul, si `mrx==0 && mry==0 && gen_btn==0` ⇒ **return sans écrire**.
C'est ce qui laisse passer le rapport réel intact quand il n'y a rien à
injecter (le drift de repos du contrôleur passe inchangé).

> ⚠️ `apply_gip`/`apply_xinput` écrivent **toujours** le stick mélangé (pas de
> gating sur `m*!=0`) : gater laissait le drift brut des octets entrants
> passer quand le blend produisait exactement 0, créant une « lutte »
> directionnelle contre le drift au repos. Le pur-idle est géré par l'early
> return ci-dessus.

---

## 9. Diagnostics (gates de build)

| Gate | Défaut source | Effet |
|------|---------------|-------|
| `KM_DIAG` | 1 | snapshot 5 Hz (counters, min/max stick) sur UART0 |
| `KM_RING` | 1 | ring-buffer trace 64 KB, drainé après 2 s d'inactivité KM |
| `COM3_LOG` | 1 (mais 0 en platformio.ini) | gate global de `km_uart_write` |
| `LAT_DIAG` | 1 | stats de latence IN (dans pass_usb_device.c) |

### KM_RING (trace 64 KB)
- `km_ring_write` / `km_ring_printf` : écriture circulaire sous `km_ring_lock`,
  overflow = drop des **nouvelles** entrées (préserve la fin d'un burst).
- `km_ring_drain_task` (core0, prio2) : toutes les 200 ms ; heartbeat toutes
  les 5 s ; après `KM_RING_IDLE_MS = 2000 ms` sans activité KM, draine tout le
  ring sur UART0 (par chunks de 128 avec `vTaskDelay(5ms)` pour ne pas affamer
  TinyUSB), une fois par cycle d'inactivité.
- Format des entrées : `T<ms> R <cmd>` (commande reçue), `T<ms> M/A/B dx,dy`
  (move), `T<ms> O ix,iy|px,py|mrx,mry` (output km_apply), `T<ms> A ep=.. ..`
  (dump périodique).

---

## 10. Constantes clés

| Constante | Valeur | Sens |
|-----------|--------|------|
| `KM_HOUSEKEEP_TICK_MS` | 8 | période de drain de l'accumulateur |
| `KM_GAIN_C` | 5046.0 | gain courbe XIM |
| `KM_GAIN_P` | 0.40 | exposant courbe XIM |
| `STALE_RELEASE_MS` | 500 | filet famine esp_timer |
| `CLICK_HOLD_MS` | 120 | durée du pulse km.click |
| `PHYSICAL_IDLE_DEADZONE` | 4000 | deadzone stick physique |
| `KM_IDLE_HOUSEKEEP_MS` | 10000 | seuil de housekeep d'inactivité |
| `KM_RING_SZ` | 65536 | taille ring trace |
| `KM_RING_IDLE_MS` | 2000 | seuil de drain du ring |
