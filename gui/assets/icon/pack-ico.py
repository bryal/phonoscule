#!/usr/bin/env python3
"""Pack PNGs into a Windows .ico.

    python pack-ico.py phonoscule.ico 256.png 128.png 64.png 48.png 32.png 24.png 16.png

Only the packing, because that is the only part with no command-line tool to do it: rasterising the
SVGs is a one-liner with whatever renderer is to hand (see README.md). Pure standard library, so it
needs nothing installed.

The .ico holds the PNGs as they are rather than converting them to BMP. Every Windows since Vista
reads that, it keeps the alpha channel intact, and it means this script never has to decode an
image -- only read the size out of each header.
"""

import struct
import sys
from pathlib import Path


def png_size(data: bytes) -> tuple[int, int]:
    """The dimensions from a PNG's IHDR, which is required to be its first chunk."""
    if data[:8] != b"\x89PNG\r\n\x1a\n" or data[12:16] != b"IHDR":
        raise ValueError("not a PNG")
    return struct.unpack(">II", data[16:24])


def ico(images: list[tuple[int, int, bytes]]) -> bytes:
    """An ICONDIR of PNG-bodied entries, in the order given."""
    header = struct.pack("<HHH", 0, 1, len(images))  # reserved, type 1 = icon, count
    # Every directory entry is a fixed 16 bytes, so the first image begins after all of them.
    offset = len(header) + 16 * len(images)
    entries, bodies = b"", b""
    for width, height, png in images:
        entries += struct.pack(
            "<BBBBHHII",
            0 if width >= 256 else width,  # 0 means 256, the field being a single byte
            0 if height >= 256 else height,
            0,  # palette entries, 0 for truecolour
            0,  # reserved
            1,  # colour planes
            32,  # bits per pixel
            len(png),
            offset,
        )
        bodies += png
        offset += len(png)
    return header + entries + bodies


def main(argv: list[str]) -> None:
    if len(argv) < 3:
        sys.exit(__doc__)
    out, sources = Path(argv[1]), [Path(a) for a in argv[2:]]

    images = []
    for source in sources:
        data = source.read_bytes()
        try:
            width, height = png_size(data)
        except ValueError as e:
            sys.exit(f"{source}: {e}")
        if width != height:
            sys.exit(f"{source}: {width}x{height} is not square")
        if not 1 <= width <= 256:
            sys.exit(f"{source}: {width}px is outside the 1-256 an .ico can hold")
        images.append((width, height, data))

    sizes = [w for w, _, _ in images]
    if len(set(sizes)) != len(sizes):
        sys.exit(f"two sources are the same size: {sorted(sizes)}")

    # Largest first: some readers take the first entry big enough rather than the closest fit.
    images.sort(key=lambda image: image[0], reverse=True)
    out.write_bytes(ico(images))
    print(f"{out}: {len(images)} sizes {sorted(sizes, reverse=True)}, {out.stat().st_size} bytes")


if __name__ == "__main__":
    main(sys.argv)
