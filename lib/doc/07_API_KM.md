# 07 — Référence API KM (commandes ASCII)

Canal : UART0 de LEFT @ 4 Mbaud 8N1, via CH343 → COM3 du 2e PC. Une commande
par ligne, terminée par `\n` ou `\r`. Parser : `parse_km_text` dans
`km_inject.c`.

> Le matching est par **préfixe** (`str_starts`), pas exact. `km.btnA(1)` et
> `km.btnA(1, extra)` matchent tous deux la même branche. La ligne est
> tronquée à 95 caractères dans le parser.

---

## Handshake

| Commande | Effet |
|----------|-------|
| `km.version()` | Répond `kmbox:   1.0.0 <DATE> <TIME>\r\n>>> ` (ligne d'identification kmbox v1.0.0). `<DATE>`/`<TIME>` = `__DATE__`/`__TIME__` du build. Émis via `km_uart_write_raw` (toujours, même en quiet build). |

C'est ce qui permet à un client kmbox non modifié de détecter le device.

---

## Mouvement (souris → stick droit)

| Commande | Effet |
|----------|-------|
| `km.move(dx,dy)` | Somme `dx`/`dy` dans l'accumulateur de vélocité 8 ms. |
| `km.move_auto(dx,dy,duration)` | **Identique à `km.move`** — l'argument `duration` est ignoré. |
| `km.move_bezier(dx,dy,...)` | **Identique à `km.move`** — les arguments de path sont ignorés (seuls dx,dy comptent). |

Parsing : `strtol` pour dx puis dy, séparateurs acceptés `' '`, `','`, `'\t'`.
Si dx absent (`endp == args`) ⇒ commande ignorée.

Chaîne de traitement : `applyMouseDelta(dx,dy)` → `g_vel_accum += d` → drain
8 ms → `xim_curve` → `rx_injected` → `blend_stick` → octets stick.

---

## Click (pulse, auto-release après 120 ms)

| Commande | Effet |
|----------|-------|
| `km.click(0)` | Pulse **RT / right trigger / « fire »** (analog=1023, bit bouton set). |
| `km.click(1)` | Pulse **LT / left trigger / « ADS »**. |
| `km.click(2)` | Pulse **X (face gauche / Square)**. |

`btn_pulse(mask)` met `btn_click=mask` et programme l'auto-release à
`now + CLICK_HOLD_MS (120 ms)`. Le release effectif a lieu dans
`km_housekeep_cb`. Le timer de synthèse émet une **frame de release** sur le
falling edge.

> Argument `cnt` (ex. `km.click(0,3)`) : le parser le **ignore** — un seul
> pulse de 120 ms quel que soit cnt.

---

## Hold / release (maintien persistant)

| Commande | Effet |
|----------|-------|
| `km.left(1)` / `km.left(0)` | Hold / release **RT / fire** (BTN_FIRE). |
| `km.right(1)` / `km.right(0)` | Hold / release **LT / ADS** (BTN_ADS). |
| `km.middle(1)` / `km.middle(0)` | Hold / release **X (face gauche)** (BTN_X). |
| `km.btnA(1)` / `(0)` | A / Cross / South. |
| `km.btnB(1)` / `(0)` | B / Circle / East. |
| `km.btnX(1)` / `(0)` | X / Square / West. |
| `km.btnY(1)` / `(0)` | Y / Triangle / North. |
| `km.lb(1)` / `(0)` | LB / L1. |
| `km.rb(1)` / `(0)` | RB / R1. |

`btn_hold(mask, on)` set/clear le bit dans `btn_held`. Persistant jusqu'au
release (ou `km_reset_injection` sur DEVICE_GONE, ou idle housekeep qui ne
touche que le click latch — **pas** le held).

> ⚠️ Subtilité de matching : `km.left(1`, `km.right(1`, `km.middle(1` sont
> matchés **avant** `km.btnX` etc. dans l'ordre du parser. `km.middle` et
> `km.btnX` mappent tous deux sur `BTN_X` (face gauche).

---

## Mapping générique → natif (résumé)

| Générique | GIP (Xbox One) | XInput (360) | DS5 |
|-----------|----------------|--------------|-----|
| BTN_A | 0x0010 | 0x1000 | byte8 0x20 (Cross) |
| BTN_B | 0x0020 | 0x2000 | byte8 0x40 (Circle) |
| BTN_X | 0x0040 | 0x4000 | byte8 0x10 (Square) |
| BTN_Y | 0x0080 | 0x8000 | byte8 0x80 (Triangle) |
| BTN_LB | 0x0100 | 0x0100 | byte9 0x01 (L1) |
| BTN_RB | 0x0200 | 0x0200 | byte9 0x02 (R1) |
| BTN_FIRE (RT) | gp[4..5]=1023 | buf[5]=0xFF | buf[6]=0xFF (R2) |
| BTN_ADS (LT) | gp[2..3]=1023 | buf[4]=0xFF | buf[5]=0xFF (L2) |

(Offsets GIP relatifs à `buf+4`, après le header GIP de 4 octets.)

---

## Commandes NON supportées (ignorées silencieusement)

| Commande | Raison |
|----------|--------|
| `km.moveto(...)` | Nécessiterait un état positionnel absolu (l'injection est purement delta). |
| `km.aim_mode(...)` | Firmware mono-mode. |
| Smooth-move (`km.move` avec duration ≠ 0) | Incompatible avec le modèle de drain fenêtré. |
| Toute autre commande non listée | Pas de branche dans le parser. |

---

## Contrôleurs reconnus pour l'injection

L'injection ne modifie un rapport que si son **format on-wire** correspond.
Tous les autres contrôleurs énumèrent de façon transparente (le passthrough
est agnostique aux descripteurs) mais leurs rapports ne sont **pas** modifiés.

| Famille | Détection | Notes |
|---------|-----------|-------|
| Xbox One / GIP | EP IN `0x82`, 1er octet `0x20`, len ≥ 20 | Scuf Instinct, PowerA, Xbox Elite, GameSir, Xbox One. |
| XInput (Xbox 360) | EP IN `0x81/0x82`, `00 14 ...`, len ≥ 14 | rapport 20 B. |
| DS4 / DS5 | 1er octet `0x01`, len ≥ 64 | DS5 + DS4 (même rapport HID 64 B). |

---

## Exemple de session typique (km-sender → COM3)

```
km.version()            → handshake
km.move(12,-3)          → flick caméra droite/haut (windowed)
km.move(8,0)
km.left(1)              → maintient le tir
km.left(0)              → relâche
km.click(0)             → un pulse de tir 120 ms
km.btnA(1)              → saute (maintenu)
km.btnA(0)
```
