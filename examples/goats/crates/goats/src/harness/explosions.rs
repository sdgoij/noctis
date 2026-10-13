//! The explosion cases (`.notes/harness.md`): a bang at the goat's feet, the fuse
//! a mine runs, and the spawn's safety — the *whole-scene* behaviour the unit tests
//! in `hazards.rs` do not already hold. Those test `blast` and `spend_device`
//! directly; these drive the world through the [`Sim`] frame, so the ground rebuild,
//! the arc the root flies and the field's own bookkeeping are all in play.

use super::{Checks, Sim};
use crate::Mode;

/// The safe disc around the spawn, in metres (`hazards::SAFE`).
const SAFE: f32 = 8.0;

#[test]
fn a_blast_flings_the_goat_and_dishes_a_crater() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    sim.place_goat(10.0, 10.0);
    sim.run(2);

    let (x, z) = (sim.goat().x, sim.goat().z);
    let health = sim.goat().health;
    let ground = sim.craters().surface(x, z);

    sim.blast_at(x, z);
    checks.check(
        "the bang dished a crater",
        sim.craters().dip_at(x, z) < 0.0,
        sim.craters().dip_at(x, z),
    );
    checks.check(
        "it hurt the goat",
        sim.goat().health < health,
        (health, sim.goat().health),
    );
    checks.check(
        "but never below the floor: a mine cannot end the run",
        sim.goat().health >= 1.0,
        sim.goat().health,
    );
    checks.check(
        "and flung it",
        sim.goat().mode == Mode::Flung,
        sim.goat().mode,
    );

    // Ride the arc: the root climbs clear of the ground, then comes back to it.
    let mut peak = sim.goat_y();
    let mut landed = false;
    for _ in 0..300 {
        sim.run(1);
        peak = peak.max(sim.goat_y());
        if sim.goat().mode != Mode::Flung {
            landed = true;
            break;
        }
    }
    checks.check(
        "the arc carries it off the ground",
        peak > ground + 2.0,
        (ground, peak),
    );
    checks.check(
        "and it touches down",
        landed,
        (sim.goat().mode, sim.goat_y()),
    );
    checks.finish();
}

#[test]
fn a_blast_out_of_reach_is_only_a_hole() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    let health = sim.goat().health;
    let mode = sim.goat().mode;

    // Well beyond `BLAST_RADIUS`, and with no frame stepped, so the case is the
    // blast's own reach and nothing the ground it digs might then trigger.
    sim.blast_at(20.0, 20.0);
    checks.check(
        "the far bang still dug a hole",
        sim.craters().dip_at(20.0, 20.0) < 0.0,
        sim.craters().dip_at(20.0, 20.0),
    );
    checks.check(
        "but did not hurt the goat",
        sim.goat().health == health,
        (health, sim.goat().health),
    );
    checks.check(
        "and did not move it",
        sim.goat().mode == mode,
        sim.goat().mode,
    );
    checks.finish();
}

#[test]
fn the_spawn_disc_holds_no_mines() {
    let mut checks = Checks::new();
    let sim = Sim::start();

    // Nothing armed within the safe disc, or `restart` would be a lie. The distance
    // is to the cell's nearest edge, exactly as the field derives it.
    let mut armed = 0;
    for cx in -5i32..=5 {
        for cz in -5i32..=5 {
            let (x, z) = Sim::cell_centre(cx, cz);
            let dx = (x.abs() - 1.0).max(0.0);
            let dz = (z.abs() - 1.0).max(0.0);
            if dx * dx + dz * dz < SAFE * SAFE && sim.mine_armed(cx, cz) {
                armed += 1;
            }
        }
    }
    checks.check("the spawn's disc holds no mines", armed == 0, armed);
    checks.finish();
}

#[test]
fn a_mine_trips_fuses_and_bangs() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);

    let Some((mx, mz)) = sim.first_armed_mine() else {
        // The field is a pure function of position, so this cannot happen.
        panic!("the derived field has an armed mine in the near field");
    };
    let (cx, cz) = (((mx - 1.0) / 2.0) as i32, ((mz - 1.0) / 2.0) as i32);

    // Stand on it; the fuse is under a fifth of a second, so step until it lands.
    sim.place_goat(mx, mz);
    let mut banged = false;
    for _ in 0..40 {
        sim.run(1);
        if sim.craters().dip_at(mx, mz) < 0.0 {
            banged = true;
            break;
        }
    }
    checks.check(
        "the mine the goat stepped on went off",
        banged,
        sim.craters().dip_at(mx, mz),
    );
    checks.check(
        "and flung the goat",
        sim.goat().mode == Mode::Flung,
        sim.goat().mode,
    );
    checks.check(
        "its cell is spent for the session",
        !sim.mine_armed(cx, cz),
        sim.mine_armed(cx, cz),
    );
    checks.finish();
}

#[test]
fn a_bang_flares_the_flash_and_knocks_the_camera() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);

    checks.check(
        "the flash starts dark",
        sim.flash_intensity() == 0.0,
        sim.flash_intensity(),
    );
    checks.check("the camera starts steady", sim.shake() == 0.0, sim.shake());

    let Some((mx, mz)) = sim.first_armed_mine() else {
        panic!("the derived field has an armed mine in the near field");
    };
    // Stand on it; step until the bang lands (a crater appears where it was).
    sim.place_goat(mx, mz);
    let mut banged = false;
    for _ in 0..40 {
        sim.run(1);
        if sim.craters().dip_at(mx, mz) < 0.0 {
            banged = true;
            break;
        }
    }
    checks.check("the mine banged", banged, sim.craters().dip_at(mx, mz));
    checks.check(
        "the bang lit the flash",
        sim.flash_intensity() > 0.0,
        sim.flash_intensity(),
    );
    checks.check("and knocked the camera", sim.shake() > 0.0, sim.shake());

    // Both ring out. The goat is flung clear for well over this window, so no second
    // bang can land and re-light the flash before it has burned down.
    sim.run(20);
    checks.check(
        "the flash burns out",
        sim.flash_intensity() == 0.0,
        sim.flash_intensity(),
    );
    checks.check("and the camera settles", sim.shake() == 0.0, sim.shake());
    checks.finish();
}

#[test]
fn a_bang_shows_fire_smoke_and_grit() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    checks.check(
        "the fx pool starts quiet",
        sim.fx_live() == 0,
        sim.fx_live(),
    );

    let Some((mx, mz)) = sim.first_armed_mine() else {
        panic!("the derived field has an armed mine in the near field");
    };
    sim.place_goat(mx, mz);
    let mut banged = false;
    for _ in 0..40 {
        sim.run(1);
        if sim.craters().dip_at(mx, mz) < 0.0 {
            banged = true;
            break;
        }
    }
    checks.check("the mine banged", banged, sim.craters().dip_at(mx, mz));
    let at_bang = sim.fx_live();
    checks.check("the bang threw fire, smoke and grit", at_bang >= 4, at_bang);
    checks.check(
        "and drew their sprites",
        sim.visible_sprites() > 0,
        sim.visible_sprites(),
    );

    // The fireball burns out first. The goat is airborne for the whole window (the
    // fling is ~0.9 s), so no second bang can land and throw more.
    sim.run(35);
    checks.check(
        "the fireball burns out first",
        sim.fx_live() < at_bang,
        (at_bang, sim.fx_live()),
    );
    checks.finish();
}

#[test]
fn an_armed_mine_shows_its_tell() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();
    sim.set_rain(0.0);
    sim.run(2);
    let Some((mx, mz)) = sim.first_armed_mine() else {
        panic!("the derived field has an armed mine in the near field");
    };

    // Within the tell (3 m) but clear of the trigger (0.6 m), asleep so nothing trips.
    sim.place_goat_asleep(mx + 1.5, mz + 1.5);
    sim.run(1);
    checks.check("the fx pool stays quiet", sim.fx_live() == 0, sim.fx_live());
    checks.check(
        "an armed mine in reach shows its tell",
        sim.visible_sprites() >= 1,
        sim.visible_sprites(),
    );

    // The spawn's safe disc holds no mines, so out of range the tell is gone.
    sim.place_goat_asleep(0.0, 0.0);
    sim.run(1);
    checks.check(
        "and hides out of range",
        sim.visible_sprites() == 0,
        sim.visible_sprites(),
    );
    checks.finish();
}
