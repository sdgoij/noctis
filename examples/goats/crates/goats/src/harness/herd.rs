//! The herd cases (`.notes/harness.md`): the bots roam around the goat, no two goats
//! overlap, and a bang the herd walks into hurts, kills and finally gives up its
//! dead.

use super::{Checks, Sim};

#[test]
fn the_herd_roams_and_never_overlaps() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);

    checks.check("the herd is spawned", sim.herd() >= 4, sim.herd());

    // Over fifteen seconds the herd moves — at least one bot takes a stride — and the
    // separation keeps any two goats from sitting inside each other.
    let mut moved = false;
    let mut min_gap = f32::INFINITY;
    for _ in 0..900 {
        sim.run(1);
        moved |= sim.herd_moving() > 0;
        min_gap = min_gap.min(sim.herd_min_gap());
    }
    checks.check("some bot took a stride", moved, moved);
    checks.check("no two goats overlap", min_gap > -0.2, min_gap);
    checks.finish();
}

#[test]
fn a_bang_flings_and_kills_a_bot_and_it_gets_up() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);

    let bot = sim.herd_states()[0];
    checks.check(
        "a bot starts whole",
        !bot.dead && bot.health > 99.0,
        bot.health,
    );

    // One bang on the bot is a throw and a bruise, not a death: the herd takes the
    // player's own curve (45 at the centre) without the player's floor, so one hit is
    // under the ceiling. A lethal bang, by contrast, kills instead of throwing.
    sim.blast_herd_at(bot.x, bot.z, 1.0);
    let hit = sim.herd_states()[0];
    checks.check("one bang threw the bot", hit.flung, hit.flung);
    checks.check(
        "but left it alive and hurt",
        !hit.dead && hit.health < 100.0,
        (hit.dead, hit.health),
    );

    // ...and the arc carries it off the ground the next frame.
    sim.run(1);
    let air = sim.herd_states()[0];
    checks.check("it left the ground", air.py > 0.0, air.py);

    // Two more on the centre kill it (the player's floor would not have).
    sim.blast_herd_at(air.x, air.z, 1.0);
    sim.blast_herd_at(air.x, air.z, 1.0);
    checks.check(
        "three centre bangs killed a bot",
        sim.herd_dead() >= 1,
        sim.herd_dead(),
    );

    // It lies there, then gets up somewhere else (`herd.deathLinger`, 8 s).
    sim.run(8 * 60 + 30);
    checks.check("the bot got up", sim.herd_dead() == 0, sim.herd_dead());
    let back = sim.herd_states()[0];
    checks.check(
        "and is whole again",
        !back.dead && back.health > 99.0,
        back.health,
    );
    checks.finish();
}

#[test]
fn the_herd_grows_and_shrinks() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    checks.check("the herd starts spawned", sim.herd() >= 4, sim.herd());

    // The settings slider's own command (`setHerdSize`): unload the goats past the
    // count, and load the ones short of it.
    sim.set_herd_size(3);
    checks.check("the herd shrank to three", sim.herd() == 3, sim.herd());
    sim.run(30);
    checks.check("and stays three as it runs", sim.herd() == 3, sim.herd());
    checks.check(
        "and the three still do not overlap",
        sim.herd_min_gap() > -0.2,
        sim.herd_min_gap(),
    );

    let cap = crate::herd::HERD_MAX;
    sim.set_herd_size(cap);
    checks.check("the herd grew to the cap", sim.herd() == cap, sim.herd());
    sim.run(30);
    checks.check(
        "and the grown herd does not overlap either",
        sim.herd_min_gap() > -0.2,
        sim.herd_min_gap(),
    );

    sim.set_herd_size(0);
    checks.check("an empty herd is allowed", sim.herd() == 0, sim.herd());
    sim.run(5);
    checks.check(
        "and the world still runs without one",
        sim.herd() == 0,
        sim.herd(),
    );
    checks.finish();
}
