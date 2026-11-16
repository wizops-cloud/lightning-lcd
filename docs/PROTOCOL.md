# MSI RTX 5090 Lightning Z: LCD screen protocol

*English · [Français](PROTOCOL.fr.md)*

> Reference implementation: `crates/lightning-lcd` (Rust). The Python script mentioned below
> is kept in `legacy/python/lcd.py`; the udev rule is in `packaging/`.

## SOLVED (2026-11-14): display confirmed visually (colors, test pattern, image, video)

The screen is an "ArtInChip USB Display" (AiCast) rebadged by MSI (0db0:7881), plus an application
firmware ("BY" frames) driven over HID. `lcd.py` (pyusb + Pillow, `/usr/bin/python3`):

```
./lcd.py demo                       # full visual test (colors, test pattern, image, video)
./lcd.py color ff0000 | test | image photo.jpg [--fit cover|contain|stretch]
./lcd.py video film.mp4 [--fps 30] [--loop]      # via ffmpeg
ffmpeg ... -c:v mjpeg -huffman default -f mjpeg - | ./lcd.py stream [--fps N]
./lcd.py status | extend | brightness N          # HID channel
```
Non-root access: `70-lightning-lcd.rules` in `/etc/udev/rules.d/` (USB + hidraw of 0db0:7881;
named `70-msi-lightning-lcd.rules` up to 0.1).

### Working sequence (frozen in `Lcd.open()`)
1. **HID**: `POST conn` (state) then, if `extendedDisplay:0`, `POST extendedDisplay {"enable":true}`.
   Each request is **repeated every 100 ms until a report is received**: the firmware only
   processes HID input about **once every 4.1 s** and discards the rest (this is why
   a command sent only once "did nothing" and why the HID channel seemed mute).
2. **Bulk**: claim interface 0, RSA authentication, **without any vendor request**: in extended
   mode, the vendor IN request 0x00 makes the screen re-enumerate within half a second (reproduced 4 times).
   The parameters (1280×800@60, JPEG) are read before the switch, otherwise default values are used.
3. **Images**: 20-byte header + baseline 4:2:0 JPEG, 1280×800. The last image stays on screen
   when the stream stops.

Peculiarities of extended mode (observed, all handled by `lcd.py`):
- authentication is only accepted **once per USB enumeration**: a 2nd attempt gets
  no response (timeout), but images go through **without re-authentication** (confirmed on screen).
  `lcd.py` stores bus:address in `$XDG_RUNTIME_DIR/msi-lightning-lcd.auth`;
- the screen **re-enumerates 13 s after an enumeration** if it has not received a real stream (observed with no
  session and after a session of a single image; not after ~90 images). `lcd.py` retries on
  open/on the first image and re-sends isolated images for 1.5 s (heuristic);
- extended mode **survives USB re-enumerations**; going back to the MSI animation: presumably a power
  cut (`{"enable":false}` not tried).

### How it was found
The protocol was established for interoperability purposes (making retail-purchased hardware
work under Linux), by observing the Windows driver and service shipped with the card and the
behavior of the screen. The display driver is an ArtInChip "Indirect Display" driver,
declared for identifiers 0db0:7880 to 0db0:7919. The constants `0xA1C62B01`, `0xA1C62B10`,
`0xA1C62B11` and the RSA public key are **identical** to those of the open source projects for
ArtInChip 33c3:0e02 screens (hevnsnt/artinchip-linux, xormal/eM3499-Monitor, themselves derived from the
AiCast Linux driver `aic_ud_proto.h` / `aic-render`). No vendor code is reused.

### Protocol (interface 0, bulk OUT 0x01 / bulk IN 0x81)
1. Vendor IN `0xC0` req `0x00`, 160 bytes: 8 × u16 LE
   `version=5, chip=0x80, media_format=0x10 (JPEG), media_bus=0, mode_num=1, width=1280, height=800, fps=60`
   then a 128-byte EDID at offset 16, then 16 bytes (`… 5a 00 01 00 …`, not decoded).
   Req `0x01`: current mode `width, height, fps` (u16 LE).
2. Authentication (mandatory, otherwise the screen disconnects after a few packets);
   common header `struct.pack('<IIHHII', magic, length, frame_id, media_format, 0, magic)`:
   - Phase 1: OUT header `0xA1C62B10` (length=256); OUT random challenge encrypted with RSA PKCS#1 v1.5
     using the public key (256 bytes); IN → the screen returns the challenge in clear (proves it holds the private key).
   - Phase 2: OUT header `0xA1C62B11` (length=256); IN 256 bytes = "signed" block (PKCS#1 type 1);
     apply the public key, strip the padding, send the content back (153 bytes here) on OUT.
   No secret on the host side: the public key is enough.
3. Image: OUT header `0xA1C62B01` (length = JPEG size, incrementing frame_id, format 0x10),
   then the JPEG (baseline, 4:2:0, 1280×800). Nothing is returned on bulk IN.

### Mode switch over HID (required): "BY" protocol
User observation: without the switch, the screen keeps showing its internal animation
(lightning bolt + "LIGHTNING") and **ignores** the images sent over bulk, even though they are accepted.

Under Windows, the card's service sends at startup a connection (`conn`) and then the switch to
extended display mode (`extendedDisplay`) on this HID channel; the screen then becomes an extended
Windows monitor.

- Transport: interface 1 (HID), 1024-byte OUT report without report ID (hidraw: `00` + 1024 bytes),
  response on the 1024-byte IN report.
- Frame: `5A | BE16 length (= len(text)+5) | text | sum8 | 5A`, zero padding;
  escaping between the two `5A`: `5A → 5B 01`, `5B → 5B 02`.
  Sum8 over length+text; the firmware does not seem to check it (requests
  with a sum over the text alone were also processed).
- Text = pseudo-HTTP:
  - `POST conn 1\r\nSeqNumber=1\r\nContentType=json\r\nDate=<ms epoch>\r\n\r\n` → response
    `1 200\r\nAckNumber=2\r\n…` + JSON:
    `{"OS":"RTOS","Manufacturer":"MSI","model":"V530-9.6 Lightning","version":{"app":"V1.0.1",
    "firmware":"V530-9.6 Lightning_20251212v01","sdk":"V1.2.0","hardware":"V2.0"},"brightness":100,
    "degree":90,"sn":"…","bootFinish":1,"realtimeDisplay":0,"extendedDisplay":0}`
  - `POST extendedDisplay 1\r\nSeqNumber=110\r\nContentType=json\r\nContentLength=15\r\n\r\n{"enable":true}`
    → **switches to extended display mode**.
  - `POST brightness 1 … {"value":0-100}`, `POST rotate 1 … {"degree":N}` (SeqNumber=100).
  - `POST transport 1 … {"type":"media","fileName":…,"fileSize":…}` then `POST transported …`:
    **file upload to the screen's flash**.
    NOT sent, not to be sent without an explicit decision (persistent write).
- Responses: `conn` (SeqNumber=1) → `1 200`, `AckNumber=2` and the state JSON; a command
  (`brightness`, SeqNumber=100) → plain acknowledgment `1 200\r\nAckNumber=101\r\n\r\n`
  (`AckNumber = SeqNumber + 1`), without JSON.
- The firmware keeps a few copies of the previous request queued: the first report received
  after a change of request may therefore answer the **old** one. Filtering on
  `AckNumber` is required (done in `crates/lightning-lcd/src/hid.rs`; the reference Python does not do it).
- `brightness` verified on the firmware side on 2026-11-14: the state read back goes to 60/70 then returns to 100.
- Intermediate false lead: "HID mute after extendedDisplay = extended mode". In reality the
  single command had been lost (4.1 s window); the state read back does give `"extendedDisplay":1`
  once the command is repeated.

### Verified / assumed
| Item | Status |
|---|---|
| RSA auth (challenge returned, valid signed block) | **proven** (device responses) |
| HID state JSON, `extendedDisplay` 0 → 1 after the command | **proven** |
| Display: red, green, blue, test pattern, image, video at 30 frames/s | **confirmed visually by the user** |
| Without the HID switch: images accepted but not displayed | observed |
| Exact orientation ("HAUT-GAUCHE" text readable; position to be clarified) | to be confirmed |
| `rotate` | known, not tested |
| "1.5 s of images" heuristic against the 13 s re-enumeration | not verified in isolation |
| H.264 | not tested; the device advertises JPEG |

### Discarded leads
- Protocol of the OLED screens of other MSI products: **false lead** (unrelated to this screen).
- `capture_clean.pcapng` / `capture_full.pcapng`: **Linux usbmon** captures (not kept in the repository) (kernel 6.19, 2026-04-03),
  only of the enumeration and of the Linux attempts; no trace of the Windows protocol.
- "Any vendor OUT disconnects", "disconnect after 2560 bytes": a consequence of the missing authentication /
  valid header, not a missing HID handshake.

### Additions of 2026-11-14 (Rust implementation)
- **Return to standalone mode**: `POST extendedDisplay … {"enable":false}` (SeqNumber=110) is
  acknowledged (`AckNumber=111`) and the state read back returns to `"extendedDisplay":0`, without USB
  re-enumeration (`lightning restore`, `--on-exit restore`). Visual return of the MSI animation to be confirmed.
  The next start switches back to extended mode in ~15 s and displays the stream again.
- **Vendor requests (0xC0, req. 0x00)**: **never** to be sent, not only in extended mode.
  After a return to standalone mode, reading the parameters made the screen re-enumerate on every
  attempt, which closed the HID channel before the switch (reconnection loop). Since the
  1280×800@60 mode is fixed, `lightning-lcd` no longer sends any vendor request.
- **Exclusivity**: a single process must drive the screen (two interleaved bulk streams
  corrupt each other). `lightning-lcd` takes a `flock` lock on
  `$XDG_RUNTIME_DIR/lightning-lcd.lock`, and also on the old name `msi-lightning-lcd.lock` to
  exclude a 0.1 version still running during an update.

### What not to do
- `POST transport` / `transported` (upload of a media file to flash): persistent write, never sent.
- Vendor requests (0xC0 req 0x00/0x01), whatever the mode: re-enumeration.

---

> **The rest of this document is the history of the initial observations (April 2026).**
> Some of its conclusions are outdated (see above), in particular the "10-byte EDID
> header" (it is actually a 16-byte structure).


## Hardware

- **Screen**: V530-9.6 Lightning, 9.6 inches, 1280x800 @60Hz
- **Connection**: USB-C (GPU) → USB-A (motherboard), USB 2.0 High Speed (480 Mbps)
- **EDID**: "USB Display", manufacturer AIC, serial number masked

## MSI USB devices detected

| Device | VID:PID | Name | Role |
|---|---|---|---|
| V530-9.6 Lightning | 0db0:7881 | Main LCD screen | Display |
| DYNAMIC DASHBOARD III | 0db0:84df | Secondary controller | Heartbeat/monitoring |
| MYSTIC LIGHT | 0db0:0076 | RGB controller | Lighting |
| USB Audio | 0db0:e5c3 | GPU audio | Sound |

## V530-9.6 Lightning (0db0:7881): USB details

### Interface 0: Bulk (image data)
- bInterfaceClass: 255 (Vendor Specific)
- EP 0x01 OUT: Bulk, 512 bytes max packet
- EP 0x81 IN: Bulk, 512 bytes max packet
- No kernel driver attached

### Interface 1: HID (commands)
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

## Control Transfers (working)

### Vendor IN (read): bmRequestType 0xC0

| Request | Response | Description |
|---|---|---|
| 0x00 | 160 bytes | 10 bytes header + 128 bytes EDID + 22 bytes extra |
| 0x01 | 6 bytes | Resolution: `00 05 20 03 3c 00` |
| 0x02-0x09 | 256 bytes | Same data as 0x00 (padded to 256) |

### Decoding req 0x01 (resolution)
```
Bytes: 00 05 20 03 3C 00
Little-endian 16-bit: 0x0500=1280 (width), 0x0320=800 (height), 0x003C=60 (Hz)
```

### EDID header (req 0x00, first 10 bytes)
```
05 00 80 00 10 00 00 00 01 00
Possible interpretation:
  05 00 = protocol version?
  80 00 = 128 = EDID size
  10 00 = offset?
  00 00 = flags?
  01 00 = number of EDID blocks
```

### Vendor OUT (write): bmRequestType 0x40
- ALL vendor OUT requests cause an immediate disconnection of the device
- The device does NOT support control OUT

## Bulk Transfers: behavior

- Interface 0 can be claimed (no kernel driver)
- Writes are accepted for **exactly 5 packets of 512 bytes = 2560 bytes**
- After 2560 bytes, the device disconnects (errno 19: No such device)
- Reads (EP 0x81 IN) always time out (no spontaneous data)
- The content of the data has NO impact: even with zeros, crash at 2560 bytes
- **Conclusion**: the device expects a specific header/handshake, probably over HID first

## HID Interface (Lightning): behavior

- Writes (1024 bytes, no report ID) are **always accepted** without error
- No response is ever received on HID IN
- None of the commands tested caused any visible change on the screen
