// bobber.js — float up and down around each entity's starting height.
//
// A module's top-level state is shared by every entity that runs the script, so
// the base heights are kept per entity handle (indexed by the handle the engine
// passes in) rather than in a single variable.

const AMPLITUDE = 0.6; // world units
const SPEED = 1.6;     // radians per second

const bases = [];

function on_start(entity, dt) {
  bases[entity] = get_position(entity)[1];
}

function on_update(entity, dt) {
  let base = bases[entity];
  if (base === undefined) {
    base = get_position(entity)[1];
    bases[entity] = base;
  }
  const p = get_position(entity);
  const y = base + Math.sin(time_elapsed() * SPEED) * AMPLITUDE;
  set_position(entity, p[0], y, p[2]);
}
