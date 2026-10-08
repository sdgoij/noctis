//! Script-presence checks for the JavaScript gate.
//!
//! The engine's scripting backend is the off-by-default `slag` feature. A build
//! without it can still load a scene full of JavaScript, attach every behaviour,
//! and then sit perfectly still — which reads as a broken scene rather than a
//! disabled feature. `run` and `build` call [`require_scripting`] so a project
//! that depends on scripts is refused, loudly, unless the backend is present or
//! the caller explicitly opts into an inert run.

use ecs::components::Script;
use ecs::scene::{ComponentDesc, SceneDesc};
use ecs::world::World;

/// The entities in `world` that attach a script with something to run.
pub fn attached_scripts(world: &World) -> usize {
    world
        .entities_with::<Script>()
        .into_iter()
        .filter(|entity| world.get::<Script>(*entity).is_some_and(Script::has_code))
        .count()
}

/// The scripts in a serialized scene document that have something to run.
pub fn scripts_in_document(json: &str) -> Result<usize, String> {
    let desc: SceneDesc = serde_json::from_str(json)
        .map_err(|error| format!("failed to parse the scene: {error}"))?;
    Ok(desc
        .entities
        .iter()
        .flat_map(|entity| &entity.components)
        .filter(|component| {
            matches!(
                component,
                ComponentDesc::Script { source, path, .. }
                    if source.is_some() || path.is_some()
            )
        })
        .count())
}

/// Fail when `count` scripts are attached but this build cannot run them.
///
/// `allow_inert` (the `--allow-inert` flag) is the explicit opt-out: run or build
/// anyway, accepting that the scripts will not execute.
pub fn require_scripting(count: usize, allow_inert: bool) -> Result<(), String> {
    if count == 0 || allow_inert || cfg!(feature = "slag") {
        return Ok(());
    }
    Err(format!(
        "{count} script(s) are attached, but this build has no JavaScript backend, so they would not run. \
         Rebuild with the `slag` feature, or pass --allow-inert to continue without them."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE_WITH_SCRIPTS: &str = r#"{
        "name": "S",
        "entities": [
            { "id": 0, "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
              "components": [ { "type": "script", "source": null, "path": "a.js", "enabled": true } ] },
            { "id": 1, "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
              "components": [ { "type": "script", "source": "function on_update() {}", "path": null, "enabled": true } ] },
            { "id": 2, "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
              "components": [ { "type": "script", "source": null, "path": null, "enabled": true } ] }
        ]
    }"#;

    #[test]
    fn scripts_in_document_counts_only_scripts_with_code() {
        assert_eq!(scripts_in_document(SCENE_WITH_SCRIPTS).unwrap(), 2);
        assert_eq!(
            scripts_in_document(r#"{ "name": "E", "entities": [] }"#).unwrap(),
            0
        );
    }

    #[test]
    fn scripts_in_document_reports_bad_json() {
        assert!(scripts_in_document("not json").is_err());
    }

    #[test]
    fn attached_scripts_counts_the_world() {
        let mut world = World::new();
        let a = world.spawn();
        world.insert(a, Script::from_path("a.js"));
        let b = world.spawn();
        world.insert(b, Script::default()); // no code
        world.spawn(); // no script
        assert_eq!(attached_scripts(&world), 1);
    }

    #[test]
    fn require_scripting_only_blocks_without_a_backend() {
        assert!(require_scripting(0, false).is_ok());
        // `--allow-inert` always bypasses the gate.
        assert!(require_scripting(3, true).is_ok());

        #[cfg(not(feature = "slag"))]
        assert!(require_scripting(3, false).is_err());
        #[cfg(feature = "slag")]
        assert!(require_scripting(3, false).is_ok());
    }
}
