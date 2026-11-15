#!/usr/bin/env python3
"""Explore MSI V530-9.6 Lightning LCD screen USB protocol."""

import usb.core
import usb.util
import sys
import time

VENDOR_ID = 0x0db0
PRODUCT_ID = 0x7881

def find_device():
    dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
    if dev is None:
        print("Device not found!")
        sys.exit(1)
    print(f"Found: {dev.product} (serial: {dev.serial_number})")
    return dev

def detach_kernel(dev):
    for i in range(2):
        if dev.is_kernel_driver_active(i):
            print(f"Detaching kernel driver from interface {i}")
            dev.detach_kernel_driver(i)

def explore_hid(dev):
    """Try reading from HID interface (interface 1, EP 0x82 IN)."""
    print("\n=== Reading from HID interface (EP 0x82) ===")
    try:
        usb.util.claim_interface(dev, 1)
        for i in range(3):
            try:
                data = dev.read(0x82, 1024, timeout=1000)
                print(f"  HID read {i}: {data.tobytes().hex()}")
            except usb.core.USBTimeoutError:
                print(f"  HID read {i}: timeout (no data)")
            except Exception as e:
                print(f"  HID read {i}: error: {e}")
        usb.util.release_interface(dev, 1)
    except Exception as e:
        print(f"  HID error: {e}")

def try_hid_query(dev):
    """Try sending a query via HID OUT (EP 0x03) and reading response."""
    print("\n=== Trying HID query (EP 0x03 OUT -> EP 0x82 IN) ===")
    try:
        usb.util.claim_interface(dev, 1)
        # Try some common probe commands
        probes = [
            bytes([0x00] * 64),          # zeros
            bytes([0x01] + [0x00] * 63), # simple command 0x01
            bytes([0x10] + [0x00] * 63), # command 0x10
            bytes([0x55] + [0x00] * 63), # command 0x55
            bytes([0xAA] + [0x00] * 63), # command 0xAA
            bytes([0xFF] + [0x00] * 63), # command 0xFF
        ]
        for probe in probes:
            try:
                written = dev.write(0x03, probe, timeout=500)
                print(f"  Sent {probe[:4].hex()}... ({written} bytes)")
                time.sleep(0.05)
                try:
                    resp = dev.read(0x82, 1024, timeout=500)
                    print(f"  -> Response: {resp.tobytes().hex()}")
                except usb.core.USBTimeoutError:
                    print(f"  -> No response (timeout)")
            except Exception as e:
                print(f"  Error with {probe[:4].hex()}: {e}")
        usb.util.release_interface(dev, 1)
    except Exception as e:
        print(f"  HID query error: {e}")

def explore_bulk(dev):
    """Try reading from bulk interface (interface 0, EP 0x81 IN)."""
    print("\n=== Reading from Bulk interface (EP 0x81) ===")
    try:
        usb.util.claim_interface(dev, 0)
        for i in range(3):
            try:
                data = dev.read(0x81, 512, timeout=1000)
                print(f"  Bulk read {i}: {data.tobytes().hex()}")
            except usb.core.USBTimeoutError:
                print(f"  Bulk read {i}: timeout (no data)")
            except Exception as e:
                print(f"  Bulk read {i}: error: {e}")
        usb.util.release_interface(dev, 0)
    except Exception as e:
        print(f"  Bulk error: {e}")

def get_descriptors(dev):
    """Try to get additional string/vendor descriptors."""
    print("\n=== String descriptors ===")
    for i in range(10):
        try:
            s = usb.util.get_string(dev, i)
            if s:
                print(f"  String {i}: {s}")
        except Exception:
            pass

    print("\n=== Control transfers (vendor requests) ===")
    for req in [0x00, 0x01, 0x06, 0x10, 0x20, 0x51, 0x80, 0xFE, 0xFF]:
        try:
            ret = dev.ctrl_transfer(0xC0, req, 0, 0, 64, timeout=500)
            print(f"  Vendor req 0x{req:02x}: {ret.tobytes().hex()}")
        except Exception as e:
            pass  # Most will fail, that's expected

def main():
    dev = find_device()
    detach_kernel(dev)
    dev.set_configuration()
    get_descriptors(dev)
    explore_hid(dev)
    try_hid_query(dev)
    explore_bulk(dev)
    print("\nDone.")

if __name__ == "__main__":
    main()
