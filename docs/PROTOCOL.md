# MSI RTX 5090 Lightning Z — protocole de l'écran LCD

> Implémentation de référence : `crates/lightning-lcd` (Rust). Le script Python cité ci-dessous
> est conservé dans `legacy/python/lcd.py` ; la règle udev est dans `packaging/`.

## RÉSOLU (14/11/2026) — affichage confirmé visuellement (couleurs, mire, image, vidéo)

L'écran est un « ArtInChip USB Display » (AiCast) rebadgé MSI (0db0:7881), plus un firmware
applicatif (trames « BY ») piloté par HID. `lcd.py` (pyusb + Pillow, `/usr/bin/python3`) :

```
./lcd.py demo                       # test visuel complet (couleurs, mire, image, vidéo)
./lcd.py color ff0000 | test | image photo.jpg [--fit cover|contain|stretch]
./lcd.py video film.mp4 [--fps 30] [--loop]      # via ffmpeg
ffmpeg ... -c:v mjpeg -huffman default -f mjpeg - | ./lcd.py stream [--fps N]
./lcd.py status | extend | brightness N          # canal HID
```
Accès non-root : `70-lightning-lcd.rules` dans `/etc/udev/rules.d/` (USB + hidraw de 0db0:7881 ;
nommée `70-msi-lightning-lcd.rules` jusqu'à la 0.1).

### Séquence qui marche (figée dans `Lcd.open()`)
1. **HID** : `POST conn` (état) puis, si `extendedDisplay:0`, `POST extendedDisplay {"enable":true}`.
   Chaque requête est **répétée toutes les 100 ms jusqu'à réception d'un rapport** : le firmware ne
   traite l'entrée HID qu'environ **une fois toutes les 4,1 s** et jette le reste (c'est pour cela
   qu'une commande envoyée une seule fois « ne faisait rien » et que le HID semblait muet).
2. **Bulk** : claim de l'interface 0, authentification RSA, **sans aucune requête vendor** : en mode
   étendu, la requête vendor IN 0x00 fait réénumérer l'écran dans la demi-seconde (reproduit 4 fois).
   Les paramètres (1280×800@60, JPEG) sont lus avant la bascule, sinon valeurs par défaut.
3. **Images** : en-tête 20 o + JPEG baseline 4:2:0 1280×800. La dernière image reste affichée
   quand le flux s'arrête.

Particularités du mode étendu (observées, toutes gérées par `lcd.py`) :
- l'authentification n'est acceptée qu'**une fois par énumération USB** : une 2ᵉ tentative reste
  sans réponse (timeout), mais les images passent **sans ré-authentification** (confirmé à l'écran).
  `lcd.py` mémorise bus:adresse dans `$XDG_RUNTIME_DIR/msi-lightning-lcd.auth` ;
- l'écran se **réénumère 13 s après une énumération** s'il n'a pas reçu un vrai flux (constaté sans
  session et après une session d'une seule image ; pas après ~90 images). `lcd.py` réessaie à
  l'ouverture/à la première image et réémet 1,5 s les images isolées (heuristique) ;
- le mode étendu **survit aux réénumérations USB** ; retour à l'animation MSI : a priori coupure
  d'alimentation (`{"enable":false}` non essayé).

### Comment ça a été trouvé
Le protocole a été établi à des fins d'interopérabilité (faire fonctionner sous Linux un matériel
acheté dans le commerce), en observant le pilote et le service Windows livrés avec la carte et le
comportement de l'écran. Le pilote d'affichage est un pilote « Indirect Display » ArtInChip,
déclaré pour les identifiants 0db0:7880 à 0db0:7919. Les constantes `0xA1C62B01`, `0xA1C62B10`,
`0xA1C62B11` et la clé publique RSA sont **identiques** à celles des projets open source pour
écrans ArtInChip 33c3:0e02 (hevnsnt/artinchip-linux, xormal/eM3499-Monitor, eux-mêmes dérivés du
pilote Linux AiCast `aic_ud_proto.h` / `aic-render`). Aucun code du constructeur n'est repris.

### Protocole (interface 0, bulk OUT 0x01 / bulk IN 0x81)
1. Vendor IN `0xC0` req `0x00`, 160 o : 8 × u16 LE
   `version=5, chip=0x80, media_format=0x10 (JPEG), media_bus=0, mode_num=1, width=1280, height=800, fps=60`
   puis EDID 128 o à l'offset 16, puis 16 o (`… 5a 00 01 00 …`, non décodés).
   Req `0x01` : mode courant `width, height, fps` (u16 LE).
2. Authentification (obligatoire, sinon l'écran se déconnecte au bout de quelques paquets) ;
   en-tête commun `struct.pack('<IIHHII', magic, length, frame_id, media_format, 0, magic)` :
   - Phase 1 : OUT en-tête `0xA1C62B10` (length=256) ; OUT défi aléatoire chiffré RSA PKCS#1 v1.5
     avec la clé publique (256 o) ; IN → l'écran renvoie le défi en clair (prouve qu'il a la clé privée).
   - Phase 2 : OUT en-tête `0xA1C62B11` (length=256) ; IN 256 o = bloc « signé » (PKCS#1 type 1) ;
     on applique la clé publique, on retire le bourrage, on renvoie le contenu (153 o ici) en OUT.
   Aucun secret côté hôte : la clé publique suffit.
3. Image : OUT en-tête `0xA1C62B01` (length = taille du JPEG, frame_id incrémental, format 0x10),
   puis le JPEG (baseline, 4:2:0, 1280×800). Rien n'est renvoyé sur bulk IN.

### Bascule de mode par HID (indispensable) — protocole « BY »
Constat utilisateur : sans bascule, l'écran continue d'afficher son animation interne
(éclair + « LIGHTNING ») et **ignore** les images envoyées en bulk, bien qu'elles soient acceptées.

Sous Windows, le service de la carte envoie au démarrage une connexion (`conn`) puis la bascule en
mode écran étendu (`extendedDisplay`) sur ce canal HID ; l'écran devient ensuite un moniteur
Windows étendu.

- Transport : interface 1 (HID), rapport OUT de 1024 o sans report ID (hidraw : `00` + 1024 o),
  réponse sur le rapport IN de 1024 o.
- Trame : `5A | longueur BE16 (= len(texte)+5) | texte | somme8 | 5A`, bourrage à zéro ;
  échappement entre les deux `5A` : `5A → 5B 01`, `5B → 5B 02`.
  Somme8 sur longueur+texte ; le firmware ne semble pas la vérifier (des requêtes
  avec somme sur le texte seul ont aussi été traitées).
- Texte = pseudo-HTTP :
  - `POST conn 1\r\nSeqNumber=1\r\nContentType=json\r\nDate=<ms epoch>\r\n\r\n` → réponse
    `1 200\r\nAckNumber=2\r\n…` + JSON :
    `{"OS":"RTOS","Manufacturer":"MSI","model":"V530-9.6 Lightning","version":{"app":"V1.0.1",
    "firmware":"V530-9.6 Lightning_20251212v01","sdk":"V1.2.0","hardware":"V2.0"},"brightness":100,
    "degree":90,"sn":"…","bootFinish":1,"realtimeDisplay":0,"extendedDisplay":0}`
  - `POST extendedDisplay 1\r\nSeqNumber=110\r\nContentType=json\r\nContentLength=15\r\n\r\n{"enable":true}`
    → **bascule en mode écran étendu**.
  - `POST brightness 1 … {"value":0-100}`, `POST rotate 1 … {"degree":N}` (SeqNumber=100).
  - `POST transport 1 … {"type":"media","fileName":…,"fileSize":…}` puis `POST transported …` :
    **upload de fichier vers la flash de l'écran**.
    NON envoyé, à ne pas envoyer sans décision explicite (écriture persistante).
- Réponses : `conn` (SeqNumber=1) → `1 200`, `AckNumber=2` et le JSON d'état ; une commande
  (`brightness`, SeqNumber=100) → simple acquittement `1 200\r\nAckNumber=101\r\n\r\n`
  (`AckNumber = SeqNumber + 1`), sans JSON.
- Le firmware garde en file quelques exemplaires de la requête précédente : le premier rapport reçu
  après un changement de requête peut donc répondre à l'**ancienne**. Il faut filtrer sur
  `AckNumber` (fait dans `crates/lightning-lcd/src/hid.rs` ; le Python de référence ne le fait pas).
- `brightness` vérifié côté firmware le 14/11/2026 : l'état relu passe à 60/70 puis revient à 100.
- Fausse piste intermédiaire : « HID muet après extendedDisplay = mode étendu ». En réalité la
  commande unique avait été perdue (fenêtre de 4,1 s) ; l'état relu donne bien `"extendedDisplay":1`
  une fois la commande répétée.

### Vérifié / supposé
| Point | Statut |
|---|---|
| Auth RSA (défi renvoyé, bloc signé valide) | **prouvé** (réponses du device) |
| JSON d'état HID, `extendedDisplay` 0 → 1 après la commande | **prouvé** |
| Affichage : rouge, vert, bleu, mire, image, vidéo 30 img/s | **confirmé visuellement par l'utilisateur** |
| Sans bascule HID : images acceptées mais non affichées | constaté |
| Orientation exacte (texte « HAUT-GAUCHE » lisible ; position à préciser) | à confirmer |
| `rotate` | connu, non testé |
| Heuristique « 1,5 s d'images » contre la réénumération des 13 s | non vérifiée isolément |
| H.264 | non testé ; le device annonce JPEG |

### Pistes écartées
- Protocole des écrans OLED d'autres produits MSI : **fausse piste** (sans rapport avec cet écran).
- `capture_clean.pcapng` / `capture_full.pcapng` : captures **usbmon Linux** (non conservées dans le dépôt) (noyau 6.19, 03/04/2026),
  uniquement de l'énumération et des essais Linux — aucune trace du protocole Windows.
- « Tout vendor OUT déconnecte », « déco après 2560 o » : conséquence de l'absence d'authentification /
  d'en-tête valide, pas un handshake HID manquant.

### Compléments du 14/11/2026 (implémentation Rust)
- **Retour au mode autonome** : `POST extendedDisplay … {"enable":false}` (SeqNumber=110) est
  acquitté (`AckNumber=111`) et l'état relu repasse à `"extendedDisplay":0`, sans réénumération
  USB (`lightning restore`, `--on-exit restore`). Retour visuel de l'animation MSI à confirmer.
  Le redémarrage suivant rebascule en mode étendu en ~15 s et réaffiche le flux.
- **Requêtes vendor (0xC0, req. 0x00)** : à ne **jamais** émettre, pas seulement en mode étendu.
  Après un retour au mode autonome, la lecture des paramètres a fait réénumérer l'écran à chaque
  tentative, ce qui fermait le canal HID avant la bascule (boucle de reconnexion). Le mode
  1280×800@60 étant fixe, `lightning-lcd` n'émet plus aucune requête vendor.
- **Exclusivité** : un seul processus doit piloter l'écran (deux flux bulk entrelacés se
  corrompent). `lightning-lcd` prend un verrou `flock` sur
  `$XDG_RUNTIME_DIR/lightning-lcd.lock`, et aussi sur l'ancien nom `msi-lightning-lcd.lock` pour
  exclure une version 0.1 encore lancée pendant une mise à jour.

### À ne pas faire
- `POST transport` / `transported` (upload d'un média en flash) : écriture persistante, jamais envoyée.
- Requêtes vendor (0xC0 req 0x00/0x01), quel que soit le mode : réénumération.

---

> **Le reste de ce document est l'historique des observations initiales (avril 2026).**
> Certaines conclusions y sont dépassées (voir ci-dessus), notamment le « header EDID de
> 10 octets » (c'est en réalité une structure de 16 octets).


## Hardware

- **Écran**: V530-9.6 Lightning, 9.6 pouces, 1280x800 @60Hz
- **Connexion**: USB-C (GPU) → USB-A (carte mère), USB 2.0 High Speed (480 Mbps)
- **EDID**: "USB Display", fabricant AIC, numéro de série masqué

## Périphériques USB MSI détectés

| Device | VID:PID | Nom | Rôle |
|---|---|---|---|
| V530-9.6 Lightning | 0db0:7881 | Écran LCD principal | Affichage |
| DYNAMIC DASHBOARD III | 0db0:84df | Contrôleur secondaire | Heartbeat/monitoring |
| MYSTIC LIGHT | 0db0:0076 | Contrôleur RGB | Éclairage |
| USB Audio | 0db0:e5c3 | Audio GPU | Son |

## V530-9.6 Lightning (0db0:7881) — Détails USB

### Interface 0 — Bulk (données image)
- bInterfaceClass: 255 (Vendor Specific)
- EP 0x01 OUT: Bulk, 512 bytes max packet
- EP 0x81 IN: Bulk, 512 bytes max packet
- Pas de kernel driver attaché

### Interface 1 — HID (commandes)
- bInterfaceClass: 3 (HID)
- EP 0x82 IN: Interrupt, 1024 bytes
- EP 0x03 OUT: Interrupt, 1024 bytes
- Kernel driver: usbhid → /dev/hidrawN

### HID Report Descriptor (Interface 1)
```
Usage Page: 0xFF00 (Vendor Defined)
Usage: 0x01
Collection (Application)
  Input:  1024 bytes, no report ID
  Output: 1024 bytes, no report ID
End Collection
```

## Control Transfers (fonctionnels)

### Vendor IN (lecture) — bmRequestType 0xC0

| Request | Réponse | Description |
|---|---|---|
| 0x00 | 160 bytes | 10 bytes header + 128 bytes EDID + 22 bytes extra |
| 0x01 | 6 bytes | Résolution: `00 05 20 03 3c 00` |
| 0x02-0x09 | 256 bytes | Même données que 0x00 (padding à 256) |

### Décodage req 0x01 (résolution)
```
Bytes: 00 05 20 03 3C 00
Little-endian 16-bit: 0x0500=1280 (width), 0x0320=800 (height), 0x003C=60 (Hz)
```

### Header EDID (req 0x00, 10 premiers octets)
```
05 00 80 00 10 00 00 00 01 00
Interprétation possible:
  05 00 = version protocole?
  80 00 = 128 = taille EDID
  10 00 = offset?
  00 00 = flags?
  01 00 = nombre de blocs EDID
```

### Vendor OUT (écriture) — bmRequestType 0x40
- TOUS les vendor OUT provoquent une déconnexion immédiate du device
- Le device ne supporte PAS les control OUT

## Bulk Transfers — Comportement

- L'interface 0 peut être claimée (pas de kernel driver)
- Les writes sont acceptés pour **exactement 5 paquets de 512 bytes = 2560 bytes**
- Après 2560 bytes, le device se déconnecte (errno 19: No such device)
- Les reads (EP 0x81 IN) timeout toujours (pas de données spontanées)
- Le contenu des données n'a PAS d'impact — même avec des zéros, crash à 2560 bytes
- **Conclusion**: le device attend un header/handshake spécifique, probablement via HID d'abord

## HID Interface (Lightning) — Comportement

- Les writes (1024 bytes, pas de report ID) sont **toujours acceptés** sans erreur
- Aucune réponse n'est jamais reçue sur le HID IN
- Aucune commande testée n'a provoqué de changement visible sur l'écran
