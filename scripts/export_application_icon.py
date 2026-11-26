"""Reproduce the static application icon exports (requires Pillow 9.0.1).

This is an offline asset step, not part of Cargo builds. Premultiplied-alpha
resampling prevents transparent RGB values from creating colored halos.
"""
import hashlib
import json
from pathlib import Path
import struct

from PIL import Image, __version__

ROOT = Path(__file__).resolve().parents[1] / "assets" / "application"
SIZES = (16, 24, 32, 48, 64, 256)
ICO_SIZES = (16, 24, 32, 48, 256)


def dib(image):
    size = image.width
    rgba = image.tobytes()
    stride = ((size + 31) // 32) * 4
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, size * size * 4, 0, 0, 0, 0)
    pixels = bytearray()
    mask = bytearray()
    for y in reversed(range(size)):
        row = bytearray(stride)
        for x in range(size):
            r, g, b, a = rgba[(y * size + x) * 4:(y * size + x + 1) * 4]
            pixels.extend((b, g, r, a))
            if a == 0:
                row[x // 8] |= 0x80 >> (x % 8)
        mask.extend(row)
    return header + pixels + mask


def main():
    if __version__ != "9.0.1":
        raise SystemExit("Use Pillow 9.0.1 for byte-reproducible exports")
    source = ROOT / "reference-original.png"
    source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    if source_hash != "3fa3552f44825f53b77397291673149242d602f9f0b5a7384381dd3a89997495":
        raise SystemExit("Approved original hash mismatch")
    original = Image.open(source).convert("RGBA")
    images = {}
    manifest = {"source": source.name, "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
                "pillow": __version__, "resampling": "premultiplied RGBA, Lanczos, original canvas", "exports": {}}
    for size in SIZES:
        image = original.convert("RGBa").resize((size, size), Image.LANCZOS).convert("RGBA")
        path = ROOT / f"token-ashtray-{size}.png"
        image.save(path)
        images[size] = image
        manifest["exports"][path.name] = {"size": size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                                          "rgba_sha256": hashlib.sha256(image.tobytes()).hexdigest()}
    payloads = [dib(images[size]) for size in ICO_SIZES]
    directory = bytearray(struct.pack("<HHH", 0, 1, len(payloads)))
    offset = 6 + 16 * len(payloads)
    for size, payload in zip(ICO_SIZES, payloads):
        directory.extend(struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(payload), offset))
        offset += len(payload)
    ico = ROOT / "token-ashtray.ico"
    ico.write_bytes(directory + b"".join(payloads))
    manifest["ico_sha256"] = hashlib.sha256(ico.read_bytes()).hexdigest()
    (ROOT / "exports.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
