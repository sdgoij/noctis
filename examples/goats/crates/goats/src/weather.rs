//! The weather state machine, ported from the original's `weather.js`.
//!
//! Four states — clear, cloudy, rain, clearing — advance on a jittered hold,
//! with the cloud cover and the rain easing toward each state's target, and a
//! gusting wind over the top. What a scriptless port needs from it is the
//! per-goat effect: rain and wind slow the goat and make it burn energy faster,
//! and a full belly takes some of the rain's edge off.
//!
//! The clouds, the rain streaks and the sky tint are the presentation layer
//! (`.notes/goats.md` M5); only the state and its factors live here.

/// The four weather states (`WEATHER_STATES`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Clear,
    Cloudy,
    Rain,
    Clearing,
}

impl Kind {
    /// The cloud cover this state eases toward, `0..=1`.
    fn cloud(self) -> f32 {
        match self {
            Kind::Clear => 0.05,
            Kind::Cloudy => 0.70,
            Kind::Rain => 1.00,
            Kind::Clearing => 0.40,
        }
    }

    /// The rain this state eases toward, `0..=1`.
    fn rain(self) -> f32 {
        match self {
            Kind::Clear | Kind::Cloudy => 0.0,
            Kind::Rain => 1.0,
            Kind::Clearing => 0.12,
        }
    }

    /// The seconds this state holds, at least and at most (`TUNING.weather.hold`).
    fn hold(self) -> (f32, f32) {
        match self {
            Kind::Clear => (22.0, 45.0),
            Kind::Cloudy => (16.0, 34.0),
            Kind::Rain => (20.0, 40.0),
            Kind::Clearing => (8.0, 16.0),
        }
    }

    /// The states this one may turn into (`WEATHER_NEXT`).
    fn next(self) -> &'static [Kind] {
        match self {
            Kind::Clear => &[Kind::Cloudy],
            Kind::Cloudy => &[Kind::Rain, Kind::Clear],
            Kind::Rain => &[Kind::Clearing],
            Kind::Clearing => &[Kind::Clear, Kind::Cloudy],
        }
    }

    /// The state's name, for the HUD.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Clear => "clear",
            Kind::Cloudy => "cloudy",
            Kind::Rain => "rain",
            Kind::Clearing => "clearing",
        }
    }
}

/// The wind's resting speed, in metres per second (`TUNING.weather.windBase`).
const WIND_BASE: f32 = 1.6;
/// The share of the goat's speed the rain takes at full strength.
const RAIN_SLOW: f32 = 0.28;
/// ...and what a full gust adds on top.
const WIND_SLOW: f32 = 0.07;
/// The share of extra energy drain heavy rain costs.
const WET_DRAIN: f32 = 0.65;
/// ...and what a full gust adds.
const WIND_DRAIN: f32 = 0.20;
/// The `windSway` that counts as a full gust.
const WIND_NORM: f32 = 1.4;
/// The share of the rain slowdown a full belly removes (`TUNING.food.rainShelter`).
const RAIN_SHELTER: f32 = 0.6;

/// The weather: its state, its eased cover and rain, and the wind over it.
#[derive(Debug)]
pub struct Weather {
    kind: Kind,
    /// Seconds left before the state may change.
    timer: f32,
    /// Cloud cover, `0..=1`.
    cloudiness: f32,
    /// Rain strength, `0..=1`.
    rain: f32,
    wind_x: f32,
    wind_z: f32,
    /// The wind's sway strength, `0..~1.4`.
    sway: f32,
    /// The sway clock, in seconds.
    sway_time: f32,
    rng: u32,
}

impl Default for Weather {
    fn default() -> Self {
        // The original's opening line: clear, but already breezy.
        Self {
            kind: Kind::Clear,
            timer: 24.0,
            cloudiness: 0.05,
            rain: 0.0,
            wind_x: 1.0,
            wind_z: 0.2,
            sway: 0.4,
            sway_time: 0.0,
            rng: 0x9e37_79b9,
        }
    }
}

impl Weather {
    /// The grass's sway clock, in seconds.
    pub fn sway_time(&self) -> f32 {
        self.sway_time
    }

    /// The wind's speed, in metres per second.
    pub fn wind_speed(&self) -> f32 {
        (self.wind_x * self.wind_x + self.wind_z * self.wind_z).sqrt()
    }

    /// The eased rain strength, `0..=1` — the water table's level is a function of
    /// this and nothing else.
    pub fn rain(&self) -> f32 {
        self.rain
    }

    /// The goat's ground-speed multiplier — the original's `weatherSpeedFor`.
    /// Rain and wind slow it; a full belly (`satiety`) shelters some of the rain.
    pub fn speed(&self, satiety: f32) -> f32 {
        let rain_slow = RAIN_SLOW * (1.0 - RAIN_SHELTER * satiety.clamp(0.0, 1.0));
        1.0 - (rain_slow + WIND_SLOW * self.wind_norm()) * self.rain
    }

    /// The goat's energy-drain multiplier — the original's `weatherDrain`.
    pub fn drain(&self) -> f32 {
        1.0 + (WET_DRAIN + WIND_DRAIN * self.wind_norm()) * self.rain
    }

    /// Jump straight to the next state's first option (`forceWeather`, the
    /// original's `C`), for previewing the cycle.
    pub fn force_next(&mut self) {
        self.kind = self.kind.next()[0];
        self.timer = self.kind.hold().0;
    }

    /// Advance the wind and the state machine one frame (`updateWind` followed by
    /// `updateWeather`).
    pub fn update(&mut self, dt: f32) {
        // Wind: a slow gusting speed and a drifting direction.
        self.sway_time += dt;
        let t = self.sway_time;
        let gust = 0.55 + 0.35 * (t * 0.7).sin() + 0.2 * (t * 1.9 + 1.3).sin();
        let dir = 0.5 * (t * 0.23).sin();
        let speed = WIND_BASE * gust.max(0.2);
        self.wind_x = dir.cos() * speed;
        self.wind_z = dir.sin() * speed;
        self.sway = (speed / WIND_BASE).min(1.4) * (0.55 + 0.45 * self.cloudiness);

        // The state machine.
        self.timer -= dt;
        if self.timer <= 0.0 {
            let options = self.kind.next();
            let pick = ((self.rnd() * options.len() as f32) as usize) % options.len();
            self.kind = options[pick];
            let (lo, hi) = self.kind.hold();
            self.timer = lo + self.rnd() * (hi - lo);
        }
        let k = (dt * 0.6).min(1.0);
        self.cloudiness += (self.kind.cloud() - self.cloudiness) * k;
        self.rain += (self.kind.rain() - self.rain) * k;
    }

    /// A HUD line: the state, the wind, and how much it is slowing the goat.
    pub fn hud(&self, satiety: f32) -> String {
        let mut text = format!("{}   wind {:.1} m/s", self.kind.name(), self.wind_speed());
        if self.rain > 0.02 {
            text.push_str(&format!("   rain {}%", (self.rain * 100.0).round() as i32));
        }
        let speed = self.speed(satiety);
        if speed < 0.98 {
            text.push_str(&format!(
                "   slowed {}%",
                ((1.0 - speed) * 100.0).round() as i32
            ));
        }
        text
    }

    /// The wind's sway as a fraction of a full gust, `0..=1`.
    fn wind_norm(&self) -> f32 {
        (self.sway / WIND_NORM).min(1.0)
    }

    /// The original's xorshift, so the cycle is the same kind of draw.
    fn rnd(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f32 / 4_294_967_296.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_is_neutral() {
        let weather = Weather::default();
        assert_eq!(weather.speed(0.0), 1.0, "clear does not slow the goat");
        assert_eq!(weather.drain(), 1.0, "clear does not wet the goat");
    }

    #[test]
    fn rain_slows_the_goat_and_wets_it() {
        let weather = Weather {
            kind: Kind::Rain,
            rain: 1.0,
            sway: WIND_NORM,
            ..Weather::default()
        };
        // Full rain, a full gust: 1 - (0.28 + 0.07) slowed, 1 + (0.65 + 0.20) drain.
        assert!(
            (weather.speed(0.0) - 0.65).abs() < 1e-5,
            "{}",
            weather.speed(0.0)
        );
        assert!((weather.drain() - 1.85).abs() < 1e-5, "{}", weather.drain());
        // A full belly shelters some of the rain's slowdown.
        assert!(weather.speed(1.0) > weather.speed(0.0));
        assert!((weather.speed(1.0) - (1.0 - (0.28 * 0.4 + 0.07))).abs() < 1e-5);
    }

    #[test]
    fn the_state_machine_cycles_within_bounds() {
        let mut weather = Weather::default();
        let first = weather.kind;
        let mut changed = false;
        // Two minutes: at least one hold must have expired.
        for _ in 0..(120 * 60) {
            weather.update(1.0 / 60.0);
            if weather.kind != first {
                changed = true;
            }
            assert!((0.0..=1.0).contains(&weather.cloudiness));
            assert!((0.0..=1.0).contains(&weather.rain));
        }
        assert!(changed, "the weather cycled out of clear");
    }

    #[test]
    fn forcing_advances_one_step() {
        let mut weather = Weather::default();
        assert_eq!(weather.kind, Kind::Clear);
        weather.force_next();
        assert_eq!(weather.kind, Kind::Cloudy);
        weather.force_next();
        assert_eq!(weather.kind, Kind::Rain);
        weather.force_next();
        assert_eq!(weather.kind, Kind::Clearing);
    }
}
