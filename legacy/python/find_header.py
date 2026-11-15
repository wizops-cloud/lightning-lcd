#!/usr/bin/env python3
"""Find the correct bulk header format by testing single small writes."""

import usb.core
import usb.util
import struct
import time
import sys

VENDOR_ID = 0x0db0
PRODUCT_ID = 0x7881

def get_dev():
    """Find and setup device."""
    for _ in range(10):
        dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
        if dev:
            try:
                usb.util.claim_interface(dev, 0)
                return dev
            except:
                time.sleep(1)
        else:
            time.sleep(1)
    return None

def test_write(data, desc=""):
    """Send a single bulk write and check if device survives."""
    dev = get_dev()
    if not dev:
        print(f"  [{desc}] Device not found!")
        return False

    try:
        dev.write(0x01, data, timeout=2000)
    except Exception as e:
        print(f"  [{desc}] Write failed: {e}")
        try:
            usb.util.release_interface(dev, 0)
        except:
            pass
        return False

    time.sleep(0.3)

    # Check if device is still alive by doing a control read
    try:
        dev.ctrl_transfer(0xC0, 0x01, 0, 0, 6, timeout=500)
        print(f"  [{desc}] Device ALIVE after write")
        try:
            usb.util.release_interface(dev, 0)
        except:
            pass
        return True
    except:
        print(f"  [{desc}] Device DEAD after write")
        try:
            usb.util.release_interface(dev, 0)
        except:
            pass
        return False

print("=== Finding correct header bytes ===\n")

# Test 1: Different first bytes with 512-byte packets
print("--- Test: Single 512-byte packet, varying first byte ---")
for first_byte in range(256):
    data = bytes([first_byte]) + b'\x00' * 511
    alive = test_write(data, f"0x{first_byte:02x}")
    if alive:
        print(f"  >>> 0x{first_byte:02x} keeps device alive!")
    time.sleep(0.5)
    # Only test a sample to save time
    if first_byte > 16 and first_byte not in [0x20, 0x30, 0x40, 0x50, 0x55, 0x60, 0x70, 0x80, 0x90, 0xA0, 0xAA, 0xB0, 0xC0, 0xD0, 0xE0, 0xF0, 0xFE, 0xFF]:
        continue

print("\nDone!")
