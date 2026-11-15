#!/usr/bin/env python3
"""Explore MSI V530-9.6 Lightning LCD - only use interface 0 (bulk), don't touch HID."""

import usb.core
import usb.util
import struct
import time
import sys

VENDOR_ID = 0x0db0
PRODUCT_ID = 0x7881

dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
if dev is None:
    print("Not found!")
    sys.exit(1)
print(f"Found: {dev.product}")

# Check kernel drivers - do NOT detach interface 1
for i in range(2):
    active = dev.is_kernel_driver_active(i)
    print(f"Interface {i} kernel driver active: {active}")

# Only claim interface 0 (no kernel driver on it)
print("\n=== Claiming interface 0 (bulk) ===")
try:
    usb.util.claim_interface(dev, 0)
    print("OK - interface 0 claimed")
except Exception as e:
    print(f"Failed: {e}")
    sys.exit(1)

# Try reading from bulk IN
print("\n=== Bulk read (EP 0x81) ===")
for i in range(3):
    try:
        data = dev.read(0x81, 512, timeout=1000)
        print(f"  Read {i}: [{len(data)}] {data.tobytes().hex()}")
    except usb.core.USBTimeoutError:
        print(f"  Read {i}: timeout")
    except Exception as e:
        print(f"  Read {i}: {e}")

# Try writing to bulk OUT
print("\n=== Bulk write probes (EP 0x01) ===")
probes = [
    ("zeros_16", b'\x00' * 16),
    ("zeros_512", b'\x00' * 512),
    ("0x01 cmd", b'\x01' + b'\x00' * 15),
    ("0x10 cmd", b'\x10' + b'\x00' * 15),
    ("0x40 cmd", b'\x40' + b'\x00' * 15),
    ("0x55aa", b'\x55\xaa' + b'\x00' * 14),
    ("JPEG header", b'\xff\xd8\xff\xe0' + b'\x00' * 12),
    ("BMP header", b'BM' + b'\x00' * 14),
]

for name, probe in probes:
    try:
        w = dev.write(0x01, probe, timeout=1000)
        print(f"  Write '{name}': {w} bytes OK")
        time.sleep(0.05)
        try:
            resp = dev.read(0x81, 512, timeout=500)
            print(f"    -> Response: [{len(resp)}] {resp.tobytes().hex()}")
        except usb.core.USBTimeoutError:
            print(f"    -> No response")
    except usb.core.USBError as e:
        print(f"  Write '{name}': STALL/error: {e}")
        # Clear halt after stall
        try:
            usb.util.clear_halt(dev, 0x01)
        except Exception:
            pass

# Also try using the HID via hidraw (no need to detach kernel driver)
print("\n=== HID via /dev/hidraw ===")
import glob
import os

for hidraw in sorted(glob.glob('/dev/hidraw*')):
    try:
        # Check if this hidraw belongs to our device
        sysfs = f"/sys/class/hidraw/{os.path.basename(hidraw)}/device/uevent"
        with open(sysfs) as f:
            uevent = f.read()
        if '0DB0' in uevent and '7881' in uevent:
            print(f"Found {hidraw} for our device")
            # Try reading
            fd = os.open(hidraw, os.O_RDWR | os.O_NONBLOCK)
            # Try writing a query
            for report_id in [0x00, 0x01, 0x05, 0x10, 0x20]:
                try:
                    os.write(fd, bytes([report_id]) + b'\x00' * 63)
                    print(f"  Wrote report 0x{report_id:02x}")
                    time.sleep(0.05)
                    try:
                        resp = os.read(fd, 1024)
                        print(f"    -> Response: {resp.hex()}")
                    except BlockingIOError:
                        print(f"    -> No response")
                except OSError as e:
                    print(f"  Report 0x{report_id:02x} error: {e}")
            os.close(fd)
    except Exception as e:
        pass

usb.util.release_interface(dev, 0)
print("\nDone.")
