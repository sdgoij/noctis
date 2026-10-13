//! The headless harness (`.notes/harness.md`).
//!
//! A driver over the simulation with a **seed** and an **observe** seam — the
//! port's answer to the original's JS scene harness, without the machinery that
//! cannot port. It drives [`spawn_world`](crate::spawn_world) and
//! [`apply_frame`](crate::apply_frame) directly, so a case places the goat, sets
//! the rain or forces a level, steps the world, and reads it back as typed Rust.
//!
//! The cases live beside this module ([`water`], and the goat, explosions and
//! weather cases to come); each is its own `#[test]`, records many fine-grained
//! checks with [`Checks`], and fails once at the end.

// The toolbox grows a case at a time, so not every helper is used by the slices
// that exist yet — the same reason the original's `tests/support` opts out.
#![allow(dead_code)]

mod explosions;
mod goat;
mod water;
mod weather;

use noctis::prelude::*;

use crate::{Goat, World, apply_frame, spawn_world};

/// One simulation run under the harness: an [`EngineContext`] and the [`World`]
/// it drives.
pub struct Sim {
    ctx: EngineContext,
    world: World,
}

impl Sim {
    /// Build the world (the terrain, the meadow, the water and the goat) and hold
    /// it, ready to drive.
    pub fn start() -> Sim {
        let mut ctx = EngineContext::new("harness");
        let world = spawn_world(&mut ctx).expect("the harness world builds");
        Sim { ctx, world }
    }

    /// Advance one frame: the clock, then `apply_frame`, then the input frame's
    /// end — so per-frame key edges behave exactly as they do under the real loop.
    pub fn step(&mut self, dt: f32) {
        self.ctx.time.advance(dt);
        apply_frame(&mut self.ctx, &mut self.world);
        self.ctx.input.begin_frame();
    }

    /// Advance `frames` frames at 60 Hz.
    pub fn run(&mut self, frames: u32) {
        for _ in 0..frames {
            self.step(1.0 / 60.0);
        }
    }

    /// Hold a letter key down.
    pub fn key_down(&mut self, c: char) {
        self.ctx
            .input
            .press_key(Key::letter(c).expect("a letter key"));
    }

    /// Release a letter key.
    pub fn key_up(&mut self, c: char) {
        self.ctx
            .input
            .release_key(Key::letter(c).expect("a letter key"));
    }

    /// Press a key (Space, the arrows, …).
    pub fn press(&mut self, key: Key) {
        self.ctx.input.press_key(key);
    }

    /// Release a key.
    pub fn release(&mut self, key: Key) {
        self.ctx.input.release_key(key);
    }

    // ---- the seed seam (the original's `pos` / `rainAmount` / `flood`) --------

    /// Stand the goat at `(x, z)`.
    pub fn place_goat(&mut self, x: f32, z: f32) {
        self.world.goat.x = x;
        self.world.goat.z = z;
    }

    /// Set the weather's rain, holding the state it belongs to so `update` does
    /// not ease it away (the original's `rainAmount`).
    pub fn set_rain(&mut self, rain: f32) {
        self.world.weather.set_rain(rain);
    }

    /// Pin the water table to a level by hand (the original's `flood <h>`).
    pub fn force_level(&mut self, level: f32) {
        self.world.water.set_force(level);
    }

    /// Hand the water table back to the rain (the original's `flood off`).
    pub fn clear_level(&mut self) {
        self.world.water.force_off();
    }

    /// Dig a crater at `(x, z)` as a blast would, and let one frame rebuild the
    /// ground, the water and the meadow around it.
    pub fn drop_crater(&mut self, x: f32, z: f32) {
        self.world.craters.add(x, z, 1.0);
        self.step(1.0 / 60.0);
    }

    /// Detonate a blast at `(x, z)` — the original's `goats.explosions.blast`.
    pub fn blast_at(&mut self, x: f32, z: f32) {
        let World {
            hazards,
            goat,
            meadow,
            craters,
            ..
        } = &mut self.world;
        hazards.detonate(x, z, 1.0, goat, meadow, craters);
    }

    /// Set the goat's energy, for a case that needs headroom under the ceiling
    /// (the original's `stats.energy`).
    pub fn set_energy(&mut self, energy: f32) {
        self.world.goat.energy = energy;
    }

    /// Stand the goat on the nearest tuft to `(x, z)`; `false` when none is in
    /// reach of the meadow.
    pub fn place_goat_on_tuft(&mut self, x: f32, z: f32) -> bool {
        match crate::grass::nearest(x, z, 6.0, &self.world.meadow, &self.world.craters) {
            Some(tuft) => {
                self.place_goat(tuft.x, tuft.z);
                true
            }
            None => false,
        }
    }

    /// The world point a mine sits at in cell `(cx, cz)`: the cell's centre.
    pub fn cell_centre(cx: i32, cz: i32) -> (f32, f32) {
        (cx as f32 * 2.0 + 1.0, cz as f32 * 2.0 + 1.0)
    }

    /// The world point of the first armed mine within the field, for a case that
    /// wants to stand the goat on one. Scans the near field (well inside `FIELD`, so
    /// the step does not clamp the goat off the mine) and returns `None` only if the
    /// derived field is somehow empty there.
    pub fn first_armed_mine(&self) -> Option<(f32, f32)> {
        for cx in 5..=20 {
            for cz in 5..=20 {
                if self.world.hazards.mine_armed(cx, cz) {
                    return Some(Sim::cell_centre(cx, cz));
                }
            }
        }
        None
    }

    /// Whether cell `(cx, cz)` holds an armed mine now.
    pub fn mine_armed(&self, cx: i32, cz: i32) -> bool {
        self.world.hazards.mine_armed(cx, cz)
    }

    // ---- the observe seam -----------------------------------------------------

    /// The goat.
    pub fn goat(&self) -> &Goat {
        &self.world.goat
    }

    /// The water table.
    pub fn water(&self) -> &crate::water::Water {
        &self.world.water
    }

    /// The craters on the ground.
    pub fn craters(&self) -> &crate::terrain::Craters {
        &self.world.craters
    }

    /// The weather.
    pub fn weather(&self) -> &crate::weather::Weather {
        &self.world.weather
    }

    /// The hazard field — the mines, traps and fuses.
    pub fn hazards(&self) -> &crate::hazards::Hazards {
        &self.world.hazards
    }

    /// The bang flash's intensity now (`0` when dark).
    pub fn flash_intensity(&self) -> f32 {
        self.ctx
            .scene
            .world
            .get::<Light>(self.world.flash.light)
            .map(|light| light.intensity)
            .unwrap_or(0.0)
    }

    /// The camera knock's strength now (`0` when steady).
    pub fn shake(&self) -> f32 {
        self.world.shake.strength()
    }

    /// The goat root's world height (the drawn `y`).
    pub fn goat_y(&self) -> f32 {
        self.ctx
            .scene
            .world
            .get::<LocalTransform>(self.world.goat.root)
            .map(|transform| transform.translation.y)
            .unwrap_or(0.0)
    }

    /// The clip the goat's animator is playing.
    pub fn clip(&self) -> AssetHandle<AnimationClip> {
        self.ctx
            .scene
            .world
            .query::<Animator>()
            .map(|(_, animator)| animator.clip)
            .next()
            .unwrap_or_default()
    }

    /// Whether the goat's playing clip's name contains `needle`.
    pub fn playing(&self, needle: &str) -> bool {
        self.ctx
            .assets
            .animations
            .get(self.clip())
            .is_some_and(|clip| clip.name.to_ascii_lowercase().contains(needle))
    }

    /// The HUD lines the frame published.
    pub fn hud(&self) -> &[String] {
        &self.ctx.screen_text
    }

    /// The weather line — the HUD's second line.
    pub fn weather_line(&self) -> &str {
        self.hud().get(1).map(String::as_str).unwrap_or("")
    }

    /// The vertex count of the meadow mesh currently registered.
    pub fn meadow_verts(&self) -> usize {
        self.mesh_verts("grass/meadow")
    }

    /// The vertex count of the ground mesh currently registered.
    pub fn terrain_verts(&self) -> usize {
        self.mesh_verts("terrain/meadow")
    }

    fn mesh_verts(&self, path: &str) -> usize {
        self.ctx
            .assets
            .meshes
            .handle(path)
            .and_then(|handle| self.ctx.assets.meshes.get(handle))
            .map(|mesh| mesh.vertices.len())
            .unwrap_or(0)
    }
}

/// Records the cases a test checks and reports them together, so one miss does not
/// hide the rest (the original's `tests/support/mod.rs`).
#[derive(Default)]
pub struct Checks {
    rows: Vec<(String, bool, String)>,
}

impl Checks {
    /// A fresh set of checks.
    pub fn new() -> Checks {
        Checks::default()
    }

    /// Record a case. `got` is printed only when it fails.
    pub fn check(&mut self, name: &str, ok: bool, got: impl std::fmt::Debug) {
        self.rows.push((name.to_string(), ok, format!("{got:?}")));
    }

    /// Print every case, then fail the test if any did not hold.
    pub fn finish(self) {
        let mut failed = 0;
        for (name, ok, got) in &self.rows {
            if *ok {
                println!("PASS  {name}");
            } else {
                failed += 1;
                println!("FAIL  {name}  (got {got})");
            }
        }
        if failed == 0 {
            println!("ALL PASS ({})", self.rows.len());
        } else {
            panic!("{failed} of {} checks failed", self.rows.len());
        }
    }
}
