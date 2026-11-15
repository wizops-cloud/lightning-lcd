#!/usr/bin/env python3
"""Explore MSI V530-9.6 Lightning LCD - avoid set_configuration."""

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
print(f"Found: {dev.product} on bus {dev.bus} dev {dev.address}")
print(f"Current config: {dev.get_active_configuration().bConfigurationValue}")

# Detach kernel drivers
for i in range(2):
    try:
        if dev.is_kernel_driver_active(i):
            dev.detach_kernel_driver(i)
            print(f"Detached kernel from iface {i}")
    except Exception as e:
        print(f"Detach iface {i}: {e}")

# DON'T call set_configuration - use existing config

# Try vendor control transfers to understand the protocol
print("\n=== Vendor control reads (IN) ===")
for req in range(0x20):
    for val in [0x0000, 0x0001, 0x0100]:
        try:
            ret = dev.ctrl_transfer(0xC0, req, val, 0, 256, timeout=200)
            if len(ret) > 0:
                print(f"  req=0x{req:02x} val=0x{val:04x}: [{len(ret)}] {ret.tobytes().hex()}")
        except Exception:
            pass

# Try class-specific requests too (for HID)
print("\n=== Class control reads ===")
for req in [0x01, 0x02, 0x03, 0x06, 0x09]:
    for val in [0x0100, 0x0200, 0x0300, 0x2200]:
        try:
            ret = dev.ctrl_transfer(0xA1, req, val, 1, 256, timeout=200)
            if len(ret) > 0:
                print(f"  req=0x{req:02x} val=0x{val:04x}: [{len(ret)}] {ret.tobytes().hex()}")
        except Exception:
            pass

# Now try bulk interface
print("\n=== Bulk interface (iface 0) ===")
try:
    usb.util.claim_interface(dev, 0)
    print("Claimed interface 0")

    # Try reading first
    try:
        data = dev.read(0x81, 512, timeout=1000)
        print(f"  Read: {data.tobytes().hex()}")
    except usb.core.USBTimeoutError:
        print("  Read: timeout")

    # Try writing a small probe
    for probe in [
        b'\x00' * 16,
        b'\x01' + b'\x00' * 15,
        b'\x55\xaa' + b'\x00' * 14,
        b'MSID' + b'\x00' * 12,
    ]:
        try:
            w = dev.write(0x01, probe, timeout=1000)
            print(f"  Write {probe[:4].hex()}: {w} bytes written")
            time.sleep(0.05)
            try:
                resp = dev.read(0x81, 512, timeout=500)
                print(f"    Response: {resp.tobytes().hex()}")
            except usb.core.USBTimeoutError:
                print(f"    No response")
        except Exception as e:
            print(f"  Write error: {e}")

    usb.util.release_interface(dev, 0)
except Exception as e:
    print(f"  Interface 0 error: {e}")

# Now try HID interface
print("\n=== HID interface (iface 1) ===")
try:
    usb.util.claim_interface(dev, 1)
    print("Claimed interface 1")

    # Read first
    try:
        data = dev.read(0x82, 1024, timeout=1000)
        print(f"  Read: {data.tobytes().hex()}")
    except usb.core.USBTimeoutError:
        print("  Read: timeout")

    # Try sending HID reports
    for report_id in [0x00, 0x01, 0x02, 0x05, 0x10, 0x20]:
        report = bytes([report_id]) + b'\x00' * 63
        try:
            w = dev.write(0x03, report, timeout=1000)
            print(f"  HID write report 0x{report_id:02x}: {w} bytes")
            time.sleep(0.05)
            try:
                resp = dev.read(0x82, 1024, timeout=500)
                print(f"    Response: {resp.tobytes().hex()}")
            except usb.core.USBTimeoutError:
                print(f"    No response")
        except Exception as e:
            print(f"  HID write 0x{report_id:02x} error: {e}")

    usb.util.release_interface(dev, 1)
except Exception as e:
    print(f"  Interface 1 error: {e}")

print("\nDone.")
