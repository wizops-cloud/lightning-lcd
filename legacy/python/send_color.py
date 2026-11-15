#!/usr/bin/env python3
"""Send a solid color framebuffer to MSI V530-9.6 Lightning LCD via bulk."""

import usb.core
import usb.util
import struct
import time
import sys

VENDOR_ID = 0x0db0
PRODUCT_ID = 0x7881
WIDTH = 1280
HEIGHT = 800

def find_and_setup():
    dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
    if dev is None:
        print("Device not found! Waiting for re-enumeration...")
        for _ in range(10):
            time.sleep(1)
            dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
            if dev:
                break
    if dev is None:
        print("Device not found!")
        sys.exit(1)
    print(f"Found: {dev.product}")
    # Only claim interface 0 (bulk), leave interface 1 (HID) to kernel
    usb.util.claim_interface(dev, 0)
    print("Interface 0 claimed")
    return dev

def send_bulk(dev, data, desc=""):
    """Send data in chunks via bulk OUT EP 0x01."""
    chunk_size = 16384
    sent = 0
    for offset in range(0, len(data), chunk_size):
        chunk = data[offset:offset + chunk_size]
        try:
            w = dev.write(0x01, chunk, timeout=5000)
            sent += w
        except Exception as e:
            print(f"  Write error at offset {offset}: {e}")
            return sent
    return sent

# Try different framebuffer formats
formats = {
    "RGB565_LE": lambda r, g, b: struct.pack('<H', ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)),
    "RGB565_BE": lambda r, g, b: struct.pack('>H', ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)),
    "RGB888": lambda r, g, b: bytes([r, g, b]),
    "BGR888": lambda r, g, b: bytes([b, g, r]),
    "RGBX8888": lambda r, g, b: bytes([r, g, b, 0xFF]),
    "BGRX8888": lambda r, g, b: bytes([b, g, r, 0xFF]),
    "XRGB8888": lambda r, g, b: bytes([0xFF, r, g, b]),
    "XBGR8888": lambda r, g, b: bytes([0xFF, b, g, r]),
}

# Use bright red - very visible if it works
color = (255, 0, 0)

for fmt_name, pixel_fn in formats.items():
    print(f"\n{'='*50}")
    print(f"Trying {fmt_name} - solid RED ({WIDTH}x{HEIGHT})")

    dev = find_and_setup()

    pixel = pixel_fn(*color)
    framebuf = pixel * (WIDTH * HEIGHT)
    print(f"  Framebuffer size: {len(framebuf)} bytes, pixel: {pixel.hex()}")

    sent = send_bulk(dev, framebuf, fmt_name)
    print(f"  Sent: {sent}/{len(framebuf)} bytes")

    if sent == len(framebuf):
        print(f"  >>> FULL FRAME SENT! Check the screen! <<<")
        time.sleep(2)  # Give time to see it
    else:
        print(f"  Incomplete send")

    try:
        usb.util.release_interface(dev, 0)
    except Exception:
        pass

    # Wait for device to recover
    time.sleep(1)

print("\nDone! Did the screen change for any of these?")
