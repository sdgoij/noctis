//! The weather cases (`.notes/harness.md`): the state machine the `C` key steps,
//! rain slowing and wetting the goat, and a full belly sheltering it — the
//! *whole-scene* read-out, driven through the [`Sim`] frame rather than the pure
//! functions the `weather.rs` unit tests already cover.

use super::{Checks, Sim};

const DT: f32 = 1.0 / 60.0;

/// The weather state the HUD is naming — the first word of the second line.
fn state(sim: &Sim) -> String {
    sim.weather_line()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string()
}

#[test]
fn c_cycles_the_weather_states() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    checks.check("the world opens clear", state(&sim) == "clear", state(&sim));

    // `C` forces the next state, so four taps walk the whole cycle.
    let mut seen = Vec::new();
    for _ in 0..4 {
        sim.key_down('C');
        sim.step(DT);
        sim.key_up('C');
        seen.push(state(&sim));
    }
    checks.check(
        "C steps clear -> cloudy -> rain -> clearing -> clear",
        seen == ["cloudy", "rain", "clearing", "clear"],
        seen,
    );
    checks.finish();
}

#[test]
fn rain_slows_and_wets_the_goat() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(1.0);
    sim.run(30);

    let weather = sim.weather();
    checks.check("the rain is falling", weather.rain() > 0.5, weather.rain());
    checks.check(
        "rain slows the goat",
        weather.speed(0.0) < 1.0,
        weather.speed(0.0),
    );
    checks.check(
        "and wets it, burning energy faster",
        weather.drain() > 1.0,
        weather.drain(),
    );
    checks.check(
        "a full belly shelters some of the slowdown",
        weather.speed(1.0) > weather.speed(0.0),
        (weather.speed(0.0), weather.speed(1.0)),
    );
    checks.finish();
}

#[test]
fn a_wet_goat_covers_less_ground() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();

    // A clear walk for a fixed time.
    sim.set_rain(0.0);
    sim.run(30);
    sim.place_goat(0.0, 0.0);
    sim.key_down('W');
    sim.run(60);
    let dry = sim.goat().x;

    // The same walk in a downpour.
    sim.key_up('W');
    sim.set_rain(1.0);
    sim.run(120);
    sim.place_goat(0.0, 0.0);
    sim.key_down('W');
    sim.run(60);
    let wet = sim.goat().x;

    checks.check("the dry walk moved", dry > 0.5, dry);
    checks.check("the wet walk covers less ground", wet < dry, (dry, wet));
    checks.finish();
}
