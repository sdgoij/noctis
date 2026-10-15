//! The water cases (`.notes/harness.md`): the *scene-level* behaviour the unit
//! tests in `water.rs` do not already hold — the goat wades, it will not sleep in
//! a pool, the HUD reads the depth, the meadow hides what the pool covers, a
//! crater holds the water it digs, and the surface carries the engine's mirror.
//!
//! The pure-logic cases (the level is monotone in the rain, a dry spell is exactly
//! dry) are unit tests; these drive the *world* through the [`Sim`] harness.

use super::{Checks, Sim};
use crate::Mode;

/// The point the flooded table covers deepest, so a case can stand the goat in a
/// real pool. The terrain is fixed, so a scan over its grid is enough.
fn deepest_pool(sim: &Sim) -> (f32, f32, f32) {
    let mut best = (0.0f32, 0.0f32, 0.0f32);
    let mut x = -44.0f32;
    while x <= 44.0 {
        let mut z = -44.0f32;
        while z <= 44.0 {
            let depth = sim.water().depth_at(x, z);
            if depth > best.2 {
                best = (x, z, depth);
            }
            z += 1.0;
        }
        x += 1.0;
    }
    best
}

#[test]
fn the_goat_wades() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(1.0);
    sim.run(30);

    let (x, z, depth) = deepest_pool(&sim);
    checks.check(
        "a downpour pools in the hollows",
        depth > 0.1,
        (x, z, depth),
    );

    sim.place_goat(x, z);
    sim.run(2);
    let (gx, gz) = (sim.goat().x, sim.goat().z);
    checks.check(
        "the goat is standing in the water",
        sim.water().in_water(gx, gz),
        sim.water().depth_at(gx, gz),
    );
    checks.check(
        "wading slows the goat",
        sim.water().speed_factor(gx, gz) < 1.0,
        sim.water().speed_factor(gx, gz),
    );
    checks.check(
        "wading costs it energy",
        sim.water().drain_factor(gx, gz) > 1.0,
        sim.water().drain_factor(gx, gz),
    );
    checks.check(
        "the HUD reads the water",
        sim.weather_line().contains("water"),
        sim.weather_line(),
    );
    checks.finish();
}

#[test]
fn the_goat_does_not_sleep_in_the_water() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();

    // Dry ground first: it sleeps when asked.
    sim.set_rain(0.0);
    sim.run(2);
    sim.key_down('Z');
    sim.step(1.0 / 60.0);
    sim.key_up('Z');
    checks.check(
        "it sleeps on dry ground",
        sim.goat().mode == Mode::Sleep,
        sim.goat().mode,
    );

    // The table rises under it: the water ends the sleep.
    sim.force_level(2.0);
    sim.step(1.0 / 60.0);
    checks.check(
        "the water wakes a sleeping goat",
        sim.goat().mode != Mode::Sleep,
        sim.goat().mode,
    );

    // And it will not fall asleep in a pool.
    sim.clear_level();
    sim.set_rain(1.0);
    sim.run(30);
    let (x, z, _) = deepest_pool(&sim);
    sim.place_goat(x, z);
    sim.run(2);
    sim.key_down('Z');
    sim.step(1.0 / 60.0);
    sim.key_up('Z');
    checks.check(
        "it will not sleep in a pool",
        sim.goat().mode != Mode::Sleep,
        (
            sim.goat().mode,
            sim.water().depth_at(sim.goat().x, sim.goat().z),
        ),
    );
    checks.finish();
}

#[test]
fn the_meadow_hides_the_pool() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    let dry = sim.meadow_verts();
    checks.check("the spawn meadow has tufts", dry > 0, dry);

    sim.set_rain(1.0);
    sim.run(30);
    let wet = sim.meadow_verts();
    checks.check("the pool hides the tufts it covers", wet < dry, (dry, wet));
    checks.finish();
}

#[test]
fn a_crater_holds_the_water_it_digs() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(1.0);
    sim.run(30);

    // A crater on ground the pool already reaches: the dish deepens the water.
    let (x, z, _) = deepest_pool(&sim);
    let before = sim.water().depth_at(x, z);
    sim.drop_crater(x, z);
    let after = sim.water().depth_at(x, z);
    checks.check(
        "the bang dished the ground",
        sim.craters().dip_at(x, z) < 0.0,
        sim.craters().dip_at(x, z),
    );
    checks.check(
        "the crater holds more water than the ground it replaced",
        before > 0.0 && after > before,
        (before, after),
    );
    checks.finish();
}

#[test]
fn the_water_carries_a_mirror_at_its_level() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);

    // The surface is the engine's receiver, its plane horizontal at the table's level.
    let start = sim.water_reflection().expect("the water is a mirror");
    checks.check(
        "the receiver's plane faces up",
        (start.normal.x, start.normal.y, start.normal.z) == (0.0, 1.0, 0.0),
        start.normal,
    );
    checks.check(
        "the plane sits at the table's level",
        (start.point.y - sim.water().level()).abs() < 1e-4,
        (start.point.y, sim.water().level()),
    );
    checks.check(
        "the clip plane rides just above the surface",
        start.clip_bias > 0.0 && start.clip_point().y > start.point.y,
        start.clip_bias,
    );
    checks.check(
        "the whole frame is reflected",
        start.resolution == 1.0,
        start.resolution,
    );
    checks.check("the reflection is on", start.enabled, start.enabled);

    // The table rises: the mirror rises with the surface it is drawn on.
    sim.force_level(2.5);
    sim.step(1.0 / 60.0);
    let risen = sim.water_reflection().expect("the water is still a mirror");
    checks.check(
        "a rising table lifts the mirror with the surface",
        (risen.point.y - sim.water().level()).abs() < 1e-4 && risen.point.y > start.point.y,
        (risen.point.y, start.point.y, sim.water().level()),
    );
    checks.finish();
}
