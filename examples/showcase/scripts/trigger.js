// trigger.js — the trigger pad teleports to the physics playground.
//
// The pad is a sensor collider: `is_overlapping` reports when the Player stands
// on it. Loading a scene replaces the world, so the pad fires once per entry
// (the `armed` latch keeps it from re-queuing every frame it is touched).

const PHYSICS = "scenes/physics.scene.json";

let armed = true;

function on_update(entity, dt) {
  const player = find_entity("Player");
  if (player === undefined) {
    return;
  }
  if (is_overlapping(entity, player)) {
    if (armed) {
      armed = false;
      load_scene(PHYSICS);
    }
  } else {
    armed = true;
  }
}
