"""Inspect the Windows application icon in a final EXE or release ZIP."""

import hashlib
import json
import pathlib
import struct
import sys
import unittest
import zipfile


BINARY = pathlib.Path(sys.argv.pop(1))
EXPECTED_SIZES = {16, 24, 32, 48, 256}
ASSETS = pathlib.Path(__file__).resolve().parents[1] / "assets" / "application"
APPROVED = json.loads((ASSETS / "exports.json").read_text())


def release_exe_bytes():
    if BINARY.suffix.lower() == ".zip":
        with zipfile.ZipFile(BINARY) as archive:
            names = [name for name in archive.namelist() if pathlib.PurePosixPath(name).name == "token-ashtray.exe"]
            if len(names) != 1:
                raise AssertionError(f"Expected one token-ashtray.exe in {BINARY}, found {len(names)}")
            return archive.read(names[0])
    return BINARY.read_bytes()


class Resources:
    def __init__(self, data):
        self.data = data
        self.u16 = lambda offset: struct.unpack_from("<H", data, offset)[0]
        self.u32 = lambda offset: struct.unpack_from("<I", data, offset)[0]
        pe = self.u32(0x3C)
        if data[pe:pe + 4] != b"PE\0\0":
            raise AssertionError("Not a PE executable")
        optional = pe + 24
        magic = self.u16(optional)
        if magic not in (0x10B, 0x20B):
            raise AssertionError(f"Unsupported PE optional header: {magic:#x}")
        directories = optional + (112 if magic == 0x20B else 96)
        resource_rva, resource_size = struct.unpack_from("<II", data, directories + 16)
        if resource_rva == 0 or resource_size == 0:
            raise AssertionError("EXE has no resources: application icon missing")
        sections = optional + self.u16(pe + 20)
        self.sections = []
        for index in range(self.u16(pe + 6)):
            section = sections + index * 40
            virtual_size, address, raw_size, raw = struct.unpack_from("<IIII", data, section + 8)
            self.sections.append((address, max(virtual_size, raw_size), raw))
        self.root = self.file_offset(resource_rva)

    def file_offset(self, rva):
        for address, size, raw in self.sections:
            if address <= rva < address + size:
                return raw + rva - address
        raise AssertionError(f"Resource RVA {rva:#x} does not map to a PE section")

    def entries(self, directory):
        count = self.u16(directory + 12) + self.u16(directory + 14)
        for index in range(count):
            name, child = struct.unpack_from("<II", self.data, directory + 16 + index * 8)
            if name & 0x80000000:
                continue
            yield name, child

    def leaf(self, child):
        if child & 0x80000000:
            for _, nested in self.entries(self.root + (child & 0x7FFFFFFF)):
                yield from self.leaf(nested)
        else:
            descriptor = self.root + child
            rva, size = struct.unpack_from("<II", self.data, descriptor)
            offset = self.file_offset(rva)
            payload = self.data[offset:offset + size]
            if len(payload) != size:
                raise AssertionError("Truncated resource payload")
            yield payload

    def type_records(self, type_id):
        for name, child in self.entries(self.root):
            if name == type_id and child & 0x80000000:
                for resource_id, nested in self.entries(self.root + (child & 0x7FFFFFFF)):
                    yield resource_id, list(self.leaf(nested))


class WindowsApplicationIcon(unittest.TestCase):
    def test_release_embeds_multisize_application_icon(self):
        resources = Resources(release_exe_bytes())
        icon_records = dict(resources.type_records(3))
        groups = dict(resources.type_records(14))
        self.assertTrue(icon_records, "Missing RT_ICON image resources")
        self.assertTrue(groups, "Missing RT_GROUP_ICON application icon")
        for group_payloads in groups.values():
            self.assertTrue(group_payloads, "RT_GROUP_ICON has no language entry")
            for group in group_payloads:
                self.assertGreaterEqual(len(group), 6)
                reserved, kind, count = struct.unpack_from("<HHH", group)
                self.assertEqual((reserved, kind), (0, 1))
                self.assertEqual(len(group), 6 + count * 14, "Truncated GRPICONDIR")
                sizes = set()
                for index in range(count):
                    width, height, _, _, _, bit_count, byte_count, resource_id = struct.unpack_from(
                        "<BBBBHHIH", group, 6 + index * 14
                    )
                    width = width or 256
                    height = height or 256
                    self.assertEqual(width, height)
                    self.assertEqual(bit_count, 32)
                    self.assertNotIn(width, sizes, "Duplicate icon size")
                    sizes.add(width)
                    self.assertIn(resource_id, icon_records, f"Missing RT_ICON {resource_id}")
                    images = icon_records[resource_id]
                    self.assertTrue(images, f"RT_ICON {resource_id} has no language entry")
                    for image in images:
                        self.assertGreater(len(image), 40, f"Empty RT_ICON {resource_id}")
                        self.assertEqual(len(image), byte_count)
                        header_size, image_width, image_height = struct.unpack_from("<Iii", image)
                        self.assertEqual(header_size, 40)
                        self.assertEqual((image_width, image_height), (width, height * 2))
                        # Compare decoded resource pixels to the committed, reviewed export
                        # manifest. This catches old artwork with otherwise valid dimensions.
                        expected = APPROVED["exports"][f"token-ashtray-{width}.png"]
                        png = ASSETS / f"token-ashtray-{width}.png"
                        self.assertEqual(hashlib.sha256(png.read_bytes()).hexdigest(), expected["sha256"])
                        stride = ((width + 31) // 32) * 4
                        self.assertEqual(len(image), 40 + width * height * 4 + stride * height)
                        rgba = bytearray()
                        for y in range(height):
                            row = height - 1 - y
                            for x in range(width):
                                offset = 40 + (row * width + x) * 4
                                b, g, r, a = image[offset:offset + 4]
                                rgba.extend((r, g, b, a))
                                mask = image[40 + width * height * 4 + row * stride + x // 8]
                                self.assertEqual(bool(mask & (0x80 >> (x % 8))), a == 0)
                        self.assertEqual(hashlib.sha256(rgba).hexdigest(), expected["rgba_sha256"],
                                         f"{width}px resource does not contain the approved artwork")
                self.assertEqual(sizes, EXPECTED_SIZES, "Application icon sizes changed")


if __name__ == "__main__":
    unittest.main()
