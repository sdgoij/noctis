//! The day/night cases (`.notes/harness.md`): the clock, and the light, the sky and
//! the ambient that follow it (`celestial`).
//!
//! The pure curve is unit-tested in `celestial`; these drive the *world*: the scene's
//! own sun obeys the clock, high noon plays the tuning the project dialled in, and
//! dusk dims and cools everything.

use super::{Checks, Sim};
use crate::celestial;

#[test]
fn the_clock_drives_the_light_and_the_ambient() {
    let mut checks = Checks::new();
    let mut sim = Sim::start();

    // The world opens in the morning, at the hour the original starts.
    checks.check(
        "it starts at the scene's hour",
        (sim.hour() - celestial::START_HOUR).abs() < 1e-4,
        sim.hour(),
    );

    // The scene's directional light is the one the clock drives: by day it shines
    // down (the direction the engine wants is the one the light travels in).
    let (direction, _, intensity) = sim.sun().expect("the scene has a sun");
    checks.check("the sun shines down", direction.y < 0.0, direction);
    checks.check("and lights the world", intensity > 0.0, intensity);

    // An hour of game time takes ten real seconds (`DAY_LENGTH` is a four-minute day).
    // Sixty 60 Hz frames are a second of game time.
    sim.set_hour(6.0);
    sim.run(60);
    checks.check(
        "the clock runs at a day per four minutes",
        (sim.hour() - 6.1).abs() < 1e-2,
        sim.hour(),
    );

    // High noon is the curve's unit: the scene's own light, and the tuned ambient
    // exactly as the project set it — the day/night curve must not disturb it.
    sim.set_hour(12.0);
    sim.step(1.0 / 60.0);
    let noon = sim.sun().expect("a sun");
    let noon_sky = sim.sky_uniforms();
    let noon_ambient = sim.ambient().expect("the frame published an ambient");
    checks.check(
        "noon keeps the tuned ambient",
        (noon_ambient[0] - 1.0).abs() < 1e-4 && (noon_ambient[3] - 1.0).abs() < 1e-4,
        noon_ambient,
    );
    checks.check(
        "noon keeps the scene's own intensity",
        (noon.2 - 2.6).abs() < 1e-3,
        noon.2,
    );

    // Night: the light has moved to the other side of the sky, dim and cool; the sky
    // and the ambient follow it down.
    sim.set_hour(22.0);
    sim.step(1.0 / 60.0);
    let night = sim.sun().expect("a moon");
    let night_sky = sim.sky_uniforms();
    let night_ambient = sim.ambient().expect("an ambient");
    checks.check(
        "the light has moved across the sky",
        (night.0.x - noon.0.x).abs() > 0.1,
        (noon.0, night.0),
    );
    checks.check("the moon shines down too", night.0.y < 0.0, night.0);
    checks.check(
        "the night is dimmer than noon",
        night.2 < 0.5 * noon.2,
        (noon.2, night.2),
    );
    checks.check("the night light is cool", night.1.z > night.1.x, night.1);
    checks.check(
        "the night sky darkens",
        night_sky[2] < noon_sky[2] && night_sky[2] >= 0.0,
        (noon_sky[2], night_sky[2]),
    );
    checks.check(
        "the sun loses the power to glow at night",
        night_sky[15] < 0.5 * noon_sky[15],
        (noon_sky[15], night_sky[15]),
    );
    checks.check(
        "the night ambient dims and cools",
        night_ambient[0] < 0.8 && night_ambient[2] > night_ambient[0],
        night_ambient,
    );

    // And the clock wraps rather than running away (two more seconds of game time
    // carry it past midnight).
    sim.set_hour(23.9);
    sim.run(120);
    checks.check(
        "the clock wraps at midnight",
        (0.0..24.0).contains(&sim.hour()) && sim.hour() < 1.0,
        sim.hour(),
    );
    checks.finish();
}
