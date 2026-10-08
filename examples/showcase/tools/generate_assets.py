#!/usr/bin/env python3
"""Generate the showcase's binary assets (model, textures, audio).

Everything the example project ships is produced here, from the Python standard
library only, so the project is reproducible from a clean checkout with no
third-party art:

* `assets/models/drone.glb` — a binary glTF containing a cube mesh, a
  metallic/roughness material, and two embedded PNG textures (a base-colour map
  and a metallic/roughness map). Embedding the textures is deliberate: the
  engine imports standalone `.png` files as textures, but only a glTF carries a
  *material*, and a scene references materials — not bare textures — so the
  material has to travel inside the model.
* `assets/textures/panel_normal.png` (+ `panel_normal.png.import.toml`) — a
  loose normal map, marked linear with `srgb = false`. It is not referenced by a
  scene; it is the kind of art file you assign in the editor's material editor,
  and it exercises the texture importer, the sidecar settings, and the
  `.noctis/` decode cache.
* `assets/audio/chime.wav` — a short bell arpeggio (a non-spatial, one-shot
  clip).
* `assets/audio/engine_hum.wav` — a seamless 2-second drone (a looping,
  spatial clip).

Run it from anywhere; it writes into the project:

    python tools/generate_assets.py

The output is deterministic — running it twice produces byte-identical files.
"""

from __future__ import annotations

import math
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, os.pardir))
ASSETS = os.path.join(ROOT, "assets")

SAMPLE_RATE = 44100


# ---------------------------------------------------------------------------
# PNG
# ---------------------------------------------------------------------------

def png_rgba(width: int, height: int, pixels: bytes) -> bytes:
    """Encode tightly packed RGBA8 `pixels` as a PNG (deterministic, no metadata)."""

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    raw = bytearray()
    stride = width * 4
    for y in range(height):
        raw += b"\x00"  # filter: none
        raw += pixels[y * stride : (y + 1) * stride]

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def base_color_pixels(width: int, height: int) -> bytes:
    """A two-tone blue metal checker with a bright panel seam every 32 pixels."""
    out = bytearray()
    for y in range(height):
        for x in range(width):
            on = ((x // 32) + (y // 32)) % 2 == 0
            r, g, b = (54, 70, 94) if on else (150, 182, 220)
            if x % 32 == 0 or y % 32 == 0:
                r, g, b = (238, 240, 245)
            out += bytes((r, g, b, 255))
    return bytes(out)


def metal_rough_pixels(width: int, height: int) -> bytes:
    """A metallic/roughness map: G = roughness, B = metallic."""
    out = bytearray()
    for y in range(height):
        for x in range(width):
            on = ((x // 32) + (y // 32)) % 2 == 0
            roughness = 90 if on else 170
            out += bytes((255, roughness, 235, 255))
    return bytes(out)


def normal_pixels(width: int, height: int) -> bytes:
    """A shallow sinusoidal normal map (a clearly *linear* data texture)."""
    out = bytearray()
    for y in range(height):
        ny = int(128 + 20 * math.sin(y / 9.0))
        for x in range(width):
            nx = int(128 + 20 * math.sin(x / 9.0))
            out += bytes((nx, ny, 255, 255))
    return bytes(out)


# ---------------------------------------------------------------------------
# glTF (binary GLB)
# ---------------------------------------------------------------------------

def cube_geometry() -> tuple[list[float], list[float], list[float], list[int]]:
    """A unit cube with per-face normals: 24 vertices, 36 indices."""
    faces = [
        ((0.0, 0.0, 1.0), [(-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1)]),
        ((0.0, 0.0, -1.0), [(1, -1, -1), (-1, -1, -1), (-1, 1, -1), (1, 1, -1)]),
        ((1.0, 0.0, 0.0), [(1, -1, 1), (1, -1, -1), (1, 1, -1), (1, 1, 1)]),
        ((-1.0, 0.0, 0.0), [(-1, -1, -1), (-1, -1, 1), (-1, 1, 1), (-1, 1, -1)]),
        ((0.0, 1.0, 0.0), [(-1, 1, 1), (1, 1, 1), (1, 1, -1), (-1, 1, -1)]),
        ((0.0, -1.0, 0.0), [(-1, -1, -1), (1, -1, -1), (1, -1, 1), (-1, -1, 1)]),
    ]
    uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]

    positions: list[float] = []
    normals: list[float] = []
    texcoords: list[float] = []
    indices: list[int] = []
    for normal, corners in faces:
        base = len(positions) // 3
        for (x, y, z), (u, v) in zip(corners, uvs):
            positions += [float(x), float(y), float(z)]
            normals += list(normal)
            texcoords += [u, v]
        indices += [base, base + 1, base + 2, base, base + 2, base + 3]
    return positions, normals, texcoords, indices


def make_glb(json_text: str, binary: bytes) -> bytes:
    json_bytes = json_text.encode("utf-8")
    while len(json_bytes) % 4 != 0:
        json_bytes += b" "
    bin_bytes = bytearray(binary)
    while len(bin_bytes) % 4 != 0:
        bin_bytes += b"\x00"
    total = 12 + 8 + len(json_bytes) + 8 + len(bin_bytes)
    out = bytearray()
    out += b"glTF"
    out += struct.pack("<I", 2)
    out += struct.pack("<I", total)
    out += struct.pack("<I", len(json_bytes))
    out += struct.pack("<I", 0x4E4F534A)  # "JSON"
    out += json_bytes
    out += struct.pack("<I", len(bin_bytes))
    out += struct.pack("<I", 0x004E4942)  # "BIN\0"
    out += bin_bytes
    return bytes(out)


# A palette of named PBR materials. A scene references *materials*, and the only
# importer that produces one today is the glTF loader, so the project ships its
# materials inside a small model. The dark entries ("Ink", "Slate") keep the
# scenes readable against the engine's neutral (white) environment fallback, and
# "Glow" is emissive.
PALETTE: list[tuple[str, list[float], float, float, list[float]]] = [
    # name     base colour RGBA              metallic  roughness  emissive RGB
    ("Slate",  [0.20, 0.23, 0.29, 1.0], 0.10, 0.75, [0.0, 0.0, 0.0]),
    ("Steel",  [0.62, 0.65, 0.70, 1.0], 1.00, 0.30, [0.0, 0.0, 0.0]),
    ("Copper", [0.85, 0.45, 0.25, 1.0], 1.00, 0.35, [0.0, 0.0, 0.0]),
    ("Gold",   [0.95, 0.76, 0.30, 1.0], 1.00, 0.25, [0.0, 0.0, 0.0]),
    ("Mint",   [0.30, 0.72, 0.50, 1.0], 0.00, 0.55, [0.0, 0.0, 0.0]),
    ("Coral",  [0.88, 0.35, 0.32, 1.0], 0.00, 0.50, [0.0, 0.0, 0.0]),
    ("Glow",   [0.10, 0.35, 0.40, 1.0], 0.00, 0.40, [0.20, 0.85, 0.95]),
    ("Ink",    [0.05, 0.06, 0.09, 1.0], 0.00, 0.85, [0.0, 0.0, 0.0]),
]


def triangle_geometry() -> tuple[list[float], list[float], list[float], list[int]]:
    """A single triangle: 3 vertices, 3 indices (a placeholder mesh)."""
    positions = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
    normals = [0.0, 0.0, 1.0] * 3
    texcoords = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0]
    indices = [0, 1, 2]
    return positions, normals, texcoords, indices


def build_palette_glb() -> bytes:
    """A materials-only model: the project's named PBR palette."""
    positions, normals, texcoords, indices = triangle_geometry()

    binary = bytearray()
    for value in positions:
        binary += struct.pack("<f", value)
    normal_offset = len(binary)
    for value in normals:
        binary += struct.pack("<f", value)
    uv_offset = len(binary)
    for value in texcoords:
        binary += struct.pack("<f", value)
    index_offset = len(binary)
    for index in indices:
        binary += struct.pack("<H", index)
    while len(binary) % 4 != 0:
        binary += b"\x00"

    materials = ",".join(
        '{{ "name": "{name}", "pbrMetallicRoughness": {{ '
        '"baseColorFactor": [{c0}, {c1}, {c2}, {c3}], '
        '"metallicFactor": {metallic}, "roughnessFactor": {roughness} }}'
        '{emissive} }}'.format(
            name=name,
            c0=color[0],
            c1=color[1],
            c2=color[2],
            c3=color[3],
            metallic=metallic,
            roughness=roughness,
            emissive=(
                ', "emissiveFactor": [{e0}, {e1}, {e2}]'.format(
                    e0=emissive[0], e1=emissive[1], e2=emissive[2]
                )
                if any(emissive)
                else ""
            ),
        )
        for name, color, metallic, roughness, emissive in PALETTE
    )

    json_text = f"""{{
  "asset": {{ "version": "2.0", "generator": "noctis showcase generator" }},
  "scene": 0,
  "scenes": [ {{ "name": "PaletteScene", "nodes": [0] }} ],
  "nodes": [ {{ "name": "PaletteMarker", "mesh": 0 }} ],
  "meshes": [ {{ "name": "PaletteMarker", "primitives": [
     {{ "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }}, "indices": 3, "material": 0 }}
  ] }} ],
  "materials": [ {materials} ],
  "buffers": [ {{ "byteLength": {len(binary)} }} ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {normal_offset}, "byteLength": 36 }},
    {{ "buffer": 0, "byteOffset": {uv_offset}, "byteLength": 24 }},
    {{ "buffer": 0, "byteOffset": {index_offset}, "byteLength": 6 }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5123, "count": 3, "type": "SCALAR" }}
  ]
}}"""
    return make_glb(json_text, bytes(binary))


def build_drone_glb() -> bytes:
    positions, normals, texcoords, indices = cube_geometry()

    binary = bytearray()
    for value in positions:
        binary += struct.pack("<f", value)

    normal_offset = len(binary)
    for value in normals:
        binary += struct.pack("<f", value)

    uv_offset = len(binary)
    for value in texcoords:
        binary += struct.pack("<f", value)

    index_offset = len(binary)
    for index in indices:
        binary += struct.pack("<H", index)

    base_png = png_rgba(128, 128, base_color_pixels(128, 128))
    mr_png = png_rgba(128, 128, metal_rough_pixels(128, 128))

    base_offset = len(binary)
    binary += base_png
    while len(binary) % 4 != 0:
        binary += b"\x00"
    mr_offset = len(binary)
    binary += mr_png
    while len(binary) % 4 != 0:
        binary += b"\x00"

    json_text = f"""{{
  "asset": {{ "version": "2.0", "generator": "noctis showcase generator" }},
  "scene": 0,
  "scenes": [ {{ "name": "DroneScene", "nodes": [0] }} ],
  "nodes": [ {{ "name": "Drone", "mesh": 0 }} ],
  "meshes": [ {{ "name": "DroneBox", "primitives": [
     {{ "attributes": {{ "POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2 }}, "indices": 3, "material": 0 }}
  ] }} ],
  "materials": [ {{ "name": "DroneBody", "pbrMetallicRoughness": {{
     "baseColorTexture": {{ "index": 0 }},
     "metallicRoughnessTexture": {{ "index": 1 }},
     "baseColorFactor": [1.0, 1.0, 1.0, 1.0],
     "metallicFactor": 1.0,
     "roughnessFactor": 1.0
  }} }} ],
  "textures": [
    {{ "source": 0, "sampler": 0 }},
    {{ "source": 1, "sampler": 0 }}
  ],
  "images": [
    {{ "bufferView": 4, "mimeType": "image/png", "name": "DroneBaseColor" }},
    {{ "bufferView": 5, "mimeType": "image/png", "name": "DroneMetalRough" }}
  ],
  "samplers": [ {{ "magFilter": 9729, "minFilter": 9987, "wrapS": 10497, "wrapT": 10497 }} ],
  "buffers": [ {{ "byteLength": {len(binary)} }} ],
  "bufferViews": [
    {{ "buffer": 0, "byteOffset": 0, "byteLength": 288 }},
    {{ "buffer": 0, "byteOffset": {normal_offset}, "byteLength": 288 }},
    {{ "buffer": 0, "byteOffset": {uv_offset}, "byteLength": 192 }},
    {{ "buffer": 0, "byteOffset": {index_offset}, "byteLength": 72 }},
    {{ "buffer": 0, "byteOffset": {base_offset}, "byteLength": {len(base_png)} }},
    {{ "buffer": 0, "byteOffset": {mr_offset}, "byteLength": {len(mr_png)} }}
  ],
  "accessors": [
    {{ "bufferView": 0, "componentType": 5126, "count": 24, "type": "VEC3", "min": [-1, -1, -1], "max": [1, 1, 1] }},
    {{ "bufferView": 1, "componentType": 5126, "count": 24, "type": "VEC3" }},
    {{ "bufferView": 2, "componentType": 5126, "count": 24, "type": "VEC2" }},
    {{ "bufferView": 3, "componentType": 5123, "count": 36, "type": "SCALAR" }}
  ]
}}"""
    return make_glb(json_text, bytes(binary))


# ---------------------------------------------------------------------------
# WAV
# ---------------------------------------------------------------------------

def chime_samples() -> list[float]:
    """A bell arpeggio: C5, E5, G5, C6 — each a decaying harmonic stack."""
    notes = [523.25, 659.25, 783.99, 1046.50]
    note_seconds = 0.22
    note_len = int(SAMPLE_RATE * note_seconds)
    out: list[float] = []
    for frequency in notes:
        for i in range(note_len):
            t = i / SAMPLE_RATE
            envelope = math.exp(-6.0 * t) * min(1.0, t / 0.004)
            value = (
                math.sin(2 * math.pi * frequency * t)
                + 0.5 * math.sin(2 * math.pi * 2 * frequency * t)
                + 0.25 * math.sin(2 * math.pi * 3 * frequency * t)
            )
            out.append(0.22 * envelope * value)
    return out


def hum_samples() -> list[float]:
    """A seamless 2-second drone (every component completes whole cycles)."""
    seconds = 2.0
    total = int(SAMPLE_RATE * seconds)
    out: list[float] = []
    for i in range(total):
        t = i / SAMPLE_RATE
        tremolo = 0.75 + 0.25 * math.sin(2 * math.pi * 3.0 * t)
        value = (
            math.sin(2 * math.pi * 55.0 * t)
            + 0.4 * math.sin(2 * math.pi * 110.0 * t)
            + 0.2 * math.sin(2 * math.pi * 165.0 * t)
        )
        out.append(0.3 * tremolo * value)
    return out


# ---------------------------------------------------------------------------

def write_bytes(relative: str, data: bytes) -> None:
    path = os.path.join(ROOT, relative)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(data)
    print(f"wrote {relative} ({len(data)} bytes)")


def main() -> int:
    write_bytes("assets/models/drone.glb", build_drone_glb())
    write_bytes("assets/models/palette.glb", build_palette_glb())
    write_bytes(
        "assets/textures/panel_normal.png",
        png_rgba(128, 128, normal_pixels(128, 128)),
    )
    # The sidecar import settings are part of the asset tree too: a normal map is
    # a data texture, so it must be decoded linearly rather than as sRGB colour.
    write_bytes(
        "assets/textures/panel_normal.png.import.toml",
        b"# panel_normal.png is a normal map: a data texture, decoded linearly.\nsrgb = false\n",
    )
    write_bytes("assets/audio/chime.wav", _wav_bytes(chime_samples()))
    write_bytes("assets/audio/engine_hum.wav", _wav_bytes(hum_samples()))
    return 0


def _wav_bytes(samples: list[float]) -> bytes:
    """The WAV bytes for mono `samples`, without touching the filesystem."""
    frames = bytearray()
    for sample in samples:
        value = max(-32768, min(32767, int(round(sample * 32767.0))))
        frames += struct.pack("<h", value)
    data = bytes(frames)
    out = bytearray()
    out += b"RIFF"
    out += struct.pack("<I", 36 + len(data))
    out += b"WAVE"
    out += b"fmt "
    out += struct.pack("<IHHIIHH", 16, 1, 1, SAMPLE_RATE, SAMPLE_RATE * 2, 2, 16)
    out += b"data"
    out += struct.pack("<I", len(data))
    out += data
    return bytes(out)


if __name__ == "__main__":
    raise SystemExit(main())
