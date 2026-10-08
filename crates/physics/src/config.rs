//! Simulation configuration for [`PhysicsWorld`](crate::PhysicsWorld).
//!
//! Everything is expressed in `math` types so callers never have to touch
//! rapier's own math to configure a world.

use math::{Vec3, vec3};

/// Configuration for a [`PhysicsWorld`](crate::PhysicsWorld).
///
/// Construct with [`PhysicsConfig::default`] and tweak through the builder
/// methods:
///
/// ```
/// use physics::PhysicsConfig;
/// use math::vec3;
///
/// let config = PhysicsConfig::default()
///     .with_gravity(vec3(0.0, -1.62, 0.0))
///     .with_timestep(1.0 / 120.0)
///     .with_substeps(2);
/// assert_eq!(config.substeps, 2);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysicsConfig {
    /// The gravity acceleration applied to dynamic bodies, in metres per
    /// second squared. Defaults to `(0, -9.81, 0)`.
    pub gravity: Vec3,
    /// The fixed simulation timestep, in seconds. Defaults to `1 / 60`.
    pub timestep: f32,
    /// How many internal rapier sub-steps each [`step`](crate::PhysicsWorld::step)
    /// is split into. Clamped to at least `1`.
    pub substeps: u32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            gravity: vec3(0.0, -9.81, 0.0),
            timestep: 1.0 / 60.0,
            substeps: 1,
        }
    }
}

impl PhysicsConfig {
    /// Builder: set the gravity vector.
    pub fn with_gravity(mut self, gravity: Vec3) -> Self {
        self.gravity = gravity;
        self
    }

    /// Builder: set the fixed timestep (seconds).
    pub fn with_timestep(mut self, timestep: f32) -> Self {
        self.timestep = timestep;
        self
    }

    /// Builder: set the number of sub-steps per frame; clamped to at least `1`.
    pub fn with_substeps(mut self, substeps: u32) -> Self {
        self.substeps = substeps.max(1);
        self
    }

    /// The effective timestep actually handed to rapier for a `dt` argument:
    /// `dt` split across [`substeps`](Self::substeps). A non-positive `dt`
    /// falls back to [`timestep`](Self::timestep).
    pub fn effective_substep(&self, dt: f32) -> f32 {
        let dt = if dt > 0.0 { dt } else { self.timestep };
        dt / self.substeps.max(1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::{approx_eq, vec3_approx_eq};

    #[test]
    fn defaults_are_earth_gravity_and_60hz() {
        let config = PhysicsConfig::default();
        assert!(vec3_approx_eq(config.gravity, vec3(0.0, -9.81, 0.0), 1e-6));
        assert!(approx_eq(config.timestep, 1.0 / 60.0, 1e-9));
        assert_eq!(config.substeps, 1);
    }

    #[test]
    fn builders_override_fields() {
        let config = PhysicsConfig::default()
            .with_gravity(vec3(0.0, 0.0, -5.0))
            .with_timestep(0.02)
            .with_substeps(4);
        assert!(vec3_approx_eq(config.gravity, vec3(0.0, 0.0, -5.0), 1e-6));
        assert!(approx_eq(config.timestep, 0.02, 1e-9));
        assert_eq!(config.substeps, 4);
    }

    #[test]
    fn substeps_are_clamped_to_one() {
        let config = PhysicsConfig::default().with_substeps(0);
        assert_eq!(config.substeps, 1);
    }

    #[test]
    fn effective_substep_splits_dt() {
        let config = PhysicsConfig::default().with_substeps(4);
        assert!(approx_eq(
            config.effective_substep(1.0 / 30.0),
            1.0 / 120.0,
            1e-9
        ));

        // A non-positive dt falls back to the configured timestep.
        let single = PhysicsConfig::default();
        assert!(approx_eq(
            single.effective_substep(0.0),
            single.timestep,
            1e-9
        ));
    }
}
