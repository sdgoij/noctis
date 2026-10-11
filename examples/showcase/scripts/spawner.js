// spawner.js — spawn physical crates the player can push around.
//
//   E  spawn a tumbling crate above the pad
//   R  instantiate the "Spawned Crate" prefab (inline JSON)
//   X  destroy everything this script created
//
// Each crate is given a cube mesh, a dynamic body with a little spin, and a
// collider through component reflection, so it falls, rolls, and can be shoved
// by the player. A lifetime script removes each crate after a while, and
// anything that falls off the floor is removed below KILL_Y.

const KILL_Y = -8.0; // crates below this are gone (they fell off the floor)
const LIFETIME = 25.0; // seconds before a crate removes itself

let eWasDown = false;
let rWasDown = false;
let xWasDown = false;

const spawned = [];

function on_start(entity, dt) {
  log("spawner ready: E = crate, R = prefab, X = clear");
}

function on_update(entity, dt) {
  const e = key_down("KeyE");
  if (e && !eWasDown) {
    const box = spawn("crate-spawn");
    const angle = (spawned.length * 1.7) % 6.28318;
    set_position(box, Math.cos(angle) * 1.2, 5.0, Math.sin(angle) * 1.2);
    make_physical(box);
    spawned.push(box);
    log("spawned crate " + spawned.length);
  }
  eWasDown = e;

  const r = key_down("KeyR");
  if (r && !rWasDown) {
    const root = instantiate_prefab(CRATE_PREFAB, entity);
    if (root !== undefined) {
      set_position(root, 3.0, 6.0, 3.0);
      make_physical(root);
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

  // Remove anything that has fallen off the floor. A stored handle stays valid
  // across frames, so this reads each crate's real height.
  for (let i = spawned.length - 1; i >= 0; i--) {
    if (get_position(spawned[i])[1] < KILL_Y) {
      destroy(spawned[i]);
      spawned.splice(i, 1);
    }
  }
}

// Give a spawned entity a cube mesh, a dynamic body, a collider, and a lifetime.
function make_physical(entity) {
  set_component(
    entity,
    "mesh_renderer",
    JSON.stringify({
      type: "mesh_renderer",
      mesh: "primitives/cube",
      material: "Copper",
      cast_shadows: true,
      receive_shadows: true,
    }),
  );
  set_component(
    entity,
    "rigid_body",
    JSON.stringify({
      type: "rigid_body",
      body_type: "dynamic",
      mass: 1.0,
      restitution: 0.2,
      friction: 0.7,
      gravity_scale: 1.0,
      linear_velocity: [0.0, 0.0, 0.0],
      angular_velocity: [0.4, 1.3, -0.3],
    }),
  );
  set_component(
    entity,
    "collider",
    JSON.stringify({
      type: "collider",
      shape: { kind: "cuboid", half_extents: [0.4, 0.4, 0.4] },
      offset: [0.0, 0.0, 0.0],
      is_sensor: false,
      friction: 0.7,
      restitution: 0.2,
    }),
  );
  add_script(entity, LIFETIME_SOURCE);
}

// A behaviour attached to each spawned crate: despawn after LIFETIME seconds.
// Module state is shared by every entity running the script, so ages are kept
// per handle.
const LIFETIME_SOURCE =
  "const ages = [];" +
  "function on_update(entity, dt) {" +
  "  ages[entity] = (ages[entity] || 0.0) + dt;" +
  "  if (ages[entity] > " +
  LIFETIME +
  ") { destroy(entity); }" +
  "}";

// A prefab as inline JSON — the same shape as a `prefabs/*.prefab.json` file.
const CRATE_PREFAB =
  '{"name":"crate-spawn","scene":{"name":"crate-spawn","format_version":1,"mode":"3d","entities":[' +
  '{"id":0,"name":"crate-spawn","transform":{"translation":[0,0.5,0],"rotation":[0,0,0,1],"scale":[1,1,1]},' +
  '"components":[' +
  '{"type":"mesh_renderer","mesh":"primitives/cube","material":"$default","cast_shadows":true,"receive_shadows":true}' +
  ']}]}}';
