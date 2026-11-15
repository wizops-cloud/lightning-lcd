#!/usr/bin/env python3
"""Explore MSI V530-9.6 Lightning LCD - focus on control transfers and hidraw."""

import usb.core
import usb.util
import struct
import time
import sys
import os
import glob

VENDOR_ID = 0x0db0
PRODUCT_ID = 0x7881
WIDTH = 1280
HEIGHT = 800

def decode_edid_partial(data):
    """Decode some interesting EDID fields."""
    if len(data) < 128:
        return
    # Manufacturer
    mfg = (data[0] << 8) | data[1]
    c1 = ((mfg >> 10) & 0x1f) + ord('A') - 1
    c2 = ((mfg >> 5) & 0x1f) + ord('A') - 1
    c3 = (mfg & 0x1f) + ord('A') - 1
    print(f"  Manufacturer: {chr(c1)}{chr(c2)}{chr(c3)}")
    print(f"  Product code: 0x{data[2]:02x}{data[3]:02x}")
    # Find descriptor strings
    for base in [54, 72, 90, 108]:
        if base + 18 <= len(data):
            tag = data[base + 3]
            if tag == 0xFC:  # Monitor name
                name = bytes(data[base+5:base+18]).decode('ascii', errors='replace').strip()
                print(f"  Monitor name: {name}")
            elif tag == 0xFF:  # Serial
                serial = bytes(data[base+5:base+18]).decode('ascii', errors='replace').strip()
                print(f"  Serial: {serial}")
            elif tag == 0xFD:  # Range limits
                print(f"  VFreq: {data[base+5]}-{data[base+6]} Hz, HFreq: {data[base+7]}-{data[base+8]} kHz")

# Connect to device
dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
if dev is None:
    print("Not found!")
    sys.exit(1)
print(f"Found: {dev.product}\n")

# Decode EDID from vendor request
print("=== EDID from vendor req 0x00 ===")
try:
    raw = dev.ctrl_transfer(0xC0, 0x00, 0, 0, 256, timeout=500)
    header = raw[:10]
    edid = raw[10:]
    print(f"  Header (10 bytes): {bytes(header).hex()}")
    print(f"  EDID ({len(edid)} bytes):")
    decode_edid_partial(edid)
except Exception as e:
    print(f"  Error: {e}")

# Explore more vendor requests
print("\n=== Vendor requests (wider scan) ===")
for req in range(0x30):
    for wval in [0, 1, 2]:
        for widx in [0, 1]:
            try:
                ret = dev.ctrl_transfer(0xC0, req, wval, widx, 256, timeout=100)
                if len(ret) > 0:
                    h = bytes(ret).hex()
                    if h != '000520033c00' and 'ffffffffffff' not in h:  # skip known
                        print(f"  req=0x{req:02x} val={wval} idx={widx}: [{len(ret)}] {h[:80]}...")
            except Exception:
                pass

# Try vendor OUT requests (write)
print("\n=== Vendor OUT control transfers ===")
for req in range(0x20):
    for wval in [0, 1, 0x0500, 0x0320]:
        try:
            dev.ctrl_transfer(0x40, req, wval, 0, timeout=200)
            print(f"  OUT req=0x{req:02x} val=0x{wval:04x}: OK (accepted)")
        except usb.core.USBError as e:
            if 'pipe' in str(e).lower():
                pass  # STALL = not supported
            else:
                print(f"  OUT req=0x{req:02x} val=0x{wval:04x}: {e}")

# Try OUT with data payload
print("\n=== Vendor OUT with data ===")
for req in [0x01, 0x02, 0x03, 0x10, 0x20]:
    for payload in [
        struct.pack('<HH', WIDTH, HEIGHT),
        struct.pack('<HHB', WIDTH, HEIGHT, 1),
        bytes([0x01, 0x00, 0x00, 0x00]),
    ]:
        try:
            dev.ctrl_transfer(0x40, req, 0, 0, payload, timeout=200)
            print(f"  OUT req=0x{req:02x} data={payload.hex()}: OK")
        except usb.core.USBError as e:
            if 'pipe' not in str(e).lower():
                print(f"  OUT req=0x{req:02x} data={payload.hex()}: {e}")

# Now try hidraw
print("\n=== HID via hidraw ===")
hidraw_path = None
for hidraw in sorted(glob.glob('/dev/hidraw*')):
    try:
        sysfs = f"/sys/class/hidraw/{os.path.basename(hidraw)}/device/uevent"
        with open(sysfs) as f:
            if '0DB0' in f.read() and '7881' in open(sysfs).read():
                hidraw_path = hidraw
                break
    except Exception:
        pass

if hidraw_path:
    print(f"Using {hidraw_path}")
    fd = os.open(hidraw_path, os.O_RDWR | os.O_NONBLOCK)

    # Try reading HID report descriptor
    import fcntl
    # HIDIOCGRDESCSIZE = 0x40044801
    # HIDIOCGRDESC = 0x50044802
    try:
        buf = bytearray(4)
        fcntl.ioctl(fd, 0x40044801, buf)  # HIDIOCGRDESCSIZE (read)
        desc_size = struct.unpack('<I', buf)[0]
        print(f"  HID report descriptor size: {desc_size}")

        buf2 = bytearray(4 + desc_size)
        struct.pack_into('<I', buf2, 0, desc_size)
        fcntl.ioctl(fd, 0xC0044802, buf2)  # HIDIOCGRDESC
        desc = buf2[4:4+desc_size]
        print(f"  HID report descriptor: {desc.hex()}")
    except Exception as e:
        print(f"  Descriptor error: {e}")

    # Try various HID report writes and reads
    print("\n  HID write/read probes:")
    for report in [
        bytes(64),
        bytes([0x05]) + bytes(63),
        bytes([0x05, 0x01]) + bytes(62),
        bytes([0x05, 0x20, 0x05, 0x00, 0x20, 0x03]) + bytes(58),  # resolution?
    ]:
        try:
            os.write(fd, report)
            print(f"    Write {report[:6].hex()}: OK")
            time.sleep(0.05)
            try:
                resp = os.read(fd, 1024)
                print(f"      -> Response: {resp.hex()}")
            except BlockingIOError:
                print(f"      -> No response")
        except OSError as e:
            print(f"    Write {report[:6].hex()}: {e}")

    os.close(fd)
else:
    print("No hidraw found for this device")

print("\nDone.")
