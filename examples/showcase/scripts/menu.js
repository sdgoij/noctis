// menu.js — the title screen: a custom cursor, and Play hands off to the showcase.
//
// The Play button is a mesh, not a widget, so it activates on Space, Enter, or a
// left click. The window's cursor is released (so the menu can draw its own) and
// the "Cursor" entity follows the pointer; loading a scene replaces the world, so
// this menu is gone once the showcase starts.

const SHOWCASE = "scenes/showcase.scene.json";
const CURSOR = "Cursor";
const ORTHO_HEIGHT = 10.0; // the menu camera's orthographic height

function on_start(entity, dt) {
  // Release the OS cursor so the menu can draw its own.
  set_mouse_captured(false);
}

function on_update(entity, dt) {
  // Follow the pointer with the custom cursor (the camera is orthographic, so the
  // screen maps linearly onto `[-half..half]`).
  const cursor = find_entity(CURSOR);
  if (cursor !== undefined) {
    const m = pair(mouse_position());
    const vp = pair(viewport_size());
    const aspect = vp[0] / vp[1];
    const x = (m[0] / vp[0] - 0.5) * ORTHO_HEIGHT * aspect;
    const y = (0.5 - m[1] / vp[1]) * ORTHO_HEIGHT;
    set_position(cursor, x, y, 0.5);
  }

  if (key_down("Space") || key_down("Enter") || mouse_down("left")) {
    load_scene(SHOWCASE);
  }
}

function pair(raw) {
  return Array.isArray(raw) ? raw : String(raw).slice(1, -1).split(",").map(Number);
}
