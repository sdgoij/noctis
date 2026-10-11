// spin2d.js — rotate in the screen plane (about Z), for 2D scenes.

const SPIN_SPEED = 1.4; // radians per second

function on_update(entity, dt) {
  const e = get_rotation_euler(entity);
  set_rotation_euler(entity, e[0], e[1], e[2] - SPIN_SPEED * dt);
}
