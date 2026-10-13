//! The herd cases (`.notes/harness.md`): the bots roam around the goat, and no two
//! goats overlap.

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
