// third_person.js — a third-person character controller for the Player.
//
// The Player is a *kinematic* capsule: the script moves and turns it directly,
// and physics uses it only to push the dynamic crates. The capsule's child
// "Player Facing" shows the heading.
//
//   WASD   move in the yaw frame (Shift is not used)
//   Space  jump when grounded
//   mouse  X turns the player, Y tilts the follow camera
//
// The Main Camera is placed behind the player each frame; it is not parented, so
// the script writes its transform directly.

const MOVE_SPEED = 5.0; // units per second
const JUMP_SPEED = 5.6; // initial upward speed on a jump
const GRAVITY = 9.81; // matches the project's gravity
const YAW_SENSITIVITY = 0.0032; // radians per pixel of mouse motion
const PITCH_SENSITIVITY = 0.0028;
const CAM_DISTANCE = 5.5; // distance behind the player
const CAM_HEIGHT = 1.4; // camera rise above the player
const PITCH_MIN = -0.35;
const PITCH_MAX = 1.1;

let yaw = 0.0; // player heading (radians about world +Y)
let pitch = 0.25; // camera elevation; positive looks down at the player
let vy = 0.0; // vertical speed we integrate ourselves (the body is kinematic)

function on_update(entity, dt) {
  // --- mouse look ---
  const m = pair(mouse_move());
  yaw -= m[0] * YAW_SENSITIVITY;
  pitch = clamp(pitch - m[1] * PITCH_SENSITIVITY, PITCH_MIN, PITCH_MAX);

  // The body is kinematic with a symmetric capsule, so turn the entity itself;
  // its "Player Facing" child swings to show the heading.
  set_rotation_euler(entity, 0.0, yaw, 0.0);

  // --- horizontal movement in the yaw frame ---
  const s = Math.sin(yaw);
  const c = Math.cos(yaw);
  // Forward is -Z rotated by yaw, right is +X rotated by yaw.
  const fx = -s;
  const fz = -c;
  const rx = c;
  const rz = -s;
  let f = 0.0;
  let r = 0.0;
  if (key_down("KeyW") || key_down("ArrowUp")) f += 1.0;
  if (key_down("KeyS") || key_down("ArrowDown")) f -= 1.0;
  if (key_down("KeyD") || key_down("ArrowRight")) r += 1.0;
  if (key_down("KeyA") || key_down("ArrowLeft")) r -= 1.0;
  let dx = fx * f + rx * r;
  let dz = fz * f + rz * r;
  const len = Math.hypot(dx, dz);
  if (len > 0.0) {
    translate(entity, (dx / len) * MOVE_SPEED * dt, 0.0, (dz / len) * MOVE_SPEED * dt);
  }

  // --- vertical: gravity + jump (a kinematic body ignores physics gravity) ---
  const grounded = is_grounded(entity);
  if (grounded && vy <= 0.0) {
    vy = 0.0;
    if (key_down("Space")) vy = JUMP_SPEED;
  } else {
    vy -= GRAVITY * dt;
  }
  if (vy !== 0.0) translate(entity, 0.0, vy * dt, 0.0);

  // --- follow camera ---
  const camera = find_entity("Main Camera");
  if (camera !== undefined) {
    const p = vec3(get_position(entity));
    const behind = CAM_DISTANCE * Math.cos(pitch);
    const rise = CAM_DISTANCE * Math.sin(pitch) + CAM_HEIGHT;
    set_position(camera, p[0] + s * behind, p[1] + rise, p[2] + c * behind);
    set_rotation_euler(camera, -pitch, yaw, 0.0);
  }
}

// Parse a "[a,b]" / "[a,b,c]" string (what the getters return) into an array.
function pair(raw) {
  return Array.isArray(raw) ? raw : String(raw).slice(1, -1).split(",").map(Number);
}

function vec3(raw) {
  return Array.isArray(raw) ? raw : String(raw).slice(1, -1).split(",").map(Number);
}

function clamp(v, lo, hi) {
  return v < lo ? lo : v > hi ? hi : v;
}
