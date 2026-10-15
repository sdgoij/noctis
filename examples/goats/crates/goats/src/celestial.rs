//! The day/night clock and the sky's curve — the original's `world.js`
//! (`SKY_KEYS`, `skySample`, `updateAmbient`) and `lighting.js` (`updateLight`).
//!
//! The port keeps its own **daytime** tuning: the sky palette, the scene light's
//! authored colour and intensity, and the ambient, because the engine's exposure and
//! image-based lighting needed them (`noctis.toml`). So this module supplies the
//! original's *shape* — the hour's light factor, the sun's arc, which body lights the
//! scene, and every colour *relative to high noon*. The port scales its own tuning by
//! those factors, and at noon each is exactly 1: the tuned look is what plays, and
//! dawn, dusk and night come from the original's curve.

use math::vec3;
use noctis::prelude::*;

/// Real seconds for one 24-hour day (`TUNING.world.dayLength`).
pub const DAY_LENGTH: f32 = 240.0;
/// The clock's multiplier while the time accelerator is held (`TUNING.world.timeFast`).
pub const TIME_FAST: f32 = 40.0;
/// The hour a new world starts at (`world.js` `worldTime`).
pub const START_HOUR: f32 = 8.0;
/// Below this daylight the cold bites (`TUNING.world.nightDrainMult`).
pub const NIGHT_LIGHT: f32 = 0.25;
/// How much faster a goat's energy drains in the cold.
pub const NIGHT_DRAIN: f32 = 1.6;

/// The cold's multiplier on a goat's energy drain (`TUNING.world.nightDrainMult`):
/// `1` in daylight, harder at night.
pub fn cold_multiplier(light: f32) -> f32 {
    if light < NIGHT_LIGHT {
        NIGHT_DRAIN
    } else {
        1.0
    }
}

/// The sky's keyframes (`SKY_KEYS`): hour, zenith rgb, horizon rgb, light.
#[allow(clippy::type_complexity)]
const KEYS: [(f32, [f32; 3], [f32; 3], f32); 10] = [
    (0.0, [8.0, 10.0, 28.0], [16.0, 20.0, 44.0], 0.0),
    (4.5, [12.0, 14.0, 34.0], [26.0, 28.0, 54.0], 0.0),
    (6.0, [70.0, 70.0, 120.0], [190.0, 120.0, 90.0], 0.25),
    (7.5, [120.0, 165.0, 220.0], [235.0, 190.0, 150.0], 0.8),
    (12.0, [110.0, 170.0, 240.0], [175.0, 210.0, 245.0], 1.0),
    (17.0, [120.0, 165.0, 220.0], [235.0, 200.0, 160.0], 0.85),
    (19.0, [90.0, 80.0, 130.0], [225.0, 130.0, 90.0], 0.35),
    (20.5, [26.0, 28.0, 58.0], [70.0, 60.0, 90.0], 0.08),
    (22.0, [10.0, 12.0, 30.0], [18.0, 22.0, 46.0], 0.0),
    (24.0, [8.0, 10.0, 28.0], [16.0, 20.0, 44.0], 0.0),
];

/// The sun's tilt off the XZ plane, so its arc is not edge-on (`updateLight`).
const ARC_TILT: f32 = 0.18;
/// The hour every relative factor is measured against.
const NOON: f32 = 12.0;

/// This frame's day/night state: the clock's sample and every curve the port scales
/// its tuning by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Daylight {
    /// The hour the sample is at, `[0, 24)`.
    pub hour: f32,
    /// The sky's zenith and horizon colours, the original's own (0..1).
    pub zenith: Vec3,
    /// The horizon's colour, the original's own (0..1).
    pub horizon: Vec3,
    /// The day's light factor: `1` at high noon, `0` through the night.
    pub light: f32,
    /// The sun's direction — the arc its disc sits on.
    pub sun: Vec3,
    /// The direction of whichever body lights the scene: the sun by day, the moon
    /// (the negation, the two sharing one line) after dusk.
    pub light_direction: Vec3,
    /// Whether the sun is up.
    pub day: bool,
    /// The scene light's level, relative to high noon (`1` at noon).
    pub light_level: f32,
    /// The scene light's hue, its strongest channel scaled to `1`.
    pub light_tint: Vec3,
    /// The sky's zenith, relative to noon, per channel (`1` at noon).
    pub zenith_scale: Vec3,
    /// The sky's horizon, relative to noon, per channel.
    pub horizon_scale: Vec3,
    /// The ambient, relative to noon, per channel.
    pub ambient_scale: Vec3,
}

/// Sample the clock at `hour`.
pub fn daylight(hour: f32) -> Daylight {
    let hour = wrap(hour);
    let (zenith, horizon, light) = sample(hour);
    let sun = sun_direction(hour);
    let day = sun.y > 0.0;
    let colour = light_colour_at(hour, light);
    let (noon_zenith, noon_horizon, _) = sample(NOON);

    Daylight {
        hour,
        zenith,
        horizon,
        light,
        sun,
        light_direction: if day { sun } else { -sun },
        day,
        light_level: magnitude(colour) / magnitude(light_colour_at(NOON, 1.0)),
        light_tint: scaled_to_one(colour),
        zenith_scale: per_channel(zenith, noon_zenith),
        horizon_scale: per_channel(horizon, noon_horizon),
        ambient_scale: per_channel(ambient(light), ambient(1.0)),
    }
}

/// The sky curve at `hour`: the zenith and horizon colours (0..1) and the light
/// factor, interpolated between the surrounding keys (`skySample`).
fn sample(hour: f32) -> (Vec3, Vec3, f32) {
    let mut i = 0;
    while i + 2 < KEYS.len() && hour > KEYS[i + 1].0 {
        i += 1;
    }
    let (h0, top0, bot0, light0) = KEYS[i];
    let (h1, top1, bot1, light1) = KEYS[i + 1];
    let span = h1 - h0;
    let t = if span > 0.0 {
        ((hour - h0) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let channel = |a: [f32; 3], b: [f32; 3]| {
        vec3(
            (a[0] + (b[0] - a[0]) * t) / 255.0,
            (a[1] + (b[1] - a[1]) * t) / 255.0,
            (a[2] + (b[2] - a[2]) * t) / 255.0,
        )
    };
    (
        channel(top0, top1),
        channel(bot0, bot1),
        light0 + (light1 - light0) * t,
    )
}

/// The sun's direction at `hour` (`updateLight`): `0` at 06:00, `PI` at 18:00, tilted
/// off the XZ plane so the arc leans.
pub fn sun_direction(hour: f32) -> Vec3 {
    let a = ((hour - 6.0) / 12.0) * std::f32::consts::PI;
    let (x, y) = (a.cos(), a.sin());
    let len = (x * x + y * y + ARC_TILT * ARC_TILT).sqrt();
    vec3(x / len, y / len, ARC_TILT / len)
}

/// The scene light's colour, already scaled by its intensity (`LIGHT_COLOR`).
fn light_colour_at(hour: f32, sky_light: f32) -> Vec3 {
    let sun_y = sun_direction(hour).y;
    let warm = (1.0 - sun_y.abs() / 0.45).clamp(0.0, 1.0);
    if sun_y > 0.0 {
        // By day: warm at the ends of the arc, white at noon.
        let intense = 0.30 + 0.70 * sky_light;
        vec3(
            intense,
            intense * (1.0 - 0.22 * warm),
            intense * (1.0 - 0.55 * warm),
        )
    } else {
        // At night the moon, cool and dim.
        let intense = 0.16;
        vec3(intense * 0.72, intense * 0.80, intense)
    }
}

/// The ambient (`LIGHT_AMBIENT`): a dim, blue-shifted daylight.
fn ambient(sky_light: f32) -> Vec3 {
    vec3(
        0.10 + 0.12 * sky_light,
        0.12 + 0.13 * sky_light,
        0.18 + 0.16 * sky_light,
    )
}

/// Advance the clock by `dt` seconds; `fast` is the accelerator held.
pub fn advance(hour: f32, dt: f32, fast: bool) -> f32 {
    let rate = if fast { TIME_FAST } else { 1.0 };
    wrap(hour + (dt / DAY_LENGTH) * 24.0 * rate)
}

/// `hour` wrapped into `[0, 24)`.
pub fn wrap(hour: f32) -> f32 {
    ((hour % 24.0) + 24.0) % 24.0
}

/// The clock as `hh:mm`, the original's HUD and console format.
pub fn clock_text(hour: f32) -> String {
    let hour = wrap(hour);
    let hh = hour.floor();
    let mm = ((hour - hh) * 60.0).floor();
    format!("{:02}:{:02}", hh as i32, mm as i32)
}

fn magnitude(v: Vec3) -> f32 {
    (v.x * v.x + v.y * v.y + v.z * v.z).sqrt()
}

/// `v` with its strongest channel scaled to `1` — a hue, not a level.
fn scaled_to_one(v: Vec3) -> Vec3 {
    let max = v.x.max(v.y).max(v.z);
    if max > 0.0 {
        vec3(v.x / max, v.y / max, v.z / max)
    } else {
        vec3(0.0, 0.0, 0.0)
    }
}

/// `a / b` per channel, guarding a zero.
fn per_channel(a: Vec3, b: Vec3) -> Vec3 {
    let div = |x: f32, y: f32| if y.abs() > 1e-6 { x / y } else { 1.0 };
    vec3(div(a.x, b.x), div(a.y, b.y), div(a.z, b.z))
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::vec3_approx_eq;

    #[test]
    fn the_curve_passes_through_the_originals_keyframes() {
        // Every key is reproduced where it sits, so the port's curve is the original's.
        for (hour, top, bottom, light) in KEYS {
            let (zenith, horizon, got) = sample(wrap(hour));
            assert!(vec3_approx_eq(
                zenith,
                vec3(top[0] / 255.0, top[1] / 255.0, top[2] / 255.0),
                1e-4
            ));
            assert!(vec3_approx_eq(
                horizon,
                vec3(bottom[0] / 255.0, bottom[1] / 255.0, bottom[2] / 255.0),
                1e-4
            ));
            assert!((got - light).abs() < 1e-4, "{hour}: {got} vs {light}");
        }
    }

    #[test]
    fn the_light_factor_is_noon_high_and_midnight_low() {
        assert!((daylight(12.0).light - 1.0).abs() < 1e-4, "high noon");
        assert!(daylight(0.0).light < 1e-4, "midnight");
        assert!(daylight(22.0).light < 1e-4, "late night");
        // ...and rises through the morning, falls through the evening.
        let dawn = daylight(7.5).light;
        assert!(dawn > 0.7 && dawn < 1.0, "{dawn}");
    }

    #[test]
    fn the_sun_arcs_up_and_the_moon_takes_over() {
        // The sun is highest at noon and below the horizon at night.
        assert!(sun_direction(12.0).y > 0.9);
        assert!(sun_direction(0.0).y < 0.0);
        assert!(daylight(12.0).day);
        assert!(!daylight(22.0).day);

        // By day the scene is lit by the sun; at night by the moon on the same line.
        let noon = daylight(12.0);
        assert!(vec3_approx_eq(noon.light_direction, noon.sun, 1e-6));
        let night = daylight(22.0);
        assert!(vec3_approx_eq(night.light_direction, -night.sun, 1e-6));
        // The moon is up when the sun is down.
        assert!(night.light_direction.y > 0.0);
    }

    #[test]
    fn noon_is_the_unit_and_night_dims_and_cools() {
        let noon = daylight(12.0);
        assert!(
            (noon.light_level - 1.0).abs() < 1e-4,
            "{}",
            noon.light_level
        );
        assert!(vec3_approx_eq(noon.light_tint, vec3(1.0, 1.0, 1.0), 1e-4));
        assert!(vec3_approx_eq(noon.zenith_scale, vec3(1.0, 1.0, 1.0), 1e-4));
        assert!(vec3_approx_eq(
            noon.ambient_scale,
            vec3(1.0, 1.0, 1.0),
            1e-4
        ));

        let night = daylight(22.0);
        assert!(
            night.light_level < 0.4,
            "the night light is dim: {}",
            night.light_level
        );
        // Moonlight is cool: the blue channel leads.
        assert!(night.light_tint.z >= night.light_tint.x);
        // The sky and the ambient both go down, and the ambient goes blue.
        assert!(night.zenith_scale.z < 1.0 && night.horizon_scale.z < 1.0);
        assert!(night.ambient_scale.x < 1.0);
        assert!(night.ambient_scale.z > night.ambient_scale.x);
    }

    #[test]
    fn the_light_warms_toward_dawn_and_dusk() {
        // The original warms the light as the sun crosses the horizon. Exactly 06:00 is
        // the flip itself — `sunY > 0` is false there, so the light is already the
        // moon's — so the warm hours are just inside the day.
        for hour in [6.5, 17.5] {
            let day = daylight(hour);
            assert!(day.day, "{hour} is daytime");
            assert!(
                day.light_tint.x > day.light_tint.y && day.light_tint.y > day.light_tint.z,
                "{hour}: {:?}",
                day.light_tint
            );
        }
        // ...and the flip is exactly at the horizon, where the disc fades out too.
        assert!(daylight(12.0).day && !daylight(6.0).day);
        assert!(daylight(17.9).day && !daylight(18.0).day);
    }

    #[test]
    fn a_day_takes_day_length_seconds() {
        let mut hour = START_HOUR;
        // Half a day: 12 hours round the clock.
        hour = advance(hour, DAY_LENGTH / 2.0, false);
        assert!((wrap(hour) - wrap(START_HOUR + 12.0)).abs() < 1e-3);
        // A whole day comes back to where it started.
        hour = advance(hour, DAY_LENGTH / 2.0, false);
        assert!((wrap(hour) - START_HOUR).abs() < 1e-3, "{hour}");
    }

    #[test]
    fn the_accelerator_runs_the_clock_faster_and_it_wraps() {
        assert!(advance(23.0, 3600.0, false) < 24.0, "an hour is not a day");
        // Ten seconds at the accelerator is most of a day.
        let fast = advance(0.0, 10.0, true);
        assert!(
            (fast - 10.0 / DAY_LENGTH * 24.0 * TIME_FAST) < 1e-3,
            "{fast}"
        );
        // The clock never leaves [0, 24), whatever is fed in.
        for hour in [-30.0, 0.0, 23.999, 48.0] {
            let wrapped = wrap(hour);
            assert!((0.0..24.0).contains(&wrapped), "{hour} -> {wrapped}");
        }
    }

    #[test]
    fn the_night_costs_a_goat_more_energy() {
        assert_eq!(cold_multiplier(0.9), 1.0, "a bright day");
        assert_eq!(cold_multiplier(NIGHT_LIGHT), 1.0, "the threshold itself");
        assert_eq!(cold_multiplier(0.0), NIGHT_DRAIN, "deep night");
        assert_eq!(cold_multiplier(0.1), NIGHT_DRAIN, "just under");
    }

    #[test]
    fn the_clock_reads_like_a_clock() {
        assert_eq!(clock_text(8.0), "08:00");
        assert_eq!(clock_text(13.5), "13:30");
        assert_eq!(clock_text(0.0), "00:00");
        assert_eq!(clock_text(24.0), "00:00");
        assert_eq!(clock_text(23.999), "23:59");
    }
}
