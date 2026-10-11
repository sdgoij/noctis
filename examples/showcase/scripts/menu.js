// menu.js — the title screen's only behaviour: Play hands off to the showcase.
//
// The Play button is a mesh, not a widget, so it activates on Space, Enter, or a
// left click. Loading a scene replaces the world, so this menu entity is gone
// once the showcase starts.

const SHOWCASE = "scenes/showcase.scene.json";

function on_update(entity, dt) {
  if (key_down("Space") || key_down("Enter") || mouse_down("left")) {
    load_scene(SHOWCASE);
  }
}
