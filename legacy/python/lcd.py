#!/usr/bin/env python3
"""Pilote userspace de l'écran LCD de la MSI RTX 5090 Lightning Z (0db0:7881).

L'écran est un « ArtInChip USB Display » (AiCast) rebadgé MSI, avec un firmware
applicatif qui joue une animation tant qu'on ne le bascule pas en « écran étendu ».
Séquence validée visuellement (14/11/2026) :
  1. HID (interface 1) : `POST extendedDisplay {"enable":true}` (protocole BY),
     répété jusqu'à réponse car le firmware ne lit le HID que toutes les ~4 s ;
  2. bulk (interface 0) : authentification RSA en deux phases, une seule fois par
     énumération USB ; AUCUNE requête vendor en mode étendu (elles font
     réénumérer l'écran) ;
  3. par image : en-tête de 20 octets + JPEG baseline 4:2:0 1280x800 sur EP 0x01.

Usage :
  lcd.py info                    # paramètres + authentification bulk
  lcd.py status                  # état renvoyé par le firmware (HID, JSON)
  lcd.py extend                  # bascule seule en mode « écran étendu »
  lcd.py brightness 0-100        # luminosité (HID ; non vérifié)
  lcd.py color RRGGBB [--hold SECONDES]
  lcd.py image fichier [--fit cover|contain|stretch] [--hold SECONDES]
  lcd.py video fichier [--fps N] [--loop]
  lcd.py stream [--fps N]        # MJPEG ou images JPEG concaténées sur stdin
  lcd.py test                    # mire de test
  lcd.py demo [--image F]        # test visuel complet : couleurs, mire, image, vidéo

Dépendances : pyusb, Pillow ; ffmpeg pour `video`.
"""
import argparse
import glob
import io
import json
import os
import select
import secrets
import struct
import subprocess
import sys
import time

import usb.core
import usb.util
from PIL import Image, ImageDraw, ImageFont, ImageOps

VID, PID = 0x0DB0, 0x7881
IFACE = 0
EP_OUT, EP_IN = 0x01, 0x81

FRAME_MAGIC = 0xA1C62B01
AUTH_DEV_MAGIC = 0xA1C62B10
AUTH_HOST_MAGIC = 0xA1C62B11
FMT_JPEG = 0x10
CHUNK = 64 * 1024

# Clé publique RSA-2048 des écrans ArtInChip (identique à
# celle du pilote ArtInChip AiCast générique). Clé publique : aucun secret.
RSA_E = 65537
RSA_N = int(
    "c9b76dbc1d6e340e1720287ec498b528958ed0661a97894d896ebdcd23082451"
    "b36f6c24885057dadc45687e59621d1361dc268f306a8a7cd3b168486ac5b7ae"
    "5f5037972fef7cc56a53b72788400d7cf583b8651d507404a0c3dbcd0d50a506"
    "01bc03bfe28c1d3a6bb3e07f4f84146b4b384413673787cc3e4d87d140165358"
    "ca02f3188e1fd847e30b61b974e1908b94e7d7f7342344155a9fb68b011420bf"
    "94116a102502806c60af3ae15ac12aaf75951097b785cfe5dfec0ed64cee419c"
    "a4b8b68b8c47b5172ddfb33e6e466f2dfff7695a4056ca99cb93f635711ee873"
    "2fe10f983fa40acf6c2e937c566075859fef3cc9d8efdfde2d376f2b8e5e7a87",
    16,
)
RSA_LEN = 256


class LcdError(Exception):
    pass


def _rsa_encrypt_pkcs1(msg: bytes) -> bytes:
    """Chiffrement RSA PKCS#1 v1.5 (type 2) avec la clé publique."""
    if len(msg) > RSA_LEN - 11:
        raise ValueError("message trop long")
    pad = b""
    while len(pad) < RSA_LEN - 3 - len(msg):
        pad += bytes(b for b in secrets.token_bytes(64) if b)
    pad = pad[: RSA_LEN - 3 - len(msg)]
    em = b"\x00\x02" + pad + b"\x00" + msg
    return pow(int.from_bytes(em, "big"), RSA_E, RSA_N).to_bytes(RSA_LEN, "big")


def _rsa_public_recover(blob: bytes) -> bytes:
    """Opération clé publique + retrait du bourrage PKCS#1 v1.5 type 1."""
    em = pow(int.from_bytes(blob, "big"), RSA_E, RSA_N).to_bytes(RSA_LEN, "big")
    if em[:2] != b"\x00\x01":
        raise LcdError("bloc signé invalide (bourrage PKCS#1 type 1 attendu)")
    i = 2
    while i < len(em) and em[i] == 0xFF:
        i += 1
    if i >= len(em) or em[i] != 0:
        raise LcdError("bloc signé invalide (séparateur)")
    return em[i + 1:]


class HidCtl:
    """Canal de commande HID (interface 1, rapports de 1024 o sans report ID).

    Trame « BY » : 5A | longueur BE16 (= len(texte)+5) | texte | somme8 | 5A,
    avec échappement 5A -> 5B 01 et 5B -> 5B 02 entre les deux 5A.
    Somme8 sur longueur+texte (le firmware ne semble
    pas la vérifier). Le texte est une pseudo-requête HTTP :
        POST <commande> 1\r\nSeqNumber=N\r\nContentType=json\r\n
        ContentLength=L\r\n\r\n<json>

    IMPORTANT : le firmware ne traite l'entrée HID qu'environ une fois toutes les
    4,1 s et jette les autres rapports. Il faut donc RÉPÉTER la requête (toutes
    les 100 ms) jusqu'à recevoir un rapport en retour — c'est ce que fait
    request(). Une requête envoyée une seule fois est presque toujours perdue.
    """

    def __init__(self):
        self.fd = None

    @staticmethod
    def find():
        for h in glob.glob("/sys/class/hidraw/hidraw*"):
            try:
                if "0003:00000DB0:00007881" in open(h + "/device/uevent").read():
                    return "/dev/" + os.path.basename(h)
            except OSError:
                pass
        return None

    def open(self):
        path = self.find()
        if not path:
            raise LcdError("interface HID de l'écran introuvable (hidraw)")
        try:
            self.fd = os.open(path, os.O_RDWR)
        except PermissionError as e:
            raise LcdError(f"accès refusé à {path} : installer 70-msi-lightning-lcd.rules "
                           "ou lancer avec sudo") from e
        return self

    def close(self):
        if self.fd is not None:
            os.close(self.fd)
            self.fd = None

    @staticmethod
    def frame(text: bytes) -> bytes:
        n = len(text) + 5
        body = bytes([n >> 8, n & 0xFF]) + text
        body += bytes([sum(body) & 0xFF])
        body = body.replace(b"\x5b", b"\x5b\x02").replace(b"\x5a", b"\x5b\x01")
        return b"\x5a" + body + b"\x5a"

    @staticmethod
    def deframe(report: bytes):
        if not report or report[0] != 0x5A:
            return None
        end = report.find(b"\x5a", 1)
        if end < 0:
            return None
        body = report[1:end].replace(b"\x5b\x01", b"\x5a").replace(b"\x5b\x02", b"\x5b")
        return body[2:-1].decode("utf-8", "replace")

    def request(self, text: str, timeout=9.0, period=0.1):
        """Répète la requête jusqu'à recevoir un rapport ; renvoie son texte ou None."""
        report = b"\x00" + self.frame(text.encode()).ljust(1024, b"\x00")
        while select.select([self.fd], [], [], 0)[0]:
            os.read(self.fd, 2048)  # purge
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            os.write(self.fd, report)
            if select.select([self.fd], [], [], period)[0]:
                return self.deframe(os.read(self.fd, 2048))
        return None

    @staticmethod
    def _json(reply):
        if not reply or "\r\n\r\n" not in reply:
            return None
        try:
            return json.loads(reply.split("\r\n\r\n", 1)[1])
        except ValueError:
            return None

    def post(self, cmd: str, obj, seq=100):
        """Le firmware répond à toute commande par son JSON d'état."""
        js = json.dumps(obj, separators=(",", ":"))
        return self._json(self.request(
            f"POST {cmd} 1\r\nSeqNumber={seq}\r\nContentType=json\r\n"
            f"ContentLength={len(js)}\r\n\r\n{js}"))

    def status(self):
        """« POST conn » : renvoie le JSON d'état du firmware (ou None)."""
        return self._json(self.request(
            "POST conn 1\r\nSeqNumber=1\r\nContentType=json\r\n"
            f"Date={int(time.time() * 1000)}\r\n\r\n"))

    def extend(self):
        """Passe l'écran en mode « écran étendu » (il affiche alors le flux USB).
        Renvoie l'état relu après la commande."""
        self.post("extendedDisplay", {"enable": True}, seq=110)
        return self.status()


class Lcd:
    def __init__(self, verbose=False):
        self.verbose = verbose
        self.dev = None
        # Valeurs de l'écran V530-9.6 ; relues par requête vendor quand c'est possible.
        self.width, self.height, self.fps = 1280, 800, 60
        self.fmt = FMT_JPEG
        self.version = self.chip = 0
        self.frame_id = 0
        self.fresh = False  # authentification faite dans cette session

    def log(self, *a):
        if self.verbose:
            print("[lcd]", *a, file=sys.stderr, flush=True)

    # -- connexion ---------------------------------------------------------
    def ensure_extended(self):
        """Bascule l'écran en mode « écran étendu » si nécessaire (jusqu'à ~10 s :
        le firmware ne lit le HID que toutes les 4 s). Renvoie True si confirmé."""
        ctl = HidCtl().open()
        try:
            st = ctl.status()
            if st is None:
                self.log("pas de réponse HID à conn : état inconnu, on continue")
                return False
            if not st.get("extendedDisplay"):
                self.log("écran en mode autonome -> envoi de extendedDisplay")
                # Pas de requête vendor ici : même en mode autonome elle peut faire
                # réénumérer l'écran et casser le canal HID avant la bascule
                # (constaté avec l'implémentation Rust le 14/11/2026).
                st = ctl.extend() or {}
            ok = bool(st.get("extendedDisplay"))
            self.log("mode étendu confirmé par le firmware" if ok
                     else f"mode étendu NON confirmé : {st}")
            return ok
        finally:
            ctl.close()

    def _find(self, wait):
        deadline = time.monotonic() + wait
        while True:
            dev = usb.core.find(idVendor=VID, idProduct=PID)
            if dev is not None:
                return dev
            if time.monotonic() > deadline:
                raise LcdError("écran 0db0:7881 introuvable")
            time.sleep(0.5)

    def _read_params_usb(self):
        """Lit le mode via les requêtes vendor (hors mode étendu uniquement)."""
        dev = self._find(10)
        try:
            p = bytes(dev.ctrl_transfer(0xC0, 0x00, 0, 0, 256, timeout=2000))
            self.version, self.chip, fmt, _bus, _n, w, h, fps = struct.unpack_from("<8H", p, 0)
            if w and h:
                self.width, self.height, self.fps = w, h, fps
            if fmt:
                self.fmt = fmt
            self.log(f"version={self.version} chip=0x{self.chip:x} format=0x{self.fmt:x} "
                     f"mode={self.width}x{self.height}@{self.fps}")
        finally:
            usb.util.dispose_resources(dev)

    def open(self, wait=10.0, extend=True, retries=4):
        """Séquence validée visuellement le 14/11/2026 :
        1. HID : extendedDisplay {"enable":true} répété jusqu'à réponse, état relu ;
        2. bulk : claim de l'interface 0, AUCUNE requête vendor, authentification RSA ;
        3. images JPEG.
        En mode étendu l'écran se réénumère ~13 s après chaque énumération tant
        qu'aucune session n'a eu lieu : on réessaie donc après une déconnexion."""
        if extend:
            try:
                self.ensure_extended()
            except LcdError as e:
                self.log(f"bascule HID impossible : {e}")
        for attempt in range(retries):
            self.dev = self._find(wait)
            try:
                usb.util.claim_interface(self.dev, IFACE)
                self._auth_once()
                return self
            except usb.core.USBError as e:
                if e.errno == 13:
                    raise LcdError(
                        "accès refusé au périphérique USB : installer "
                        "70-msi-lightning-lcd.rules (voir ANALYSIS.md) ou lancer avec sudo"
                    ) from e
                self.log(f"session interrompue ({e}), nouvelle tentative dans 3 s")
                usb.util.dispose_resources(self.dev)
                self.dev = None
                time.sleep(3)
        raise LcdError("impossible d'ouvrir une session avec l'écran")

    def close(self):
        if self.dev is not None:
            try:
                usb.util.release_interface(self.dev, IFACE)
            except usb.core.USBError:
                pass
            usb.util.dispose_resources(self.dev)
            self.dev = None

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def _out(self, data, timeout=5000):
        n = self.dev.write(EP_OUT, data, timeout=timeout)
        if n != len(data):
            raise LcdError(f"écriture bulk partielle ({n}/{len(data)})")

    def _in(self, size=RSA_LEN, timeout=3000):
        return bytes(self.dev.read(EP_IN, size, timeout=timeout))

    @staticmethod
    def _head(magic, length, frame_id=0, fmt=0):
        return struct.pack("<IIHHII", magic, length, frame_id & 0xFFFF, fmt, 0, magic)

    # En mode étendu, l'écran n'accepte l'authentification qu'UNE fois par
    # énumération USB : une seconde tentative reste sans réponse (timeout), alors
    # que les images continuent d'être acceptées sans ré-authentification. On
    # mémorise donc le numéro de périphérique (il change à chaque réénumération).
    @staticmethod
    def _state_path():
        base = os.environ.get("XDG_RUNTIME_DIR") or "/tmp"
        return os.path.join(base, "msi-lightning-lcd.auth")

    def _auth_key(self):
        return f"{self.dev.bus}:{self.dev.address}"

    def _auth_once(self):
        try:
            if open(self._state_path()).read().strip() == self._auth_key():
                self.log("session déjà authentifiée pour cette énumération USB")
                return
        except OSError:
            pass
        try:
            self.authenticate()
            self.fresh = True
        except usb.core.USBError as e:
            if e.errno != 110:
                raise
            self.log("authentification sans réponse : écran supposé déjà authentifié")
        try:
            with open(self._state_path(), "w") as f:
                f.write(self._auth_key())
        except OSError:
            pass

    def authenticate(self):
        # Phase 1 : l'écran prouve qu'il détient la clé privée.
        challenge = secrets.token_bytes(32)
        self._out(self._head(AUTH_DEV_MAGIC, RSA_LEN))
        self._out(_rsa_encrypt_pkcs1(challenge))
        resp = self._in()
        if resp[: len(challenge)] != challenge:
            raise LcdError("authentification phase 1 : réponse inattendue " + resp[:16].hex())
        self.log("auth phase 1 OK (l'écran a déchiffré le défi)")
        # Phase 2 : l'écran envoie un bloc signé, on renvoie son contenu.
        self._out(self._head(AUTH_HOST_MAGIC, RSA_LEN))
        signed = self._in()
        plain = _rsa_public_recover(signed)
        self._out(plain)
        self.log(f"auth phase 2 OK (bloc signé valide, {len(plain)} octets renvoyés)")

    # -- images ------------------------------------------------------------
    def send_jpeg(self, jpeg: bytes):
        try:
            self._send_jpeg(jpeg)
        except usb.core.USBError as e:
            # Réénumération « des 13 s » juste après l'ouverture : on rouvre une fois.
            if self.frame_id > 2:
                raise
            self.log(f"déconnexion en début de flux ({e}), reconnexion")
            usb.util.dispose_resources(self.dev)
            time.sleep(3)
            self.open(extend=False)
            self._send_jpeg(jpeg)

    def _send_jpeg(self, jpeg: bytes):
        self._out(self._head(FRAME_MAGIC, len(jpeg), self.frame_id, self.fmt))
        for pos in range(0, len(jpeg), CHUNK):
            self._out(jpeg[pos:pos + CHUNK], timeout=10000)
        self.frame_id += 1

    def encode(self, img: Image.Image, quality=85) -> bytes:
        if img.size != (self.width, self.height):
            raise LcdError(f"image {img.size} != mode {self.width}x{self.height}")
        buf = io.BytesIO()
        # JPEG baseline 4:2:0 : seul format validé avec le décodeur matériel ArtInChip.
        img.convert("RGB").save(buf, "JPEG", quality=quality, subsampling=2,
                                optimize=False, progressive=False)
        return buf.getvalue()

    def send_image(self, img, quality=85):
        self.send_jpeg(self.encode(img, quality))

    def fit(self, img, mode="cover"):
        size = (self.width, self.height)
        img = ImageOps.exif_transpose(img).convert("RGB")
        if mode == "stretch":
            return img.resize(size, Image.LANCZOS)
        if mode == "contain":
            return ImageOps.pad(img, size, Image.LANCZOS, color=(0, 0, 0))
        return ImageOps.fit(img, size, Image.LANCZOS)

    def hold(self, jpeg, seconds, period=1.0):
        """Réémet la même image périodiquement (certains firmwares reviennent
        à l'écran d'attente si le flux s'arrête)."""
        end = None if seconds < 0 else time.monotonic() + seconds
        while end is None or time.monotonic() < end:
            time.sleep(period)
            self.send_jpeg(jpeg)


def split_jpegs(stream, bufsize=1 << 16):
    """Découpe un flux MJPEG (images JPEG concaténées) en images."""
    buf = bytearray()
    while True:
        chunk = stream.read(bufsize)
        if not chunk:
            return
        buf += chunk
        while True:
            s = buf.find(b"\xff\xd8")
            if s < 0:
                del buf[:-1]
                break
            e = buf.find(b"\xff\xd9", s + 2)
            if e < 0:
                if s:
                    del buf[:s]
                break
            yield bytes(buf[s:e + 2])
            del buf[:e + 2]


def _font(size):
    try:
        return ImageFont.load_default(size=size)
    except TypeError:  # Pillow < 10.1
        return ImageFont.load_default()


def test_pattern(w, h):
    """Mire : barres de couleur, dégradé, cadre, et repères de coins."""
    img = Image.new("RGB", (w, h))
    d = ImageDraw.Draw(img)
    bars = [(255, 255, 255), (255, 255, 0), (0, 255, 255), (0, 255, 0),
            (255, 0, 255), (255, 0, 0), (0, 0, 255), (0, 0, 0)]
    bw = w // len(bars)
    for i, c in enumerate(bars):
        d.rectangle((i * bw, 0, (i + 1) * bw, h * 2 // 3), fill=c)
    for x in range(w):
        g = x * 255 // (w - 1)
        d.line((x, h * 2 // 3, x, h), fill=(g, g, g))
    d.rectangle((0, 0, w - 1, h - 1), outline=(255, 0, 0), width=4)
    f = _font(44)
    # Repères : carré blanc + « HAUT-GAUCHE » en haut à gauche, « BAS-DROITE » en bas à droite.
    d.rectangle((12, 12, 420, 90), fill=(255, 255, 255), outline=(0, 0, 0), width=3)
    d.text((24, 24), "HAUT-GAUCHE", fill=(0, 0, 0), font=f)
    d.rectangle((w - 400, h - 90, w - 12, h - 12), fill=(0, 0, 0), outline=(255, 255, 255), width=3)
    d.text((w - 388, h - 78), "BAS-DROITE", fill=(255, 255, 255), font=f)
    return img


def pump(lcd, frames, fps):
    """Envoie des JPEG à la cadence `fps` (0 = aussi vite que possible)."""
    n, t0 = 0, time.monotonic()
    for jpeg in frames:
        lcd.send_jpeg(jpeg)
        n += 1
        if fps:
            delay = t0 + n / fps - time.monotonic()
            if delay > 0:
                time.sleep(delay)
    dt = time.monotonic() - t0
    lcd.log(f"{n} images en {dt:.1f}s ({n / dt if dt else 0:.1f} img/s)")
    return n


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("-v", "--verbose", action="store_true")
    ap.add_argument("-q", "--quality", type=int, default=85, help="qualité JPEG (défaut 85)")
    sub = ap.add_subparsers(dest="cmd", required=True)
    ap.add_argument("--no-extend", action="store_true",
                    help="ne pas basculer l'écran en mode étendu via HID")
    sub.add_parser("info")
    sub.add_parser("status")
    p = sub.add_parser("demo")
    p.add_argument("--image", default="/usr/share/backgrounds/fedora-workstation/flight_dark.webp")
    sub.add_parser("extend")
    p = sub.add_parser("brightness")
    p.add_argument("level", type=int)
    p = sub.add_parser("color")
    p.add_argument("rgb")
    p.add_argument("--hold", type=float, default=0, help="réémettre N s (-1 = infini)")
    p = sub.add_parser("image")
    p.add_argument("file")
    p.add_argument("--fit", choices=["cover", "contain", "stretch"], default="cover")
    p.add_argument("--hold", type=float, default=0)
    p = sub.add_parser("test")
    p.add_argument("--hold", type=float, default=0)
    p = sub.add_parser("video")
    p.add_argument("file", help="fichier, URL ou périphérique lisible par ffmpeg")
    p.add_argument("--fps", type=float, default=30)
    p.add_argument("--loop", action="store_true")
    p.add_argument("--ffmpeg-input-args", default="", help="options ffmpeg avant -i")
    p = sub.add_parser("stream")
    p.add_argument("--fps", type=float, default=None,
                   help="cadence max (défaut : fps de l'écran ; 0 = illimité, l'écran\n"
                        "accepte tout sans contre-pression et jette des images)")
    a = ap.parse_args()

    lcd = Lcd(verbose=a.verbose or a.cmd in ("info", "extend"))
    try:
        if a.cmd in ("status", "extend", "brightness"):
            ctl = HidCtl().open()
            try:
                if a.cmd == "status":
                    st = ctl.status()
                    print(json.dumps(st, indent=2) if st else
                          "pas de réponse HID")
                elif a.cmd == "extend":
                    print("état avant :", ctl.status())
                    print("état après :", ctl.extend())
                else:
                    if not 0 <= a.level <= 100:
                        raise LcdError("luminosité attendue entre 0 et 100")
                    print("réponse :", ctl.post("brightness", {"value": a.level}))
            finally:
                ctl.close()
            return 0
        lcd.open(extend=not a.no_extend)
        if a.cmd == "info":
            print(f"mode {lcd.width}x{lcd.height}@{lcd.fps}, format 0x{lcd.fmt:x}, "
                  f"version {lcd.version}, chip 0x{lcd.chip:x}, authentification OK")
        elif a.cmd in ("color", "image", "test"):
            if a.cmd == "color":
                rgb = a.rgb.lstrip("#")
                if len(rgb) != 6:
                    raise LcdError("couleur attendue au format RRGGBB")
                img = Image.new("RGB", (lcd.width, lcd.height),
                                tuple(bytes.fromhex(rgb)))
            elif a.cmd == "image":
                img = lcd.fit(Image.open(a.file), a.fit)
            else:
                img = test_pattern(lcd.width, lcd.height)
            jpeg = lcd.encode(img, a.quality)
            lcd.send_jpeg(jpeg)
            lcd.log(f"image envoyée ({len(jpeg)} octets JPEG)")
            if a.hold:
                lcd.hold(jpeg, a.hold)
            elif lcd.fresh:
                # Observé : après une énumération, une session d'une seule image
                # n'empêche pas la réénumération « des 13 s », une session de ~90
                # images si. Par précaution on réémet l'image pendant 1,5 s.
                lcd.hold(jpeg, 1.5, period=1 / 30)
        elif a.cmd == "demo":
            W, H = lcd.width, lcd.height

            def show(label, img, seconds):
                print(label, flush=True)
                jpeg = lcd.encode(img, a.quality)
                lcd.send_jpeg(jpeg)
                lcd.hold(jpeg, seconds)

            for name, rgb in (("ROUGE", (255, 0, 0)), ("VERT", (0, 255, 0)), ("BLEU", (0, 0, 255))):
                show(f"[couleur] {name} uni, 3 s", Image.new("RGB", (W, H), rgb), 3)
            show("[mire] barres + dégradé, « HAUT-GAUCHE » / « BAS-DROITE », 8 s",
                 test_pattern(W, H), 8)
            if a.image and os.path.exists(a.image):
                show(f"[image] {a.image}, 6 s", lcd.fit(Image.open(a.image)), 6)
            else:
                print("[image] ignorée (fichier absent)")
            print("[vidéo] mire animée ffmpeg (compteur qui défile), 8 s à 30 img/s", flush=True)
            proc = subprocess.Popen(
                ["ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin", "-re",
                 "-f", "lavfi", "-t", "8", "-i", f"testsrc2=size={W}x{H}:rate=30",
                 "-vf", "format=yuvj420p", "-c:v", "mjpeg", "-q:v", "5",
                 "-huffman", "default", "-f", "mjpeg", "-"], stdout=subprocess.PIPE)
            try:
                n = pump(lcd, split_jpegs(proc.stdout), 0)
            finally:
                proc.terminate()
            print(f"        {n} images envoyées")
            show("[fin] mire fixe (reste affichée)", test_pattern(W, H), 0)
        elif a.cmd == "video":
            vf = (f"scale={lcd.width}:{lcd.height}:force_original_aspect_ratio=decrease,"
                  f"pad={lcd.width}:{lcd.height}:(ow-iw)/2:(oh-ih)/2,fps={a.fps},format=yuvj420p")
            qv = max(2, min(31, round((100 - a.quality) / 3) + 2))
            cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-nostdin"]
            if a.loop:
                cmd += ["-stream_loop", "-1"]
            cmd += a.ffmpeg_input_args.split() + ["-re", "-i", a.file, "-an", "-vf", vf,
                                                  "-c:v", "mjpeg", "-q:v", str(qv),
                                                  "-huffman", "default",
                                                  "-f", "mjpeg", "-"]
            proc = subprocess.Popen(cmd, stdout=subprocess.PIPE)
            try:
                pump(lcd, split_jpegs(proc.stdout), 0)
            finally:
                proc.terminate()
        elif a.cmd == "stream":
            pump(lcd, split_jpegs(sys.stdin.buffer), lcd.fps if a.fps is None else a.fps)
    except KeyboardInterrupt:
        pass
    except (LcdError, usb.core.USBError, OSError) as e:
        print(f"erreur : {e}", file=sys.stderr)
        return 1
    finally:
        lcd.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
