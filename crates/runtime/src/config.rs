//! Runtime configuration.

use ecs::assets::{AssetHandle, HdrTexture};
use math::{Vec3, vec3};
use physics::PhysicsConfig;

/// Configuration for a [`Runtime`](crate::Runtime).
///
/// The common case is `RuntimeConfig::default()`; tune it through the builder
/// methods or struct update syntax:
///
/// ```
/// use runtime::RuntimeConfig;
/// use math::vec3;
///
/// let config = RuntimeConfig::default()
///     .with_gravity(vec3(0.0, -1.62, 0.0))
///     .with_audio(false);
/// assert!(!config.audio);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuntimeConfig {
    /// The base physics configuration (substeps and solver settings). Its
    /// `gravity` and `timestep` are overridden by the fields below.
    pub physics: PhysicsConfig,
    /// Whether to open an audio device. When `false` the runtime uses an
    /// [`AudioEngine::silent`](audio::AudioEngine::silent) engine; when `true`
    /// it still falls back to silent if no device is available.
    pub audio: bool,
    /// Gravity applied to dynamic bodies, in metres per second squared.
    pub gravity: Vec3,
    /// Fixed simulation timestep, in seconds.
    pub timestep: f32,
    /// The equirectangular HDR environment to precompute image-based lighting
    /// from (the project's `[render] environment`). `None` uses the renderer's
    /// neutral, fully-lit fallback.
    pub environment: Option<AssetHandle<HdrTexture>>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            physics: PhysicsConfig::default(),
            audio: true,
            gravity: vec3(0.0, -9.81, 0.0),
            timestep: 1.0 / 60.0,
            environment: None,
        }
    }
}

impl RuntimeConfig {
    /// The effective [`PhysicsConfig`], with [`gravity`](Self::gravity) and
    /// [`timestep`](Self::timestep) applied on top of [`physics`](Self::physics).
    pub fn physics_config(&self) -> PhysicsConfig {
        PhysicsConfig {
            gravity: self.gravity,
            timestep: self.timestep,
            substeps: self.physics.substeps.max(1),
        }
    }

    /// Builder: enable or disable audio.
    pub fn with_audio(mut self, audio: bool) -> Self {
        self.audio = audio;
        self
    }

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

    /// Builder: set the HDR environment the renderer precomputes IBL from.
    pub fn with_environment(mut self, environment: AssetHandle<HdrTexture>) -> Self {
        self.environment = Some(environment);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use math::vec3_approx_eq;

    #[test]
    fn defaults_are_earth_gravity_and_60hz() {
        let config = RuntimeConfig::default();
        assert!(config.audio);
        assert!(vec3_approx_eq(config.gravity, vec3(0.0, -9.81, 0.0), 1e-6));
        assert!((config.timestep - 1.0 / 60.0).abs() < 1e-9);
        assert!(config.environment.is_none(), "no environment by default");
    }

    #[test]
    fn an_environment_handle_is_carried_through() {
        let handle = AssetHandle::<HdrTexture>::from_raw(7);
        let config = RuntimeConfig::default().with_environment(handle);
        assert_eq!(config.environment, Some(handle));
        assert_eq!(
            config.environment.map(AssetHandle::id).map(|id| id.0),
            Some(7)
        );
    }

    #[test]
    fn physics_config_applies_overrides() {
        let config = RuntimeConfig::default()
            .with_gravity(vec3(0.0, -3.0, 0.0))
            .with_timestep(0.02);
        let physics = config.physics_config();
        assert!(vec3_approx_eq(physics.gravity, vec3(0.0, -3.0, 0.0), 1e-6));
        assert!((physics.timestep - 0.02).abs() < 1e-9);
    }

    #[test]
    fn substeps_are_preserved_and_clamped() {
        let config = RuntimeConfig {
            physics: PhysicsConfig::default().with_substeps(4),
            ..RuntimeConfig::default()
        };
        assert_eq!(config.physics_config().substeps, 4);

        let config = RuntimeConfig {
            physics: PhysicsConfig::default().with_substeps(0),
            ..RuntimeConfig::default()
        };
        assert_eq!(config.physics_config().substeps, 1);
    }
}
