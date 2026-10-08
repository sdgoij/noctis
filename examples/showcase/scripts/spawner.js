// spawner.js — spawn, destroy, attach behaviours, and instantiate a prefab.
//
// Press:
//   Space — spawn a spinning crate above the spawner
//   R     — instantiate the "Spawned Crate" prefab (inline JSON) as a child
//   X     — destroy everything this script created
//
// `key_down` is level-triggered (true for every frame the key is held), so the
// script tracks the previous frame to act on the *edge*.

let spaceWasDown = false;
let rWasDown = false;
let xWasDown = false;

const spawned = [];

function on_start(entity, dt) {
  log("spawner ready: Space = spawn, R = prefab, X = clear");
}

function on_update(entity, dt) {
  const space = key_down("Space");
  if (space && !spaceWasDown) {
    const box = spawn("spawned-crate");
    set_position(box, 0.0, 5.0 + spawned.length * 0.8, 0.0);
    add_script(box, SPINNER_SOURCE);
    spawned.push(box);
    log("spawned crate " + spawned.length);
  }
  spaceWasDown = space;

  const r = key_down("KeyR");
  if (r && !rWasDown) {
    const root = instantiate_prefab(CRATE_PREFAB, entity);
    if (root !== undefined) {
      set_position(root, 3.0, 0.5, 3.0);
      spawned.push(root);
    }
    log("instantiated the crate prefab");
  }
  rWasDown = r;

  const x = key_down("KeyX");
  if (x && !xWasDown) {
    while (spawned.length > 0) {
      destroy(spawned.pop());
    }
    destroy_by_name("crate-spawn");
    log("cleared spawned crates");
  }
  xWasDown = x;
}

// A behaviour attached to each spawned cube. `add_script` takes source text.
const SPINNER_SOURCE =
  "function on_update(entity, dt) {" +
  "  const v = vec3(get_rotation_euler(entity));" +
  "  set_rotation_euler(entity, v[0] + dt, v[1] + dt, v[2]);" +
  "}" +
  "function vec3(raw) {" +
  "  return Array.isArray(raw) ? raw : String(raw).slice(1, -1).split(',').map(Number);" +
  "}";

// A prefab as inline JSON — the same shape as a `prefabs/*.prefab.json` file.
const CRATE_PREFAB =
  '{"name":"crate-spawn","scene":{"name":"crate-spawn","format_version":1,"mode":"3d","entities":[' +
  '{"id":0,"name":"crate-spawn","transform":{"translation":[0,0.5,0],"rotation":[0,0,0,1],"scale":[1,1,1]},' +
  '"components":[' +
  '{"type":"mesh_renderer","mesh":"primitives/cube","material":"$default","cast_shadows":true,"receive_shadows":true}' +
  ']}]}}';
