#!/usr/bin/env python3
"""Explore DYNAMIC DASHBOARD III - might control the Lightning LCD."""

import os
import sys
import glob
import struct
import time
import fcntl

# Find hidraw devices for both MSI devices
print("=== Finding hidraw devices ===")
for hidraw in sorted(glob.glob('/dev/hidraw*')):
    try:
        sysfs = f"/sys/class/hidraw/{os.path.basename(hidraw)}/device/uevent"
        with open(sysfs) as f:
            uevent = f.read()
        if '0DB0' in uevent:
            print(f"\n{hidraw}:")
            print(f"  {uevent.strip()}")

            # Get HID report descriptor
            fd = os.open(hidraw, os.O_RDWR | os.O_NONBLOCK)
            try:
                buf = bytearray(4)
                fcntl.ioctl(fd, 0x40044801, buf)  # HIDIOCGRDESCSIZE
                desc_size = struct.unpack('<I', buf)[0]
                print(f"  Report descriptor size: {desc_size}")

                buf2 = bytearray(4 + desc_size)
                struct.pack_into('<I', buf2, 0, desc_size)
                fcntl.ioctl(fd, 0xC0044802, buf2)  # HIDIOCGRDESC
                desc = buf2[4:4+desc_size]
                print(f"  Report descriptor: {desc.hex()}")

                # Decode basic HID descriptor info
                i = 0
                indent = 0
                while i < len(desc):
                    b = desc[i]
                    bSize = b & 0x03
                    if bSize == 3:
                        bSize = 4
                    bType = (b >> 2) & 0x03
                    bTag = (b >> 4) & 0x0F

                    types = {0: "Main", 1: "Global", 2: "Local"}
                    main_tags = {8: "Input", 9: "Output", 10: "Collection", 11: "Feature", 12: "End Collection"}
                    global_tags = {0: "Usage Page", 1: "Logical Min", 2: "Logical Max", 3: "Physical Min",
                                   4: "Physical Max", 5: "Unit Exp", 6: "Unit", 7: "Report Size",
                                   8: "Report ID", 9: "Report Count"}
                    local_tags = {0: "Usage", 1: "Usage Min", 2: "Usage Max"}

                    val = 0
                    if bSize >= 1 and i+1 < len(desc): val = desc[i+1]
                    if bSize >= 2 and i+2 < len(desc): val = desc[i+1] | (desc[i+2] << 8)
                    if bSize >= 4 and i+4 < len(desc): val = desc[i+1] | (desc[i+2] << 8) | (desc[i+3] << 16) | (desc[i+4] << 24)

                    type_name = types.get(bType, f"Type{bType}")
                    if bType == 0:
                        tag_name = main_tags.get(bTag, f"Tag{bTag}")
                        if bTag == 12:
                            indent -= 1
                    elif bType == 1:
                        tag_name = global_tags.get(bTag, f"Tag{bTag}")
                    elif bType == 2:
                        tag_name = local_tags.get(bTag, f"Tag{bTag}")
                    else:
                        tag_name = f"Tag{bTag}"

                    prefix = "  " * (indent + 2)
                    print(f"{prefix}{type_name}: {tag_name} = 0x{val:04x} ({val})")

                    if bType == 0 and bTag == 10:  # Collection
                        indent += 1

                    i += 1 + bSize

            except Exception as e:
                print(f"  Descriptor error: {e}")

            # Try reading
            print(f"\n  Reading...")
            for _ in range(3):
                try:
                    data = os.read(fd, 1024)
                    print(f"    Read: {data.hex()}")
                except BlockingIOError:
                    print(f"    No data available")
                    break
                except Exception as e:
                    print(f"    Read error: {e}")
                    break

            os.close(fd)
    except PermissionError:
        print(f"  {hidraw}: permission denied")
    except Exception as e:
        print(f"  {hidraw}: {e}")

print("\nDone.")
