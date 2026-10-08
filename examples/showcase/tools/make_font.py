#!/usr/bin/env python3
"""Generate the showcase's font from scratch.

The example project must be reproducible from a clean checkout with no
third-party binaries, so it cannot ship a proprietary system font. Instead this
script *builds* a small TrueType font from a hand-drawn 5x7 pixel bitmap and
writes it to `assets/fonts/NoctisSans.ttf`.

The font is deliberately minimal but a genuinely well-formed sfnt:

* tables: `OS/2`, `cmap`, `glyf`, `head`, `hhea`, `hmtx`, `loca`, `maxp`,
  `name`, `post`;
* long `loca`, format-4 `cmap`, and a real `head.checkSumAdjustment`;
* `unitsPerEm = 1024`; the em square maps `PIX = 128` units per bitmap pixel,
  so a glyph is `5 * 128 = 640` units wide and `7 * 128 = 896` units tall;
* uppercase and lowercase share outlines (a single-way "caps" font), which keeps
  the outline set small while still covering the printable text the scenes use.

Run it from anywhere; it writes next to the script:

    python tools/make_font.py

The output is deterministic — running it twice produces byte-identical bytes.
"""

from __future__ import annotations

import math
import os
import struct
import sys

# ---------------------------------------------------------------------------
# Metrics
# ---------------------------------------------------------------------------

UPM = 1024          # units per em
PIX = 128           # units per bitmap pixel
COLS = 5
ROWS = 7
ASCENT = 896        # hhea.ascender    (7 * 128)
DESCENT = -128      # hhea.descender
LINE_GAP = 128      # hhea.lineGap
ADVANCE = 768       # per-glyph advance (5 * 128 + 128)

FAMILY = "Noctis Sans"
STYLE = "Regular"
VERSION = "1.0"

# ---------------------------------------------------------------------------
# The 5x7 bitmap glyphs
# ---------------------------------------------------------------------------

GLYPHS: dict[str, list[str]] = {
    " ": [
        ".....",
        ".....",
        ".....",
        ".....",
        ".....",
        ".....",
        ".....",
    ],
    "0": [
        ".###.",
        "#...#",
        "#...#",
        "#...#",
        "#...#",
        "#...#",
        ".###.",
    ],
    "1": [
        "..#..",
        ".##..",
        "..#..",
        "..#..",
        "..#..",
        "..#..",
        ".###.",
    ],
    "2": [
        ".###.",
        "#...#",
        "....#",
        "...#.",
        "..#..",
        ".#...",
        "#####",
    ],
    "3": [
        "#####",
        "...#.",
        "..#..",
        "...#.",
        "....#",
        "#...#",
        ".###.",
    ],
    "4": [
        "...#.",
        "..##.",
        ".#.#.",
        "#..#.",
        "#####",
        "...#.",
        "...#.",
    ],
    "5": [
        "#####",
        "#....",
        "####.",
        "....#",
        "....#",
        "#...#",
        ".###.",
    ],
    "6": [
        "..##.",
        ".#...",
        "#....",
        "####.",
        "#...#",
        "#...#",
        ".###.",
    ],
    "7": [
        "#####",
        "....#",
        "...#.",
        "..#..",
        ".#...",
        ".#...",
        ".#...",
    ],
    "8": [
        ".###.",
        "#...#",
        "#...#",
        ".###.",
        "#...#",
        "#...#",
        ".###.",
    ],
    "9": [
        ".###.",
        "#...#",
        "#...#",
        ".####",
        "....#",
        "...#.",
        ".##..",
    ],
    "A": [
        "..#..",
        ".#.#.",
        "#...#",
        "#...#",
        "#####",
        "#...#",
        "#...#",
    ],
    "B": [
        "####.",
        "#...#",
        "#...#",
        "####.",
        "#...#",
        "#...#",
        "####.",
    ],
    "C": [
        ".###.",
        "#...#",
        "#....",
        "#....",
        "#....",
        "#...#",
        ".###.",
    ],
    "D": [
        "###..",
        "#..#.",
        "#...#",
        "#...#",
        "#...#",
        "#..#.",
        "###..",
    ],
    "E": [
        "#####",
        "#....",
        "#....",
        "####.",
        "#....",
        "#....",
        "#####",
    ],
    "F": [
        "#####",
        "#....",
        "#....",
        "####.",
        "#....",
        "#....",
        "#....",
    ],
    "G": [
        ".###.",
        "#...#",
        "#....",
        "#.###",
        "#...#",
        "#...#",
        ".###.",
    ],
    "H": [
        "#...#",
        "#...#",
        "#...#",
        "#####",
        "#...#",
        "#...#",
        "#...#",
    ],
    "I": [
        ".###.",
        "..#..",
        "..#..",
        "..#..",
        "..#..",
        "..#..",
        ".###.",
    ],
    "J": [
        "..###",
        "...#.",
        "...#.",
        "...#.",
        "...#.",
        "#..#.",
        ".##..",
    ],
    "K": [
        "#...#",
        "#..#.",
        "#.#..",
        "##...",
        "#.#..",
        "#..#.",
        "#...#",
    ],
    "L": [
        "#....",
        "#....",
        "#....",
        "#....",
        "#....",
        "#....",
        "#####",
    ],
    "M": [
        "#...#",
        "##.##",
        "#.#.#",
        "#...#",
        "#...#",
        "#...#",
        "#...#",
    ],
    "N": [
        "#...#",
        "##..#",
        "#.#.#",
        "#..##",
        "#...#",
        "#...#",
        "#...#",
    ],
    "O": [
        ".###.",
        "#...#",
        "#...#",
        "#...#",
        "#...#",
        "#...#",
        ".###.",
    ],
    "P": [
        "####.",
        "#...#",
        "#...#",
        "####.",
        "#....",
        "#....",
        "#....",
    ],
    "Q": [
        ".###.",
        "#...#",
        "#...#",
        "#...#",
        "#.#.#",
        "#..#.",
        ".##.#",
    ],
    "R": [
        "####.",
        "#...#",
        "#...#",
        "####.",
        "#.#..",
        "#..#.",
        "#...#",
    ],
    "S": [
        ".####",
        "#....",
        "#....",
        ".###.",
        "....#",
        "....#",
        "####.",
    ],
    "T": [
        "#####",
        "..#..",
        "..#..",
        "..#..",
        "..#..",
        "..#..",
        "..#..",
    ],
    "U": [
        "#...#",
        "#...#",
        "#...#",
        "#...#",
        "#...#",
        "#...#",
        ".###.",
    ],
    "V": [
        "#...#",
        "#...#",
        "#...#",
        "#...#",
        "#...#",
        ".#.#.",
        "..#..",
    ],
    "W": [
        "#...#",
        "#...#",
        "#...#",
        "#.#.#",
        "#.#.#",
        "##.##",
        "#...#",
    ],
    "X": [
        "#...#",
        "#...#",
        ".#.#.",
        "..#..",
        ".#.#.",
        "#...#",
        "#...#",
    ],
    "Y": [
        "#...#",
        "#...#",
        ".#.#.",
        "..#..",
        "..#..",
        "..#..",
        "..#..",
    ],
    "Z": [
        "#####",
        "....#",
        "...#.",
        "..#..",
        ".#...",
        "#....",
        "#####",
    ],
    ".": [
        ".....",
        ".....",
        ".....",
        ".....",
        ".....",
        ".##..",
        ".##..",
    ],
    ":": [
        ".....",
        ".##..",
        ".##..",
        ".....",
        ".##..",
        ".##..",
        ".....",
    ],
    "-": [
        ".....",
        ".....",
        ".....",
        "#####",
        ".....",
        ".....",
        ".....",
    ],
    "!": [
        "..#..",
        "..#..",
        "..#..",
        "..#..",
        "..#..",
        ".....",
        "..#..",
    ],
}

# Which glyph each character maps to (lowercase folds to uppercase).
CHAR_TO_GLYPH: dict[str, str] = {}
for _ch in "0123456789":
    CHAR_TO_GLYPH[_ch] = _ch
for _ch in "ABCDEFGHIJKLMNOPQRSTUVWXYZ":
    CHAR_TO_GLYPH[_ch] = _ch
for _ch in "abcdefghijklmnopqrstuvwxyz":
    CHAR_TO_GLYPH[_ch] = _ch.upper()
for _ch in " ":
    CHAR_TO_GLYPH[_ch] = " "
for _ch in ".:-!":
    CHAR_TO_GLYPH[_ch] = _ch


# ---------------------------------------------------------------------------
# Outlines
# ---------------------------------------------------------------------------

def rects_for(pattern: list[str]) -> list[tuple[int, int, int]]:
    """Merge each row's runs of `#` into rectangles `(col0, col1, row)`."""
    rects: list[tuple[int, int, int]] = []
    for row, line in enumerate(pattern):
        col = 0
        while col < COLS:
            if line[col] == "#":
                start = col
                while col < COLS and line[col] == "#":
                    col += 1
                rects.append((start, col, row))
            else:
                col += 1
    return rects


def glyph_points(pattern: list[str]) -> list[list[tuple[int, int]]]:
    """One contour (four on-curve points, counter-clockwise) per rectangle."""
    contours: list[list[tuple[int, int]]] = []
    for col0, col1, row in rects_for(pattern):
        x0, x1 = col0 * PIX, col1 * PIX
        y_bottom = (ROWS - 1 - row) * PIX
        y_top = (ROWS - row) * PIX
        contours.append([(x0, y_bottom), (x1, y_bottom), (x1, y_top), (x0, y_top)])
    return contours


def encode_glyph(contours: list[list[tuple[int, int]]]) -> bytes:
    """Encode one simple glyph (name: `glyf` entry). Empty contours -> no outline."""
    if not contours:
        return b""
    xs = [p[0] for c in contours for p in c]
    ys = [p[1] for c in contours for p in c]
    out = bytearray()
    out += struct.pack(">hhhhh", len(contours), min(xs), min(ys), max(xs), max(ys))

    ends = []
    point_index = -1
    for contour in contours:
        point_index += len(contour)
        ends.append(point_index)
    out += struct.pack(">%dH" % len(ends), *ends)
    out += struct.pack(">H", 0)  # instructionLength

    points = [p for c in contours for p in c]
    out += bytes([0x01] * len(points))  # every point on-curve; 16-bit deltas

    previous = 0
    for x, _ in points:
        out += struct.pack(">h", x - previous)
        previous = x
    previous = 0
    for _, y in points:
        out += struct.pack(">h", y - previous)
        previous = y
    return bytes(out)


# ---------------------------------------------------------------------------
# Tables
# ---------------------------------------------------------------------------

def be(fmt: str, *values: int) -> bytes:
    return struct.pack(">" + fmt, *values)


def build_glyphs() -> tuple[bytes, list[int], int, int, tuple[int, int, int, int]]:
    """Build `glyf` + long-format `loca` offsets and the derived maxima."""
    order = [".notdef"] + list(GLYPHS.keys())
    data = bytearray()
    offsets = [0]
    max_points = 0
    max_contours = 0
    xmin = ymin = 1 << 30
    xmax = ymax = -(1 << 30)

    for name in order:
        if name == ".notdef":
            encoded = b""
        else:
            contours = glyph_points(GLYPHS[name])
            encoded = encode_glyph(contours)
            points = sum(len(c) for c in contours)
            max_points = max(max_points, points)
            max_contours = max(max_contours, len(contours))
            for contour in contours:
                for px, py in contour:
                    xmin = min(xmin, px)
                    ymin = min(ymin, py)
                    xmax = max(xmax, px)
                    ymax = max(ymax, py)
        data += encoded
        while len(data) % 4 != 0:  # keep every glyph 4-byte aligned
            data += b"\x00"
        offsets.append(len(data))

    if (xmin, ymin, xmax, ymax) == (1 << 30, 1 << 30, -(1 << 30), -(1 << 30)):
        xmin = ymin = xmax = ymax = 0
    return bytes(data), offsets, max_points, max_contours, (xmin, ymin, xmax, ymax)


def build_loca(offsets: list[int]) -> bytes:
    return b"".join(be("I", offset) for offset in offsets)


def build_hmtx(glyph_count: int, lsb: int) -> bytes:
    return b"".join(be("Hh", ADVANCE, lsb) for _ in range(glyph_count))


def build_head(bbox: tuple[int, int, int, int]) -> bytes:
    xmin, ymin, xmax, ymax = bbox
    return b"".join(
        [
            be("I", 0x00010000),   # version
            be("I", 0x00010000),   # fontRevision 1.0
            be("I", 0),            # checkSumAdjustment (patched later)
            be("I", 0x5F0F3CF5),   # magicNumber
            be("H", 0x0003),       # flags
            be("H", UPM),          # unitsPerEm
            be("q", 0),            # created
            be("q", 0),            # modified
            be("h", xmin),
            be("h", ymin),
            be("h", xmax),
            be("h", ymax),
            be("H", 0),            # macStyle
            be("H", 8),            # lowestRecPPEM
            be("h", 2),            # fontDirectionHint
            be("h", 1),            # indexToLocFormat (long)
            be("h", 0),            # glyphDataFormat
        ]
    )


def build_hhea(glyph_count: int, x_max_extent: int) -> bytes:
    return b"".join(
        [
            be("I", 0x00010000),
            be("h", ASCENT),
            be("h", DESCENT),
            be("h", LINE_GAP),
            be("H", ADVANCE),         # advanceWidthMax
            be("h", 0),               # minLeftSideBearing
            be("h", 0),               # minRightSideBearing
            be("h", x_max_extent),    # xMaxExtent
            be("h", 1),               # caretSlopeRise
            be("h", 0),               # caretSlopeRun
            be("h", 0),               # caretOffset
            be("h", 0),
            be("h", 0),
            be("h", 0),
            be("h", 0),               # 4 reserved
            be("h", 0),               # metricDataFormat
            be("H", glyph_count),     # numberOfHMetrics
        ]
    )


def build_maxp(glyph_count: int, max_points: int, max_contours: int) -> bytes:
    return b"".join(
        [
            be("I", 0x00010000),
            be("H", glyph_count),
            be("H", max_points),
            be("H", max_contours),
            be("H", 0),   # maxCompositePoints
            be("H", 0),   # maxCompositeContours
            be("H", 1),   # maxZones
            be("H", 0),   # maxTwilightPoints
            be("H", 0),   # maxStorage
            be("H", 0),   # maxFunctionDefs
            be("H", 0),   # maxInstructionDefs
            be("H", 0),   # maxStackElements
            be("H", 0),   # maxSizeOfInstructions
            be("H", 0),   # maxComponentElements
            be("H", 0),   # maxComponentDepth
        ]
    )


def build_cmap(char_to_gid: dict[int, int]) -> bytes:
    segments = sorted(char_to_gid.items())
    segs = [(ch, ch, (gid - ch) & 0xFFFF) for ch, gid in segments]
    segs.append((0xFFFF, 0xFFFF, 0x0001))
    seg_count = len(segs)

    body = b"".join(be("H", end) for _, end, _ in segs)
    body += be("H", 0)  # reservedPad
    body += b"".join(be("H", start) for start, _, _ in segs)
    body += b"".join(be("H", delta) for _, _, delta in segs)
    body += b"".join(be("H", 0) for _ in segs)  # idRangeOffset

    length = 14 + len(body)
    seg2 = seg_count * 2
    power = 2 ** int(math.floor(math.log2(seg_count)))
    search_range = power * 2
    entry_selector = int(math.log2(power))
    range_shift = seg2 - search_range

    subtable = be("HHHHHHH", 4, length, 0, seg2, search_range, entry_selector, range_shift)
    subtable += body

    table = be("HH", 0, 1)                  # version, numTables
    table += be("HHI", 3, 1, 12)            # Windows / Unicode BMP -> offset 12
    table += subtable
    return table


def build_name() -> bytes:
    records = [
        (0, "Copyright (c) the Noctis example project. CC0 / public domain."),
        (1, FAMILY),
        (2, STYLE),
        (4, f"{FAMILY} {STYLE}"),
        (5, f"Version {VERSION}"),
        (6, FAMILY.replace(" ", "") + "-" + STYLE),
    ]
    count = len(records)
    string_offset = 6 + 12 * count
    storage = bytearray()
    table = bytearray(be("HHH", 0, count, string_offset))
    for name_id, text in records:
        encoded = text.encode("utf-16-be")
        table += be("HHHHHH", 3, 1, 0x0409, name_id, len(encoded), string_offset + len(storage))
        storage += encoded
    return bytes(table) + bytes(storage)


def build_post() -> bytes:
    return b"".join(
        [
            be("I", 0x00030000),  # version 3.0: no glyph names
            be("i", 0),           # italicAngle
            be("h", -100),        # underlinePosition
            be("h", 50),          # underlineThickness
            be("I", 0),           # isFixedPitch
            be("I", 0),
            be("I", 0),
            be("I", 0),
            be("I", 0),
        ]
    )


def build_os2() -> bytes:
    return b"".join(
        [
            be("H", 4),            # version
            be("h", ADVANCE),      # xAvgCharWidth
            be("H", 400),          # usWeightClass
            be("H", 5),            # usWidthClass
            be("H", 0),            # fsType (installable embedding)
            be("h", 0),            # ySubscriptXSize
            be("h", 0),            # ySubscriptYSize
            be("h", 0),            # ySubscriptXOffset
            be("h", 0),            # ySubscriptYOffset
            be("h", 0),            # ySuperscriptXSize
            be("h", 0),            # ySuperscriptYSize
            be("h", 0),            # ySuperscriptXOffset
            be("h", 0),            # ySuperscriptYOffset
            be("h", 50),           # yStrikeoutSize
            be("h", 300),          # yStrikeoutPosition
            be("h", 0),            # sFamilyClass
            bytes(10),             # panose
            be("I", 0),            # ulUnicodeRange1 (basic latin)
            be("I", 1),            # ulUnicodeRange2
            be("I", 0),
            be("I", 0),
            b"NOCT",               # achVendID
            be("H", 0x0040),       # fsSelection: REGULAR
            be("H", 32),           # usFirstCharIndex
            be("H", 122),          # usLastCharIndex
            be("h", ASCENT),       # sTypoAscender
            be("h", DESCENT),      # sTypoDescender
            be("h", LINE_GAP),     # sTypoLineGap
            be("H", ASCENT),       # usWinAscent
            be("H", -DESCENT),     # usWinDescent
            be("I", 1),            # ulCodePageRange1 (Latin 1)
            be("I", 0),
            be("h", 0),            # sxHeight
            be("h", ASCENT),       # sCapHeight
            be("H", 0),            # usDefaultChar
            be("H", 32),           # usBreakChar
            be("H", 1),            # usMaxContext
            be("H", 0),            # usLowerOpticalPointSize
            be("H", 0xFFFF),       # usUpperOpticalPointSize
        ]
    )


# ---------------------------------------------------------------------------
# sfnt assembly
# ---------------------------------------------------------------------------

def checksum(data: bytes) -> int:
    padded = data + b"\x00" * ((4 - len(data) % 4) % 4)
    total = 0
    for i in range(0, len(padded), 4):
        total += struct.unpack(">I", padded[i : i + 4])[0]
    return total & 0xFFFFFFFF


def assemble(tables: dict[str, bytes]) -> bytes:
    tags = sorted(tables.keys())
    count = len(tags)
    power = 2 ** int(math.floor(math.log2(count)))
    search_range = power * 16
    entry_selector = int(math.log2(power))
    range_shift = count * 16 - search_range

    header = be("IHHHH", 0x00010000, count, search_range, entry_selector, range_shift)

    offset = 12 + 16 * count
    directory = bytearray()
    body = bytearray()
    head_offset = 0
    for tag in tags:
        data = tables[tag]
        if tag == "head":
            head_offset = offset
        directory += tag.encode("ascii")
        directory += be("III", checksum(data), offset, len(data))
        body += data
        pad = (4 - len(data) % 4) % 4
        body += b"\x00" * pad
        offset += len(data) + pad

    font = bytearray(header + directory + body)

    # Publish the checksum adjustment: the whole-font checksum must equal
    # 0xB1B0AFBA once `head.checkSumAdjustment` is added in.
    adjustment = (0xB1B0AFBA - checksum(bytes(font))) & 0xFFFFFFFF
    font[head_offset + 8 : head_offset + 12] = be("I", adjustment)
    return bytes(font)


def main() -> int:
    for name, pattern in GLYPHS.items():
        if len(pattern) != ROWS or any(len(line) != COLS for line in pattern):
            print(f"glyph {name!r} is not {COLS}x{ROWS}", file=sys.stderr)
            return 1

    glyf, offsets, max_points, max_contours, bbox = build_glyphs()
    glyph_count = len(offsets) - 1
    char_to_gid = {ord(ch): 1 + list(GLYPHS.keys()).index(name) for ch, name in CHAR_TO_GLYPH.items()}

    tables = {
        "OS/2": build_os2(),
        "cmap": build_cmap(char_to_gid),
        "glyf": glyf,
        "head": build_head(bbox),
        "hhea": build_hhea(glyph_count, bbox[2]),
        "hmtx": build_hmtx(glyph_count, 0),
        "loca": build_loca(offsets),
        "maxp": build_maxp(glyph_count, max_points, max_contours),
        "name": build_name(),
        "post": build_post(),
    }
    font = assemble(tables)

    here = os.path.dirname(os.path.abspath(__file__))
    output = os.path.join(here, os.pardir, "assets", "fonts", "NoctisSans.ttf")
    os.makedirs(os.path.dirname(output), exist_ok=True)
    with open(output, "wb") as handle:
        handle.write(font)
    print(f"wrote {os.path.normpath(output)} ({len(font)} bytes, {glyph_count} glyphs)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
