#!/usr/bin/env python3
"""Try sending an image to MSI V530-9.6 Lightning LCD (1280x800)."""

import usb.core
import usb.util
import sys
import struct
import time
from PIL import Image

VENDOR_ID = 0x0db0
PRODUCT_ID = 0x7881
WIDTH = 1280
HEIGHT = 800

def create_test_image():
    """Create a simple test pattern image."""
    img = Image.new("RGB", (WIDTH, HEIGHT))
    pixels = img.load()
    for y in range(HEIGHT):
        for x in range(WIDTH):
            # Red/green/blue vertical stripes
            section = x * 3 // WIDTH
            if section == 0:
                pixels[x, y] = (255, 0, 0)
            elif section == 1:
                pixels[x, y] = (0, 255, 0)
            else:
                pixels[x, y] = (0, 0, 255)
    return img

def img_to_rgb565(img):
    """Convert PIL image to RGB565 (common for LCD screens)."""
    data = bytearray()
    pixels = img.load()
    for y in range(img.height):
        for x in range(img.width):
            r, g, b = pixels[x, y]
            rgb565 = ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
            data.extend(struct.pack('<H', rgb565))
    return bytes(data)

def img_to_rgb888(img):
    """Convert PIL image to raw RGB888."""
    return img.tobytes()

def img_to_bgr888(img):
    """Convert PIL image to raw BGR888."""
    data = bytearray()
    pixels = img.load()
    for y in range(img.height):
        for x in range(img.width):
            r, g, b = pixels[x, y]
            data.extend([b, g, r])
    return bytes(data)

def img_to_jpeg(img, quality=85):
    """Convert to JPEG."""
    import io
    buf = io.BytesIO()
    img.save(buf, format='JPEG', quality=quality)
    return buf.getvalue()

def find_device():
    dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
    if dev is None:
        print("Device not found!")
        sys.exit(1)
    print(f"Found: {dev.product}")
    return dev

def setup_device(dev):
    for i in range(2):
        try:
            if dev.is_kernel_driver_active(i):
                dev.detach_kernel_driver(i)
                print(f"Detached kernel driver from interface {i}")
        except Exception:
            pass
    try:
        dev.set_configuration()
    except Exception as e:
        print(f"set_configuration: {e}")
        # Re-find after possible re-enumeration
        time.sleep(0.5)
        dev = find_device()
        for i in range(2):
            try:
                if dev.is_kernel_driver_active(i):
                    dev.detach_kernel_driver(i)
            except Exception:
                pass
    return dev

def try_bulk_send(dev, data, description=""):
    """Try sending data via bulk endpoint (EP 0x01 OUT)."""
    print(f"\n--- Trying bulk send: {description} ({len(data)} bytes) ---")
    try:
        usb.util.claim_interface(dev, 0)
        # Send in chunks of 16384 bytes
        chunk_size = 16384
        sent = 0
        for offset in range(0, len(data), chunk_size):
            chunk = data[offset:offset + chunk_size]
            written = dev.write(0x01, chunk, timeout=2000)
            sent += written
        print(f"  Sent {sent} bytes total")

        # Try reading response
        try:
            resp = dev.read(0x81, 512, timeout=1000)
            print(f"  Response: {resp.tobytes().hex()}")
        except usb.core.USBTimeoutError:
            print(f"  No response (timeout)")

        usb.util.release_interface(dev, 0)
        return True
    except Exception as e:
        print(f"  Error: {e}")
        try:
            usb.util.release_interface(dev, 0)
        except Exception:
            pass
        return False

def try_hid_command_then_bulk(dev, cmd, data, description=""):
    """Send a HID command first, then bulk data."""
    print(f"\n--- Trying HID cmd + bulk: {description} ---")
    try:
        # Send HID command
        usb.util.claim_interface(dev, 1)
        dev.write(0x03, cmd, timeout=1000)
        print(f"  HID cmd sent: {cmd[:16].hex()}...")
        time.sleep(0.05)
        try:
            resp = dev.read(0x82, 1024, timeout=500)
            print(f"  HID response: {resp.tobytes().hex()}")
        except usb.core.USBTimeoutError:
            print(f"  HID: no response")
        usb.util.release_interface(dev, 1)

        # Send bulk data
        usb.util.claim_interface(dev, 0)
        chunk_size = 16384
        sent = 0
        for offset in range(0, len(data), chunk_size):
            chunk = data[offset:offset + chunk_size]
            written = dev.write(0x01, chunk, timeout=2000)
            sent += written
        print(f"  Bulk sent: {sent} bytes")
        try:
            resp = dev.read(0x81, 512, timeout=1000)
            print(f"  Bulk response: {resp.tobytes().hex()}")
        except usb.core.USBTimeoutError:
            print(f"  Bulk: no response")
        usb.util.release_interface(dev, 0)
        return True
    except Exception as e:
        print(f"  Error: {e}")
        for i in range(2):
            try:
                usb.util.release_interface(dev, i)
            except Exception:
                pass
        return False

def try_control_then_bulk(dev, data, description=""):
    """Try vendor control transfer to init, then bulk send."""
    print(f"\n--- Trying control init + bulk: {description} ({len(data)} bytes) ---")
    try:
        # Try common display init vendor requests
        for req_type, req, val, idx in [
            (0x40, 0x01, 0x0000, 0x0000),  # vendor OUT
            (0x40, 0x10, 0x0000, 0x0000),
            (0x40, 0x20, WIDTH, HEIGHT),
            (0x40, 0x51, 0x0000, 0x0000),
        ]:
            try:
                dev.ctrl_transfer(req_type, req, val, idx, timeout=500)
                print(f"  Control 0x{req:02x} (val=0x{val:04x}, idx=0x{idx:04x}): OK")
            except Exception:
                pass

        # Send image via bulk
        usb.util.claim_interface(dev, 0)
        chunk_size = 16384
        sent = 0
        for offset in range(0, len(data), chunk_size):
            chunk = data[offset:offset + chunk_size]
            written = dev.write(0x01, chunk, timeout=2000)
            sent += written
        print(f"  Bulk sent: {sent} bytes")
        try:
            resp = dev.read(0x81, 512, timeout=1000)
            print(f"  Response: {resp.tobytes().hex()}")
        except usb.core.USBTimeoutError:
            print(f"  No response")
        usb.util.release_interface(dev, 0)
        return True
    except Exception as e:
        print(f"  Error: {e}")
        try:
            usb.util.release_interface(dev, 0)
        except Exception:
            pass
        return False

def main():
    print("Creating test image (1280x800)...")
    img = create_test_image()

    rgb565_data = img_to_rgb565(img)
    rgb888_data = img_to_rgb888(img)
    jpeg_data = img_to_jpeg(img)

    print(f"RGB565: {len(rgb565_data)} bytes")
    print(f"RGB888: {len(rgb888_data)} bytes")
    print(f"JPEG:   {len(jpeg_data)} bytes")

    dev = find_device()
    dev = setup_device(dev)

    # Strategy 1: Raw bulk send with different formats
    try_bulk_send(dev, jpeg_data, "JPEG raw bulk")
    time.sleep(0.5)

    try_bulk_send(dev, rgb565_data, "RGB565 raw bulk")
    time.sleep(0.5)

    # Strategy 2: Header + image data
    # Common pattern: 4-byte magic + width(2) + height(2) + format(1) + data
    for magic in [b'MSID', b'\x55\xAA\x55\xAA', b'\x00\x00\x00\x00']:
        header = magic + struct.pack('<HHB', WIDTH, HEIGHT, 0x01)
        payload = header + jpeg_data
        try_bulk_send(dev, payload, f"Header({magic.hex()}) + JPEG")
        time.sleep(0.3)

    # Strategy 3: Control transfer init + bulk
    try_control_then_bulk(dev, jpeg_data, "Control init + JPEG")
    time.sleep(0.5)

    # Strategy 4: HID command + bulk data
    # Try a command that might mean "receive image"
    for cmd_byte in [0x01, 0x10, 0x20, 0x40, 0x55]:
        cmd = bytes([cmd_byte]) + struct.pack('<IHH', len(jpeg_data), WIDTH, HEIGHT) + bytes(1024 - 9)
        try_hid_command_then_bulk(dev, cmd, jpeg_data, f"HID cmd 0x{cmd_byte:02x} + JPEG")
        time.sleep(0.3)

    print("\n=== Done - check if the screen changed! ===")

if __name__ == "__main__":
    main()
