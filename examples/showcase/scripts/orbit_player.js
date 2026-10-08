// orbit_player.js — drive the entity around the XZ plane with WASD.
//
// Shows keyboard input. `key_down` takes a winit key name (`KeyW`, `ArrowUp`,
// `ShiftLeft`, `Space`, ...).

const SPEED = 4.5;   // units per second
const SPRINT = 2.0;  // multiplier while Shift is held

function on_update(entity, dt) {
  let dx = 0.0;
  let dz = 0.0;
  if (key_down("KeyW") || key_down("ArrowUp")) dz -= 1.0;
  if (key_down("KeyS") || key_down("ArrowDown")) dz += 1.0;
  if (key_down("KeyA") || key_down("ArrowLeft")) dx -= 1.0;
  if (key_down("KeyD") || key_down("ArrowRight")) dx += 1.0;
  if (dx === 0.0 && dz === 0.0) return;

  const length = Math.sqrt(dx * dx + dz * dz);
  const speed = SPEED * (key_down("ShiftLeft") || key_down("ShiftRight") ? SPRINT : 1.0);
  translate(entity, (dx / length) * speed * dt, 0.0, (dz / length) * speed * dt);
}
