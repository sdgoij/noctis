// return.js — the physics playground's Sensor Zone sends the Player back to the
// showcase, carrying the same object (see `trigger.js` for the run-down).
//
// `armed` starts false and re-arms only after the Player leaves the zone, so the
// Player is never teleported on the frame they arrive overlapping it.

const SHOWCASE = "scenes/showcase.scene.json";
const CARRY = "Player";

let armed = false;

function on_update(entity, dt) {
  const player = find_entity("Player");
  if (player === undefined) {
    return;
  }
  if (is_overlapping(entity, player)) {
    if (armed) {
      armed = false;
      teleport_scene(SHOWCASE, CARRY);
    }
  } else {
    armed = true;
  }
}
