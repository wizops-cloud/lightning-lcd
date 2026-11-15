#!/usr/bin/env python3
"""Probe MSI devices via HID (hidraw) - both Lightning and Dashboard."""

import os
import time
import struct
import select

LIGHTNING_HIDRAW = "/dev/hidraw2"   # V530-9.6 Lightning
DASHBOARD_HIDRAW = "/dev/hidraw10"  # DYNAMIC DASHBOARD III

def probe_device(path, name, reports):
    print(f"\n{'='*60}")
    print(f"Probing {name} ({path})")
    print(f"{'='*60}")

    try:
        fd = os.open(path, os.O_RDWR)
    except Exception as e:
        print(f"  Cannot open: {e}")
        return

    for desc, data in reports:
        print(f"\n  >>> Sending: {desc}")
        print(f"      Data: {data[:32].hex()}{'...' if len(data) > 32 else ''} ({len(data)} bytes)")
        try:
            os.write(fd, data)
            print(f"      Write OK")

            # Wait for response
            time.sleep(0.1)
            r, _, _ = select.select([fd], [], [], 0.5)
            if r:
                resp = os.read(fd, 4096)
                print(f"      Response: {resp.hex()}")
            else:
                print(f"      No response")
        except OSError as e:
            print(f"      Error: {e}")

    os.close(fd)

# === Probe Lightning HID (no report ID, 1024 bytes) ===
lightning_probes = []

# Simple queries
for first_byte in [0x00, 0x01, 0x02, 0x05, 0x10, 0x20, 0x40, 0x55, 0x80, 0xAA, 0xFF]:
    lightning_probes.append(
        (f"cmd 0x{first_byte:02x}", bytes([first_byte]) + b'\x00' * 1023)
    )

# Try some common patterns
lightning_probes.append(("get info", bytes([0x05, 0x01]) + b'\x00' * 1022))
lightning_probes.append(("get version", bytes([0x05, 0x02]) + b'\x00' * 1022))
lightning_probes.append(("get status", bytes([0x05, 0x03]) + b'\x00' * 1022))
lightning_probes.append(("get resolution", bytes([0x05, 0x10]) + b'\x00' * 1022))

probe_device(LIGHTNING_HIDRAW, "V530-9.6 Lightning", lightning_probes)

# === Probe Dashboard HID (report IDs 1 and 2) ===
dashboard_probes = []

# Report ID 1: 1 + 63 bytes
for first_byte in [0x00, 0x01, 0x02, 0x05, 0x10, 0x20, 0x40, 0x55, 0x80, 0xAA, 0xFF]:
    dashboard_probes.append(
        (f"Report1 cmd 0x{first_byte:02x}", bytes([0x01, first_byte]) + b'\x00' * 62)
    )

# Report ID 2: 1 + 1023 bytes (output only)
for first_byte in [0x00, 0x01, 0x10, 0x55]:
    dashboard_probes.append(
        (f"Report2 cmd 0x{first_byte:02x}", bytes([0x02, first_byte]) + b'\x00' * 1022)
    )

# Try sending image-like data via Report ID 2
# Small red JPEG-like header
dashboard_probes.append(
    ("Report2 JPEG header", bytes([0x02, 0xFF, 0xD8, 0xFF, 0xE0]) + b'\x00' * 1019)
)

probe_device(DASHBOARD_HIDRAW, "DYNAMIC DASHBOARD III", dashboard_probes)

print("\n\nDone!")
