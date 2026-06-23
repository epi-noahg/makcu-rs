# 02 — Protocole IPC (UART1, 5 Mbps)

Référence : `*/include/pass_ipc.h` (copie identique des deux côtés),
`MAKCM_ESP32s3_Pass_Left_IDF/src/ipc.c`,
`MAKCM_ESP32s3_Pass_Right/src/ipc.cpp`.

> ⚠️ `pass_ipc.h` est **dupliqué** à l'identique dans les deux projets
> (`Left_IDF/include/` et `Pass_Right/include/`). Toute modification du
> format de trame **doit être faite dans les deux copies**. Voir piège #1 de
> `08_PIEGES_ET_TODO.md`.

## 1. Format de trame sur le fil

Little-endian, payload de taille variable :

```
+------+------+------+--------+--------+--------+-----------+-------+
| 0xA5 | 0x5A | type | ep_addr| seq_lo | seq_hi | len_lo/hi | ...   |
+------+------+------+--------+--------+--------+-----------+-------+
                                                  payload (N octets) | crc_lo | crc_hi
```

| Champ | Offset | Taille | Description |
|-------|--------|--------|-------------|
| magic0 | 0 | 1 | `0xA5` |
| magic1 | 1 | 1 | `0x5A` |
| type | 2 | 1 | `enum pass_ipc_type` |
| ep_addr | 3 | 1 | adresse d'endpoint (avec bit de direction) si pertinent, sinon 0 |
| seq | 4 | 2 (u16 LE) | identifiant de séquence (apparie réponse↔requête de contrôle) |
| length | 6 | 2 (u16 LE) | **taille du payload uniquement** (hors magic/header/CRC) |
| payload | 8 | N | données |
| crc16 | 8+N | 2 (u16 LE) | CRC CCITT-FALSE |

**Taille totale = N + 10** (`PASS_IPC_HDR_OVERHEAD = 10`).
**Payload max = 1024** (`PASS_IPC_MAX_PAYLOAD`).

### CRC
- Algorithme : **CRC-16/CCITT-FALSE** — poly `0x1021`, init `0xFFFF`, pas de
  réflexion, pas de XOR final.
- **Couverture** : depuis l'octet `type` (offset 2) jusqu'au **dernier octet
  de payload** inclus. Soit les 6 octets d'entête (`type`, `ep_addr`,
  `seq_lo`, `seq_hi`, `len_lo`, `len_hi`) **+ les N octets de payload**.
- **N'inclut PAS** les deux octets magic.
- Fonction de référence : `pass_ipc_crc16(&frame[2], 6 + len)` dans `ipc_send`.
- Le déframer recompute sur la même région (6 octets d'entête reconstruits +
  payload) puis compare à `crc_rcvd`.

```c
// pass_ipc.h
static inline uint16_t pass_ipc_crc16(const uint8_t *data, size_t len) {
    uint16_t crc = 0xFFFF;
    for (size_t i = 0; i < len; ++i) {
        crc ^= ((uint16_t)data[i]) << 8;
        for (int b = 0; b < 8; ++b)
            crc = (crc & 0x8000) ? (uint16_t)((crc << 1) ^ 0x1021)
                                 : (uint16_t)(crc << 1);
    }
    return crc;
}
```

### seq_id
- u16, wrap 0..65535.
- Côté Left, incrémenté par `++ctl_seq_counter` à chaque SETUP forwardé
  (`pass_usb_device.c`).
- Permet à Left d'apparier `FRAME_CTRL_IN_DATA` / `FRAME_CTRL_STATUS` au SETUP
  qu'il a initié (`seq == ctl_pending_seq`).
- Pour `FRAME_PING`, le répondeur ré-émet le même seq.
- Pour les frames de données d'endpoint (IN/OUT), seq n'est pas utilisé (0).

## 2. Catalogue des types de trame (`enum pass_ipc_type`)

| Type | Valeur | Sens | Payload | Usage |
|------|--------|------|---------|-------|
| `FRAME_DESC_DEVICE` | `0x01` | R→L | 18 B descripteur device | snapshot device |
| `FRAME_DESC_CONFIG` | `0x02` | R→L | config descriptor complet | snapshot config |
| `FRAME_DESC_STRING` | `0x03` | R→L | `[u8 idx][UTF-16LE...]` | string descriptor |
| `FRAME_DESC_MSOS1_EE` | `0x04` | R→L | (réservé) | 0xEE string MS OS 1.0 — **non utilisé** |
| `FRAME_DESC_MSOS1_CID` | `0x05` | R→L | (réservé) | Compat ID MS OS 1.0 — **non utilisé** |
| `FRAME_DEVICE_READY` | `0x06` | R→L | vide | « snapshot terminé, tu peux énumérer » |
| `FRAME_DEVICE_GONE` | `0x07` | R→L | vide | vrai appareil débranché |
| `FRAME_CTRL_SETUP` | `0x10` | L→R | 8 B SETUP [+ data OUT] | relaie un transfert de contrôle |
| `FRAME_CTRL_OUT_DATA` | `0x11` | L→R | (réservé) | **non utilisé** (setup+data combinés) |
| `FRAME_EP_OUT` | `0x12` | L→R | données OUT | endpoint OUT (PC cible → contrôleur) |
| `FRAME_CTRL_IN_DATA` | `0x20` | R→L | données IN de contrôle | réponse au SETUP (seq apparié) |
| `FRAME_CTRL_STATUS` | `0x21` | R→L | `[u8 status]` | statut du transfert de contrôle |
| `FRAME_EP_IN` | `0x22` | R→L | données IN | endpoint IN (contrôleur → PC cible) |
| `FRAME_KM_INJECT` | `0x30` | (réservé) | — | injection KM via IPC — **non utilisé** (KM va en direct sur UART0) |
| `FRAME_LOG` | `0xF0` | R→L | ligne ASCII | log tunnelé vers COM3 |
| `FRAME_PING` | `0xF1` | bidir | vide | echo (même seq) |

### Statuts de transfert (`enum pass_ipc_xfer_status`, payload de `FRAME_CTRL_STATUS`)
| Nom | Valeur |
|-----|--------|
| `XFER_OK` | 0 |
| `XFER_STALL` | 1 |
| `XFER_NAK` | 2 |
| `XFER_TIMEOUT` | 3 |
| `XFER_ERROR` | 4 |

## 3. Détail des payloads particuliers

### FRAME_DESC_DEVICE (0x01)
- Exactement **18 octets** (`sizeof(tusb_desc_device_t)` / descripteur device
  USB standard). Left vérifie `len == 18` avant `memcpy(&desc_device, ...)`.

### FRAME_DESC_CONFIG (0x02)
- Le config descriptor **complet** (jusqu'à `wTotalLength`).
- Left valide : `len >= 4 && len <= 1024`, puis relit `wTotal = payload[2] |
  payload[3]<<8` et exige **`wTotal == len`** (cohérence). Sinon rejet
  silencieux.
- Stocké dans `desc_config[1024]`, `desc_config_len = len`.

### FRAME_DESC_STRING (0x03)
- Payload = `[octet 0 : index de string][octets 1.. : corps UTF-16LE]`.
- Côté Right (`ship_one`) : on saute l'entête de 2 octets du descripteur
  (`sd->val + 2`) pour n'envoyer que le **corps** UTF-16LE.
- Côté Left : `idx = payload[0]`, corps = `payload+1`, tronqué à 128 octets
  (`sizeof(strings[0].utf16)`). Max **8 strings** stockées (`strings_count < 8`).
- Strings expédiées : `iManufacturer`, `iProduct`, `iSerialNumber` (via
  `usb_device_info`). **Pas** les strings d'interface ni la 0xEE (limitation,
  voir `08`).

### FRAME_CTRL_SETUP (0x10)
- Soit **8 octets** (SETUP seul, pour IN ou OUT sans data),
- soit **8 + wLength** octets (SETUP + data stage OUT concaténés) — Left
  coalesce setup+data en une seule frame à l'étape DATA.

### FRAME_CTRL_IN_DATA (0x20)
- Côté Right, dans `control_xfer_complete` : on envoie `t->data_buffer + 8`
  (on saute les 8 octets de SETUP que l'URB de contrôle IDF a en tête) sur
  `actual_num_bytes - 8` octets — **uniquement si `actual_num_bytes > 8`**.

## 4. Machine à états du déframer (RX)

Identique des deux côtés (`ipc_feed`). États :

```
S_WAIT_MAGIC0 → S_WAIT_MAGIC1 → S_READ_TYPE → S_READ_EP
  → S_READ_SEQ_LO → S_READ_SEQ_HI → S_READ_LEN_LO → S_READ_LEN_HI
  → S_READ_PAYLOAD (×len) → S_READ_CRC_LO → S_READ_CRC_HI → [dispatch] → S_WAIT_MAGIC0
```

Points subtils :
- **Re-sync magic** : en `S_WAIT_MAGIC1`, si l'octet n'est ni `0x5A` ni
  `0xA5`, retour à `S_WAIT_MAGIC0` ; si c'est encore `0xA5`, on **reste** en
  `S_WAIT_MAGIC1` (gère `A5 A5 5A`).
- **Garde de longueur** : en `S_READ_LEN_HI`, si `rx_len > 1024`, abandon →
  `S_WAIT_MAGIC0` (anti-corruption).
- **Payload vide** : si `rx_len == 0`, on saute directement à `S_READ_CRC_LO`.
- **CRC échec** :
  - Left logue `CRC fail type=... len=...` (`ESP_LOGW`).
  - Right **drop silencieux** (pas de log).
  - Dans les deux cas, retour `S_WAIT_MAGIC0`.

### Watchdog d'inactivité (RIGHT uniquement)
`ipc_pump_serial` (Right) : si on est **mid-frame** (`rx_state != S_WAIT_MAGIC0`)
et qu'aucun octet n'est arrivé depuis **> 10 ms**, reset à `S_WAIT_MAGIC0`.
Protège contre un octet perdu qui figerait le déframer (le 5 Mbps peut
overflow sous charge). **Left n'a pas ce watchdog** (sa boucle `ipc_rx_task`
lit en continu avec timeout 1 tick) — voir piège #5.

## 5. Émission (TX)

`ipc_send` (identique des deux côtés, à la primitive UART près) :
1. Assemble toute la frame dans un buffer pile (`magic..crc`).
2. Calcule le CRC sur `&frame[2], 6+len`.
3. Émet **atomiquement** sous mutex (`ipc_tx_mutex`) :
   - Left : `uart_write_bytes(UART_NUM_1, ...)`.
   - Right : `IpcSerial.write(frame, 8+len+2)` puis `diag_on_ipc_tx_bytes`.
4. Rejette si `len > 1024` (retourne false).

> Le mutex est indispensable : `ipc_send` est appelé depuis plusieurs tâches /
> cœurs. Sans lui, deux frames pourraient s'entrelacer sur le fil.

## 6. Pinout IPC (rappel)

| Signal | LEFT (ESP32-S3) | RIGHT (ESP32-S3) |
|--------|-----------------|------------------|
| UART1 TX | GPIO 2 | GPIO 1 |
| UART1 RX | GPIO 1 | GPIO 2 |

(Left TX=2 → Right RX=2 ; Right TX=1 → Left RX=1. Les numéros se croisent
parce que chaque côté nomme ses propres broches.)

UART1 : 8 data bits, pas de parité, 1 stop bit, pas de contrôle de flux,
buffer RX 8192 octets.
