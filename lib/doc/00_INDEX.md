# Documentation MAKCM Pass Package — Index

> Documentation technique complète du firmware passthrough USB à deux MCU
> ESP32-S3 (MAKCM). Objectif : permettre la modification sûre d'un projet
> **embarqué** où chaque détail bas-niveau compte.

Cette documentation a été écrite après lecture intégrale et ligne-à-ligne du
code source (commit présent dans `MAKCM_Pass_Package(1)/`). Tout ce qui est
affirmé ici est tiré du code, pas d'une supposition. Les divergences
constatées entre le code et le `README.md` racine sont signalées
explicitement (section « Pièges »).

## Carte de la documentation

| Fichier | Contenu |
|---------|---------|
| `00_INDEX.md` | Ce fichier. Carte + glossaire + conventions. |
| `01_ARCHITECTURE.md` | Vue d'ensemble système, rôles des 2 MCU, schéma de flux de bout en bout, cœurs/tâches/priorités. |
| `02_PROTOCOLE_IPC.md` | Format de trame UART1 (5 Mbps), types de frame, CRC, machine à états du déframer, séquencement. |
| `03_LEFT_MCU.md` | LEFT (device USB) : `main.c`, cycle de vie USB, cache descripteurs, driver de classe TinyUSB custom (`pass_usb_device.c`). |
| `04_KM_INJECT.md` | Le pipeline d'injection : parser KM, accumulateur 8 ms, courbe XIM, blend utilisateur-prioritaire, adaptateurs protocole (GIP/XInput/DS5), timers, latches. |
| `05_RIGHT_MCU.md` | RIGHT (host USB) : `PassUsbHost.cpp`, énumération, snapshot descripteurs, forwarding d'URB, LED diag. |
| `06_BUILD_ET_CONFIG.md` | PlatformIO, sdkconfig, partitions, boards, drapeaux de build (log/quiet), pinout. |
| `07_API_KM.md` | Référence complète des commandes KM ASCII acceptées + comportement exact. |
| `08_PIEGES_ET_TODO.md` | Pièges, incohérences code/README, bugs latents potentiels, limitations connues, points d'attention avant modification. |

## Glossaire

| Terme | Définition |
|-------|------------|
| **LEFT** / **Left** | MCU côté **device** USB. Se branche sur le PC cible. Émule le vrai contrôleur. Projet `MAKCM_ESP32s3_Pass_Left_IDF` (pur ESP-IDF). |
| **RIGHT** / **Right** | MCU côté **host** USB. Le vrai contrôleur s'y branche (USB-OTG). Projet `MAKCM_ESP32s3_Pass_Right` (Arduino + client `usb_host` IDF). |
| **IPC** | Lien série binaire LEFT↔RIGHT sur UART1 @ 5 Mbps. Transporte descripteurs, URB IN/OUT, transferts de contrôle. |
| **KM** | Canal de commandes « kmbox-like » : ASCII, UART0 de LEFT @ 4 Mbaud, via pont CH343 vers le 2ᵉ PC (COM3). |
| **GIP** | Game Input Protocol (Xbox One / Scuf / PowerA / Elite / GameSir). Rapport IN sur EP `0x82`, 1er octet `0x20`. |
| **XInput** | Format Xbox 360, rapport 20 octets démarrant par `00 14`. |
| **DS4/DS5** | DualShock 4/5, rapport HID 64 octets démarrant par `0x01`. |
| **URB** | USB Request Block : une transaction USB (IN, OUT ou contrôle). |
| **synth / synthèse** | Re-soumission périodique (4 ms) d'un rapport IN réel mis en cache, avec l'injection appliquée, pour combler les trous de polling des contrôleurs GIP « on-change ». |
| **blend** | Mélange stick réel utilisateur + stick injecté (`blend_stick`). |
| **drain** | Vidage de l'accumulateur de vélocité toutes les 8 ms vers la courbe XIM. |
| **CH343** | Pont USB↔UART qui relie UART0 de LEFT au 2ᵉ PC (COM3). |
| **MPS** | Max Packet Size. Ici 64 octets partout (full-speed). |

## Conventions de signe (CRITIQUE)

Le firmware travaille en **convention souris** en interne : **+Y = vers le bas**.

- Le fil **XInput** et **GIP** utilise **+Y = vers le haut** → les extracteurs
  (`extract_physical_*`) et les writers (`apply_*`) **négativent Y**.
- Le fil **DS5** utilise déjà **+Y = vers le bas** (octet stick brut) → DS5
  écrit/lit **sans négation**.
- La saturation symétrique `±32767` est imposée partout (`clamp_s16`,
  `blend_stick`) parce que `-32768` ne peut pas être négativé en `int16`
  sans wrap (voir `04_KM_INJECT.md` § clamp).

## Comment utiliser cette doc

Pour une modification donnée, lire dans l'ordre :
1. `01_ARCHITECTURE.md` pour situer le composant.
2. Le fichier du composant concerné (`03`/`04`/`05`).
3. `08_PIEGES_ET_TODO.md` **toujours** avant de toucher au code — il liste
   les invariants à ne pas casser.
