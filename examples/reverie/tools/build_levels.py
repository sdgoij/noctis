#!/usr/bin/env python3
"""Generate the level scenes and their game-loop scripts.

Each `levels/levelN.txt` (the original game's ASCII format) becomes:

  * `scenes/levelN.scene.json` — the level as data: an orthographic camera, the
    ground as unlit quad runs, and the coin/goal/player/HUD entities; and
  * `scripts/levelN.js` — the whole game loop for that level (movement, tile
    collision, coins, goal, camera), with the level's grid and points baked in.

The scene holds the *visuals*; the script holds the *behaviour and the data it
needs to collide* (the tile grid). Run it from the project root:

    python tools/build_levels.py
"""

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
LEVELS = ["level1", "level2", "level3"]

TILE = 16.0  # world units per tile (1 unit = 1 original pixel)
ORTHO_HEIGHT = 270.0
QUAT_X_90 = [0.7071067811865476, 0.0, 0.0, 0.7071067811865476]  # +90 deg about X


def linear(hex_color):
    value = hex_color.lstrip("#")
    out = []
    for i in (0, 2, 4):
        c = int(value[i : i + 2], 16) / 255.0
        out.append(c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4)
    return out


BACKDROP = linear("#071A1D")


def parse_level(text):
    """Parse the original level format into grid, coins, spawn, and goal."""
    lines = text.splitlines()
    meta = {}
    i = 0
    while i < len(lines) and lines[i].strip():
        key, _, value = lines[i].partition(":")
        meta[key.strip()] = value.strip()
        i += 1
    i += 1  # skip the blank separator

    raw = [line for line in lines[i:] if line.strip() != "" or line != ""]
    width = max((len(line) for line in raw), default=0)
    rows = [line.ljust(width, ".") for line in raw]

    grid = []
    coins = []
    spawn = None
    goal = None
    for row, line in enumerate(rows):
        cells = []
        for col, ch in enumerate(line):
            if ch == "C":
                coins.append([col, row])
                cells.append("0")
            elif ch == "S":
                spawn = [col, row]
                cells.append("0")
            elif ch == "F":
                goal = [col, row]
                cells.append("0")
            elif ch in ("P", "G"):
                cells.append("1")
            else:
                cells.append("0")
        grid.append("".join(cells))

    return {
        "name": meta.get("name", ""),
        "music": meta.get("music", ""),
        "grid": grid,
        "coins": coins,
        "spawn": spawn,
        "goal": goal,
        "width": width,
        "height": len(rows),
    }


def tile_center(col, row, level_height):
    """World-space centre of a level cell (world y grows up, tiles grow down)."""
    x = col * TILE + TILE * 0.5
    y = level_height - (row * TILE + TILE * 0.5)
    return [x, y]


def ground_runs(grid):
    """Split each row's solid tiles into runs uniform in 'is this the surface'."""
    height = len(grid)
    width = len(grid[0]) if grid else 0
    runs = []
    for row in range(height):
        col = 0
        while col < width:
            if grid[row][col] != "1":
                col += 1
                continue
            top = row == 0 or grid[row - 1][col] != "1"
            start = col
            col += 1
            while col < width and grid[row][col] == "1":
                is_top = row == 0 or grid[row - 1][col] != "1"
                if is_top != top:
                    break
                col += 1
            runs.append((row, start, col - 1, top))
    return runs


def entity(eid, name, translation, scale, components, parent=0):
    return {
        "id": eid,
        "name": name,
        "parent": parent,
        "active": True,
        "visible": True,
        "transform": {
            "translation": translation,
            "rotation": QUAT_X_90,
            "scale": scale,
        },
        "components": components,
    }


def root_entity():
    return {
        "id": 0,
        "name": "Root",
        "parent": None,
        "active": True,
        "visible": True,
        "transform": {
            "translation": [0.0, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0],
        },
        "components": [],
    }


def mesh(material):
    return {
        "type": "mesh_renderer",
        "mesh": "primitives/plane",
        "material": material,
        "cast_shadows": False,
        "receive_shadows": False,
    }


def build_scene(level, index):
    level_width = level["width"] * TILE
    level_height = level["height"] * TILE
    spawn = level["spawn"]

    entities = [root_entity()]
    eid = 1

    # Camera.
    camera_y = tile_center(*spawn, level_height)[1]
    entities.append(
        entity(
            eid,
            "Camera",
            [min(level_width * 0.5, spawn[0] * TILE + TILE), camera_y, 10.0],
            [1.0, 1.0, 1.0],
            [
                {
                    "type": "camera",
                    "projection": {
                        "kind": "orthographic",
                        "height": ORTHO_HEIGHT,
                        "near": 0.1,
                        "far": 100.0,
                    },
                    "is_active": True,
                    "priority": 0,
                    "clear_color": [BACKDROP[0], BACKDROP[1], BACKDROP[2], 1.0],
                    "hdr": False,
                }
            ],
        )
    )
    eid += 1

    # Ground: one quad per uniform run.
    for row, start, end, top in ground_runs(level["grid"]):
        cells = end - start + 1
        cx = (start + end + 1) * 0.5 * TILE
        cy = level_height - (row * TILE + TILE * 0.5)
        entities.append(
            entity(
                eid,
                "Ground",
                [cx, cy, 0.0],
                [cells * TILE, 1.0, TILE],
                [mesh("GroundTop" if top else "Ground")],
            )
        )
        eid += 1

    # Coins (destroyed by name when collected).
    for col, row in level["coins"]:
        x, y = tile_center(col, row, level_height)
        entities.append(
            entity(eid, f"Coin_{col}_{row}", [x, y, -1.0], [12.0, 1.0, 12.0], [mesh("Coin")])
        )
        eid += 1

    # Goal.
    gx, gy = tile_center(level["goal"][0], level["goal"][1], level_height)
    entities.append(entity(eid, "Goal", [gx, gy, -1.0], [TILE, 1.0, TILE], [mesh("Goal")]))
    eid += 1

    # Player (moved by the script).
    px = spawn[0] * TILE + 6.0
    py = level_height - (spawn[1] * TILE + 7.5)
    entities.append(entity(eid, "Player", [px, py, 1.0], [12.0, 1.0, 15.0], [mesh("Player")]))
    eid += 1

    # HUD text (repositioned and rewritten by the script).
    entities.append(
        entity(
            eid,
            "HUD",
            [0.0, 0.0, 5.0],
            [1.0, 1.0, 1.0],
            [
                {
                    "type": "text",
                    "text": "COINS 0",
                    "font": "assets/fonts/NoctisSans.ttf",
                    "size": 12.0,
                    "color": [1.0, 1.0, 1.0, 1.0],
                    "anchor": "bottom_left",
                    "depth_test": False,
                }
            ],
        )
    )
    eid += 1

    # The game loop.
    entities.append(
        entity(
            eid,
            "Game",
            [0.0, 0.0, 0.0],
            [1.0, 1.0, 1.0],
            [
                {
                    "type": "script",
                    "source": None,
                    "path": f"scripts/{LEVELS[index]}.js",
                    "enabled": True,
                }
            ],
        )
    )

    return {
        "name": f"Level {index + 1}",
        "format_version": 1,
        "mode": "2d",
        "entities": entities,
    }


JS_TEMPLATE = r"""// Auto-generated by tools/build_levels.py. Do not edit by hand.
//
// This is the whole game loop for one level of Reverie Amidst Ruins, ported
// from the original Rust (player.rs / camera.rs / main.rs) into one script:
// movement, tile collision, coins, the goal, and the camera. The data below is
// in level tiles with y growing downward, exactly like levels/*.txt; the script
// flips to the engine's y-up world when it places the player.

const TILE = 16.0;
const PLAYER_W = 12.0;
const PLAYER_H = 15.0;
const SPEED = 120.0;
const GROUND_ACCEL = 1200.0;
const AIR_ACCEL = 650.0;
const GRAVITY = 800.0;
const JUMP_SPEED = 300.0;
const JUMP_RELEASE = 100.0;
const MAX_FALL = 450.0;
const DEATH_PLANE = 70.0;
const ORTHO_HEIGHT = 270.0;
const FOLLOW_SPEED = 10.0;

const GRID = @@GRID@@;
const GRID_W = @@WIDTH@@;
const GRID_H = @@HEIGHT@@;
const SPAWN = @@SPAWN@@;
const COINS = @@COINS@@;
const GOAL = @@GOAL@@;
const NEXT = @@NEXT@@;
const CURRENT = @@CURRENT@@;

let player;
let hud;
let camera;
let px = 0.0;
let py = 0.0;
let vx = 0.0;
let vy = 0.0;
let grounded = false;
let collected = 0;
let coinsLeft = [];
let wasJump = false;
let ready = false;

function on_start(entity, dt) {
  player = find_entity("Player");
  hud = find_entity("HUD");
  camera = find_entity("Camera");
  coinsLeft = COINS.map(function () { return true; });
  collected = 0;
  respawn();
  updateHud();
  followCamera(0.0, true);
  ready = true;
}

function on_update(entity, dt) {
  if (!ready) return;

  if (key_down("KeyR")) { load_scene(CURRENT); return; }

  const left = key_down("KeyA") || key_down("ArrowLeft");
  const right = key_down("KeyD") || key_down("ArrowRight");
  const jump = key_down("Space") || key_down("ArrowUp") || key_down("KeyW");
  const dir = (right ? 1.0 : 0.0) - (left ? 1.0 : 0.0);

  const target = dir * SPEED;
  const accel = grounded ? GROUND_ACCEL : AIR_ACCEL;
  vx = approach(vx, target, accel * dt);
  moveX(vx * dt);

  if (jump && !wasJump && grounded) { vy = -JUMP_SPEED; grounded = false; }
  if (!jump && vy < -JUMP_RELEASE) { vy = -JUMP_RELEASE; }
  wasJump = jump;

  vy = Math.min(vy + GRAVITY * dt, MAX_FALL);
  grounded = false;
  moveY(vy * dt);

  place();

  if (py > GRID_H * TILE + DEATH_PLANE) { respawn(); }

  for (let i = 0; i < COINS.length; i++) {
    if (!coinsLeft[i]) continue;
    const c = COINS[i];
    if (overlaps(c[0] * TILE, c[1] * TILE, TILE, TILE)) {
      coinsLeft[i] = false;
      collected += 1;
      destroy_by_name("Coin_" + c[0] + "_" + c[1]);
      updateHud();
    }
  }

  if (overlaps(GOAL[0] * TILE, GOAL[1] * TILE, TILE, TILE)) {
    load_scene(NEXT !== null ? NEXT : CURRENT);
    return;
  }

  followCamera(dt, false);
  placeHud();
}

function respawn() {
  px = SPAWN[0] * TILE;
  py = SPAWN[1] * TILE;
  vx = 0.0;
  vy = 0.0;
  grounded = false;
  place();
}

// The player's world position is the centre of its hitbox.
function place() {
  if (player === undefined) return;
  set_position(player, px + PLAYER_W * 0.5, GRID_H * TILE - (py + PLAYER_H * 0.5), 1.0);
}

function overlaps(x, y, w, h) {
  return px < x + w && px + PLAYER_W > x && py < y + h && py + PLAYER_H > y;
}

function approach(current, target, amount) {
  if (current < target) return Math.min(current + amount, target);
  return Math.max(current - amount, target);
}

function clamp(v, lo, hi) {
  if (v < lo) return lo;
  if (v > hi) return hi;
  return v;
}

function solid(col, row) {
  if (row < 0 || row >= GRID_H || col < 0 || col >= GRID_W) return false;
  return GRID[row].charAt(col) === "1";
}

function hitsSolid(x, y) {
  const left = Math.floor(x / TILE);
  const right = Math.floor((x + PLAYER_W - 0.01) / TILE);
  const top = Math.floor(y / TILE);
  const bottom = Math.floor((y + PLAYER_H - 0.01) / TILE);
  for (let row = top; row <= bottom; row += 1) {
    for (let col = left; col <= right; col += 1) {
      if (solid(col, row)) return true;
    }
  }
  return false;
}

function moveX(amount) {
  px += amount;
  if (hitsSolid(px, py)) {
    if (amount > 0.0) px = Math.floor((px + PLAYER_W) / TILE) * TILE - PLAYER_W - 0.01;
    else if (amount < 0.0) px = Math.floor(px / TILE) * TILE + TILE + 0.01;
    vx = 0.0;
  }
  const maxX = GRID_W * TILE - PLAYER_W;
  if (px < 0.0) { px = 0.0; vx = 0.0; }
  if (px > maxX) { px = maxX; vx = 0.0; }
}

function moveY(amount) {
  py += amount;
  if (hitsSolid(px, py)) {
    if (amount > 0.0) {
      py = Math.floor((py + PLAYER_H) / TILE) * TILE - PLAYER_H - 0.01;
      grounded = true;
    } else if (amount < 0.0) {
      py = Math.floor(py / TILE) * TILE + TILE + 0.01;
    }
    vy = 0.0;
  }
}

function viewSize() {
  let w = 0.0;
  let h = 0.0;
  const vp = viewport_size();
  if (vp !== undefined) { w = vp[0]; h = vp[1]; }
  const aspect = (w > 0.0 && h > 0.0) ? (w / h) : (4.0 / 3.0);
  return [ORTHO_HEIGHT * aspect, ORTHO_HEIGHT];
}

function followCamera(dt, snap) {
  if (camera === undefined) return;
  const v = viewSize();
  const cxp = px + PLAYER_W * 0.5;
  const cyp = GRID_H * TILE - (py + PLAYER_H * 0.5);
  const tx = clamp(cxp + 0.10 * v[0], v[0] * 0.5, Math.max(v[0] * 0.5, GRID_W * TILE - v[0] * 0.5));
  const ty = clamp(cyp, v[1] * 0.5, Math.max(v[1] * 0.5, GRID_H * TILE - v[1] * 0.5));
  const cp = get_position(camera);
  if (snap || cp === undefined) { set_position(camera, tx, ty, 10.0); return; }
  const k = 1.0 - Math.exp(-FOLLOW_SPEED * dt);
  set_position(camera, cp[0] + (tx - cp[0]) * k, cp[1] + (ty - cp[1]) * k, 10.0);
}

function placeHud() {
  if (hud === undefined || camera === undefined) return;
  const cp = get_position(camera);
  if (cp === undefined) return;
  const v = viewSize();
  set_position(hud, cp[0] - v[0] * 0.5 + 6.0, cp[1] + v[1] * 0.5 - 18.0, 5.0);
}

function updateHud() {
  if (hud === undefined) return;
  // The component name is its canonical type name; the JSON's "type" tag is
  // snake_case.
  set_component(hud, "Text", JSON.stringify({
    type: "text",
    text: "COINS " + collected + " / " + COINS.length,
    font: "assets/fonts/NoctisSans.ttf",
    size: 12.0,
    color: [1.0, 1.0, 1.0, 1.0],
    anchor: "bottom_left",
    depth_test: false
  }));
}
"""


def build_script(level, index):
    script = JS_TEMPLATE
    script = script.replace("@@GRID@@", json.dumps(level["grid"]))
    script = script.replace("@@WIDTH@@", str(level["width"]))
    script = script.replace("@@HEIGHT@@", str(level["height"]))
    script = script.replace("@@SPAWN@@", json.dumps(level["spawn"]))
    script = script.replace("@@COINS@@", json.dumps(level["coins"]))
    script = script.replace("@@GOAL@@", json.dumps(level["goal"]))
    if index + 1 < len(LEVELS):
        script = script.replace("@@NEXT@@", json.dumps(f"scenes/{LEVELS[index + 1]}.scene.json"))
    else:
        script = script.replace("@@NEXT@@", "null")
    script = script.replace("@@CURRENT@@", json.dumps(f"scenes/{LEVELS[index]}.scene.json"))
    return script


def main():
    for index, name in enumerate(LEVELS):
        text = (ROOT / "levels" / f"{name}.txt").read_text()
        level = parse_level(text)

        scene = build_scene(level, index)
        scene_path = ROOT / "scenes" / f"{name}.scene.json"
        scene_path.write_text(json.dumps(scene, indent=2) + "\n")

        script_path = ROOT / "scripts" / f"{name}.js"
        script_path.parent.mkdir(parents=True, exist_ok=True)
        script_path.write_text(build_script(level, index))

        print(f"wrote {scene_path.name} ({len(scene['entities'])} entities) and {script_path.name}")


if __name__ == "__main__":
    main()
