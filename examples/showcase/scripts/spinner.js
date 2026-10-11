// spinner.js — rotate the entity about its local Y axis.
//
// The Slag backend calls these as *bare global functions*:
//
//     function on_start(entity, dt)   // once, when the script first runs
//     function on_update(entity, dt)  // every frame
//     function on_destroy(entity, dt) // when the entity is removed
//
// There is no `api` object and no `export`. `entity` is a numeric handle; `dt`
// is the frame time in seconds. Vector-valued getters return an array.

const SPIN_SPEED = 1.0; // radians per second

function on_update(entity, dt) {
  const e = get_rotation_euler(entity);
  set_rotation_euler(entity, e[0], e[1] + SPIN_SPEED * dt, e[2]);
}
