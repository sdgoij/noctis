//! The goat cases (`.notes/harness.md`): the scripted timeline the original's
//! `scene_logic.rs` drives — gaits on input, a jump that carries and lands, a
//! grazed meal that tops up energy and belly, and exhaustion that caps the gait
//! and finally kills.
//!
//! These drive the *world* through the [`Sim`] harness, so they exercise the whole
//! frame — the state machine, the root step, the clip the animator plays and the
//! stats — rather than one function apiece.

use super::{Checks, Sim};
use crate::Mode;
use noctis::prelude::Key;

const DT: f32 = 1.0 / 60.0;

#[test]
fn holding_forward_walks_the_goat() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    let (x0, z0) = (sim.goat().x, sim.goat().z);

    sim.key_down('W');
    sim.run(30);
    checks.check(
        "holding W is a walk",
        sim.goat().mode == Mode::Walk,
        sim.goat().mode,
    );
    checks.check(
        "the walk plays the walk clip",
        sim.playing("walk"),
        sim.clip(),
    );
    checks.check(
        "and it moves forward (the model faces +X)",
        sim.goat().x > x0 && (sim.goat().z - z0).abs() < 1e-3,
        (sim.goat().x - x0, sim.goat().z - z0),
    );
    checks.finish();
}

#[test]
fn the_gaits_follow_the_modifiers() {
    let mut checks = Checks::new();

    let mut run_sim = Sim::start();
    run_sim.set_rain(0.0);
    run_sim.run(2);
    run_sim.key_down('W');
    run_sim.press(Key::ShiftLeft);
    run_sim.run(30);
    checks.check(
        "shift runs",
        run_sim.goat().mode == Mode::Run && run_sim.playing("run"),
        run_sim.goat().mode,
    );

    let mut trot_sim = Sim::start();
    trot_sim.set_rain(0.0);
    trot_sim.run(2);
    trot_sim.key_down('W');
    trot_sim.press(Key::ControlLeft);
    trot_sim.run(30);
    checks.check(
        "control trots",
        trot_sim.goat().mode == Mode::Trot && trot_sim.playing("trot"),
        trot_sim.goat().mode,
    );

    // A run covers more ground than a walk in the same time.
    checks.check(
        "running is faster than walking",
        run_sim.goat().x > trot_sim.goat().x && trot_sim.goat().x > 0.0,
        (run_sim.goat().x, trot_sim.goat().x),
    );
    checks.finish();
}

#[test]
fn a_and_d_turn_the_goat() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    let yaw0 = sim.goat().yaw;

    sim.key_down('A');
    sim.run(30);
    let left = sim.goat().yaw;
    checks.check("A turns left (yaw up)", left > yaw0, (yaw0, left));

    sim.key_up('A');
    sim.key_down('D');
    sim.run(30);
    let right = sim.goat().yaw;
    checks.check("D turns right (yaw down)", right < left, (left, right));
    checks.finish();
}

#[test]
fn a_jump_carries_and_lands() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    // Walk first, so the jump is the moving take-off the original's does (a standing
    // jump has nothing to carry).
    sim.key_down('W');
    sim.run(30);
    let energy = sim.goat().energy;
    let x0 = sim.goat().x;

    sim.press(Key::Space);
    sim.step(DT);
    sim.release(Key::Space);
    checks.check(
        "space takes off",
        sim.goat().mode == Mode::Jump,
        sim.goat().mode,
    );
    checks.check(
        "the jump plays the jump clip",
        sim.playing("jump"),
        sim.clip(),
    );
    checks.check(
        "and costs energy",
        sim.goat().energy < energy,
        (energy, sim.goat().energy),
    );

    // Its vertical arc is the clip's own root motion, so the root stays on the ground;
    // what carries is the frozen take-off speed, until the clip ends and it lands.
    let mut landed = false;
    for _ in 0..180 {
        sim.run(1);
        if sim.goat().mode != Mode::Jump {
            landed = true;
            break;
        }
    }
    checks.check(
        "the jump carries it forward",
        sim.goat().x > x0 + 0.05,
        (x0, sim.goat().x),
    );
    checks.check(
        "and it lands back in the gait",
        landed && sim.goat().mode == Mode::Walk,
        sim.goat().mode,
    );
    checks.finish();
}

#[test]
fn grazing_tops_up_energy_and_belly() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    sim.set_energy(20.0);

    let on_tuft = sim.place_goat_on_tuft(4.0, 4.0);
    checks.check("the spawn meadow grows a tuft to eat", on_tuft, on_tuft);
    sim.run(2);
    let energy = sim.goat().energy;
    let satiety = sim.goat().satiety;

    sim.key_down('E');
    sim.step(DT);
    sim.key_up('E');
    checks.check(
        "E grazes a tuft",
        sim.goat().mode == Mode::Eat,
        sim.goat().mode,
    );
    checks.check(
        "a bite restores energy",
        sim.goat().energy > energy + 4.0,
        (energy, sim.goat().energy),
    );
    checks.check(
        "and fills the belly",
        sim.goat().satiety > satiety,
        (satiety, sim.goat().satiety),
    );

    sim.run(180);
    checks.check(
        "the meal ends when its clip does",
        sim.goat().mode != Mode::Eat,
        sim.goat().mode,
    );
    checks.finish();
}

#[test]
fn exhaustion_caps_the_gait_and_kills() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    sim.set_energy(0.0);
    sim.key_down('W');
    sim.press(Key::ShiftLeft);
    sim.run(5);

    checks.check(
        "running on empty exhausts the goat",
        sim.goat().exhausted,
        sim.goat().exhausted,
    );
    checks.check(
        "an exhausted goat is capped to a walk",
        sim.goat().mode == Mode::Walk,
        sim.goat().mode,
    );

    // Exhausted and moving, it burns health until it drops.
    sim.run(2400);
    checks.check(
        "burning health on empty kills it",
        sim.goat().mode == Mode::Dead,
        (sim.goat().mode, sim.goat().health),
    );

    // `R` starts a fresh life back at the origin.
    sim.key_up('W');
    sim.release(Key::ShiftLeft);
    sim.key_down('R');
    sim.step(DT);
    sim.key_up('R');
    checks.check(
        "R restarts the goat",
        sim.goat().mode != Mode::Dead,
        sim.goat().mode,
    );
    checks.check(
        "at the origin with full stats",
        sim.goat().x == 0.0 && sim.goat().z == 0.0 && sim.goat().health > 99.0,
        (sim.goat().x, sim.goat().z, sim.goat().health),
    );
    checks.finish();
}
