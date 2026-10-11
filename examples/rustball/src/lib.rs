//! Rust Ball — a Noctis example project whose gameplay is Rust.
//!
//! The project carries a Rust plugin (a `Cargo.toml` at its root). It registers
//! one custom component, [`Spin`], and installs four systems:
//!
//! * **paint** (once): registers a small PBR palette and colours every named
//!   object, so the scene's `$default` meshes are not all white;
//! * **spin**: turns every entity that carries [`Spin`] about its local up axis —
//!   a component and its behaviour both live in this crate, and the component
//!   round-trips through the scene file and is editable in the editor;
//! * **ball**: simulates a ball that keeps bouncing across the floor and, once it
//!   drifts past the edge and falls, respawns above the centre — the engine cannot
//!   drive a rigid body from a plugin (no impulse access), so the ball's motion is
//!   this system's;
//! * **camera**: flies the active camera from player input — hold the middle mouse
//!   button to look around, `W`/`A`/`S`/`D` to move, `E`/`Q` for up and down, and
//!   the wheel to dolly.

#![forbid(unsafe_code)]

use noctis::prelude::*;

/// Radians the camera turns per logical point of mouse drag.
const LOOK_SPEED: f32 = 0.0032;
/// World units the camera flies per second per movement key.
const MOVE_SPEED: f32 = 6.0;
/// World units the camera dollies per point of scroll.
const ZOOM_SPEED: f32 = 0.75;
/// The pitch limit, just shy of straight up/down so the up vector stays defined.
const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.02;

/// A constant spin about an entity's local `+Y` axis.
///
/// Registered by the plugin, so a scene may carry it as
/// `{ "type": "custom", "name": "Spin", "data": { "speed": 1.6 } }`, the editor
/// shows `speed` as a number field, and a script can read or write it through the
/// reflection calls (`has_component`/`get_component`/`set_component`).
#[derive(Debug, Default, Serialize, Deserialize, Component)]
pub struct Spin {
    /// Angular speed in radians per second.
    pub speed: f32,
}

// -- Painting ---------------------------------------------------------------

/// The colour each named object is painted with, as
/// `(entity name, base colour, metallic, roughness)`.
const PALETTE: &[(&str, [f32; 4], f32, f32)] = &[
    ("Ground", [0.16, 0.18, 0.22, 1.0], 0.0, 0.9),
    ("Ball", [0.85, 0.30, 0.24, 1.0], 0.0, 0.35),
    ("Beacon", [0.22, 0.56, 0.95, 1.0], 0.35, 0.30),
];

/// Register the palette and paint every named object that has one.
///
/// A scene in a project with no imported assets can only name the built-in
/// `$default` material, so the plugin supplies the colours instead: it inserts a
/// PBR material per palette entry and points each object's [`MeshRenderer`] at it.
fn paint_scene(assets: &mut AssetRegistry, world: &mut World) {
    let mut paints: Vec<(&str, AssetHandle<Material>)> = Vec::new();
    for (name, color, metallic, roughness) in PALETTE {
        let handle = assets.materials.insert_named(
            format!("paint/{name}"),
            Material {
                name: (*name).to_string(),
                base_color: *color,
                metallic: *metallic,
                roughness: *roughness,
                ..Material::default()
            },
        );
        paints.push((name, handle));
    }

    // Collect first: mutating each `MeshRenderer` needs the world borrowed mutably.
    let named: Vec<(Entity, String)> = world
        .query::<Name>()
        .map(|(entity, name)| (entity, name.as_str().to_string()))
        .collect();
    for (entity, name) in named {
        let Some((_, handle)) = paints.iter().find(|(entry, _)| *entry == name) else {
            continue;
        };
        if let Some(renderer) = world.get_mut::<MeshRenderer>(entity) {
            renderer.material = *handle;
        }
    }
}

// -- The ball ---------------------------------------------------------------

/// Gravity for the simulated ball, in metres per second squared.
const GRAVITY: f32 = 9.81;
/// The ball's radius, matching the `primitives/sphere` mesh.
const BALL_RADIUS: f32 = 0.5;
/// Half the floor's side; the ball only bounces when `|x|` and `|z|` are within it.
const FLOOR_HALF: f32 = 8.0;
/// Restitution of a floor bounce.
const RESTITUTION: f32 = 0.75;
/// The minimum upward speed a bounce imparts, so the ball never settles.
const MIN_BOUNCE: f32 = 5.0;
/// The height the ball (re)spawns at.
const SPAWN_HEIGHT: f32 = 10.0;
/// Below this height the ball has missed the floor and respawns.
const RESPAWN_BELOW: f32 = -16.0;
/// A fresh horizontal launch, cycled per respawn so the motion varies.
const LAUNCH: &[[f32; 3]] = &[
    [4.0, 0.0, 1.5],
    [-3.6, 0.0, 3.4],
    [2.2, 0.0, -4.4],
    [-4.6, 0.0, -2.0],
];

/// The horizontal launch velocity for the `n`-th spawn.
fn launch(n: u32) -> Vec3 {
    let [x, y, z] = LAUNCH[(n as usize) % LAUNCH.len()];
    math::vec3(x, y, z)
}

/// The simulated ball's kinematic state.
#[derive(Clone, Copy, Debug)]
struct Ball {
    position: Vec3,
    velocity: Vec3,
    orientation: Quat,
    respawns: u32,
}

impl Default for Ball {
    fn default() -> Self {
        Self {
            position: math::vec3(0.0, SPAWN_HEIGHT, 0.0),
            velocity: launch(0),
            orientation: Quat::identity(),
            respawns: 0,
        }
    }
}

impl Ball {
    /// Advance the ball by `dt` seconds: gravity, a floor bounce inside the
    /// footprint, a roll, and a respawn once it has fallen past the floor.
    fn step(&mut self, dt: f32) {
        let previous_y = self.position.y;
        self.velocity.y -= GRAVITY * dt;
        self.position = self.position + self.velocity * dt;

        // Bounce only when crossing the floor downward *within* the footprint; a
        // ball outside it, or already beneath the floor, falls through.
        let over_floor = self.position.x.abs() <= FLOOR_HALF && self.position.z.abs() <= FLOOR_HALF;
        if over_floor
            && self.velocity.y < 0.0
            && previous_y >= BALL_RADIUS
            && self.position.y <= BALL_RADIUS
        {
            self.position.y = BALL_RADIUS;
            self.velocity.y = (-self.velocity.y * RESTITUTION).max(MIN_BOUNCE);
        }

        // Roll about the axis perpendicular to the horizontal motion.
        let horizontal = math::vec3(self.velocity.x, 0.0, self.velocity.z);
        let speed = math::length(horizontal);
        if speed > 1e-4 {
            let axis = math::normalize(math::vec3(horizontal.z, 0.0, -horizontal.x));
            let angle = speed / BALL_RADIUS * dt;
            self.orientation = (Quat::from_axis_angle(axis, angle) * self.orientation).normalized();
        }

        if self.position.y < RESPAWN_BELOW {
            self.respawn();
        }
    }

    /// Reset above the centre, launched in the next direction.
    fn respawn(&mut self) {
        self.position = math::vec3(0.0, SPAWN_HEIGHT, 0.0);
        self.velocity = launch(self.respawns);
        self.orientation = Quat::identity();
        self.respawns += 1;
    }
}

/// The entity named `name`, if any.
fn find_named(world: &World, name: &str) -> Option<Entity> {
    world
        .query::<Name>()
        .find(|(_, n)| n.as_str() == name)
        .map(|(entity, _)| entity)
}

// -- The camera -------------------------------------------------------------

/// The player camera's yaw and pitch, in radians.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Look {
    yaw: f32,
    pitch: f32,
}

impl Look {
    /// The orientation this yaw and pitch describe. Looking straight ahead
    /// (`yaw == 0`, `pitch == 0`) faces the world `-Z` axis.
    fn rotation(&self) -> Quat {
        (Quat::from_rotation_y(self.yaw)
            * Quat::from_axis_angle(math::vec3(1.0, 0.0, 0.0), self.pitch))
        .normalized()
    }
}

/// Recover yaw and pitch from an orientation, so a camera's authored rotation is
/// honoured on the first frame instead of snapping to the identity.
fn look_from_rotation(rotation: Quat) -> Look {
    let forward = rotation.rotate_vec3(math::vec3(0.0, 0.0, -1.0));
    Look {
        yaw: (-forward.x).atan2(-forward.z),
        pitch: forward.y.clamp(-1.0, 1.0).asin(),
    }
}

/// The `-1..=1` reading of a key pair (`positive` minus `negative`).
fn key_axis(input: &InputState, positive: char, negative: char) -> f32 {
    let down = |c: char| i32::from(Key::letter(c).is_some_and(|key| input.key_down(key)));
    down(positive) as f32 - down(negative) as f32
}

/// Advance a free-fly camera by one frame of `input`.
///
/// Holding the middle mouse button looks around; `W`/`A`/`S`/`D` fly along the
/// camera's own axes; `E`/`Q` move up and down; the wheel dollies along the view.
/// The rotation is written every frame so the camera tracks [`Look`] exactly.
fn step_camera(look: &mut Look, transform: &mut LocalTransform, input: &InputState, dt: f32) {
    if input.mouse.is_down(MouseButton::Middle) {
        let [dx, dy] = input.mouse.delta;
        look.yaw -= dx * LOOK_SPEED;
        look.pitch = (look.pitch - dy * LOOK_SPEED).clamp(-MAX_PITCH, MAX_PITCH);
    }

    let rotation = look.rotation();
    transform.rotation = rotation;

    let forward = rotation.rotate_vec3(math::vec3(0.0, 0.0, -1.0));
    let right = rotation.rotate_vec3(math::vec3(1.0, 0.0, 0.0));
    let up = rotation.rotate_vec3(math::vec3(0.0, 1.0, 0.0));

    let motion = forward * key_axis(input, 'W', 'S')
        + right * key_axis(input, 'D', 'A')
        + up * key_axis(input, 'E', 'Q');
    transform.translation = transform.translation + motion * (MOVE_SPEED * dt);
    transform.translation = transform.translation + forward * (input.mouse.scroll * ZOOM_SPEED);
}

/// The project's plugin. `#[noctis::plugin]` exports the `noctis_plugin` symbol
/// the tool links — a `Cargo.toml` at the project root is all `noctis` needs to
/// find it.
#[noctis::plugin]
#[derive(Default)]
pub struct RustBall;

impl Plugin for RustBall {
    /// Register [`Spin`] before any scene that uses it loads.
    fn register(&self, components: &mut ComponentRegistry) {
        components.register::<Spin>();
    }

    /// Install the paint, spin, ball, and camera systems.
    fn build(&self, app: &mut App) {
        app.on_startup("rustball.paint", |ctx| {
            paint_scene(&mut ctx.assets, &mut ctx.scene.world);
        });

        app.on_update("rustball.spin", |ctx| {
            let dt = ctx.time.delta();
            // Collect first: the pass that mutates a transform cannot also hold a
            // borrow of the `Spin` column.
            let spinning: Vec<(Entity, f32)> = ctx
                .scene
                .world
                .query::<Spin>()
                .map(|(entity, spin)| (entity, spin.speed))
                .collect();
            for (entity, speed) in spinning {
                if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(entity) {
                    transform.rotation =
                        (Quat::from_rotation_y(speed * dt) * transform.rotation).normalized();
                }
            }
        });

        let mut ball = Ball::default();
        app.on_update("rustball.ball", move |ctx| {
            ball.step(ctx.time.delta());
            let Some(entity) = find_named(&ctx.scene.world, "Ball") else {
                return;
            };
            if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(entity) {
                transform.translation = ball.position;
                transform.rotation = ball.orientation;
            }
        });

        let mut look = Look::default();
        let mut seeded = false;
        app.on_update("rustball.camera", move |ctx| {
            let camera = ctx
                .scene
                .world
                .query::<Camera>()
                .find(|(_, camera)| camera.is_active)
                .map(|(entity, _)| entity);
            let Some(camera) = camera else {
                return;
            };
            let dt = ctx.time.delta();
            if let Some(transform) = ctx.scene.world.get_mut::<LocalTransform>(camera) {
                if !seeded {
                    look = look_from_rotation(transform.rotation);
                    seeded = true;
                }
                step_camera(&mut look, transform, &ctx.input, dt);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera on `+Z` at distance 5, facing `-Z`.
    fn camera_start() -> LocalTransform {
        LocalTransform::from_translation(math::vec3(0.0, 0.0, 5.0))
    }

    /// A ball at `position` moving at `velocity`, with no respawns yet.
    fn ball_at(position: Vec3, velocity: Vec3) -> Ball {
        Ball {
            position,
            velocity,
            orientation: Quat::identity(),
            respawns: 0,
        }
    }

    #[test]
    fn the_plugin_registers_its_component_and_systems() {
        let mut registry = ComponentRegistry::new();
        RustBall.register(&mut registry);
        assert!(registry.has("Spin"));

        let mut app = App::new("test");
        RustBall.build(&mut app);
        assert_eq!(app.startup.names(), ["rustball.paint"]);
        assert_eq!(
            app.update.names(),
            ["rustball.spin", "rustball.ball", "rustball.camera"]
        );
    }

    #[test]
    fn painting_colours_named_objects() {
        let mut assets = AssetRegistry::with_builtins();
        let mut world = World::new();

        let ground = world.spawn();
        world.insert(ground, Name::new("Ground"));
        world.insert(ground, MeshRenderer::default());

        let unnamed = world.spawn();
        world.insert(unnamed, Name::new("Untitled"));
        world.insert(unnamed, MeshRenderer::default());

        paint_scene(&mut assets, &mut world);

        let material = world
            .get::<MeshRenderer>(ground)
            .expect("the Ground has a renderer")
            .material;
        let color = assets
            .materials
            .get(material)
            .expect("the palette material exists")
            .base_color;
        assert_eq!(color, PALETTE[0].1, "the Ground took its palette colour");

        // An object with no palette entry keeps the material it had.
        assert_eq!(
            world.get::<MeshRenderer>(unnamed).unwrap().material,
            AssetHandle::NONE
        );
    }

    #[test]
    fn the_spin_system_turns_a_tagged_entity() {
        let mut app = App::new("test");
        RustBall.build(&mut app);

        let entity = app.context.scene.world.spawn();
        app.context.scene.world.insert(entity, camera_start());
        app.context.scene.world.insert(entity, Spin { speed: 2.0 });

        app.tick(0.1);

        let rotation = app
            .context
            .scene
            .world
            .get::<LocalTransform>(entity)
            .expect("the entity keeps its transform")
            .rotation;
        assert_ne!(rotation, Quat::identity(), "the tagged entity turned");
    }

    #[test]
    fn the_ball_bounces_up_from_the_floor() {
        let mut ball = ball_at(
            math::vec3(0.0, BALL_RADIUS + 0.001, 0.0),
            math::vec3(0.0, -5.0, 0.0),
        );
        ball.step(1.0 / 60.0);
        assert!(
            ball.velocity.y > 0.0,
            "the ball bounced: {}",
            ball.velocity.y
        );
        assert!(
            (ball.position.y - BALL_RADIUS).abs() < 1e-3,
            "the ball rests on the surface: {}",
            ball.position.y
        );
    }

    #[test]
    fn the_ball_falls_through_outside_the_floor() {
        let mut ball = ball_at(
            math::vec3(FLOOR_HALF + 1.0, BALL_RADIUS + 0.001, 0.0),
            math::vec3(0.0, -5.0, 0.0),
        );
        ball.step(1.0 / 60.0);
        assert!(ball.velocity.y < 0.0, "no bounce outside the footprint");
        assert!(ball.position.y < BALL_RADIUS, "the ball keeps falling");
    }

    #[test]
    fn the_ball_respawns_after_missing_the_floor() {
        let mut ball = ball_at(
            math::vec3(0.0, RESPAWN_BELOW - 1.0, 0.0),
            math::vec3(0.0, -3.0, 0.0),
        );
        ball.respawns = 2;
        ball.step(1.0 / 60.0);
        assert_eq!(ball.respawns, 3, "the respawn count advanced");
        assert_eq!(ball.position, math::vec3(0.0, SPAWN_HEIGHT, 0.0));
        assert_eq!(ball.velocity, launch(2));
    }

    #[test]
    fn the_ball_eventually_misses_the_floor() {
        let mut ball = Ball::default();
        // Twenty seconds is several crossings of the 16-unit floor.
        for _ in 0..(60 * 20) {
            ball.step(1.0 / 60.0);
        }
        assert!(ball.respawns >= 1, "the ball left the floor and respawned");
    }

    #[test]
    fn look_round_trips_through_the_rotation() {
        for (yaw, pitch) in [(0.0, 0.0), (0.7, 0.3), (-1.2, -0.4)] {
            let recovered = look_from_rotation(Look { yaw, pitch }.rotation());
            assert!((recovered.yaw - yaw).abs() < 1e-4, "yaw {recovered:?}");
            assert!(
                (recovered.pitch - pitch).abs() < 1e-4,
                "pitch {recovered:?}"
            );
        }
    }

    #[test]
    fn middle_drag_looks_and_clamps_pitch() {
        let mut look = Look::default();
        let mut transform = camera_start();
        let mut input = InputState::new();
        input.press_mouse(MouseButton::Middle);
        input.mouse.delta = [100.0, 50.0];

        step_camera(&mut look, &mut transform, &input, 1.0 / 60.0);
        assert!(look.yaw < 0.0, "dragging right turns the view right");
        assert!(look.pitch < 0.0, "dragging down looks down");

        // No drag without the middle button held.
        let yaw = look.yaw;
        input.release_mouse(MouseButton::Middle);
        input.mouse.delta = [100.0, 0.0];
        step_camera(&mut look, &mut transform, &input, 1.0 / 60.0);
        assert_eq!(look.yaw, yaw, "the view holds when not dragging");

        // A huge downward drag cannot flip the camera past straight down.
        input.press_mouse(MouseButton::Middle);
        input.mouse.delta = [0.0, 1.0e6];
        step_camera(&mut look, &mut transform, &input, 1.0 / 60.0);
        assert!(look.pitch >= -MAX_PITCH - 1e-6, "pitch clamped");
    }

    #[test]
    fn wasd_fly_along_the_camera_axes() {
        let mut look = Look::default(); // facing -Z
        let mut transform = camera_start();
        let mut input = InputState::new();

        let w = Key::letter('W').expect("W is a letter key");
        input.press_key(w);
        step_camera(&mut look, &mut transform, &input, 0.5);
        assert!(
            transform.translation.z < camera_start().translation.z,
            "W flies forward (-Z): {:?}",
            transform.translation
        );
        assert!(transform.translation.x.abs() < 1e-5);

        input.release_key(w);
        input.press_key(Key::letter('D').expect("D is a letter key"));
        let before = transform.translation;
        step_camera(&mut look, &mut transform, &input, 0.5);
        assert!(transform.translation.x > before.x, "D strafes right (+X)");
    }

    #[test]
    fn the_wheel_dollies_along_the_view() {
        let mut look = Look::default();
        let mut transform = camera_start();
        let mut input = InputState::new();
        input.add_scroll(4.0);

        step_camera(&mut look, &mut transform, &input, 0.0);
        assert!(
            transform.translation.z < camera_start().translation.z,
            "scrolling in moves forward: {:?}",
            transform.translation
        );
    }
}
