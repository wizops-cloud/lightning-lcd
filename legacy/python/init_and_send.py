#!/usr/bin/env python3
"""Try init via HID then send image via bulk to MSI V530-9.6 Lightning LCD."""

import usb.core
import usb.util
import struct
import time
import sys
import os
import select
from PIL import Image

VENDOR_ID = 0x0db0
PRODUCT_ID = 0x7881
WIDTH = 1280
HEIGHT = 800

LIGHTNING_HIDRAW = "/dev/hidraw2"

def create_test_image():
    """Red/green/blue stripes."""
    img = Image.new("RGB", (WIDTH, HEIGHT))
    pixels = img.load()
    for y in range(HEIGHT):
        for x in range(WIDTH):
            third = x * 3 // WIDTH
            if third == 0:
                pixels[x, y] = (255, 0, 0)
            elif third == 1:
                pixels[x, y] = (0, 255, 0)
            else:
                pixels[x, y] = (0, 0, 255)
    return img

def img_to_jpeg(img, quality=85):
    import io
    buf = io.BytesIO()
    img.save(buf, format='JPEG', quality=quality)
    return buf.getvalue()

def hid_write(fd, data):
    """Write to HID, pad to 1024 bytes."""
    if len(data) < 1024:
        data = data + b'\x00' * (1024 - len(data))
    os.write(fd, data[:1024])

def hid_read(fd, timeout=0.2):
    """Try to read from HID."""
    r, _, _ = select.select([fd], [], [], timeout)
    if r:
        return os.read(fd, 4096)
    return None

print("=== MSI V530-9.6 Lightning LCD Init & Send ===\n")

# Create test image
img = create_test_image()
jpeg_data = img_to_jpeg(img, quality=80)
rgb_data = img.tobytes()  # RGB888
print(f"Image: {WIDTH}x{HEIGHT}, JPEG={len(jpeg_data)} bytes, RGB={len(rgb_data)} bytes")

# Open HID
print(f"\nOpening HID: {LIGHTNING_HIDRAW}")
hid_fd = os.open(LIGHTNING_HIDRAW, os.O_RDWR | os.O_NONBLOCK)

# Open USB device for bulk
dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
if not dev:
    print("USB device not found!")
    sys.exit(1)
print(f"USB: {dev.product}")

# Read initial info via control transfers
print("\n--- Reading device info ---")
try:
    info = dev.ctrl_transfer(0xC0, 0x01, 0, 0, 6, timeout=500)
    print(f"Resolution info: {bytes(info).hex()}")
except Exception as e:
    print(f"Control read failed: {e}")

# Try different init sequences via HID before sending bulk data
init_sequences = [
    # Sequence 1: Simple enable display
    [
        ("enable display", bytes([0x01, 0x01]) + b'\x00' * 1022),
    ],
    # Sequence 2: Set mode + resolution + start
    [
        ("set mode", bytes([0x05, 0x01, 0x00]) + b'\x00' * 1021),
        ("set resolution", bytes([0x05, 0x10]) + struct.pack('<HH', WIDTH, HEIGHT) + b'\x00' * 1018),
        ("start stream", bytes([0x05, 0x20, 0x01]) + b'\x00' * 1021),
    ],
    # Sequence 3: JPEG mode
    [
        ("jpeg mode", bytes([0x10, 0x01]) + struct.pack('<I', len(jpeg_data)) + struct.pack('<HH', WIDTH, HEIGHT) + b'\x00' * 1012),
    ],
    # Sequence 4: Raw RGB mode
    [
        ("rgb mode", bytes([0x10, 0x02]) + struct.pack('<I', len(rgb_data)) + struct.pack('<HH', WIDTH, HEIGHT) + b'\x00' * 1012),
    ],
    # Sequence 5: Display command matching what we found
    [
        ("lcd display on", bytes([0x20, 0x01]) + b'\x00' * 1022),
        ("set life mode", bytes([0x20, 0x03]) + b'\x00' * 1022),
    ],
]

for seq_idx, seq in enumerate(init_sequences):
    print(f"\n{'='*50}")
    print(f"Trying init sequence {seq_idx + 1}")

    # Re-find device if disconnected
    dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
    if not dev:
        print("Device disconnected, waiting...")
        for _ in range(5):
            time.sleep(1)
            dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
            if dev:
                break
    if not dev:
        print("Device not found, skipping")
        continue

    # Reopen HID if needed
    try:
        os.fstat(hid_fd)
    except Exception:
        hid_fd = os.open(LIGHTNING_HIDRAW, os.O_RDWR | os.O_NONBLOCK)

    # Send HID init commands
    for name, cmd in seq:
        try:
            hid_write(hid_fd, cmd)
            print(f"  HID: {name} - sent")
            resp = hid_read(hid_fd, 0.1)
            if resp:
                print(f"    Response: {resp.hex()}")
        except Exception as e:
            print(f"  HID: {name} - error: {e}")

    time.sleep(0.2)

    # Now try bulk write
    try:
        usb.util.claim_interface(dev, 0)
        print(f"  Bulk: interface claimed")

        # Try sending JPEG with size header
        header = struct.pack('<II HH', 0x4A504547, len(jpeg_data), WIDTH, HEIGHT)  # "JPEG" + size + dims
        payload = header + jpeg_data

        sent = 0
        chunk_size = 512
        for offset in range(0, min(len(payload), 32768), chunk_size):
            chunk = payload[offset:offset + chunk_size]
            if len(chunk) < chunk_size:
                chunk = chunk + b'\x00' * (chunk_size - len(chunk))
            try:
                w = dev.write(0x01, chunk, timeout=2000)
                sent += w
            except Exception as e:
                print(f"  Bulk write error at {sent}: {e}")
                break

        print(f"  Bulk: sent {sent} bytes")

        # Try reading response
        try:
            resp = dev.read(0x81, 512, timeout=500)
            print(f"  Bulk response: {bytes(resp).hex()}")
        except:
            pass

        usb.util.release_interface(dev, 0)
    except Exception as e:
        print(f"  Bulk error: {e}")

    time.sleep(1)

os.close(hid_fd)
print("\n=== Done ===")
print("Did the screen change for any sequence?")
