//! Building a project into a distributable game folder.
//!
//! A build resolves a scene (an explicit `--scene`, a `[[build]]` target's scene,
//! or the project's `default_scene`), imports the project's assets, and bundles
//! the files the scene references alongside the runtime executable, the scene
//! document, a project manifest (so the exported runtime can import its assets),
//! and a game manifest. This is what drives `noctis build` and `noctis run
//! --export`.

use std::path::{Path, PathBuf};

use ecs::scene::SceneDesc;
use project::{AssetPath, BuildTarget, Project};

use crate::export::{ExportFile, ExportReport, export_game};

/// Overrides applied on top of a project's build target (or its default scene).
#[derive(Clone, Debug, Default)]
pub struct BuildOptions {
    /// Where to write the game folder. Defaults to the target's `output`, else
    /// `<project>/dist/<name>`.
    pub destination: Option<PathBuf>,
    /// The game/executable name. Defaults to the target's name, else the project's.
    pub name: Option<String>,
    /// The scene, project-relative. Overrides the target and project default.
    pub scene: Option<String>,
    /// The runtime executable to ship. Defaults to the sibling `noctis-game`
    /// (see [`crate::export::default_runtime_executable`]).
    pub runtime: Option<PathBuf>,
}

/// Build `project_dir` into a self-contained game folder.
///
/// `target` names a `[[build]]` target; when omitted the ad-hoc defaults apply
/// (`options`, then the project's `default_scene` and name).
pub fn build_project(
    project_dir: &Path,
    target: Option<&str>,
    options: &BuildOptions,
) -> Result<ExportReport, String> {
    let project = Project::open(project_dir)
        .map_err(|error| format!("failed to open project {}: {error}", project_dir.display()))?;

    let target: Option<&BuildTarget> = match target {
        Some(name) => Some(
            project
                .build_target(name)
                .map_err(|error| error.to_string())?,
        ),
        None => None,
    };

    // Scene: explicit > target > project default.
    let scene = options
        .scene
        .clone()
        .or_else(|| target.and_then(|t| t.scene.clone()))
        .or_else(|| project.default_scene().map(str::to_string))
        .ok_or("the project has no default scene; pass --scene or add a [[build]] target")?;
    let scene_asset = AssetPath::parse(&scene).map_err(|error| error.to_string())?;
    let scene_path = project.file_for(&scene_asset);
    let scene_json = std::fs::read_to_string(&scene_path)
        .map_err(|error| format!("failed to read scene '{}': {error}", scene_path.display()))?;

    let name = options
        .name
        .clone()
        .or_else(|| target.map(|t| t.name.clone()))
        .unwrap_or_else(|| project.name().to_string());
    let destination = options
        .destination
        .clone()
        .or_else(|| {
            // A target's `output` is project-relative (an absolute path is used
            // as given).
            target.and_then(|t| t.output.as_deref()).map(|output| {
                if Path::new(output).is_absolute() {
                    PathBuf::from(output)
                } else {
                    project.dir(output)
                }
            })
        })
        .unwrap_or_else(|| project.dir("dist").join(&name));

    let (files, environment) = bundle_files(&project, &scene_json)?;
    export_game(
        &destination,
        &name,
        &scene_json,
        &files,
        environment.as_deref(),
        options.runtime.as_deref(),
    )
    .map_err(|error| error.to_string())
}

/// The project files a scene references, plus the project's `[render]`
/// environment, ready to bundle. A file that is referenced but absent from disk
/// is an error (the build would ship a broken game).
///
/// The environment is returned alongside the files because it is referenced by
/// the *manifest*, not by the scene, so the exported `noctis.toml` has to name it
/// even though no entity refers to it.
fn bundle_files(
    project: &Project,
    scene_json: &str,
) -> Result<(Vec<ExportFile>, Option<String>), String> {
    let imported = project
        .import_assets()
        .map_err(|error| format!("failed to import project assets: {error}"))?;
    for error in &imported.errors {
        log::warn!(
            "asset '{}' failed to import: {}",
            error.path.as_str(),
            error.message
        );
    }
    let desc: SceneDesc = serde_json::from_str(scene_json)
        .map_err(|error| format!("failed to parse the scene for the build: {error}"))?;
    let referenced = project::referenced_files(&desc, &imported.lookup)
        .map_err(|error| format!("cannot bundle the scene: {error}"))?;
    let mut files = Vec::with_capacity(referenced.len());
    for asset in referenced {
        files.push(export_file(project, &asset)?);
    }

    // The environment is part of the game even though no scene references it.
    let environment = project
        .manifest()
        .render
        .environment
        .as_deref()
        .map(str::to_string);
    if let Some(path) = &environment {
        let asset = AssetPath::parse(path).map_err(|error| error.to_string())?;
        if !files.iter().any(|file| file.relative == asset.as_str()) {
            files.push(export_file(project, &asset)?);
        }
    }
    Ok((files, environment))
}

/// A `(relative, source)` pair for one project asset, erroring when the file is
/// missing on disk.
fn export_file(project: &Project, asset: &AssetPath) -> Result<ExportFile, String> {
    let source = project.file_for(asset);
    if !source.is_file() {
        return Err(format!(
            "cannot bundle the game: '{}' is referenced but missing on disk",
            asset.as_str()
        ));
    }
    Ok(ExportFile {
        relative: asset.as_str().to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "noctis-build-{tag}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent dir");
        }
        std::fs::write(path, contents).expect("write");
    }

    const MANIFEST: &str = "[project]\nname = \"Game\"\n\n[project.settings]\ndefault_scene = \"scenes/main.scene.json\"\n";

    #[test]
    fn build_bundles_the_referenced_script() {
        let project = temp_dir("project");
        write(&project.join("noctis.toml"), MANIFEST);
        write(
            &project.join("scripts/hello.js"),
            "function on_update() {}\n",
        );
        write(
            &project.join("scenes/main.scene.json"),
            r#"{
                "name": "Main",
                "entities": [{
                    "id": 0, "name": "Box",
                    "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                    "components": [
                        { "type": "script", "source": null, "path": "scripts/hello.js", "enabled": true }
                    ]
                }]
            }"#,
        );
        let out = temp_dir("out");
        let options = BuildOptions {
            destination: Some(out.clone()),
            ..BuildOptions::default()
        };

        let report = build_project(&project, None, &options).expect("build");
        assert_eq!(report.files, vec!["scripts/hello.js".to_string()]);
        assert!(out.join("scene.json").is_file());
        assert!(out.join("scripts/hello.js").is_file());
        assert!(out.join("noctis.toml").is_file());

        std::fs::remove_dir_all(&project).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn build_errors_on_a_reference_with_no_file() {
        let project = temp_dir("missing");
        write(&project.join("noctis.toml"), MANIFEST);
        write(
            &project.join("scenes/main.scene.json"),
            r#"{
                "name": "Main",
                "entities": [{
                    "id": 0,
                    "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                    "components": [
                        { "type": "audio_source", "clip": "audio/gone.wav",
                          "volume": 1.0, "pitch": 1.0, "looped": false, "playing": true,
                          "spatial": true, "max_distance": 20.0, "rolloff": 1.0 }
                    ]
                }]
            }"#,
        );
        let out = temp_dir("missing-out");
        let options = BuildOptions {
            destination: Some(out.clone()),
            ..BuildOptions::default()
        };

        let error = build_project(&project, None, &options).expect_err("must fail");
        assert!(error.contains("audio/gone.wav"), "{error}");

        std::fs::remove_dir_all(&project).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    /// A minimal valid Radiance HDR (2x1, new-style per-component RLE).
    fn tiny_hdr() -> Vec<u8> {
        let mut out = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 2\n".to_vec();
        let pixels = [[200u8, 100, 50, 129], [180, 90, 45, 129]];
        out.extend_from_slice(&[2, 2, 0, 2]); // scanline header: width 2
        for (first, second) in pixels[0].iter().zip(pixels[1].iter()) {
            out.push(2);
            out.push(*first);
            out.push(*second);
        }
        out
    }

    #[test]
    fn build_bundles_the_environment_and_names_it_in_the_export() {
        let project = temp_dir("env-project");
        write(
            &project.join("noctis.toml"),
            "[project]\nname = \"Game\"\n\n[project.settings]\ndefault_scene = \"scenes/main.scene.json\"\n\n[render]\nenvironment = \"assets/hdris/sky.hdr\"\n",
        );
        write(
            &project.join("scenes/main.scene.json"),
            "{ \"name\": \"Main\", \"entities\": [] }",
        );
        std::fs::create_dir_all(project.join("assets/hdris")).expect("env dir");
        std::fs::write(project.join("assets/hdris/sky.hdr"), tiny_hdr()).expect("write hdr");

        // The environment is an importable asset (not just a file on disk).
        let opened = Project::open(&project).expect("open");
        let imported = opened.import_assets().expect("import");
        assert!(imported.errors.is_empty(), "{:#?}", imported.errors);
        let hdr = imported
            .registry
            .hdr
            .get_by_path("assets/hdris/sky.hdr")
            .expect("the environment imported");
        assert_eq!((hdr.width, hdr.height), (2, 1));

        let out = temp_dir("env-out");
        let options = BuildOptions {
            destination: Some(out.clone()),
            ..BuildOptions::default()
        };
        let report = build_project(&project, None, &options).expect("build");

        // No scene references it, so bundling it is purely the manifest's doing.
        assert_eq!(report.files, vec!["assets/hdris/sky.hdr".to_string()]);
        assert_eq!(
            std::fs::read(out.join("assets/hdris/sky.hdr")).expect("bundled"),
            tiny_hdr()
        );
        let manifest = std::fs::read_to_string(out.join("noctis.toml")).expect("read");
        assert!(
            manifest.contains("environment = \"assets/hdris/sky.hdr\""),
            "{manifest}"
        );

        std::fs::remove_dir_all(&project).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn an_unknown_target_is_an_error() {
        let project = temp_dir("target");
        write(&project.join("noctis.toml"), MANIFEST);
        let error =
            build_project(&project, Some("nope"), &BuildOptions::default()).expect_err("must fail");
        assert!(error.contains("no build target named"), "{error}");
        std::fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn a_build_target_selects_the_scene_name_and_output() {
        let project = temp_dir("target2");
        write(
            &project.join("noctis.toml"),
            "[project]\nname = \"Game\"\n\n[[build]]\nname = \"release\"\nscene = \"scenes/level.scene.json\"\noutput = \"out/release\"\n",
        );
        write(
            &project.join("scenes/level.scene.json"),
            "{ \"name\": \"Level\", \"entities\": [] }",
        );

        let report =
            build_project(&project, Some("release"), &BuildOptions::default()).expect("build");
        // `output` is project-relative; the name comes from the target.
        assert_eq!(report.directory, project.join("out/release"));
        assert_eq!(
            report.executable.file_name().and_then(|n| n.to_str()),
            Some(crate::export::executable_name("release").as_str())
        );
        assert!(report.scene.is_file());
        assert!(report.project_manifest.is_none(), "no assets to bundle");

        std::fs::remove_dir_all(&project).ok();
    }
}
