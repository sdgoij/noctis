//! Runtime configuration.

use ecs::assets::{AssetHandle, AssetRegistry, HdrTexture};
use math::{Vec3, vec3};
use physics::PhysicsConfig;
use project::Project;

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
    /// The tonemap exposure (the project's `[render] exposure`): a master brightness
    /// for the whole image.
    pub exposure: f32,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            physics: PhysicsConfig::default(),
            audio: true,
            gravity: vec3(0.0, -9.81, 0.0),
            timestep: 1.0 / 60.0,
            environment: None,
            exposure: 1.0,
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

    /// Builder: set the tonemap exposure (a master brightness).
    pub fn with_exposure(mut self, exposure: f32) -> Self {
        self.exposure = exposure.max(0.0);
        self
    }

    /// Apply the settings a project's manifest implies: gravity from
    /// `[project.settings]` and the IBL environment from `[render] environment`
    /// (looked up in `assets`, which must already hold the imported HDRIs).
    ///
    /// A project that names an environment which did not import is an error — the
    /// game would silently fall back to the renderer's neutral (fully white) IBL
    /// and look washed out. Every host that runs a project calls this, so native
    /// and web cannot diverge.
    pub fn apply_project(
        &mut self,
        project: &Project,
        assets: &AssetRegistry,
    ) -> Result<(), String> {
        let gravity = project.manifest().project.settings.gravity;
        self.gravity = vec3(gravity[0], gravity[1], gravity[2]);
        self.exposure = project.manifest().render.exposure.max(0.0);
        if let Some(path) = project.manifest().render.environment.as_deref() {
            let handle = assets.hdr.handle(path).ok_or_else(|| {
                format!(
                    "the project's [render] environment {path:?} is not an importable .hdr asset"
                )
            })?;
            self.environment = Some(handle);
        }
        Ok(())
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
        assert!(
            (config.exposure - 1.0).abs() < 1e-9,
            "exposure defaults to 1.0"
        );
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
    fn apply_project_takes_gravity_and_the_environment() {
        use math::vec3_approx_eq;

        let manifest = project::ProjectManifest::from_toml(
            "[project]\nname = \"P\"\n[project.settings]\ngravity = [0.0, -3.0, 0.0]\n\
             [render]\nenvironment = \"assets/sky.hdr\"\nexposure = 0.5\n",
        )
        .unwrap();
        let project = project::Project::from_manifest("/tmp/game", manifest);

        let mut assets = AssetRegistry::with_builtins();
        let handle = assets.hdr.insert_named(
            "assets/sky.hdr",
            HdrTexture {
                name: "sky".into(),
                width: 2,
                height: 1,
                data: vec![0.0; 2 * 3],
            },
        );

        let mut config = RuntimeConfig::default();
        config.apply_project(&project, &assets).expect("apply");
        assert!(vec3_approx_eq(config.gravity, vec3(0.0, -3.0, 0.0), 1e-6));
        assert_eq!(config.environment, Some(handle));
        assert!(
            (config.exposure - 0.5).abs() < 1e-9,
            "the exposure is taken"
        );

        // A named environment that did not import is an error, not a silent
        // fallback to the white neutral IBL.
        let empty = AssetRegistry::with_builtins();
        let mut config = RuntimeConfig::default();
        assert!(config.apply_project(&project, &empty).is_err());
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
