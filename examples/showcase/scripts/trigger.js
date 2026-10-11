// trigger.js — the trigger pad teleports the Player into the physics playground.
//
// The pad is a sensor collider: `is_overlapping` reports when the Player stands
// on it. `teleport_scene` carries the Player *object* into the target scene (at
// that scene's "Entry" marker), so the same character keeps playing there.
//
// `armed` starts false and re-arms only once the pad is *left*: the pad then
// fires on a genuine enter, never on the frame the Player arrives on it — so two
// pads facing each other cannot bounce the Player back and forth forever.

const PHYSICS = "scenes/physics.scene.json";
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
      teleport_scene(PHYSICS, CARRY);
    }
  } else {
    armed = true;
  }
}
