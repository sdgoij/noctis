#!/usr/bin/env python3
"""Generate the unlit palette materials the levels shade with.

The engine authors materials from glTF (it is the only importer that produces a
``Material`` asset), so we emit a tiny materials-only ``.gltf``: every material
is ``KHR_materials_unlit`` with a flat base color, the flat-shading path a 2D
game needs. Run it from the project root:

    python tools/build_assets.py
"""

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent


def linear(hex_color):
    """sRGB hex ('#rrggbb') to linear RGB (glTF baseColorFactor is linear)."""
    value = hex_color.lstrip("#")
    out = []
    for i in (0, 2, 4):
        c = int(value[i : i + 2], 16) / 255.0
        out.append(c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4)
    return out


# The player's yellow and the level's teal/green palette (from the original
# source palette in render.rs), plus a warm marker for the goal.
PALETTE = {
    "Ground": "#174B45",
    "GroundTop": "#2F8F5B",
    "Coin": "#A8D96F",
    "Goal": "#FF5C7A",
    "Player": "#F2FF66",
}

BACKDROP = "#071A1D"  # the camera's clear color


def main():
    materials = []
    for name, hex_color in PALETTE.items():
        r, g, b = linear(hex_color)
        materials.append(
            {
                "name": name,
                "pbrMetallicRoughness": {
                    "baseColorFactor": [r, g, b, 1.0],
                    "metallicFactor": 0.0,
                    "roughnessFactor": 1.0,
                },
                "extensions": {"KHR_materials_unlit": {}},
            }
        )

    document = {
        "asset": {"version": "2.0", "generator": "reverie tools/build_assets.py"},
        "extensionsUsed": ["KHR_materials_unlit"],
        "materials": materials,
    }

    out = ROOT / "assets" / "models" / "palette.gltf"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(document, indent=2) + "\n")
    print("wrote", out)


if __name__ == "__main__":
    main()
