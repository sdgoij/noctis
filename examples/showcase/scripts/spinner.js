// spinner.js — rotate the entity about its local Y axis.
//
// The Slag backend calls these as *bare global functions*:
//
//     function on_start(entity, dt)   // once, when the script first runs
//     function on_update(entity, dt)  // every frame
//     function on_destroy(entity, dt) // when the entity is removed
//
// There is no `api` object and no `export`. `entity` is a numeric handle; `dt`
// is the frame time in seconds. Vector-valued getters return a "[x,y,z]" string,
// so `vec3` below parses them back into an array.

const SPIN_SPEED = 1.0; // radians per second

function on_update(entity, dt) {
  const e = vec3(get_rotation_euler(entity));
  set_rotation_euler(entity, e[0], e[1] + SPIN_SPEED * dt, e[2]);
}

// Accept either a "[x,y,z]" string (what the getters return) or an array.
function vec3(raw) {
  return Array.isArray(raw) ? raw : String(raw).slice(1, -1).split(",").map(Number);
}
