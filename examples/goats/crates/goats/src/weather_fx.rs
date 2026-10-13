//! The weather's look (M5): the cloud puffs and the rain streaks, ported from the
//! original's `weather.js`.
//!
//! Both are world-space sprites on the engine's billboard path — the same path the
//! bang's fire uses. The clouds drift on the wind and wrap around the goat; the
//! rain falls in a box that follows the goat. The original draws rain as a
//! screen-space overlay; the port puts it in the world, so terrain occludes it.

use crate::fx::Visual;
use crate::terrain::hash;
use noctis::prelude::*;

/// The cloud puffs in the ring (`buildClouds`).
pub const CLOUD_COUNT: usize = 46;
/// A cloud is recycled this far from the goat, in metres (`CLOUD_WRAP`).
const CLOUD_WRAP: f32 = 100.0;
/// The clouds drift slower than the ground wind (`cloudDrift`).
const CLOUD_DRIFT: f32 = 0.35;

/// Rain streaks at full rain (`rainMax`).
pub const RAIN_COUNT: usize = 160;
/// The rain box follows the goat; this is its half-width, in metres.
const RAIN_HALF: f32 = 12.0;
/// The top of the rain box, in metres.
const RAIN_TOP: f32 = 15.0;
/// Rain falls this fast, in metres per second, before the drop's own speed.
const RAIN_FALL: f32 = 22.0;

/// One drifting cloud puff: an offset from the goat, its height, size and the
/// cover above which it is hidden.
#[derive(Clone, Copy, Debug, Default)]
struct Cloud {
    ox: f32,
    oz: f32,
    y: f32,
    size: f32,
    cover: f32,
}

/// The cloud ring.
pub struct Clouds {
    puffs: Vec<Cloud>,
    dust: AssetHandle<Texture>,
}

impl Clouds {
    /// A ring of puffs laid out from the seed hash (`buildClouds`), drawn with the
    /// soft `dust` puff.
    pub fn ring(dust: AssetHandle<Texture>) -> Clouds {
        let puffs = (0..CLOUD_COUNT)
            .map(|i| {
                let i = i as f32;
                let a = hash(i * 2.1) * std::f32::consts::TAU;
                let r = 26.0 + hash(i * 3.3) * 74.0;
                Cloud {
                    ox: a.cos() * r,
                    oz: a.sin() * r,
                    y: 26.0 + hash(i * 5.5) * 22.0,
                    size: 12.0 + hash(i * 7.1) * 22.0,
                    cover: hash(i * 9.9),
                }
            })
            .collect();
        Clouds { puffs, dust }
    }

    /// Drift on the wind and wrap around the goat (`updateClouds`).
    pub fn update(&mut self, dt: f32, wind: (f32, f32)) {
        let (dx, dz) = (wind.0 * CLOUD_DRIFT * dt, wind.1 * CLOUD_DRIFT * dt);
        for cloud in &mut self.puffs {
            cloud.ox += dx;
            cloud.oz += dz;
            if cloud.ox > CLOUD_WRAP {
                cloud.ox -= 2.0 * CLOUD_WRAP;
            } else if cloud.ox < -CLOUD_WRAP {
                cloud.ox += 2.0 * CLOUD_WRAP;
            }
            if cloud.oz > CLOUD_WRAP {
                cloud.oz -= 2.0 * CLOUD_WRAP;
            } else if cloud.oz < -CLOUD_WRAP {
                cloud.oz += 2.0 * CLOUD_WRAP;
            }
        }
    }

    /// The puffs to draw at this cover, positioned about `(goat_x, goat_z)`
    /// (`drawClouds`). A cloud above the cover is hidden; a fresh one grows in.
    pub fn visuals(&self, goat_x: f32, goat_z: f32, cloudiness: f32) -> Vec<Visual> {
        let mut out = Vec::new();
        if cloudiness < 0.04 {
            return out;
        }
        for cloud in &self.puffs {
            if cloud.cover > cloudiness {
                continue;
            }
            let grow = (1.0f32).min((cloudiness - cloud.cover) / 0.3 + 0.4);
            out.push(Visual {
                billboard: Billboard {
                    texture: self.dust,
                    size: math::vec2(cloud.size, cloud.size * 0.62),
                    color: [0.64, 0.66, 0.71, 0.75 * cloudiness * grow],
                    blend: BillboardBlend::Alpha,
                    depth_test: true,
                    ..Billboard::default()
                },
                position: math::vec3(goat_x + cloud.ox, cloud.y, goat_z + cloud.oz),
            });
        }
        out
    }
}

/// One falling rain streak: an offset from the goat, its height, and its own fall
/// speed.
#[derive(Clone, Copy, Debug, Default)]
struct Drop {
    ox: f32,
    oz: f32,
    y: f32,
    speed: f32,
}

/// The rain.
pub struct Rain {
    drops: Vec<Drop>,
    dust: AssetHandle<Texture>,
}

impl Rain {
    /// The streak field, seeded once (`buildRain`), drawn with the soft `dust`
    /// puff stretched into a streak.
    pub fn field(dust: AssetHandle<Texture>) -> Rain {
        let drops = (0..RAIN_COUNT)
            .map(|i| {
                let i = i as f32;
                Drop {
                    ox: (hash(i * 1.7) * 2.0 - 1.0) * RAIN_HALF,
                    oz: (hash(i * 2.9) * 2.0 - 1.0) * RAIN_HALF,
                    y: hash(i * 3.9) * RAIN_TOP,
                    speed: 0.6 + hash(i * 4.3),
                }
            })
            .collect();
        Rain { drops, dust }
    }

    /// Fall and wrap one frame, drifting with the wind (`rainFall`). The box
    /// follows the goat, so this only moves the drops within it.
    pub fn update(&mut self, dt: f32, wind: (f32, f32)) {
        let (dx, dz) = (wind.0 * 0.03 * dt, wind.1 * 0.03 * dt);
        for drop in &mut self.drops {
            drop.y = fall(drop.y, drop.speed, dt);
            drop.ox += dx;
            drop.oz += dz;
            if drop.ox > RAIN_HALF {
                drop.ox -= 2.0 * RAIN_HALF;
            } else if drop.ox < -RAIN_HALF {
                drop.ox += 2.0 * RAIN_HALF;
            }
            if drop.oz > RAIN_HALF {
                drop.oz -= 2.0 * RAIN_HALF;
            } else if drop.oz < -RAIN_HALF {
                drop.oz += 2.0 * RAIN_HALF;
            }
        }
    }

    /// The streaks to draw at this rain strength, positioned about
    /// `(goat_x, goat_z)` (`drawRain`). Only `rain * RAIN_COUNT` are shown.
    pub fn visuals(&self, goat_x: f32, goat_z: f32, rain: f32, ground: f32) -> Vec<Visual> {
        let active = ((RAIN_COUNT as f32 * rain).round() as usize).min(RAIN_COUNT);
        self.drops[..active]
            .iter()
            .map(|drop| Visual {
                billboard: Billboard {
                    texture: self.dust,
                    // A thin, near-vertical streak; the atlas stretches to it.
                    size: math::vec2(0.03, 0.5 + 0.15 * drop.speed),
                    color: [0.71, 0.79, 0.91, 0.5],
                    uv: [0.0, 0.0, 1.0, 1.0],
                    blend: BillboardBlend::Alpha,
                    depth_test: true,
                    ..Billboard::default()
                },
                position: math::vec3(goat_x + drop.ox, ground + drop.y, goat_z + drop.oz),
            })
            .collect()
    }
}

/// One drop's height a frame later: it falls, and wraps to the top at the ground
/// (`rainFall`).
fn fall(y: f32, speed: f32, dt: f32) -> f32 {
    let next = y - (RAIN_FALL * (0.7 + speed)).min(40.0) * dt;
    if next < 0.0 { next + RAIN_TOP } else { next }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clouds_hide_under_thin_cover_and_show_under_thick() {
        let mut clouds = Clouds::ring(AssetHandle::default());
        clouds.update(0.0, (0.0, 0.0));
        assert!(
            clouds.visuals(0.0, 0.0, 0.0).is_empty(),
            "a clear sky draws no clouds"
        );
        let full = clouds.visuals(0.0, 0.0, 1.0);
        assert_eq!(full.len(), CLOUD_COUNT, "full cover draws the whole ring");
        assert!(full.len() > clouds.visuals(0.0, 0.0, 0.4).len());
    }

    #[test]
    fn clouds_drift_on_the_wind_and_wrap() {
        let mut clouds = Clouds::ring(AssetHandle::default());
        let before = clouds.visuals(0.0, 0.0, 1.0);
        let first = before[0].position;
        clouds.update(10.0, (5.0, 0.0));
        let after = clouds.visuals(0.0, 0.0, 1.0);
        // Some cloud moved, and none is outside the wrap ring.
        assert!(after.iter().any(|v| (v.position.x - first.x).abs() > 1.0));
        assert!(after.iter().all(|v| v.position.x.abs() <= CLOUD_WRAP + 1.0));
    }

    #[test]
    fn rain_scales_with_its_strength() {
        let rain = Rain::field(AssetHandle::default());
        assert!(rain.visuals(0.0, 0.0, 0.0, 0.0).is_empty(), "dry is empty");
        assert_eq!(
            rain.visuals(0.0, 0.0, 1.0, 0.0).len(),
            RAIN_COUNT,
            "full rain draws every streak"
        );
        assert!(rain.visuals(0.0, 0.0, 0.5, 0.0).len() < RAIN_COUNT);
    }

    #[test]
    fn rain_falls_and_wraps() {
        // A mid-air drop comes down.
        assert!(fall(10.0, 0.6, 1.0 / 60.0) < 10.0);
        // A drop at the ground wraps to the top instead of sinking.
        let wrapped = fall(0.05, 0.6, 1.0 / 60.0);
        assert!(wrapped > RAIN_TOP - 1.0, "wrapped to the top: {wrapped}");
    }

    #[test]
    fn rain_stays_in_its_box() {
        let mut rain = Rain::field(AssetHandle::default());
        for _ in 0..600 {
            rain.update(1.0 / 60.0, (8.0, -3.0));
        }
        for visual in rain.visuals(0.0, 0.0, 1.0, 0.0) {
            assert!(visual.position.y >= 0.0 && visual.position.y <= RAIN_TOP + 1.0);
            assert!(visual.position.x.abs() <= RAIN_HALF + 1.0);
        }
    }
}
