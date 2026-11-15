#!/usr/bin/env python3
"""Simple test - write small chunks to MSI V530-9.6 Lightning LCD."""

import usb.core
import usb.util
import time
import sys

VENDOR_ID = 0x0db0
PRODUCT_ID = 0x7881

print("Waiting 3s for device to stabilize...")
time.sleep(3)

dev = usb.core.find(idVendor=VENDOR_ID, idProduct=PRODUCT_ID)
if dev is None:
    print("Not found!")
    sys.exit(1)
print(f"Found: {dev.product}")
print(f"  Interface 0 kernel driver: {dev.is_kernel_driver_active(0)}")
print(f"  Interface 1 kernel driver: {dev.is_kernel_driver_active(1)}")

# Claim only interface 0
usb.util.claim_interface(dev, 0)
print("Interface 0 claimed OK")

# Test 1: small writes of 512 bytes (max packet size)
print("\n=== Small 512-byte writes ===")
for i in range(20):
    color_byte = (i * 13) & 0xFF
    chunk = bytes([color_byte]) * 512
    try:
        w = dev.write(0x01, chunk, timeout=2000)
        print(f"  Write {i}: {w} bytes (0x{color_byte:02x})")
    except Exception as e:
        print(f"  Write {i}: FAILED - {e}")
        break

# Test 2: gradually increase write size
print("\n=== Increasing write sizes ===")
for size in [64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536]:
    # Re-check device
    try:
        chunk = bytes([0xFF, 0x00, 0x00]) * (size // 3 + 1)
        chunk = chunk[:size]
        w = dev.write(0x01, chunk, timeout=2000)
        print(f"  Write {size} bytes: OK ({w} written)")
    except Exception as e:
        print(f"  Write {size} bytes: FAILED - {e}")
        break

# Test 3: total bulk throughput
print("\n=== Continuous write (solid red RGB888, 512-byte chunks) ===")
total_needed = 1280 * 800 * 3  # 3,072,000 bytes for RGB888
red_chunk = bytes([0xFF, 0x00, 0x00] * 170 + [0xFF, 0x00])  # ~512 bytes
red_chunk = (bytes([0xFF, 0x00, 0x00]) * 171)[:512]
total_sent = 0
errors = 0
start = time.time()
while total_sent < total_needed:
    try:
        w = dev.write(0x01, red_chunk, timeout=2000)
        total_sent += w
    except Exception as e:
        errors += 1
        if errors > 3:
            print(f"  Too many errors at {total_sent} bytes: {e}")
            break
        time.sleep(0.01)

elapsed = time.time() - start
print(f"  Sent {total_sent}/{total_needed} bytes in {elapsed:.2f}s ({total_sent/elapsed/1024:.0f} KB/s)")
print(f"  CHECK THE SCREEN!")

time.sleep(3)

try:
    usb.util.release_interface(dev, 0)
except Exception:
    pass
print("Done.")
