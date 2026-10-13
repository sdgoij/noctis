//! The Goats plugin: the project's `#[noctis::plugin]` crate, which owns the
//! running `World` and drives it one frame at a time.
//!
//! The simulation itself — the goat, the ground, the meadow, the water, the
//! weather and the hazards — is the `goats` crate. This crate is the one `noctis`
//! links: the project's virtual workspace names it the first default member, so
//! the engine finds and links it here (`crates/plugin`), beside the simulation it
//! drives. The design is `.notes/goats.md`; the harness that drives the same
//! simulation is `.notes/harness.md`.

#![forbid(unsafe_code)]

use noctis::prelude::*;

/// The project's plugin. `#[noctis::plugin]` exports the `noctis_plugin` symbol
/// the tool links — a `Cargo.toml` at the project root is all `noctis` needs to
/// find it.
#[noctis::plugin]
#[derive(Default)]
pub struct Goats;

impl Plugin for Goats {
    fn build(&self, app: &mut App) {
        let mut world: Option<goats::World> = None;
        app.on_update("goats.frame", move |ctx| {
            // The world is built on the first tick, so the closure owns it.
            if world.is_none() {
                world = goats::spawn_world(ctx);
            }
            if let Some(world) = world.as_mut() {
                goats::apply_frame(ctx, world);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use goats::{EYE_HEIGHT, terrain};

    #[test]
    fn the_plugin_spawns_the_goat_and_the_terrain() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        let world = &app.context.scene.world;
        assert!(
            world
                .query::<Name>()
                .any(|(_, name)| name.as_str() == "Terrain"),
            "the terrain entity was spawned"
        );
        // The terrain is a plain mesh; the goat is skinned and plays a clip.
        assert!(world.count::<MeshRenderer>() >= 1);
        assert!(world.count::<SkinnedMeshRenderer>() >= 1);
        assert!(world.count::<Animator>() >= 1, "the goat plays a clip");
        assert!(app.context.assets.meshes.handle("terrain/meadow").is_some());
    }

    #[test]
    fn holding_w_walks_the_goat_forward() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0); // build the world

        let root = app
            .context
            .scene
            .world
            .query::<Name>()
            .find(|(_, name)| name.as_str() == "Goat Root")
            .map(|(entity, _)| entity)
            .expect("the goat root");
        let before = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;
        let idle = app
            .context
            .scene
            .world
            .query::<Animator>()
            .map(|(_, animator)| animator.clip)
            .next()
            .expect("an animator");

        app.context
            .input
            .press_key(Key::letter('W').expect("W is a letter key"));
        for _ in 0..30 {
            app.tick(1.0 / 60.0);
        }

        let after = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;
        assert!(
            after.x > before.x + 0.1,
            "the goat walked forward: {before:?} -> {after:?}"
        );
        // The gait switched off idle and is playing forwards. (The engine's
        // sampler, which advances `time`, is the runtime's, not this bare `App`.)
        let animator = app
            .context
            .scene
            .world
            .query::<Animator>()
            .map(|(_, animator)| *animator)
            .next()
            .expect("an animator");
        assert!(animator.clip != idle, "the gait left idle: {animator:?}");
        assert!(
            animator.speed.abs() > 0.0,
            "a gait is playing: {animator:?}"
        );
    }

    /// The goat's root entity.
    fn goat_root(app: &App) -> Entity {
        app.context
            .scene
            .world
            .query::<Name>()
            .find(|(_, name)| name.as_str() == "Goat Root")
            .map(|(entity, _)| entity)
            .expect("the goat root")
    }

    /// The clip the goat's animator is playing.
    fn playing(app: &App) -> AssetHandle<AnimationClip> {
        app.context
            .scene
            .world
            .query::<Animator>()
            .map(|(_, animator)| animator.clip)
            .next()
            .expect("an animator")
    }

    /// `true` when `clip` is one of the clips whose name contains `needle`.
    fn is_role(app: &App, clip: AssetHandle<AnimationClip>, needle: &str) -> bool {
        app.context
            .assets
            .animations
            .get(clip)
            .is_some_and(|clip| clip.name.to_ascii_lowercase().contains(needle))
    }

    fn key(c: char) -> Key {
        Key::letter(c).expect("a letter key")
    }

    #[test]
    fn pressing_space_jumps_then_lands_back_in_the_gait() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);
        let root = goat_root(&app);

        app.context.input.press_key(key('W'));
        app.tick(1.0 / 60.0);
        let before = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;

        app.context.input.press_key(Key::Space);
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "jump"),
            "the jump clip plays: {:?}",
            playing(&app)
        );
        let duration = app
            .context
            .assets
            .animations
            .get(playing(&app))
            .expect("the jump clip")
            .duration;

        // The goat keeps travelling while it is airborne.
        for _ in 0..8 {
            app.tick(1.0 / 60.0);
        }
        let mid = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;
        assert!(
            mid.x > before.x + 0.05,
            "airborne travel: {before:?} -> {mid:?}"
        );

        // It lands, and with `W` still held it takes up the walk again.
        for _ in 0..(duration * 60.0).ceil() as u32 + 2 {
            app.tick(1.0 / 60.0);
        }
        assert!(
            is_role(&app, playing(&app), "walk"),
            "the walk resumed: {:?}",
            playing(&app)
        );
    }

    #[test]
    fn z_sleeps_and_wakes_the_goat() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        app.context.input.press_key(key('Z'));
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "sleep"),
            "asleep: {:?}",
            playing(&app)
        );

        // A second, fresh press wakes it.
        app.context.input.release_key(key('Z'));
        app.context.input.press_key(key('Z'));
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "idle"),
            "awake: {:?}",
            playing(&app)
        );
    }

    #[test]
    fn moving_wakes_a_sleeping_goat() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        app.context.input.press_key(key('Z'));
        app.tick(1.0 / 60.0);
        assert!(is_role(&app, playing(&app), "sleep"));

        app.context.input.press_key(key('W'));
        app.tick(1.0 / 60.0);
        // Waking returns to idle for the one frame (the original's `wakeUp`),
        // then the free state follows the held key into a walk.
        assert!(
            is_role(&app, playing(&app), "idle"),
            "woke to idle: {:?}",
            playing(&app)
        );
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "walk"),
            "then walked: {:?}",
            playing(&app)
        );
    }

    #[test]
    fn the_idle_take_cycles_on_each_entry() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        // The model's first idle take, on the frame the world is built.
        let first = playing(&app);
        assert!(is_role(&app, first, "idle"), "{first:?}");

        // Walk a step, then stop: re-entering idle takes the next take.
        app.context.input.press_key(key('W'));
        app.tick(1.0 / 60.0);
        app.context.input.release_key(key('W'));
        app.tick(1.0 / 60.0);
        let second = playing(&app);
        assert!(is_role(&app, second, "idle"), "{second:?}");
        assert_ne!(
            first, second,
            "the idle take advanced: {first:?} then {second:?}"
        );
    }

    #[test]
    fn a_turns_left_and_d_turns_right() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);
        let root = goat_root(&app);

        // The goat's world forward (it faces its local `+X`). Turning left
        // swings it toward `-Z`, turning right toward `+Z`.
        let forward_z = |app: &App| {
            app.context
                .scene
                .world
                .get::<LocalTransform>(root)
                .expect("a transform")
                .rotation
                .rotate_vec3(math::vec3(1.0, 0.0, 0.0))
                .z
        };

        app.context.input.press_key(key('A'));
        for _ in 0..10 {
            app.tick(1.0 / 60.0);
        }
        assert!(forward_z(&app) < -0.1, "A turned left: {}", forward_z(&app));

        app.context.input.release_key(key('A'));
        app.context.input.press_key(key('D'));
        for _ in 0..20 {
            app.tick(1.0 / 60.0);
        }
        assert!(forward_z(&app) > 0.1, "D turned right: {}", forward_z(&app));
    }

    #[test]
    fn running_to_empty_exhausts_the_goat_to_a_walk() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        app.context.input.press_key(key('W'));
        app.context.input.press_key(Key::ShiftLeft);
        for _ in 0..1600 {
            app.tick(1.0 / 60.0);
        }
        // Energy ran out, so the gait is capped to a walk even with Shift held.
        assert!(
            is_role(&app, playing(&app), "walk"),
            "the exhausted goat walks: {:?}",
            playing(&app)
        );
    }

    #[test]
    fn exhaustion_kills_the_goat_and_r_starts_a_new_life() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);
        let root = goat_root(&app);

        app.context.input.press_key(key('W'));
        app.context.input.press_key(Key::ShiftLeft);
        for _ in 0..3600 {
            app.tick(1.0 / 60.0);
        }
        assert!(
            is_role(&app, playing(&app), "death"),
            "the goat died: {:?}",
            playing(&app)
        );

        // `R` starts a new life back at the origin.
        app.context.input.release_key(key('W'));
        app.context.input.release_key(Key::ShiftLeft);
        app.context.input.press_key(key('R'));
        app.tick(1.0 / 60.0);
        assert!(
            is_role(&app, playing(&app), "idle"),
            "restarted: {:?}",
            playing(&app)
        );
        let at = app
            .context
            .scene
            .world
            .get::<LocalTransform>(root)
            .expect("a transform")
            .translation;
        assert!(
            at.x.abs() < 1e-3 && at.z.abs() < 1e-3,
            "back at the origin: {at:?}"
        );
    }

    #[test]
    fn an_exhausted_idle_goat_drops_off_to_sleep() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        app.context.input.press_key(key('W'));
        app.context.input.press_key(Key::ShiftLeft);
        for _ in 0..1600 {
            app.tick(1.0 / 60.0);
        }
        // Stop; exhausted and still, it falls asleep after the delay.
        app.context.input.release_key(key('W'));
        app.context.input.release_key(Key::ShiftLeft);
        for _ in 0..140 {
            app.tick(1.0 / 60.0);
        }
        assert!(
            is_role(&app, playing(&app), "sleep"),
            "dropped off: {:?}",
            playing(&app)
        );
    }

    #[test]
    fn the_plugin_spawns_the_meadow() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);

        assert!(
            app.context
                .scene
                .world
                .query::<Name>()
                .any(|(_, name)| name.as_str() == "Meadow"),
            "the meadow entity was spawned"
        );
        assert!(app.context.assets.materials.handle("grass/tuft").is_some());
        let mesh = app
            .context
            .assets
            .meshes
            .handle("grass/meadow")
            .and_then(|handle| app.context.assets.meshes.get(handle))
            .expect("the meadow mesh");
        assert!(!mesh.vertices.is_empty(), "the meadow has tufts at spawn");
    }

    #[test]
    fn the_environment_asset_is_a_usable_sky() {
        // The project's `[render] environment`. The engine's fallback is a fully
        // white, fully-lit environment, which overexposes anything pale (the goat
        // went flat white); a real sky gives a moderate ambient instead. The asset
        // is the *project's*, two directories up from this crate.
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/hdris/meadow.hdr");
        let bytes = std::fs::read(path).expect("the environment file");
        let hdr = load_hdr_bytes("meadow", &bytes).expect("a decodable Radiance HDR");

        assert!(
            hdr.width >= 64 && hdr.height >= 32,
            "a usable sky resolution"
        );
        assert_eq!(
            hdr.data.len(),
            (hdr.width * hdr.height * 3) as usize,
            "linear RGB samples"
        );
        let avg = hdr.data.iter().sum::<f32>() / hdr.data.len() as f32;
        assert!(
            (0.1..0.8).contains(&avg),
            "a moderate sky radiance: bright enough to light, dim enough not to clip ({avg})"
        );
    }

    /// The distance from the active camera to the goat (the orbit's target).
    fn camera_distance(app: &App) -> f32 {
        let scene = &app.context.scene.world;
        let cam = scene
            .query::<Camera>()
            .find(|(_, camera)| camera.is_active)
            .map(|(entity, _)| entity)
            .expect("an active camera");
        let pos = scene
            .get::<LocalTransform>(cam)
            .expect("a camera transform")
            .translation;
        let root_y = scene
            .get::<LocalTransform>(goat_root(app))
            .expect("a goat transform")
            .translation
            .y;
        math::length(pos - math::vec3(0.0, root_y + EYE_HEIGHT, 0.0))
    }

    #[test]
    fn the_wheel_dollies_the_camera_a_fraction_of_a_unit() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        // Tests build no scene, so give the plugin a camera to orbit about.
        let cam = app.context.scene.world.spawn_with(Camera {
            is_active: true,
            ..Camera::default()
        });
        app.context.scene.world.insert(
            cam,
            LocalTransform::from_translation(math::vec3(0.0, 4.5, 9.0)),
        );
        app.tick(1.0 / 60.0); // build the world and seed the orbit
        let before = camera_distance(&app);

        // One wheel notch is 40 scroll points; the camera should creep ~0.4 units
        // closer rather than slam to the near limit.
        app.context.input.add_scroll(40.0);
        app.tick(1.0 / 60.0);
        let after = camera_distance(&app);
        assert!(
            (before - after - 0.4).abs() < 0.05,
            "one notch dollies ~0.4 units: {before} -> {after}"
        );
    }

    #[test]
    fn the_camera_stays_above_the_ground() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        let cam = app.context.scene.world.spawn_with(Camera {
            is_active: true,
            ..Camera::default()
        });
        app.context.scene.world.insert(
            cam,
            LocalTransform::from_translation(math::vec3(0.0, 4.5, 9.0)),
        );
        app.tick(1.0 / 60.0);

        // Drag to swing the camera under the goat, and dolly well out.
        app.context.input.press_mouse(MouseButton::Left);
        app.context.input.add_mouse_motion(0.0, -4000.0);
        app.context.input.add_scroll(400.0);
        app.tick(1.0 / 60.0);

        let pos = app
            .context
            .scene
            .world
            .get::<LocalTransform>(cam)
            .expect("a camera transform")
            .translation;
        assert!(
            pos.y > terrain::height(pos.x, pos.z),
            "the camera is above the ground: y {} vs {}",
            pos.y,
            terrain::height(pos.x, pos.z)
        );
    }

    /// The vertex count of the meadow mesh currently registered.
    fn meadow_vertices(app: &App) -> usize {
        let handle = app
            .context
            .assets
            .meshes
            .handle("grass/meadow")
            .expect("the meadow mesh");
        app.context
            .assets
            .meshes
            .get(handle)
            .expect("the meadow mesh is registered")
            .vertices
            .len()
    }

    /// The weather line the plugin published this frame.
    fn weather_line(app: &App) -> String {
        app.context.screen_text.get(1).cloned().unwrap_or_default()
    }

    #[test]
    fn rain_raises_the_water_and_hides_the_tufts_it_reaches() {
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0); // build the world on a clear day
        let dry = meadow_vertices(&app);

        // Force the weather through to rain (Clear -> Cloudy -> Rain)...
        app.context.input.press_key(key('C'));
        app.tick(1.0 / 60.0);
        app.context.input.release_key(key('C'));
        app.context.input.press_key(key('C'));
        // ...and let the rain ease up and fill the table.
        for _ in 0..(8 * 60) {
            app.tick(1.0 / 60.0);
        }

        let wet = meadow_vertices(&app);
        assert!(
            wet < dry,
            "the water hid the tufts it reached: {dry} -> {wet}"
        );
    }

    #[test]
    fn the_spawn_bowl_stays_dry_under_the_heaviest_rain() {
        // The bowl the goat spawns in sits at the table's own ceiling, so even a
        // downpour leaves the goat's feet dry — the HUD never reports water there.
        let mut app = App::new("test");
        Goats.build(&mut app);
        app.tick(1.0 / 60.0);
        app.context.input.press_key(key('C'));
        app.tick(1.0 / 60.0);
        app.context.input.release_key(key('C'));
        app.context.input.press_key(key('C'));
        for _ in 0..(8 * 60) {
            app.tick(1.0 / 60.0);
        }

        assert!(
            !weather_line(&app).contains("water"),
            "the spawn bowl is dry: {}",
            weather_line(&app)
        );
    }
}
