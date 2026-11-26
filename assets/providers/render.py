"""Regenerate embedded RGBA/preview PNGs from the pinned SVGs, without network.
Development-only dependencies: Python GI Rsvg 2, pycairo and Pillow.
The application ships pre-rendered pixels and requires none of these packages.
"""
from pathlib import Path
from io import BytesIO
import gi
import cairo
from PIL import Image

gi.require_version('Rsvg', '2.0')
from gi.repository import Rsvg

SIZE = 96
base = Path(__file__).resolve().parent
for path in sorted(base.glob('*.svg')):
    source = path.read_bytes()
    # A currentColor icon follows the same off-white foreground as the board.
    source = source.replace(b'currentColor', b'#f1f5f9')
    handle = Rsvg.Handle.new_from_data(source)
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, SIZE, SIZE)
    viewport = Rsvg.Rectangle()
    viewport.x = viewport.y = 0
    viewport.width = viewport.height = SIZE
    handle.render_document(cairo.Context(surface), viewport)
    buffer = BytesIO()
    surface.write_to_png(buffer)
    rendered = Image.open(BytesIO(buffer.getvalue())).convert('RGBA')
    rendered.save(path.with_suffix('.png'))
    path.with_suffix('.rgba').write_bytes(rendered.tobytes())
