# Script composite values: arrays, not strings

**Status: shipped.** The Slag API below landed (rev `e0d2b847`); the JavaScript
getters return arrays, the six scripts and the docs are migrated, and the suite
is green. This note is the design contract for the change and now a record of
it; one latent script bug surfaced during migration (see *Found while
migrating*).

## Motivation

Every JavaScript composite getter serializes on the engine side and parses on the
script side, once per scripted entity, per frame:

- **Engine** (`crates/script/src/slag.rs`): `get_position`, `get_rotation_euler`,
  `get_scale`, `get_velocity` build `"[x,y,z]"` with `vec_json`; `mouse_move`,
  `mouse_position`, `viewport_size` build `"[x,y]"` with `format!`. Each call
  allocates a Rust `String` **and** a Slag `JsValue::string` (a heap copy).
- **Script**: the `vec3`/`pair` helper does
  `String(raw).slice(1, -1).split(",").map(Number)` — a coerced string, a slice,
  a three-element string array, a mapped number array, plus the parse.

The other two backends are already native, so this is a **JavaScript-only** gap:

- the Rust path (`ScriptApi::get_position`/`get_rotation_euler`/`get_scale`/
  `get_velocity` in `crates/script/src/api.rs`) returns `math::Vec3` directly;
- the WebAssembly ABI already returns the vector getters as **multi-value floats**
  `(i32) -> (f32, f32, f32)` (see the table in `crates/script/src/wasm_host.rs`).

Measured (native, the 8 showcase scripts, one entity each, steady-state update
loop): the frame is ~167 allocations / ~11.9 KB after the recent reflection and
`eval`/source-clone fixes (commits `a7ff194`, `c5938a9`, `100af37`). The
string round-trip is on the order of a quarter of that count — the exact figure
to be re-measured after this lands. This closes the last structural source of
per-frame allocation on the scripting path, and removes the JS/wasm asymmetry.

## The convention (decided)

**A read that returns a fixed-shape tuple of numbers returns a real array.**
**A read that returns open-ended or heterogeneous structure stays JSON.**

So the vector/pair getters become arrays; `get_component` (and the editor's
`inspect`/`list_entities`) keep returning JSON, because their shape is the
component's or an entity list's, not a tuple we can name. This is the same rule
the WebAssembly ABI already follows (floats for vectors, JSON over memory for
components).

## Part 1 — The Slag API request (blocker)

Slag's embed API (`crates/runtime/src/embed.rs`) can build an ordinary **object**
(`FunctionCall::create_object` / `Context::create_object`, `JsObject::set`), but it
**cannot build a plain Array**: `Context::array_push` needs an array to start
from, and `builtins::array::array_from_values` is crate-private. A host function
that wants to return a vector therefore has no way to return it as a value and
must serialize it. That is the gap.

### Requested

Add, matching the existing `create_object` shape:

```rust
// impl FunctionCall — the re-entrant surface a host function runs in; this is
// the one the Noctis script host actually calls.
pub fn create_array(&self, values: &[JsValue]) -> Result<JsValue, JsError>;

// impl Context — for symmetry with Context::create_object.
pub fn create_array(&self, values: &[JsValue]) -> Result<JsValue, JsError>;
```

### Semantics

- Returns a fresh, **dense, ordinary Array** with `%Array.prototype%` as its
  prototype; `length == values.len()`; element *i* is an own data property in
  insertion order.
- `Array.isArray(result) === true` — **required**: the scripts branch on it.
- Allocates only the array and its elements; no serialization, no intermediate
  string.
- **Re-entrant** via `FunctionCall::create_array`: callable from inside a
  `HostFn` while the enclosing script is suspended — the same window
  `FunctionCall::create_object` already runs in — so no handle is held across a
  collection (the array is returned to the script and consumed immediately).
- Errors surface as `Err(JsError)`, mirroring `create_object`/`create_function`.

### Implementation hint

The body already exists: `builtins::array::array_from_values(agent, &[Value])`
(`crates/runtime/src/builtins/array.rs:302`) creates exactly this array for the
`fs.readdirSync`/`process` shims. The new method is a public, re-entrant wrapper
over it, converting `&[JsValue]` to `&[Value]` (as `FunctionCall::call` already
does for its arguments).

### Not requested (deliberately)

A numeric convenience such as `create_number_array(&[f64])`. We would use it, but
`JsValue::number` is allocation-free, so building the three `JsValue`s first costs
nothing and a general `create_array` is more broadly useful. Skip it unless it
falls out.

### Return

We pin Slag by git rev in `Cargo.toml` (`github.com/sdgoij/slag`); this lands as a
new rev, and the engine bumps it.

## Part 2 — Engine change (JavaScript getters)

All in `crates/script/src/slag.rs`, inside `install_api`:

| getter | before | after |
| --- | --- | --- |
| `get_position(entity)` | `"[x,y,z]"` | `[x, y, z]` |
| `get_rotation_euler(entity)` | `"[roll,pitch,yaw]"` | `[roll, pitch, yaw]` |
| `get_scale(entity)` | `"[x,y,z]"` | `[x, y, z]` |
| `get_velocity(entity)` | `"[x,y,z]"` | `[x, y, z]` |
| `mouse_move()` | `"[dx,dy]"` | `[dx, dy]` |
| `mouse_position()` | `"[x,y]"` | `[x, y]` |
| `viewport_size()` | `"[w,h]"` | `[w, h]` |

- Each becomes `call.create_array(&[JsValue::number(x as f64), ...])`.
- An absent/bad handle keeps returning **`undefined`** (unchanged) so
  `if (camera !== undefined)` style guards still work; drop the string path.
- `vec_json` is removed; `JsValue::string` remains only for `log`/`get_component`.
- **No change to `bridge.rs`**: the snapshot already stores `Vec3`; only the JS
  closures serialized it. **No change to `api.rs` or `wasm_host.rs`** (already
  native), beyond a doc note that the JS getters now match the wasm ABI.
- The module-doc caveat at the top of `slag.rs` ("Vector getters … return a JSON
  string … because the minimal embedding surface … does not describe composite
  value construction") is deleted — the gap is closed.

## Part 3 — Script migration (clean break)

No compatibility shim: the engine returns arrays only, and every call site moves.
The scripts' helpers are `Array.isArray(raw) ? raw : String(raw)…`, so the parse
branch is dead weight once the engine changes — delete the helpers and use the
array directly.

- `examples/showcase/scripts/bobber.js` — drop `vec3`; `get_position` → array.
- `examples/showcase/scripts/spawner.js` — drop `vec3`.
- `examples/showcase/scripts/spin2d.js` — drop `vec3` (`get_rotation_euler`).
- `examples/showcase/scripts/spinner.js` — drop `vec3` (`get_rotation_euler`).
- `examples/showcase/scripts/menu.js` — drop `pair` (`mouse_position`,
  `viewport_size`).
- `examples/showcase/scripts/third_person.js` — drop `pair`/`vec3`
  (`mouse_move`, `get_position`).

The `vec3`/`pair` helper is duplicated across six scripts; removing them is a
bonus of this change. (`set_position`/`translate`/`set_rotation_euler`/
`set_scale`/`set_velocity`/`add_velocity`/`apply_impulse` keep their scalar
arguments — only the reads change.)

## Part 4 — Documentation

- `crates/script/src/slag.rs` — the module-doc caveat (Part 2).
- `AGENTS.md` — the `script` known-limitations paragraph (the "Vector getters
  return a JSON string" caveat).
- `.notes/controls.md` — the documented surface (`mouse_move() -> "[dx, dy]"`,
  etc.) and the physics-read lines.
- `.notes/embedding.md` — the reflection section ("the JavaScript boundary is
  string/number based … vector getters already return `"[x,y,z]"` strings" and the
  dedicated-bindings list).
- `examples/showcase/README.md` — the API summary and the `vec3` note.
- `.notes/script-values.md` — this note, flipped from *proposed* to *shipped*.

## Part 5 — Editor extensions

By the convention above, the editor-extension reads follow the same rule.
`.notes/extensions.md` said reads are snapshots returning "JSON strings (the
pattern the script API already uses for composite values)". Of those:

- `noctis.editor.selection()` — "a JSON array of entity refs": a homogeneous list,
  so it should return a **real array**.
- `noctis.editor.list_entities()` and `noctis.editor.inspect(handle)` — structured
  records; **stay JSON** (the `get_component` case).
- `editor_json` (the WebAssembly extension import) — a structured record buffer;
  **stays** as-is.

Shipped: `noctis.editor.selection()` returns the array directly — the bootstrap's
`_reads()` already parses the snapshot, so it was a one-line change in the
`BOOTSTRAP` JS (no host or Slag change). The entity list and `inspect` stay JSON.
`.notes/extensions.md` and the `examples/extensions` scatter extension are updated.

## Found while migrating

The clean break surfaced a latent bug in `examples/showcase/scripts/spawner.js`:
it keeps the handles from `spawn`/`instantiate_prefab` in a `spawned` array and
reads them on later frames. Handles are **per-call** indices into a table the
host clears at the start of every call (by design — see `AGENTS.md`), so a handle
stored last frame is meaningless this frame: `get_position` returns `undefined`
(or, worse, a *different* entity). The old `vec3(undefined)` string-parse swallowed
this as `NaN`, so the kill-plane check silently never fired; direct indexing now
throws. The migration guards the read (`spawned[i]` position `!== undefined`),
which makes it not crash — but the kill plane still does not actually work.

Fixing it properly needs a **stable cross-frame entity reference** the script API
does not have today (`entity` is stable — it is always handle `0` — but anything
from `spawn`/`find_entity` is per-call). That is a separate feature; file it, do
not fold it into this change.

## Out of scope

- `get_component`/`set_component` and reflection JSON: inherently structured.
- The WebAssembly ABI vector getters: already multi-value floats.
- The Rust `ScriptApi`: already `Vec3`.
- The *setters*: scalar arguments stay.
- `websocket`/`fs`-style typed arrays in Slag: unrelated.

## Phasing

1. **P0 — Slag.** Add `create_array` (both surfaces); bump the pinned rev in
   `Cargo.toml`; a smoke test.
2. **P1 — engine.** Rewrite the seven getters; remove `vec_json`; update the
   `slag.rs` docs and its `mouse_reads_reach_a_real_script` test.
3. **P2 — scripts.** Migrate the six showcase scripts; rebuild the `docs/` demo.
4. **P3 — docs.** The Part 4 list.
5. **P4 — extensions.** `selection()` returns an array; `.notes/extensions.md`
   updated. Done.

## Validation

- `cargo test --workspace` green; the `showcase_scripts` integration test runs the
  migrated scripts through the real engine (so a script that stops reading its
  vector fails).
- New unit tests: a getter returns an array (`Array.isArray`) with the right
  elements, and `undefined` for a bad handle.
- Re-measure the allocation probe (the getter share should be gone); the wasm ABI
  is unaffected.

## Open questions

1. Slag method name: `create_array` (matches `create_object`) — agreed?
2. Extensions (Part 5): **resolved** — `selection()` was the only read to
   convert; the rest are structured records.
3. Anything else returning a JSON string that this convention should move
   (`viewport_size` is in; anything I missed)?
