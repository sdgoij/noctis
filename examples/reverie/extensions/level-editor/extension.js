// Reverie Level Editor — a Noctis editor extension for `examples/reverie`.
//
// It draws the level grid in the central viewport and lets you paint it with the
// mouse (left = paint, right = erase, wheel = zoom, Shift+left = pan). It
// validates the level as you edit, and says *why* a level is not playable:
//
//   * the **structural** checks mirror `src/game/level.rs` exactly — the metadata
//     the parser indexes by name, the four-colour `#rrggbb` palette, exactly one
//     spawn and one goal, and only tile characters the parser knows;
//   * the **playability** check re-runs the game's own movement integrator
//     (`src/game/player.rs`: the same constants, the same collision resolution
//     and the same `1/60` step) as a search over jump arcs from the spawn, and
//     reports whether the goal — and each coin — is reachable.
//
// Invalid cells are outlined in red; the coins the sweep could not reach are
// crossed out; when the goal cannot be reached the viewport banners it.
//
// The sweep is an *approximation*: it takes off from three points in a standing
// cell and over a small set of jump holds and running speeds, so it can miss a
// route that needs frame-perfect timing (a long, hand-made maze can read as
// "no route found" when a player can still get through). Everything it *does*
// accept is a trajectory the real game would produce, so a "playable" verdict and
// every "unreachable coin" marker are trustworthy in that direction.
//
// It is also not cheap: the editor's JavaScript engine runs the game's integrator
// far slower than the game itself does, and a sweep of a 72x17 level simulates a
// few thousand arcs (~200k frames). So the search is *incremental* — it is
// advanced a few arcs per frame from the `"update"` hook (see `SEARCH.slice`),
// which keeps the editor responsive while the verdict fills in — and it stops as
// soon as the goal and every coin have been reached, which is the usual case.
//
// The level files are project files the manifest declares by slot, read and
// written through `noctis.files` — JavaScript never sees a path. The game embeds
// `levels/*.txt` with `include_str!`, so a save is picked up by the next build.

// ---------------------------------------------------------------------------
// The game's tile alphabet and physics (mirrors src/game/*.rs)
// ---------------------------------------------------------------------------

var TILE = 16;
var HITBOX_W = 12;
var HITBOX_H = 15;
var SPEED = 120;
var GROUND_ACCEL = 1200;
var AIR_ACCEL = 650;
var GRAVITY = 800;
var JUMP_SPEED = 300;
var JUMP_RELEASE = 100;
var MAX_FALL = 450;
var JUMP_BUFFER = 0.1;
var DEATH_PLANE = 70;
var STEP = 1 / 60;

// The game's tile alphabet and metadata (mirrors src/game/level.rs)
var SOLID_CHARS = { G: true, P: true };
var KNOWN_CHARS = { G: true, P: true, C: true, S: true, F: true, ".": true, " ": true };

// The metadata the game's parser indexes by name; a missing key panics it.
var REQUIRED_META = ["name", "music", "background", "palette"];
var DEFAULT_META = [
  ["name", "New Level"],
  ["music", "echoes-of-solace"],
  ["background", "snow"],
  ["palette", "#071A1D, #174B45, #2F8F5B, #A8D96F"]
];

// What a level's `music:` and `background:` may name (mirrors the game: the `SONGS`
// table in `src/game/audio.rs` and the backgrounds `src/game/render.rs` matches
// on). The game *refuses to load* a level naming anything else — `play_music`
// and the renderer both `bail!` — so the editor validates against these lists and
// offers exactly them.
var MUSIC_NAMES = [
  "reverie-amidst-ruins",
  "echoes-of-solace",
  "colors",
  "the-final-storm"
];
var BACKGROUND_NAMES = ["snow", "nebula", "storm"];

var BRUSHES = [
  { key: "G", label: "Ground" },
  { key: "P", label: "Platform" },
  { key: "C", label: "Coin" },
  { key: "S", label: "Spawn" },
  { key: "F", label: "Goal" },
  { key: ".", label: "Erase" }
];

var COLORS = {
  backdrop: "#0a0e13f4",
  field: "#0d171aff",
  edge: "#2f8f5bff",
  ground: "#2f8f5bff",
  platform: "#174b45ff",
  grid: "#ffffff18",
  coin: "#ffd75fff",
  spawn: "#66ff99ff",
  goal: "#ff6faeff",
  reach: "#7fd7ff2e",
  unreachable: "#ff5a5aff",
  route: "#ffffffb0",
  text: "#cfe0d9ff",
  dim: "#7f948cff",
  good: "#7fe6a0ff",
  warn: "#ffcf6fff",
  error: "#ff7a7aff",
  hover: "#ffffffff"
};

// ---------------------------------------------------------------------------
// The level model
// ---------------------------------------------------------------------------

/// Split like Rust's `str::lines()`: `\r\n` and `\r` fold to `\n`, and a single
/// trailing newline does not produce an empty final line.
function split_lines(text) {
  var lines = String(text).replace(/\r\n/g, "\n").replace(/\r/g, "\n").split("\n");
  if (lines.length > 0 && lines[lines.length - 1] === "") {
    lines.pop();
  }
  return lines;
}

// The sizes the editor will make a level: small enough to read in the viewport,
// large enough to scroll. The sliders quantize to a multiple of a tile, and the
// editor clamps rather than refusing a silly number.
var MIN_WIDTH = 16;
var MAX_WIDTH = 256;
var MIN_HEIGHT = 8;
var MAX_HEIGHT = 64;

/// Round to a multiple of `step` (the size sliders' values are quantized so a size
/// can actually be hit with a drag).
function quantize(value, step) {
  return Math.round(Number(value) / step) * step;
}

/// A size clamped to the editor's bounds, or `fallback` when it is not a number.
function clamp_size(value, min, max, fallback) {
  var n = Math.round(Number(value));
  if (n !== n) {
    return fallback;
  }
  if (n < min) {
    return min;
  }
  return n > max ? max : n;
}

/// A blank level of `width` x `height`: the default metadata, an empty grid with a
/// floor to build on, and a spawn and goal already placed so it is a level the game
/// will accept.
function blank_level(width, height) {
  var meta = [];
  for (var i = 0; i < DEFAULT_META.length; i++) {
    meta.push([DEFAULT_META[i][0], DEFAULT_META[i][1]]);
  }
  var w = clamp_size(width, MIN_WIDTH, MAX_WIDTH, 40);
  var h = clamp_size(height, MIN_HEIGHT, MAX_HEIGHT, 14);
  var rows = [];
  for (var y = 0; y < h; y++) {
    var row = [];
    for (var x = 0; x < w; x++) {
      row.push(y === h - 1 ? "G" : ".");
    }
    rows.push(row);
  }
  // A spawn on the floor and a goal at the far end: a level you can run.
  rows[h - 2][2] = "S";
  rows[h - 2][w - 3] = "F";
  var level = { meta: meta, rows: rows, width: w, height: h };
  index_level(level);
  return level;
}

/// Grow or shrink `level` to `width` x `height`, anchored at the top-left.
///
/// Growing pads the new columns and rows with air (paint the floor you want);
/// shrinking drops the cells past the new edge — but refuses when that would cut
/// off the spawn, the goal or a coin, because the editor never silently drops
/// something you placed. Returns a message when it refuses, `null` when it resized.
function resize_level(level, width, height) {
  var w = clamp_size(width, MIN_WIDTH, MAX_WIDTH, level.width);
  var h = clamp_size(height, MIN_HEIGHT, MAX_HEIGHT, level.height);
  if (w === level.width && h === level.height) {
    return null;
  }
  var cut = [];
  if (level.spawn !== null && (level.spawn[0] >= w || level.spawn[1] >= h)) {
    cut.push("the spawn");
  }
  if (level.goal !== null && (level.goal[0] >= w || level.goal[1] >= h)) {
    cut.push("the goal");
  }
  var lost = 0;
  for (var i = 0; i < level.coins.length; i++) {
    if (level.coins[i][0] >= w || level.coins[i][1] >= h) {
      lost++;
    }
  }
  if (lost > 0) {
    cut.push(lost + " coin(s)");
  }
  if (cut.length > 0) {
    return (
      w + "x" + h + " would cut off " + cut.join(", ") + " — move them first, or grow"
    );
  }
  var rows = [];
  for (var y = 0; y < h; y++) {
    var row = y < level.height ? level.rows[y].slice(0, w) : [];
    while (row.length < w) {
      row.push(".");
    }
    rows.push(row);
  }
  level.rows = rows;
  level.width = w;
  level.height = h;
  index_level(level);
  return null;
}

/// Replace the current level with a fresh grid of `width` x `height`.
function new_level(width, height) {
  state.level = blank_level(width, height);
  state.newW = state.level.width;
  state.newH = state.level.height;
  sync_metadata_menus(state.level);
  state.dirty = true;
  refresh_search(true);
}

/// Apply a new size to the current level, reporting what happened for the console
/// and the panel.
function resize_to(width, height) {
  if (state.level === null) {
    return "error: no level loaded";
  }
  var problem = resize_level(state.level, width, height);
  if (problem !== null) {
    return "error: " + problem;
  }
  state.newW = state.level.width;
  state.newH = state.level.height;
  touch();
  // The grid's own shape changed, so the records describe cells that may no
  // longer exist: this earns a fresh sweep.
  refresh_search(true);
  noctis.log("reverie level: resized to " + state.newW + "x" + state.newH);
  return "ok resized to " + state.newW + "x" + state.newH;
}

/// Parse the game's level text. Returns `{ level, warnings }` or `{ errors }`;
/// the grid is normalized to a rectangle (the game uses row 0's length, which
/// makes a ragged level behave oddly — that is reported as a warning).
function parse_level(text) {
  var lines = split_lines(text);
  var meta = [];
  var index = 0;

  for (; index < lines.length; index++) {
    var line = lines[index];
    if (line.trim() === "") {
      index++;
      break;
    }
    var at = line.indexOf(":");
    if (at < 0) {
      return { errors: ["line " + (index + 1) + ": expected 'key: value'"] };
    }
    meta.push([line.slice(0, at).trim(), line.slice(at + 1).trim()]);
  }

  var raw = [];
  for (; index < lines.length; index++) {
    raw.push(lines[index]);
  }
  if (raw.length === 0) {
    return { errors: ["the level has no grid (a blank line separates the metadata from it)"] };
  }

  // The game takes the width from row 0; normalize to the longest row so a ragged
  // level behaves predictably here (and warn about it).
  var width = 0;
  for (var r = 0; r < raw.length; r++) {
    if (raw[r].length > width) {
      width = raw[r].length;
    }
  }
  var warnings = [];
  var rows = [];
  for (var y = 0; y < raw.length; y++) {
    var cells = raw[y].split("");
    if (cells.length !== width) {
      warnings.push("row " + (y + 1) + " is " + cells.length + " wide, not " + width);
      while (cells.length < width) {
        cells.push(".");
      }
    }
    rows.push(cells);
  }

  var level = { meta: meta, rows: rows, width: width, height: rows.length };
  index_level(level);
  return { level: level, warnings: warnings };
}

/// Recompute the derived structure (spawn, goal, coins) from the grid.
///
/// The search runs hundreds of thousands of frames, so the terrain and the
/// collectables are flattened into plain arrays here: `solid[cell]` is 1 for a
/// solid tile, `coinCell[cell]` the coin's index or -1, `goalCell` the goal's
/// cell or -1. `cell` is `y * width + x` — one array index per lookup, no string
/// key and no allocation in the hot loop.
function index_level(level) {
  level.spawns = [];
  level.goals = [];
  level.coins = [];
  var cells = level.width * level.height;
  var solid = [];
  var coinCell = [];
  for (var i = 0; i < cells; i++) {
    solid.push(0);
    coinCell.push(-1);
  }
  var solids = 0;
  for (var y = 0; y < level.height; y++) {
    var base = y * level.width;
    for (var x = 0; x < level.width; x++) {
      var c = level.rows[y][x];
      if (SOLID_CHARS[c] === true) {
        solid[base + x] = 1;
        solids++;
      } else if (c === "S") {
        level.spawns.push([x, y]);
      } else if (c === "F") {
        level.goals.push([x, y]);
      } else if (c === "C") {
        coinCell[base + x] = level.coins.length;
        level.coins.push([x, y]);
      }
    }
  }
  level.solid = solid;
  level.coinCell = coinCell;
  level.solids = solids;
  level.spawn = level.spawns.length > 0 ? level.spawns[0] : null;
  level.goal = level.goals.length > 0 ? level.goals[0] : null;
  level.goalCell = level.goal === null ? -1 : level.goal[1] * level.width + level.goal[0];
}

/// Write the level back out in the game's text format.
function serialize_level(level) {
  var out = [];
  for (var i = 0; i < level.meta.length; i++) {
    out.push(level.meta[i][0] + ": " + level.meta[i][1]);
  }
  out.push("");
  for (var y = 0; y < level.height; y++) {
    out.push(level.rows[y].join(""));
  }
  return out.join("\n") + "\n";
}

/// Read a metadata key, or `undefined`.
function meta_value(level, key) {
  for (var i = 0; i < level.meta.length; i++) {
    if (level.meta[i][0] === key) {
      return level.meta[i][1];
    }
  }
  return undefined;
}

/// Point a metadata key at a value, adding the key when the file lacks it (the
/// game's parser indexes the metadata by name, so a level it must load has to
/// carry all four).
function set_meta(level, key, value) {
  for (var i = 0; i < level.meta.length; i++) {
    if (level.meta[i][0] === key) {
      level.meta[i][1] = value;
      return;
    }
  }
  level.meta.push([key, value]);
}

/// Whether `names` holds `value`.
function named_in(names, value) {
  for (var i = 0; i < names.length; i++) {
    if (names[i] === value) {
      return true;
    }
  }
  return false;
}

/// The index `names` holds `value` at, or 0 — a menu always names one of its
/// options.
function index_of(names, value) {
  for (var i = 0; i < names.length; i++) {
    if (names[i] === value) {
      return i;
    }
  }
  return 0;
}

/// Point the panel's music and background menus at the level's own names.
///
/// The menus hold an index, and the level holds a name, so the one is derived from
/// the other whenever the level changes — rather than the menu writing its own
/// index back over a name the level already had (which would silently rewrite the
/// file of a level that named something the game does not know).
function sync_metadata_menus(level) {
  state.musicIndex = index_of(MUSIC_NAMES, meta_value(level, "music"));
  state.backgroundIndex = index_of(BACKGROUND_NAMES, meta_value(level, "background"));
}

/// Point a level at a song or background by name (see [`set_meta`]), keeping the
/// panel's menus in step and adopting the new signature — metadata is not part of
/// the search's records, so no arc is re-simulated for it.
function choose_metadata(level, key, value) {
  set_meta(level, key, value);
  sync_metadata_menus(level);
  touch();
  refresh_search(false);
}

// ---------------------------------------------------------------------------
// The game's movement (a faithful port of `Player::update`, per 1/60s step)
// ---------------------------------------------------------------------------

function solid_at(level, tx, ty) {
  if (tx < 0 || ty < 0 || tx >= level.width || ty >= level.height) {
    return false;
  }
  return level.solid[ty * level.width + tx] === 1;
}

// ---------------------------------------------------------------------------
// The jump-arc search
// ---------------------------------------------------------------------------

/// How much simulation the search may spend.
///
/// The unit is the *frame* it simulates, and the editor's JavaScript engine runs
/// the game's integrator slowly — an arc of a running jump is ~40-80 frames, and
/// a whole `level1` sweep is a few thousand arcs, i.e. a fraction of a second of
/// gameplay replayed for every second of wall clock. So the search is spread out:
/// `slice` is the work one advance does (a couple of arcs per frame, via the
/// `"update"` hook) and `limit` is the total before the verdict is reported as
/// partial rather than "unreachable".
var SEARCH = {
  slice: 2,
  limit: 12000,
  maxFrames: 96, // 1.6s at 60Hz — a fall past the death plane is caught first
  sample: 3 // frames between the route's samples
};

/// Where in a cell the search takes off from, in preference order: the cell's own
/// centre (the only legal spot in a one-tile-wide shaft, where a body may not
/// overhang), then the ends of the body's footprint — a body may overhang a ledge
/// by a hitbox width, and those pixels decide a marginal jump (see
/// [`standing_at`]).
var TAKEOFFS = [2, -(HITBOX_W - 1), TILE - 1];

/// The policies tried from every takeoff point: a running jump at four holds, a
/// straight-up jump, and walking off a ledge. Everything the search accepts is a
/// trajectory the real integrator produced; the simulation is the expensive part,
/// so the set stays small (a wider sweep would only add routes the narrow one
/// already finds).
function build_policies() {
  var holds = [1, 5, 14, 26];
  var policies = [];
  var dirs = [-1, 1];
  for (var d = 0; d < dirs.length; d++) {
    var dir = dirs[d];
    for (var h = 0; h < holds.length; h++) {
      policies.push({ dir: dir, hold: holds[h], vx: dir * SPEED });
    }
    policies.push({ dir: dir, hold: -1, vx: dir * SPEED });
  }
  policies.push({ dir: 0, hold: 5, vx: 0 });
  policies.push({ dir: 0, hold: 26, vx: 0 });
  return policies;
}

var POLICIES = build_policies();

/// The simulation's scratch: one arc reuses these instead of allocating, because
/// a large level runs hundreds of thousands of frames through them.
///
/// `ARC` holds the samples and the outcome of the arc in flight; the coin flags
/// are a fixed-size table plus the list of the entries this arc set, so resetting
/// between arcs costs only what the arc touched.
var ARC = {
  len: 0,
  xs: [],
  ys: [],
  goal: false,
  goalLen: 0,
  landed: false,
  minTx: 0,
  maxTx: 0,
  minTy: 0,
  maxTy: 0,
  x: 0,
  y: 0
};
var COIN_SEEN = [];
var COIN_TOUCHED = [];
var COIN_TOUCHED_N = 0;

/// Build one of every object shape this extension evaluates, once, at load.
///
/// Slag caches an object literal's whole shape (`Step::ObjectFast`) on the shared
/// canonical empty map, and that cache insert
/// (`crux::map::Map::set_composite_child`) takes no write barrier — so a literal
/// shape *first* evaluated after a minor collection puts an unrecorded old→young
/// edge in the heap, which Slag's own verifier (always on in a debug build)
/// reports as a "write-barrier miss". The search allocates enough to force a
/// collection, so warming every shape here, while the heap is still young, leaves
/// the cache complete and the buggy insert never runs again. The real fix is one
/// line — `write_barrier_handle(self, child)` in `set_composite_child` — which
/// must land in the pinned Slag revision.
function warm_shapes() {
  return [
    { dir: 0, hold: 0, vx: 0 },
    {
      len: 0,
      xs: null,
      ys: null,
      goal: false,
      goalLen: 0,
      landed: false,
      minTx: 0,
      maxTx: 0,
      minTy: 0,
      maxTy: 0,
      x: 0,
      y: 0
    },
    {
      tx: 0,
      ty: 0,
      cursor: 0,
      seeded: false,
      queued: false,
      index: 0,
      land: null,
      bbox: null,
      flag: null,
      spec: null,
      coinAt: null,
      coinFlat: null,
      goalLen: null
    },
    {
      signature: null,
      width: 0,
      height: 0,
      at: null,
      reached: null,
      viaEntry: null,
      viaArc: null,
      entries: null,
      head: 0,
      seed: -1,
      coins: null,
      coinsLeft: 0,
      goal: false,
      goalEntry: -1,
      goalArc: -1,
      goalLen: 0,
      route: null,
      spent: 0,
      seeded: false,
      done: false,
      truncated: false
    },
    { minX: 0, maxX: 0, minY: 0, maxY: 0, any: false },
    { errors: null },
    { errors: null, warnings: null, marks: null },
    {
      errors: null,
      warnings: null,
      marks: null,
      fresh: false,
      search: null,
      settled: false,
      reachedGoal: false,
      unreachableCoins: null
    },
    { meta: null, rows: null, width: 0, height: 0 },
    { level: null, warnings: null },
    { width: 0, height: 0 }
  ];
}

/// How much simulation the search is given when a path has no frames to spread
/// the work over (the CLI's `--run` verbs). The editor pumps the rest, a slice
/// per frame; an explicit `level check` runs to [`SEARCH`]'s limit instead.
///
/// A playable level settles as soon as its goal and coins are all reached (a
/// sound early exit). Only an unreachable verdict has to exhaust the reachable
/// set, which is what `limit` bounds.
var EAGER_ARCS = 240;

var WARMED_SHAPES = warm_shapes();

/// Simulate one trajectory from `(x, y)`, writing its outcome into [`ARC`].
///
/// A transcription of the game's own integrator (`src/game/player.rs`,
/// `Player::update`): the same constants, the same collision resolution, the same
/// `1/60` step — so every trajectory the search accepts is one the game would
/// produce. It is written as one flat function over locals with the collision
/// probes inlined, because a validation runs hundreds of thousands of frames
/// through it and, in the editor's JavaScript engine, a call per frame costs
/// about as much as the physics itself.
///
/// `hold` is how many frames jump is held (`-1` never jumps).
function arc_from(level, startX, startY, grounded, dir, hold, vx) {
  var x = startX;
  var y = startY;
  var vy = 0;
  var buffer = 0;
  var onGround = grounded === true;
  var leftGround = !onGround;
  var target = dir * SPEED;
  var jumps = hold >= 0;
  var width = level.width;
  var height = level.height;
  var solid = level.solid;
  var coinCell = level.coinCell;
  var goalCell = level.goalCell;
  var maxX = width * TILE - HITBOX_W;
  var death = height * TILE + DEATH_PLANE;
  var sampleIn = SEARCH.sample;

  for (var i = COIN_TOUCHED_N - 1; i >= 0; i--) {
    COIN_SEEN[COIN_TOUCHED[i]] = 0;
  }
  COIN_TOUCHED_N = 0;

  ARC.len = 0;
  ARC.goal = false;
  ARC.goalLen = 0;
  ARC.landed = false;
  ARC.x = x;
  ARC.y = y;
  ARC.xs[0] = x;
  ARC.ys[0] = y;
  ARC.len = 1;
  // The tile box the body's collision probes can read along this arc. It is what
  // `revalidate` uses to decide, without re-simulating, whether an edit could have
  // changed the arc at all (see [`new_search`]).
  ARC.minTx = Math.floor(x / TILE);
  ARC.maxTx = ARC.minTx;
  ARC.minTy = Math.floor(y / TILE);
  ARC.maxTy = ARC.minTy;

  for (var frame = 0; frame < SEARCH.maxFrames; frame++) {
    // --- horizontal: approach the target speed, move, resolve the overlap ---
    var accel = onGround ? GROUND_ACCEL : AIR_ACCEL;
    var spare = accel * STEP;
    if (vx < target) {
      vx += spare;
      if (vx > target) {
        vx = target;
      }
    } else if (vx > target) {
      vx -= spare;
      if (vx < target) {
        vx = target;
      }
    }
    var move = vx * STEP;
    x += move;

    var left = Math.floor(x / TILE);
    if (left < 0) {
      left = 0;
    }
    var right = Math.floor((x + HITBOX_W - 0.01) / TILE);
    var top = Math.floor(y / TILE);
    if (top < 0) {
      top = 0;
    }
    var bottom = Math.floor((y + HITBOX_H - 0.01) / TILE);
    var hit = false;
    for (var ty = top; ty <= bottom && !hit; ty++) {
      if (ty >= height) {
        break;
      }
      var base = ty * width;
      for (var tx = left; tx <= right; tx++) {
        if (tx >= width) {
          break;
        }
        if (solid[base + tx] === 1) {
          hit = true;
          break;
        }
      }
    }
    if (hit) {
      if (move > 0) {
        x = Math.floor((x + HITBOX_W) / TILE) * TILE - HITBOX_W - 0.01;
      } else if (move < 0) {
        x = Math.floor(x / TILE) * TILE + TILE + 0.01;
      }
      vx = 0;
    }
    if (x < 0) {
      x = 0;
      vx = 0;
    } else if (x > maxX) {
      x = maxX;
      vx = 0;
    }

    // --- jump: a press is buffered, and an early release cuts the climb ---
    if (jumps && frame === 0) {
      buffer = JUMP_BUFFER;
    }
    if (onGround && buffer > 0) {
      vy = -JUMP_SPEED;
      onGround = false;
      buffer = 0;
    }
    buffer -= STEP;
    if (buffer < 0) {
      buffer = 0;
    }
    if (!(jumps && frame <= hold) && vy < -JUMP_RELEASE) {
      vy = -JUMP_RELEASE;
    }

    // --- vertical ---
    vy += GRAVITY * STEP;
    if (vy > MAX_FALL) {
      vy = MAX_FALL;
    }
    onGround = false;
    move = vy * STEP;
    y += move;
    left = Math.floor(x / TILE);
    if (left < 0) {
      left = 0;
    }
    right = Math.floor((x + HITBOX_W - 0.01) / TILE);
    top = Math.floor(y / TILE);
    if (top < 0) {
      top = 0;
    }
    bottom = Math.floor((y + HITBOX_H - 0.01) / TILE);
    hit = false;
    for (ty = top; ty <= bottom && !hit; ty++) {
      if (ty >= height) {
        break;
      }
      base = ty * width;
      for (tx = left; tx <= right; tx++) {
        if (tx >= width) {
          break;
        }
        if (solid[base + tx] === 1) {
          hit = true;
          break;
        }
      }
    }
    if (hit) {
      if (move > 0) {
        y = Math.floor((y + HITBOX_H) / TILE) * TILE - HITBOX_H - 0.01;
      } else if (move < 0) {
        y = Math.floor(y / TILE) * TILE + TILE + 0.01;
      }
      onGround = vy > 0;
      vy = 0;
    }

    // --- what the body overlaps: coins, the goal ---
    left = Math.floor(x / TILE);
    if (left < 0) {
      left = 0;
    }
    right = Math.floor((x + HITBOX_W - 0.01) / TILE);
    top = Math.floor(y / TILE);
    if (top < 0) {
      top = 0;
    }
    bottom = Math.floor((y + HITBOX_H - 0.01) / TILE);
    if (left < ARC.minTx) {
      ARC.minTx = left;
    }
    if (right > ARC.maxTx) {
      ARC.maxTx = right;
    }
    if (top < ARC.minTy) {
      ARC.minTy = top;
    }
    if (bottom > ARC.maxTy) {
      ARC.maxTy = bottom;
    }
    for (ty = top; ty <= bottom; ty++) {
      if (ty >= height) {
        break;
      }
      base = ty * width;
      for (tx = left; tx <= right; tx++) {
        if (tx >= width) {
          break;
        }
        var cell = base + tx;
        var coin = coinCell[cell];
        // `!== 1`, not `=== 0`: a search outlives an edit, so the coin list it is
        // running against can have grown since this table was last cleared, and an
        // untouched entry reads as `undefined`.
        if (coin >= 0 && COIN_SEEN[coin] !== 1) {
          COIN_SEEN[coin] = 1;
          COIN_TOUCHED[COIN_TOUCHED_N] = coin;
          COIN_TOUCHED_N++;
        }
        if (cell === goalCell) {
          ARC.goal = true;
        }
      }
    }

    // --- the route's samples ---
    sampleIn--;
    if (sampleIn <= 0) {
      sampleIn = SEARCH.sample;
      ARC.xs[ARC.len] = x;
      ARC.ys[ARC.len] = y;
      ARC.len++;
    }
    if (ARC.goal && ARC.goalLen === 0) {
      // Finish the route at the frame the goal was touched.
      ARC.xs[ARC.len] = x;
      ARC.ys[ARC.len] = y;
      ARC.len++;
      ARC.goalLen = ARC.len;
    }

    if (y > death) {
      return;
    }
    if (!onGround) {
      leftGround = true;
    } else if (leftGround) {
      ARC.landed = true;
      ARC.x = x;
      ARC.y = y;
      return;
    }
  }
}

/// Whether the player's body at `(x, y)` is inside no tile and stands on one.
///
/// A body may overhang a ledge by up to its own width, so the support is *any*
/// solid tile under the body's footprint — which is why the search takes off from
/// the ends of that footprint, not just the cell's centre.
function standing_at(level, x, y) {
  var row = Math.floor(y / TILE);
  var left = Math.max(Math.floor(x / TILE), 0);
  var right = Math.min(Math.floor((x + HITBOX_W - 0.01) / TILE), level.width - 1);
  var supported = false;
  for (var tx = left; tx <= right; tx++) {
    if (solid_at(level, tx, row)) {
      return false;
    }
    if (solid_at(level, tx, row + 1)) {
      supported = true;
    }
  }
  return supported;
}

/// The standing cell a landing is in, or `null` when it is not a place to stand.
///
/// The player's body may overhang a platform's edge: any tile the body covers is
/// a support, so a landing is accepted when *one* of them is solid and the body's
/// own row is free there. The returned cell is the canonical place to stand in
/// that tile.
function stand_of(level, x, y) {
  var ty = Math.round((y + HITBOX_H) / TILE) - 1;
  if (ty < 0 || ty + 1 >= level.height) {
    return null;
  }
  var left = Math.max(Math.floor(x / TILE), 0);
  var right = Math.min(Math.floor((x + HITBOX_W - 0.01) / TILE), level.width - 1);
  for (var tx = left; tx <= right; tx++) {
    if (solid_at(level, tx, ty + 1) && !solid_at(level, tx, ty)) {
      return { tx: tx, ty: ty };
    }
  }
  return null;
}

/// The tile box an edit touched since the last re-validation, in tile coordinates.
///
/// A grid change only ever touches a handful of cells (`paint_cell` marks each one),
/// and this is the box they all fit in: one comparison per arc boxes in the arcs the
/// change could have been read by. It is one object, mutated, so an edit allocates
/// nothing.
var CHANGE = { minX: 0, maxX: 0, minY: 0, maxY: 0, any: false };

/// A search in progress, over *arc records*.
///
/// The sweep simulates one trajectory per (standing cell, takeoff point, policy) —
/// an "arc" — and records what it did: where it landed, which coins it touched,
/// whether it touched the goal, and the tile box its body could have read on the
/// way. Reachability, the collected coins and the route are *derived* from those
/// records, which is what makes an edit cheap: an arc can only behave differently
/// under an edited grid if the change sits in tiles it read, so [`revalidate`]
/// re-simulates just the arcs the change touched and re-derives the rest without
/// touching the physics. Restarting the sweep instead replays the whole level — a
/// `level1` sweep is a few thousand arcs.
///
/// The records are parallel arrays on one entry per standing cell, because one
/// object per arc would spend the frame in the allocator.
///
/// Entry 0 is a pseudo-cell: the spawn's own drop (the game drops the player there
/// with air control, and never jumps).
function new_search(level) {
  var cells = level.width * level.height;
  var at = [];
  var reached = [];
  var viaEntry = [];
  var viaArc = [];
  for (var i = 0; i < cells; i++) {
    at.push(-1);
    reached.push(0);
    viaEntry.push(-1);
    viaArc.push(-1);
  }
  // One flag per *cell*, not per coin: a coin is identified by the cell it sits in,
  // because an edit can add or remove a coin and renumber `level.coins` under a
  // search that is still alive (a coin's index is only meaningful within one grid).
  var coins = [];
  for (var c = 0; c < cells; c++) {
    coins.push(0);
  }
  // The arc scratch's coin flags are module-level and shared. They are only ever
  // asked "has this arc counted coin i yet", so they need no sizing either.
  COIN_SEEN = [];
  COIN_TOUCHED_N = 0;
  return {
    signature: null,
    width: level.width,
    height: level.height,
    at: at,
    reached: reached,
    viaEntry: viaEntry,
    viaArc: viaArc,
    entries: [],
    queue: [],
    head: 0,
    seed: -1,
    coins: coins,
    coinsLeft: level.coins.length,
    goal: false,
    goalEntry: -1,
    goalArc: -1,
    goalLen: 0,
    route: null,
    spent: 0,
    seeded: false,
    done: false,
    truncated: false
  };
}

/// A standing cell the sweep reached: the arcs simulated from it and the boxes
/// [`revalidate`] needs. See [`new_search`] for the record arrays.
function new_entry(tx, ty, seeded) {
  return {
    tx: tx,
    ty: ty,
    cursor: 0,
    seeded: seeded === true,
    queued: false,
    index: 0,
    land: [],
    bbox: [],
    flag: [],
    spec: [],
    coinAt: [],
    coinFlat: [],
    goalLen: []
  };
}

/// Queue an entry for expansion, once.
function enqueue(search, index) {
  var entry = search.entries[index];
  if (entry === undefined || entry.queued) {
    return;
  }
  entry.queued = true;
  search.queue.push(index);
}

/// Note that an edit changed one cell, for [`revalidate`].
function touch_at(tx, ty) {
  if (!CHANGE.any) {
    CHANGE.minX = tx;
    CHANGE.maxX = tx;
    CHANGE.minY = ty;
    CHANGE.maxY = ty;
    CHANGE.any = true;
    return;
  }
  if (tx < CHANGE.minX) {
    CHANGE.minX = tx;
  }
  if (tx > CHANGE.maxX) {
    CHANGE.maxX = tx;
  }
  if (ty < CHANGE.minY) {
    CHANGE.minY = ty;
  }
  if (ty > CHANGE.maxY) {
    CHANGE.maxY = ty;
  }
}

/// The route polyline, filled in place.
///
/// It is one reused object rather than a fresh one per search because Slag's
/// verifier (see [`warm_shapes`]) catches a *young* map stored into a *long-lived*
/// one: assigning a freshly built `{ xs, ys }` into the search — which happens at
/// exactly the moment a sweep finds the goal — is precisely that. `ROUTE` is built
/// at load, so it is never younger than the search that points at it.
var ROUTE = { xs: [], ys: [] };

/// The polyline from the spawn to the frame the goal was touched, in [`ROUTE`].
///
/// The chain is walked backwards through the `via` links the fold left on each
/// cell, then replayed forwards: a sweep records thousands of arcs and only this
/// one chain is ever drawn, so the trajectories themselves are not kept alive.
function build_route(level, search) {
  ROUTE.xs.length = 0;
  ROUTE.ys.length = 0;
  if (search.goalEntry < 0) {
    return ROUTE;
  }
  var entries = [];
  var arcs = [];
  var e = search.goalEntry;
  var a = search.goalArc;
  var guard = 0;
  while (e >= 0 && guard < 8192) {
    guard++;
    entries.push(e);
    arcs.push(a);
    if (e === search.seed) {
      break;
    }
    var entry = search.entries[e];
    var cell = entry.ty * level.width + entry.tx;
    a = search.viaArc[cell];
    e = search.viaEntry[cell];
  }
  for (var i = entries.length - 1; i >= 0; i--) {
    replay_arc(level, search.entries[entries[i]], arcs[i]);
  }
  return ROUTE;
}

/// Re-simulate one recorded arc and append its samples to [`ROUTE`].
///
/// The arc's own parameters travel with its record (`spec`), so this is the same
/// simulation the sweep ran; an arc that touched the goal stops where the goal was
/// touched (`goalLen`).
function replay_arc(level, record, a) {
  var base = a * 6;
  var spec = record.spec;
  arc_from(
    level,
    spec[base],
    spec[base + 1],
    spec[base + 2] === 1,
    spec[base + 3],
    spec[base + 4],
    spec[base + 5]
  );
  var limit = record.goalLen[a] > 0 ? record.goalLen[a] : ARC.len;
  for (var i = 0; i < limit; i++) {
    ROUTE.xs.push(ARC.xs[i]);
    ROUTE.ys.push(ARC.ys[i]);
  }
}

/// Write the arc [`ARC`] holds into record `a` of `entry` — appending it when `a`
/// is the record count — and remember what it did.
///
/// An arc's own parameters travel with its record (`spec`) so the route can replay
/// it, and the tile box its body could read (`bbox`) is what tells [`revalidate`]
/// whether a later edit could have changed its mind.
function write_record(level, search, entry, a, sx, sy, grounded, dir, hold, vx) {
  if (a === entry.land.length) {
    entry.land.push(-1);
    entry.bbox.push(0, 0, 0, 0);
    entry.flag.push(0);
    entry.spec.push(0, 0, 0, 0, 0, 0);
    entry.coinAt.push(0, 0);
    entry.goalLen.push(0);
  }
  var b = 4 * a;
  entry.land[a] = -1;
  entry.bbox[b] = ARC.minTx - 1;
  entry.bbox[b + 1] = ARC.maxTx + 1;
  entry.bbox[b + 2] = ARC.minTy - 1;
  entry.bbox[b + 3] = ARC.maxTy + 1;
  entry.flag[a] = ARC.goal ? 1 : 0;
  entry.goalLen[a] = ARC.goalLen;
  var s = 6 * a;
  entry.spec[s] = sx;
  entry.spec[s + 1] = sy;
  entry.spec[s + 2] = grounded === true ? 1 : 0;
  entry.spec[s + 3] = dir;
  entry.spec[s + 4] = hold;
  entry.spec[s + 5] = vx;
  entry.coinAt[2 * a] = entry.coinFlat.length;
  entry.coinAt[2 * a + 1] = COIN_TOUCHED_N;
  for (var k = 0; k < COIN_TOUCHED_N; k++) {
    // Recorded as the cell, not the index: see the coin note in [`new_search`].
    var touched = level.coins[COIN_TOUCHED[k]];
    entry.coinFlat.push(touched[1] * level.width + touched[0]);
  }
  if (ARC.landed) {
    var stand = stand_of(level, ARC.x, ARC.y);
    if (stand !== null) {
      var cell = stand.ty * level.width + stand.tx;
      entry.land[a] = cell;
      if (search.at[cell] < 0) {
        var reached = new_entry(stand.tx, stand.ty, false);
        reached.index = search.entries.length;
        search.at[cell] = reached.index;
        search.entries.push(reached);
      }
    }
  }
  return a;
}

/// Re-simulate record `a` of `entry` — one of the arcs an edit's box covered — and
/// rewrite it in place.
function resweep_arc(level, search, entry, a) {
  var s = 6 * a;
  var spec = entry.spec;
  var grounded = spec[s + 2] === 1;
  search.spent++;
  arc_from(level, spec[s], spec[s + 1], grounded, spec[s + 3], spec[s + 4], spec[s + 5]);
  return write_record(
    level,
    search,
    entry,
    a,
    spec[s],
    spec[s + 1],
    grounded,
    spec[s + 3],
    spec[s + 4],
    spec[s + 5]
  );
}

/// Fold a recorded arc into the search: the coins and the goal it touched, and the
/// cell it landed on.
///
/// A cell the sweep had not reached becomes reached — its entry is queued for
/// expansion, and the arc that got there is remembered so [`build_route`] can walk
/// back along it. A search also ends as soon as the goal *and* every coin have been
/// reached: it is monotone, so nothing a later arc finds could change that verdict,
/// and a playable level settles in a fraction of a full sweep.
function fold_arc(level, search, entry, a) {
  var start = entry.coinAt[2 * a];
  var count = entry.coinAt[2 * a + 1];
  for (var k = 0; k < count; k++) {
    var spot = entry.coinFlat[start + k];
    if (search.coins[spot] !== 1) {
      search.coins[spot] = 1;
      search.coinsLeft--;
    }
  }
  if ((entry.flag[a] & 1) === 1 && !search.goal) {
    search.goal = true;
    search.goalEntry = entry.index;
    search.goalArc = a;
    search.goalLen = entry.goalLen[a];
    search.route = build_route(level, search);
  }
  if (search.goal && search.coinsLeft === 0) {
    search.done = true;
  }
  var cell = entry.land[a];
  if (cell < 0 || search.reached[cell] === 1) {
    return;
  }
  search.reached[cell] = 1;
  search.viaEntry[cell] = entry.index;
  search.viaArc[cell] = a;
  enqueue(search, search.at[cell]);
}

/// Advance the search by up to `slice` simulated arcs. Returns whether there is
/// more work (a false return means the verdict is settled).
///
/// A slice may cut inside a standing cell's policy sweep, so the sweep resumes
/// from `cursor` next call rather than losing the policies it had left.
function advance_search(level, search, slice) {
  if (search.done) {
    return false;
  }
  var from = search.spent;
  if (!search.seeded) {
    search.seeded = true;
    if (level.spawn === null) {
      search.done = true;
      return false;
    }
    // The game drops the player at the spawn and lets them fall, with air
    // control: three trajectories, none of them jumping.
    var spawnX = level.spawn[0] * TILE;
    var spawnY = level.spawn[1] * TILE;
    var seed = new_entry(level.spawn[0], level.spawn[1], true);
    seed.index = search.entries.length;
    search.seed = seed.index;
    search.entries.push(seed);
    enqueue(search, seed.index);
    for (var d = -1; d <= 1; d++) {
      arc_from(level, spawnX, spawnY, false, d, -1, 0);
      search.spent++;
      fold_arc(
        level,
        search,
        seed,
        write_record(level, search, seed, seed.land.length, spawnX, spawnY, false, d, -1, 0)
      );
    }
  }

  var perNode = TAKEOFFS.length * POLICIES.length;
  while (
    !search.done &&
    search.head < search.queue.length &&
    search.spent - from < slice &&
    search.spent < SEARCH.limit
  ) {
    var entry = search.entries[search.queue[search.head]];
    var takeoffY = entry.ty * TILE + 0.99;
    settle_entry(level, search, entry);
    while (entry.cursor < perNode) {
      if (search.spent >= SEARCH.limit) {
        break;
      }
      var o = Math.floor(entry.cursor / POLICIES.length);
      var p = entry.cursor - o * POLICIES.length;
      entry.cursor++;
      search.spent++;
      var takeoffX = entry.tx * TILE + TAKEOFFS[o];
      if (!standing_at(level, takeoffX, takeoffY)) {
        continue;
      }
      var policy = POLICIES[p];
      arc_from(level, takeoffX, takeoffY, true, policy.dir, policy.hold, policy.vx);
      fold_arc(
        level,
        search,
        entry,
        write_record(
          level,
          search,
          entry,
          entry.land.length,
          takeoffX,
          takeoffY,
          true,
          policy.dir,
          policy.hold,
          policy.vx
        )
      );
      if (search.done || search.spent - from >= slice) {
        break;
      }
    }
    if (entry.cursor >= perNode) {
      search.head++;
    } else {
      // The slice is spent: this cell's sweep resumes next call.
      break;
    }
  }

  if (search.spent >= SEARCH.limit) {
    search.truncated = true;
    search.done = true;
  } else if (search.head >= search.queue.length && !search.done) {
    search.done = true;
  }
  return !search.done;
}

/// Re-simulate an entry's stale records and fold all of its records into the
/// search, so that what it contributes describes the current grid.
///
/// Folding is idempotent — a coin is either collected or not, a cell reached or not
/// — so this is safe to run whether or not the entry has been visited before. It
/// runs when the re-derivation walk reaches an entry *and* when the sweep expands
/// one, so a record an edit invalidated is never folded uninspected, whichever way
/// the entry becomes reachable again.
function settle_entry(level, search, entry) {
  for (var r = 0; r < entry.land.length; r++) {
    if ((entry.flag[r] & 2) === 2) {
      resweep_arc(level, search, entry, r);
    }
    fold_arc(level, search, entry, r);
  }
}

/// Re-validate a search against an edited grid, in place.
///
/// `change` is the box of tiles the edit touched. The grid is the only input an arc
/// reads, so an arc whose recorded box the change did not reach cannot have behaved
/// differently; only the arcs the change *did* reach need re-simulating, and the
/// walk below folds both kinds back into a fresh derivation of the reached cells,
/// the coins, the goal and the route. No restart, and no simulation for the rest.
///
/// The derivation is always recomputed, never patched: nothing stale can survive it.
function revalidate(level, search, change) {
  // 1. Mark the arcs the change could have been read by. A change in a cell's own
  //    neighbourhood can also change which of its takeoffs stand, so those entries'
  //    arcs are all suspect — whether or not one of them came nearer.
  for (var i = 0; i < search.entries.length; i++) {
    var entry = search.entries[i];
    var near =
      entry.tx >= change.minX - 1 &&
      entry.tx <= change.maxX + 1 &&
      entry.ty >= change.minY - 1 &&
      entry.ty <= change.maxY + 1;
    for (var a = 0; a < entry.land.length; a++) {
      var stale = near;
      if (!stale) {
        var b = 4 * a;
        stale =
          entry.bbox[b] <= change.maxX &&
          entry.bbox[b + 1] >= change.minX &&
          entry.bbox[b + 2] <= change.maxY &&
          entry.bbox[b + 3] >= change.minY;
      }
      if (stale) {
        entry.flag[a] |= 2;
      }
    }
  }

  // 2. Re-derive. The reached set, the coins, the goal and the route are all
  //    recomputed from the records.
  for (var c = 0; c < search.reached.length; c++) {
    search.reached[c] = 0;
    search.viaEntry[c] = -1;
    search.viaArc[c] = -1;
  }
  for (var coin = 0; coin < search.coins.length; coin++) {
    search.coins[coin] = 0;
  }
  // `coinsLeft` counts the level's coins, which an edit may have added to or
  // removed since the search was made.
  search.coinsLeft = level.coins.length;
  search.goal = false;
  search.goalEntry = -1;
  search.goalArc = -1;
  search.goalLen = 0;
  search.route = null;
  search.queue.length = 0;
  search.head = 0;
  search.spent = 0;
  search.truncated = false;
  search.done = false;
  for (var e = 0; e < search.entries.length; e++) {
    search.entries[e].queued = false;
  }
  if (search.seed >= 0) {
    enqueue(search, search.seed);
  }
  // A worklist over the reached cells: visiting an entry re-simulates its stale
  // arcs and folds its records back in, which queues whatever they land on. It is
  // cut short the moment the goal and every coin are reached (see [`fold_arc`]).
  while (search.head < search.queue.length && !search.done) {
    var current = search.entries[search.queue[search.head]];
    search.head++;
    settle_entry(level, search, current);
  }
  // The sweep walks the same queue, so hand it back its start: entries the change
  // emptied of work are simply already finished when it reaches them.
  search.head = 0;
}

/// Run a search to completion (or `arcs` arcs), for a path with no frames to
/// spread the work over.
function search_eagerly(level, search, arcs) {
  while (!search.done && search.spent < arcs) {
    advance_search(level, search, SEARCH.slice);
  }
  return search;
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate the palette: four `#rrggbb` colours, as the game's parser demands.
function palette_problem(value) {
  var parts = String(value).split(",");
  if (parts.length !== 4) {
    return "palette must hold exactly 4 colours (found " + parts.length + ")";
  }
  for (var i = 0; i < parts.length; i++) {
    var color = parts[i].trim();
    if (!/^#[0-9a-fA-F]{6}$/.test(color)) {
      return "palette colour " + (i + 1) + " ('" + color + "') must be #rrggbb";
    }
  }
  return null;
}

/// The structural report, filled in place.
///
/// Rebuilt objects handed back into a long-lived slot (`state.report`) are the
/// young-into-old store Slag's verifier catches (see [`warm_shapes`]), so the
/// report and its lists are allocated once and refilled, and its marks are taken
/// from a pool of pre-built `{ x, y }` cells rather than built per check.
var STRUCTURE = { errors: [], warnings: [], marks: [] };
var MARK_POOL = [];

/// A pooled mark cell for a failing position.
function mark_at(x, y) {
  if (STRUCTURE.marks.length === MARK_POOL.length) {
    MARK_POOL.push({ x: 0, y: 0 });
  }
  var mark = MARK_POOL[STRUCTURE.marks.length];
  mark.x = x;
  mark.y = y;
  STRUCTURE.marks.push(mark);
}

/// The cheap, always-current checks — no simulation. Fills [`STRUCTURE`].
function structural_report(level, warnings) {
  var report = STRUCTURE;
  report.errors.length = 0;
  report.warnings.length = 0;
  report.marks.length = 0;
  if (warnings) {
    for (var w = 0; w < warnings.length; w++) {
      report.warnings.push(warnings[w]);
    }
  }

  for (var i = 0; i < REQUIRED_META.length; i++) {
    var key = REQUIRED_META[i];
    if (meta_value(level, key) === undefined) {
      report.errors.push("metadata '" + key + "' is missing (the game requires all four)");
    }
  }
  var palette = meta_value(level, "palette");
  if (palette !== undefined) {
    var problem = palette_problem(palette);
    if (problem !== null) {
      report.errors.push(problem);
    }
  }
  // The game's own tables decide these two: a name it does not know stops the
  // game dead when the level loads, so a bad one is an error here.
  var music = meta_value(level, "music");
  if (music !== undefined && !named_in(MUSIC_NAMES, music)) {
    report.errors.push(
      "music '" + music + "' is not one of the game's songs: " + MUSIC_NAMES.join(", ")
    );
  }
  var background = meta_value(level, "background");
  if (background !== undefined && !named_in(BACKGROUND_NAMES, background)) {
    report.errors.push(
      "background '" +
        background +
        "' is not one the game can draw: " +
        BACKGROUND_NAMES.join(", ")
    );
  }

  if (level.spawns.length === 0) {
    report.errors.push("no spawn: paint one with the Spawn brush");
  } else if (level.spawns.length > 1) {
    report.errors.push(level.spawns.length + " spawns: the game allows exactly one");
    for (var s = 0; s < level.spawns.length; s++) {
      mark_at(level.spawns[s][0], level.spawns[s][1]);
    }
  }
  if (level.goals.length === 0) {
    report.errors.push("no goal: paint one with the Goal brush");
  } else if (level.goals.length > 1) {
    report.errors.push(level.goals.length + " goals: the game allows exactly one");
    for (var g = 0; g < level.goals.length; g++) {
      mark_at(level.goals[g][0], level.goals[g][1]);
    }
  }

  var unknown = 0;
  for (var y = 0; y < level.height; y++) {
    for (var x = 0; x < level.width; x++) {
      var c = level.rows[y][x];
      if (KNOWN_CHARS[c] !== true) {
        unknown++;
        if (unknown <= 8) {
          report.errors.push("unknown tile '" + c + "' at " + x + "," + y);
          mark_at(x, y);
        }
      }
    }
  }

  // The spawn only needs to be empty; the player falls from it and the search
  // decides whether the fall is survivable.
  if (level.spawn !== null && solid_at(level, level.spawn[0], level.spawn[1])) {
    report.errors.push("the spawn is inside a solid tile");
    mark_at(level.spawn[0], level.spawn[1]);
  }
  if (level.goal !== null && solid_at(level, level.goal[0], level.goal[1])) {
    report.errors.push("the goal is inside a solid tile");
    mark_at(level.goal[0], level.goal[1]);
  }

  for (var c2 = 0; c2 < level.coins.length; c2++) {
    if (solid_at(level, level.coins[c2][0], level.coins[c2][1])) {
      report.warnings.push(
        "coin at " + level.coins[c2][0] + "," + level.coins[c2][1] + " is inside a solid tile"
      );
    }
  }
  return report;
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

var state = {
  slot: null,
  level: null,
  report: null,
  search: null,
  signature: null, // the grid signature of the current grid
  dirty: false,
  // The size the panel's sliders hold, for `New level` and `Resize`, and the
  // indices its music/background menus hold.
  newW: 40,
  newH: 14,
  musicIndex: 0,
  backgroundIndex: 0,
  active: true,
  showGrid: false,
  showReach: true,
  showRoute: true,
  brush: "G",
  painting: false,
  lastCell: null,
  view: { zoom: 1, panX: 0, panY: 0, cell: 8, ox: 0, oy: 0, width: 0, height: 0 }
};

/// A cheap signature of everything validation depends on.
function grid_signature(level) {
  var parts = [level.width + "x" + level.height];
  for (var y = 0; y < level.height; y++) {
    parts.push(level.rows[y].join(""));
  }
  for (var m = 0; m < level.meta.length; m++) {
    parts.push(level.meta[m][0] + "=" + level.meta[m][1]);
  }
  return parts.join("\n");
}

/// Note that the grid changed: the level is unsaved and its validation is stale.
function touch() {
  state.dirty = true;
  state.signature = grid_signature(state.level);
}

/// A blank level: the default metadata and an empty grid with a floor to build on.
/// Bring the structural report and the reachability search up to date with the grid.
///
/// A grid change (see [`touch_at`]) re-validates the search in place — see
/// [`revalidate`] — so painting does not replay the level. `force` (a load, a new
/// level, a resize, or the panel's Recheck) restarts the sweep instead.
function refresh_search(force) {
  var level = state.level;
  if (level === null) {
    state.report = null;
    state.search = null;
    return;
  }
  state.report = structural_report(level, state.report ? state.report.warnings : null);
  var signature = grid_signature(level);
  var search = state.search;
  var incremental =
    !force &&
    search !== null &&
    search.width === level.width &&
    search.height === level.height;
  if (incremental) {
    if (CHANGE.any) {
      revalidate(level, search, CHANGE);
    }
  } else {
    state.search = new_search(level);
  }
  CHANGE.any = false;
  state.search.signature = signature;
  state.signature = signature;
}

/// Give the reachability search a slice of simulation.
function pump_search(slice) {
  var level = state.level;
  var search = state.search;
  if (level === null || search === null) {
    return;
  }
  // An edit is re-validated when the stroke ends (`handle_pointer`); until then the
  // sweep would be advancing records a newer grid no longer describes.
  if (CHANGE.any) {
    return;
  }
  advance_search(level, search, slice);
}

/// The report [`current_report`] fills in place.
///
/// Filled, not rebuilt: a freshly built object handed back every frame is a young
/// map landing in a long-lived slot, which Slag's verifier reports as a missing
/// write barrier (see [`warm_shapes`]). Its lists are reused too, and only ever
/// taken from the same long-lived structural report or filled with numbers, so
/// nothing young is ever stored into them.
var REPORT = {
  errors: [],
  warnings: [],
  marks: [],
  fresh: false,
  search: null,
  settled: false,
  reachedGoal: false,
  unreachableCoins: []
};

/// The structural checks plus the live search result, merged. Fills [`REPORT`].
function current_report() {
  var level = state.level;
  if (level === null) {
    return null;
  }
  var search = state.search;
  REPORT.errors = state.report ? state.report.errors : REPORT.errors;
  REPORT.warnings = state.report ? state.report.warnings : REPORT.warnings;
  REPORT.marks = state.report ? state.report.marks : REPORT.marks;
  REPORT.fresh = state.report !== null && search !== null && search.signature === state.signature;
  REPORT.search = search;
  // A search only speaks for the grid it ran on, and only once it finished: a
  // partial sweep must not call a reachable coin unreachable.
  REPORT.settled = REPORT.fresh && search.done && !search.truncated;
  REPORT.reachedGoal = REPORT.fresh && search.goal;
  REPORT.unreachableCoins.length = 0;
  if (REPORT.settled) {
    for (var i = 0; i < level.coins.length; i++) {
      var coin = level.coins[i];
      if (search.coins[coin[1] * level.width + coin[0]] !== 1) {
        REPORT.unreachableCoins.push(i);
      }
    }
  }
  return REPORT;
}

/// The standing cells the search reached, as `{ count, minX, maxX, minY, maxY }`.
function reached_span(search, level) {
  var count = 0;
  var minX = 1e9;
  var maxX = -1e9;
  var minY = 1e9;
  var maxY = -1e9;
  for (var cell = 0; cell < search.reached.length; cell++) {
    if (search.reached[cell] === 0) {
      continue;
    }
    count++;
    var tx = cell % level.width;
    var ty = (cell - tx) / level.width;
    if (tx < minX) {
      minX = tx;
    }
    if (tx > maxX) {
      maxX = tx;
    }
    if (ty < minY) {
      minY = ty;
    }
    if (ty > maxY) {
      maxY = ty;
    }
  }
  return { count: count, minX: minX, maxX: maxX, minY: minY, maxY: maxY };
}

/// The search's state in one phrase, for a status line or a log.
function search_phrase() {
  var report = current_report();
  var level = state.level;
  if (report === null || level === null || report.search === null) {
    return "not checked";
  }
  var search = report.search;
  if (!search.seeded) {
    return "no spawn to search from";
  }
  var span = reached_span(search, level);
  var where =
    "reached " +
    span.count +
    " cells (x " +
    span.minX +
    "-" +
    span.maxX +
    ", y " +
    span.minY +
    "-" +
    span.maxY +
    ") over " +
    search.spent +
    " arcs";
  if (!search.done) {
    return "validating… " + where;
  }
  if (search.truncated) {
    return "partial: the search hit its " + SEARCH.limit + "-arc budget — " + where;
  }
  if (!search.goal) {
    return "no route found to the goal (the sweep is a conservative approximation) — " + where;
  }
  if (report.unreachableCoins.length > 0) {
    return report.unreachableCoins.length + " coin(s) out of reach — " + where;
  }
  return "playable — " + where;
}

/// The verdict in a few words: the viewport banner and the status line.
function verdict_short() {
  var report = current_report();
  var level = state.level;
  if (report === null || level === null) {
    return "no level";
  }
  if (report.errors.length > 0) {
    return "INVALID — " + report.errors[0];
  }
  var search = report.search;
  if (search === null || !search.seeded) {
    return "not checked";
  }
  if (!search.done) {
    return "validating…";
  }
  if (search.truncated) {
    return "partial verdict (search budget)";
  }
  if (!search.goal) {
    return "no route found (conservative sweep)";
  }
  if (report.unreachableCoins.length > 0) {
    return report.unreachableCoins.length + " coin(s) out of reach";
  }
  return "playable";
}

/// A note naming the coins the search could not reach, for a log line.
function unreachable_note() {
  var report = current_report();
  var level = state.level;
  if (report === null || level === null || report.unreachableCoins.length === 0) {
    return "";
  }
  var parts = [];
  for (var i = 0; i < report.unreachableCoins.length; i++) {
    var coin = level.coins[report.unreachableCoins[i]];
    parts.push(coin[0] + "," + coin[1]);
  }
  return " (unreachable coins: " + parts.join(" ") + ")";
}

/// A one-line summary of the level and its state.
function status_line() {
  var level = state.level;
  if (level === null) {
    return "no level loaded (slots: " + noctis.files.slots().join(", ") + ")";
  }
  var parts = [
    state.slot,
    level.width + "x" + level.height,
    level.coins.length + " coins",
    level.solids + " tiles",
    verdict_short()
  ];
  if (state.dirty) {
    parts.push("unsaved");
  }
  return parts.join(" | ");
}

// ---------------------------------------------------------------------------
// Loading and saving through the declared file slots
// ---------------------------------------------------------------------------

function slot_of(name) {
  var slots = noctis.files.slots();
  var wanted = String(name === undefined || name === null ? "" : name).toLowerCase();
  for (var i = 0; i < slots.length; i++) {
    if (slots[i].toLowerCase() === wanted) {
      return slots[i];
    }
  }
  // A bare number addresses `levelN`.
  if (/^[0-9]+$/.test(wanted)) {
    var guess = "level" + wanted;
    for (var j = 0; j < slots.length; j++) {
      if (slots[j].toLowerCase() === guess) {
        return slots[j];
      }
    }
  }
  return null;
}

function load_slot(name) {
  var slot = slot_of(name);
  if (slot === null) {
    noctis.log("level: no such slot '" + name + "' (slots: " + noctis.files.slots().join(", ") + ")");
    return;
  }
  var text = noctis.files.read(slot);
  if (text === undefined) {
    noctis.log(
      "level: cannot read '" +
        slot +
        "' — is a project open? (run: noctis editor --project examples/reverie)"
    );
    return;
  }
  var parsed = parse_level(text);
  if (parsed.errors) {
    noctis.log("level: " + slot + " did not parse: " + parsed.errors.join("; "));
    return;
  }
  state.slot = slot;
  state.level = parsed.level;
  state.dirty = false;
  state.report = { errors: [], warnings: parsed.warnings, marks: [] };
  state.signature = grid_signature(parsed.level);
  // The size controls start from the level just loaded, so `Resize` is a nudge
  // from where the level already is.
  state.newW = parsed.level.width;
  state.newH = parsed.level.height;
  sync_metadata_menus(parsed.level);
  state.view.zoom = 1;
  state.view.panX = 0;
  state.view.panY = 0;
  refresh_search(true);
  // Give the search a start here so a path with no frames to pump it (the CLI)
  // still reports something; the editor finishes it over the next frames.
  search_eagerly(state.level, state.search, EAGER_ARCS);
  noctis.log(
    "reverie level: loaded " +
      slot +
      " — " +
      status_line() +
      " | " +
      search_phrase() +
      unreachable_note()
  );
}

function save_level() {
  if (state.level === null || state.slot === null) {
    noctis.log("level: nothing to save");
    return "error: no level loaded";
  }
  var report = current_report();
  if (report.errors.length > 0) {
    return "error: refusing to save a level the game would reject: " + report.errors[0];
  }
  var text = serialize_level(state.level);
  if (noctis.files.write(state.slot, text) !== true) {
    return "error: could not write " + state.slot + " (is a project open?)";
  }
  state.dirty = false;
  noctis.log(
    "reverie level: saved " +
      state.slot +
      " (" +
      text.length +
      " bytes). The game embeds levels/*.txt at compile time — rebuild to play it."
  );
  return "ok saved " + state.slot;
}

// ---------------------------------------------------------------------------
// The viewport
// ---------------------------------------------------------------------------

/// Fit the level into the viewport, applying the view's zoom and pan.
function layout_view(level, size, view) {
  var margin = 10;
  var header = 24;
  var availableW = Math.max(size.width - margin * 2, 32);
  var availableH = Math.max(size.height - header - margin, 32);
  var fit = Math.min(availableW / level.width, availableH / level.height);
  view.cell = Math.max(2, fit * view.zoom);
  view.ox = margin + (availableW - level.width * view.cell) / 2 + view.panX;
  view.oy = header + (availableH - level.height * view.cell) / 2 + view.panY;
}

function clamp(value, low, high) {
  return Math.min(Math.max(value, low), high);
}

function hovered_cell(p, view) {
  if (!(view.cell > 0)) {
    return null;
  }
  return {
    tx: Math.floor((p.x - view.ox) / view.cell),
    ty: Math.floor((p.y - view.oy) / view.cell)
  };
}

function in_bounds(level, tx, ty) {
  return tx >= 0 && ty >= 0 && tx < level.width && ty < level.height;
}

/// Paint one cell. Spawn and goal are unique, so painting them moves the old one.
function paint_cell(level, tx, ty, brush) {
  if (!in_bounds(level, tx, ty)) {
    return false;
  }
  if (level.rows[ty][tx] === brush) {
    return false;
  }
  if (brush === "S" || brush === "F") {
    var existing = brush === "S" ? level.spawns : level.goals;
    for (var i = 0; i < existing.length; i++) {
      level.rows[existing[i][1]][existing[i][0]] = ".";
    }
  }
  level.rows[ty][tx] = brush;
  index_level(level);
  touch();
  touch_at(tx, ty);
  return true;
}

/// Handle the pointer over the viewport: zoom, pan and paint.
function handle_pointer(p, level, view) {
  if (p.inside && p.wheel !== 0) {
    var anchorTx = (p.x - view.ox) / view.cell;
    var anchorTy = (p.y - view.oy) / view.cell;
    view.zoom = clamp(view.zoom * Math.exp(-p.wheel * 0.0016), 0.35, 12);
    layout_view(level, { width: view.width, height: view.height }, view);
    view.panX += p.x - anchorTx * view.cell - view.ox;
    view.panY += p.y - anchorTy * view.cell - view.oy;
  }

  if (p.inside && p.down) {
    if (p.shift) {
      view.panX += p.dx;
      view.panY += p.dy;
      return;
    }
    var cell = hovered_cell(p, view);
    var key = cell === null ? null : cell.tx + "," + cell.ty;
    if (cell !== null && key !== state.lastCell && in_bounds(level, cell.tx, cell.ty)) {
      state.lastCell = key;
      paint_cell(level, cell.tx, cell.ty, state.brush);
    }
    state.painting = true;
    return;
  }

  if (p.inside && p.right_down) {
    var erase = hovered_cell(p, view);
    if (erase !== null && in_bounds(level, erase.tx, erase.ty)) {
      var eraseKey = erase.tx + "," + erase.ty;
      if (eraseKey !== state.lastCell) {
        state.lastCell = eraseKey;
        paint_cell(level, erase.tx, erase.ty, ".");
      }
    }
    state.painting = true;
    return;
  }

  // A release ends the gesture: re-validate the search on the finished edit —
  // incrementally, so only the arcs the paint could have been read by are replayed.
  if (!p.down && !p.right_down) {
    if (state.painting) {
      state.painting = false;
      refresh_search(false);
    }
    state.lastCell = null;
  }
}

function paint_viewport(vui, p, size) {
  var level = state.level;
  vui.rect(0, 0, size.width, size.height, COLORS.backdrop);
  if (level === null) {
    vui.text(14, 14, "Reverie level editor: no level loaded", COLORS.warn, 14);
    vui.text(14, 34, "open a project with the extension's level slots", COLORS.dim, 12);
    return;
  }

  var view = state.view;
  view.width = size.width;
  view.height = size.height;
  layout_view(level, size, view);
  if (p.inside) {
    vui.capture();
  }
  handle_pointer(p, level, view);

  var report = current_report();
  var search = report.search;
  var cell = view.cell;
  var ox = view.ox;
  var oy = view.oy;
  var fieldW = level.width * cell;
  var fieldH = level.height * cell;

  // The field, and a border that carries the playability verdict.
  var edge = COLORS.edge;
  if (report.errors.length > 0) {
    edge = COLORS.error;
  } else if (!report.settled) {
    edge = COLORS.dim;
  } else if (!report.reachedGoal) {
    edge = COLORS.unreachable;
  } else if (report.unreachableCoins.length > 0) {
    edge = COLORS.warn;
  } else {
    edge = COLORS.good;
  }
  vui.rect(ox, oy, fieldW, fieldH, COLORS.field);
  vui.stroke(ox - 1, oy - 1, fieldW + 2, fieldH + 2, edge, 2);

  // The standing cells the search reached, behind the tiles (batched per row).
  if (state.showReach && report.fresh && search.seeded) {
    for (var sty = top; sty < bottom; sty++) {
      var seat = sty * level.width;
      var sx = left;
      while (sx < right) {
        if (search.reached[seat + sx] === 0) {
          sx++;
          continue;
        }
        var tintTo = sx + 1;
        while (tintTo < right && search.reached[seat + tintTo] === 1) {
          tintTo++;
        }
        vui.rect(ox + sx * cell, oy + sty * cell, (tintTo - sx) * cell, cell, COLORS.reach);
        sx = tintTo;
      }
    }
  }

  // The tiles. A run of the same solid tile is one rectangle: a level is mostly
  // floors, and one op per tile would be hundreds of ops a frame (each an object
  // the recording marshals — see the note on allocation in `paint_viewport`'s
  // callers, and the Slag write-barrier note in `warm_shapes`).
  var left = clamp(Math.floor((0 - ox) / cell), 0, level.width);
  var right = clamp(Math.ceil((size.width - ox) / cell), 0, level.width);
  var top = clamp(Math.floor((0 - oy) / cell), 0, level.height);
  var bottom = clamp(Math.ceil((size.height - oy) / cell), 0, level.height);
  var tile = "";
  var run = 0;
  var inset = 0;
  for (var ty = top; ty < bottom; ty++) {
    var rowY = oy + ty * cell;
    var tx = left;
    while (tx < right) {
      tile = level.rows[ty][tx];
      if (tile === "G" || tile === "P") {
        run = tx + 1;
        while (run < right && level.rows[ty][run] === tile) {
          run++;
        }
        vui.rect(
          ox + tx * cell,
          rowY,
          (run - tx) * cell,
          cell,
          tile === "G" ? COLORS.ground : COLORS.platform
        );
        tx = run;
        continue;
      }
      var x = ox + tx * cell;
      if (tile === "C") {
        inset = Math.max(1, cell * 0.28);
        vui.rect(x + inset, rowY + inset, cell - inset * 2, cell - inset * 2, COLORS.coin);
      } else if (tile === "S" || tile === "F") {
        vui.rect(x, rowY, cell, cell, tile === "S" ? COLORS.spawn : COLORS.goal);
        if (cell >= 15) {
          // A whole point, not `cell * 0.8`: egui bakes every distinct size into
          // its font atlas and keeps it, so a zoom would mint one per step.
          vui.text(x + 3, rowY + 1, tile, COLORS.field, 14);
        }
      }
      tx++;
    }
  }

  if (state.showGrid && cell >= 5) {
    for (var gx = 0; gx <= level.width; gx++) {
      vui.line(ox + gx * cell, oy, ox + gx * cell, oy + fieldH, COLORS.grid, 1);
    }
    for (var gy = 0; gy <= level.height; gy++) {
      vui.line(ox, oy + gy * cell, ox + fieldW, oy + gy * cell, COLORS.grid, 1);
    }
  }

  // Coins the search could not reach.
  for (var u = 0; u < report.unreachableCoins.length; u++) {
    var coin = level.coins[report.unreachableCoins[u]];
    var cx = ox + coin[0] * cell;
    var cy = oy + coin[1] * cell;
    vui.line(cx + 2, cy + 2, cx + cell - 2, cy + cell - 2, COLORS.unreachable, 2);
    vui.line(cx + cell - 2, cy + 2, cx + 2, cy + cell - 2, COLORS.unreachable, 2);
  }

  // Cells the structural checks flagged.
  for (var m = 0; m < report.marks.length; m++) {
    var mark = report.marks[m];
    vui.stroke(
      ox + mark.x * cell,
      oy + mark.y * cell,
      cell,
      cell,
      COLORS.unreachable,
      2
    );
  }

  // The route the search found.
  if (state.showRoute && report.fresh && search.route !== null) {
    var routeX = search.route.xs;
    var routeY = search.route.ys;
    var scale = cell / TILE;
    for (var r = 1; r < routeX.length; r++) {
      vui.line(
        ox + (routeX[r - 1] + HITBOX_W / 2) * scale,
        oy + (routeY[r - 1] + HITBOX_H) * scale,
        ox + (routeX[r] + HITBOX_W / 2) * scale,
        oy + (routeY[r] + HITBOX_H) * scale,
        COLORS.route,
        2
      );
    }
  }

  // The hovered cell, so the brush is predictable.
  if (p.inside) {
    var hover = hovered_cell(p, view);
    if (hover !== null && in_bounds(level, hover.tx, hover.ty)) {
      vui.stroke(ox + hover.tx * cell, oy + hover.ty * cell, cell, cell, COLORS.hover, 1);
    }
  }

  // The header line and the verdict banner.
  var verdictColor = COLORS.good;
  if (report.errors.length > 0) {
    verdictColor = COLORS.error;
  } else if (!report.settled) {
    verdictColor = COLORS.dim;
  } else if (!report.reachedGoal) {
    verdictColor = COLORS.unreachable;
  } else if (report.unreachableCoins.length > 0) {
    verdictColor = COLORS.warn;
  }

  vui.text(
    12,
    4,
    (state.slot || "-") +
      "  " +
      level.width +
      "x" +
      level.height +
      "  brush " +
      state.brush +
      (state.dirty ? "  (unsaved)" : ""),
    COLORS.text,
    13
  );
  vui.text(12, size.height - 20, verdict_short(), verdictColor, 14);
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

function draw_panel(ui) {
  ui.heading("Reverie level");

  var level = state.level;
  if (level === null) {
    ui.label("No level loaded.");
    var emptySlots = noctis.files.slots();
    for (var i = 0; i < emptySlots.length; i++) {
      if (ui.button("Load " + emptySlots[i])) {
        load_slot(emptySlots[i]);
      }
    }
    if (ui.button("New level")) {
      state.slot = emptySlots.length > 0 ? emptySlots[0] : null;
      new_level(state.newW, state.newH);
    }
    state.active = ui.checkbox("Show in the viewport", state.active);
    return;
  }

  var report = current_report();
  ui.label(state.slot + (state.dirty ? "  (unsaved)" : ""));
  ui.label(status_line());
  ui.separator();

  // The level's settings come first so the controls a level is edited through stay
  // in view: a dock's height is capped (a long panel must not starve the viewport),
  // so only the growing lists below are ever scrolled out of sight.

  // The size: the sliders (quantized so a size can be hit) drive both the `New`
  // button below and `Resize` here, and start from the level just loaded.
  state.newW = quantize(ui.slider("Width", state.newW, MIN_WIDTH, MAX_WIDTH), 4);
  state.newH = quantize(ui.slider("Height", state.newH, MIN_HEIGHT, MAX_HEIGHT), 1);
  ui.label("size: " + state.newW + " x " + state.newH);
  if (ui.button("Resize to " + state.newW + "x" + state.newH)) {
    noctis.run("level resize " + state.newW + " " + state.newH);
  }
  ui.separator();

  // The level's music and background: exactly the names the game's own tables
  // know (a name it does not know stops the game when it loads the level), chosen
  // from a menu of them. A change here is a user action — the menus start from the
  // level's own names (`sync_metadata_menus`), so drawing the panel never rewrites
  // a level that names something else.
  var music = ui.select("Music", state.musicIndex, MUSIC_NAMES);
  if (music !== state.musicIndex) {
    choose_metadata(level, "music", MUSIC_NAMES[music]);
  }
  var background = ui.select("Background", state.backgroundIndex, BACKGROUND_NAMES);
  if (background !== state.backgroundIndex) {
    choose_metadata(level, "background", BACKGROUND_NAMES[background]);
  }
  ui.separator();

  // The declared level slots, and the level's own actions.
  var names = noctis.files.slots();
  for (var s = 0; s < names.length; s++) {
    if (ui.button((names[s] === state.slot ? "• " : "") + "Load " + names[s])) {
      load_slot(names[s]);
    }
  }
  if (ui.button("Reload from disk")) {
    load_slot(state.slot);
  }
  if (ui.button("New " + state.newW + "x" + state.newH)) {
    new_level(state.newW, state.newH);
  }
  if (ui.button("Save")) {
    noctis.run("level save");
  }
  if (ui.button("Recheck reachability")) {
    refresh_search(true);
  }
  ui.separator();

  // The brushes.
  for (var b = 0; b < BRUSHES.length; b++) {
    var mark = BRUSHES[b].key === state.brush ? "• " : "";
    if (ui.button(mark + BRUSHES[b].label)) {
      state.brush = BRUSHES[b].key;
    }
  }
  ui.separator();

  // The checkboxes are stateful: the host returns the value the user set.
  state.showGrid = ui.checkbox("Grid", state.showGrid);
  state.showReach = ui.checkbox("Reachable cells", state.showReach);
  state.showRoute = ui.checkbox("Route", state.showRoute);
  state.active = ui.checkbox("Show in the viewport", state.active);
  ui.separator();

  // The findings, last so no button's id can shift under a click.
  var shown = 0;
  for (var e = 0; e < report.errors.length && shown < 6; e++, shown++) {
    ui.label("error: " + report.errors[e]);
  }
  for (var w = 0; w < report.warnings.length && shown < 6; w++, shown++) {
    ui.label("warning: " + report.warnings[w]);
  }
  var search = report.search;
  if (search !== null && search.seeded) {
    ui.label(search_phrase());
    ui.label(report.reachedGoal ? "goal reachable" : "goal not reached (so far)");
    ui.label(report.unreachableCoins.length + " coin(s) out of reach");
  }
  ui.label("left: paint  right: erase  wheel: zoom  shift+drag: pan");
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Every frame the reachability search gets a slice of simulation, so a large
/// level settles while the editor stays responsive.
noctis.on("update", function () {
  pump_search(SEARCH.slice);
});

noctis.ui.viewport(function (vui) {
  if (!state.active) {
    return false;
  }
  paint_viewport(vui, vui.pointer(), vui.size());
  return true;
});

noctis.ui.panel("Reverie Level", draw_panel);

noctis.command("level", function (parts) {
  var arg = (parts[1] || "").toLowerCase();
  if (arg === "" || arg === "status") {
    return "ok " + status_line();
  }
  if (arg === "save") {
    return save_level();
  }
  if (arg === "check") {
    // An explicit check earns a full sweep; a path with no frames to pump the
    // search (the CLI) is why this runs it here rather than hinting to wait.
    refresh_search(true);
    search_eagerly(state.level, state.search, SEARCH.limit);
    return "ok " + status_line() + " | " + search_phrase() + unreachable_note();
  }
  if (arg === "reload") {
    load_slot(state.slot);
    return "ok " + status_line();
  }
  if (arg === "new") {
    // `level new`, `level new <w> <h>`, or `level new <slot>`.
    var newW = state.newW;
    var newH = state.newH;
    var rest = 2;
    if (/^[0-9]+$/.test(parts[2] || "") && /^[0-9]+$/.test(parts[3] || "")) {
      newW = Number(parts[2]);
      newH = Number(parts[3]);
      rest = 4;
    }
    state.slot = slot_of(parts[rest]) || state.slot;
    new_level(newW, newH);
    search_eagerly(state.level, state.search, EAGER_ARCS);
    return "ok new " + state.level.width + "x" + state.level.height + " | " + status_line();
  }
  if (arg === "resize") {
    var resized = resize_to(parts[2], parts[3]);
    if (resized.indexOf("error") === 0) {
      return resized;
    }
    search_eagerly(state.level, state.search, EAGER_ARCS);
    return resized + " | " + status_line();
  }
  if (arg === "music" || arg === "background") {
    var catalog = arg === "music" ? MUSIC_NAMES : BACKGROUND_NAMES;
    var wanted = (parts[2] || "").toLowerCase();
    if (!named_in(catalog, wanted)) {
      return "error: " + arg + " must be one of " + catalog.join(", ");
    }
    choose_metadata(state.level, arg, wanted);
    return "ok " + arg + " " + wanted + " | " + status_line();
  }
  if (arg === "paint") {
    // The scripted form of the pointer: `level paint <tx> <ty> <char>`.
    if (state.level === null) {
      return "error: no level loaded";
    }
    var painted = String(parts[4] || "").toUpperCase();
    if (!paint_cell(state.level, Number(parts[2]), Number(parts[3]), painted)) {
      return "error: nothing painted at " + parts[2] + "," + parts[3];
    }
    // The pointer re-validates on release; a scripted paint has no release.
    refresh_search(false);
    search_eagerly(state.level, state.search, SEARCH.limit);
    return (
      "ok painted " + parts[2] + "," + parts[3] + " " + painted + " | " +
      status_line() + " | " + search_phrase() + unreachable_note()
    );
  }
  if (arg === "brush") {
    var wanted = (parts[2] || "").toUpperCase();
    if (KNOWN_CHARS[wanted] !== true) {
      return "error: brush must be one of " + Object.keys(KNOWN_CHARS).join(" ");
    }
    state.brush = wanted;
    return "ok brush " + wanted;
  }
  var slot = slot_of(parts[1]);
  if (slot === null) {
    return "error: unknown level '" + parts[1] + "' (slots: " + noctis.files.slots().join(", ") + ")";
  }
  load_slot(slot);
  return "ok " + status_line();
});

noctis.on("ready", function () {
  var slots = noctis.files.slots();
  if (slots.length === 0) {
    noctis.log(
      "reverie level: no level slots — run the editor with --project examples/reverie"
    );
    return;
  }
  load_slot(slots[0]);
});
