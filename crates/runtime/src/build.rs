//! Building a project into a distributable game folder.
//!
//! A build resolves a scene (an explicit `--scene`, a `[[build]]` target's scene,
//! or the project's `default_scene`), imports the project's assets, and bundles
//! the files the scene references alongside the runtime executable, the scene
//! document, a project manifest (so the exported runtime can import its assets),
//! and a game manifest. This is what drives `noctis build` and `noctis run
//! --export`.

use std::path::{Path, PathBuf};

use ecs::app::Plugin;
use ecs::reflect::ComponentRegistry;
use ecs::scene::SceneDesc;
use project::{AssetKind, AssetPath, BuildTarget, Profile, Project, detect};

use crate::export::{ExportFile, ExportReport, export_dev_game, export_game, game_client};

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
    /// The runtime executable to ship. When `None`, the client is built (with
    /// the project's scripts embedded for a release build).
    pub runtime: Option<PathBuf>,
    /// The build profile, overriding the target's; defaults to release.
    pub profile: Option<Profile>,
}

/// Build `project_dir` into a self-contained game folder.
///
/// `target` names a `[[build]]` target; when omitted the ad-hoc defaults apply
/// (`options`, then the project's `default_scene` and name). `plugin` is the
/// project's Rust (a Rust project's plugin, else `&NullPlugin`): its components'
/// asset references are bundled, and a Rust project's game client links it.
pub fn build_project(
    project_dir: &Path,
    target: Option<&str>,
    options: &BuildOptions,
    plugin: &dyn Plugin,
) -> Result<ExportReport, String> {
    let project = Project::open(project_dir)
        .map_err(|error| format!("failed to open project {}: {error}", project_dir.display()))?;

    // The plugin's components are needed to scan a scene's `Custom` references.
    let mut components = ComponentRegistry::new();
    plugin.register(&mut components);
    let is_rust = project.rust_crate_dir().is_some();

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

    // The artifact name: `--name`, else the target's `name`, else the project
    // directory (a project at `examples/showcase` builds a `showcase` artifact).
    // `[project].name` is a display name (window title, editor) and is never used
    // as a file name.
    let name = options
        .name
        .clone()
        .or_else(|| target.map(|t| t.name.clone()))
        .unwrap_or_else(|| {
            project
                .root()
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| crate::export::valid_name(name))
                .unwrap_or("game")
                .to_string()
        });

    // The profile decides how scripts ship (release embeds them, dev ships them as
    // loose files) and names the output folder, so it is resolved first.
    let profile = options
        .profile
        .or_else(|| target.map(|target| target.profile))
        .unwrap_or_default();
    let destination = match options.destination.clone() {
        // An explicit `--out` is used verbatim.
        Some(destination) => destination,
        // A target's `output` is used verbatim too (an absolute path as given).
        None => match target.and_then(|t| t.output.as_deref()) {
            Some(output) => {
                if Path::new(output).is_absolute() {
                    PathBuf::from(output)
                } else {
                    project.dir(output)
                }
            }
            // Otherwise output is organised by profile: `dist/release/` and
            // `dist/dev/`. A named artifact (a `[[build]]` target, or `--name`)
            // gets its own folder under the profile; the project's own build is
            // the profile folder itself, since the project already names the game.
            None => {
                let directory = project.dir("dist").join(profile_directory(profile));
                if target.is_some() || options.name.is_some() {
                    directory.join(&name)
                } else {
                    directory
                }
            }
        },
    };

    // A development build runs the project in place: the client plus a manifest
    // that points at the project, so its scripts (which the runtime watches) are
    // edited where they live, not in a copy. Nothing is copied.
    if profile == Profile::Dev {
        let client = match &options.runtime {
            Some(runtime) => runtime.clone(),
            None if is_rust => crate::plugin::build_client(&project, Profile::Dev, &[])?,
            None => game_client(Profile::Dev, None)?,
        };
        let environment = project.manifest().render.environment.clone();
        return export_dev_game(
            &destination,
            &name,
            project.root(),
            &scene,
            environment.as_deref(),
            Some(&client),
        )
        .map_err(|error| error.to_string());
    }

    // A release build embeds the scripts in the client and copies the rest.
    let bundle = bundle_files(&project, &scene_json, true, &components)?;
    let pack = if bundle.scripts.is_empty() {
        None
    } else {
        Some(write_script_pack(&project, &bundle.scripts)?)
    };
    let client = match &options.runtime {
        Some(runtime) => runtime.clone(),
        None if is_rust => crate::plugin::build_client(&project, profile, &bundle.scripts)?,
        None => game_client(profile, pack.as_deref())?,
    };
    export_game(
        &destination,
        &name,
        &scene_json,
        &bundle.files,
        bundle.environment.as_deref(),
        Some(&client),
    )
    .map_err(|error| error.to_string())
}

/// The `dist/` subdirectory a build profile writes to: `dev` or `release`.
fn profile_directory(profile: Profile) -> &'static str {
    match profile {
        Profile::Dev => "dev",
        Profile::Release => "release",
    }
}

/// Write the embed pack `noctis build` hands to the client build: the scripts to
/// bake in, as `{ "scripts": [{ "path", "source" }] }` with absolute source paths.
/// It lives under the project's `.noctis/` cache (gitignored); the list changes
/// each build, so it is not a stable artifact.
fn write_script_pack(project: &Project, scripts: &[ExportFile]) -> Result<PathBuf, String> {
    let directory = crate::export::absolute(&project.dir(project::CACHE_DIR));
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("failed to create {}: {error}", directory.display()))?;
    let entries: Vec<serde_json::Value> = scripts
        .iter()
        .map(|file| {
            // `include_bytes!` resolves a relative path against the generated
            // file, so the source must be absolute.
            let source = crate::export::absolute(&file.source);
            serde_json::json!({ "path": file.relative, "source": source.to_string_lossy() })
        })
        .collect();
    let document = serde_json::json!({ "scripts": entries });
    let text = serde_json::to_string_pretty(&document)
        .map_err(|error| format!("failed to serialise the embed pack: {error}"))?;
    let path = directory.join("scripts.pack.json");
    // Rewrite only on change: the client's build script reruns when the pack
    // changes, so a stable file keeps the client from recompiling every build.
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, &text)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    }
    Ok(path)
}

/// A `(relative, source)` pair for one project asset, erroring when the file is
/// missing on disk.
/// The project files a scene references, plus the project's `[render]`
/// environment, ready to bundle. A file that is referenced but absent from disk
/// is an error (the build would ship a broken game).
///
/// Returns `(files, scripts, environment)`. When `embed_scripts` is set, the
/// scene's script files are returned separately in `scripts` (to bake into the
/// client) instead of in `files` (to copy into the folder).
///
/// The environment is returned alongside the files because it is referenced by
/// the *manifest*, not by the scene, so the exported `noctis.toml` has to name it
/// even though no entity refers to it.
/// The files a build bundles: the ones copied into the folder, the scripts to
/// embed in the client (empty when not embedding), and the `[render]`
/// environment named by the manifest.
struct Bundle {
    files: Vec<ExportFile>,
    scripts: Vec<ExportFile>,
    environment: Option<String>,
}

fn bundle_files(
    project: &Project,
    scene_json: &str,
    embed_scripts: bool,
    components: &ComponentRegistry,
) -> Result<Bundle, String> {
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
    let referenced = project::referenced_files_with(&desc, &imported.lookup, components)
        .map_err(|error| format!("cannot bundle the scene: {error}"))?;
    let mut files = Vec::with_capacity(referenced.len());
    let mut scripts = Vec::new();
    for asset in referenced {
        let file = export_file(project, &asset)?;
        // An embedded script rides inside the client, not as a loose file.
        if embed_scripts && detect(asset.as_str()) == AssetKind::Script {
            scripts.push(file);
        } else {
            files.push(file);
        }
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
    Ok(Bundle {
        files,
        scripts,
        environment,
    })
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
    use ecs::app::NullPlugin;

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

    /// A stand-in runtime executable, so a build test never invokes cargo.
    fn test_runtime() -> PathBuf {
        std::env::current_exe().expect("the test executable")
    }

    const MANIFEST: &str = "[project]\nname = \"Game\"\n\n[project.settings]\ndefault_scene = \"scenes/main.scene.json\"\n";

    /// A minimal valid 16-bit mono PCM WAV (a short tone).
    fn tiny_wav() -> Vec<u8> {
        let samples: [i16; 8] = [0, 1000, 2000, 1000, 0, -1000, -2000, -1000];
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut wav = Vec::with_capacity(44 + data.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // channels
        wav.extend_from_slice(&8000u32.to_le_bytes()); // sample rate
        wav.extend_from_slice(&16000u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        wav
    }

    #[test]
    fn a_release_build_copies_referenced_assets() {
        let project = temp_dir("assets-project");
        write(&project.join("noctis.toml"), MANIFEST);
        std::fs::create_dir_all(project.join("assets/audio")).expect("audio dir");
        std::fs::write(project.join("assets/audio/beep.wav"), tiny_wav()).expect("write wav");
        write(
            &project.join("scenes/main.scene.json"),
            r#"{
                "name": "Main",
                "entities": [{
                    "id": 0, "name": "Box",
                    "transform": { "translation": [0,0,0], "rotation": [0,0,0,1], "scale": [1,1,1] },
                    "components": [
                        { "type": "audio_source", "clip": "assets/audio/beep.wav",
                          "volume": 1.0, "pitch": 1.0, "looped": false, "playing": true,
                          "spatial": true, "max_distance": 20.0, "rolloff": 1.0 }
                    ]
                }]
            }"#,
        );
        let out = temp_dir("assets-out");
        // A release build copies the referenced (non-script) assets.
        let report = build_project(
            &project,
            None,
            &BuildOptions {
                destination: Some(out.clone()),
                runtime: Some(test_runtime()),
                ..BuildOptions::default()
            },
            &NullPlugin,
        )
        .expect("build");

        assert_eq!(report.files, vec!["assets/audio/beep.wav".to_string()]);
        assert_eq!(
            std::fs::read(out.join("assets/audio/beep.wav")).expect("bundled"),
            tiny_wav()
        );
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

        let error = build_project(&project, None, &options, &NullPlugin).expect_err("must fail");
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
            runtime: Some(test_runtime()),
            ..BuildOptions::default()
        };
        let report = build_project(&project, None, &options, &NullPlugin).expect("build");

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
        let error = build_project(
            &project,
            Some("nope"),
            &BuildOptions::default(),
            &NullPlugin,
        )
        .expect_err("must fail");
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

        let options = BuildOptions {
            runtime: Some(test_runtime()),
            ..BuildOptions::default()
        };
        let report =
            build_project(&project, Some("release"), &options, &NullPlugin).expect("build");
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

    #[test]
    fn a_release_build_embeds_scripts_and_omits_them() {
        let project = temp_dir("embed-project");
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
        let out = temp_dir("embed-out");
        let options = BuildOptions {
            destination: Some(out.clone()),
            runtime: Some(test_runtime()),
            profile: Some(Profile::Release),
            ..BuildOptions::default()
        };

        let report = build_project(&project, None, &options, &NullPlugin).expect("build");

        // The script is baked into the client, so it is neither copied into the
        // folder nor listed among the bundled files — but it is in the pack.
        assert!(report.files.is_empty(), "{:?}", report.files);
        assert!(!out.join("scripts/hello.js").exists());
        let pack = project.join(project::CACHE_DIR).join("scripts.pack.json");
        let pack = std::fs::read_to_string(&pack).expect("the embed pack was written");
        assert!(pack.contains("\"path\": \"scripts/hello.js\""), "{pack}");

        std::fs::remove_dir_all(&project).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn output_is_organised_by_profile() {
        // The project directory names the artifact, so put it in `.../game`.
        let project = temp_dir("profile-dirs").join("game");
        write(&project.join("noctis.toml"), MANIFEST);
        write(
            &project.join("scenes/main.scene.json"),
            "{ \"name\": \"Main\", \"entities\": [] }",
        );
        let runtime = test_runtime();

        // Release (the default) lands in `dist/release`.
        let release = build_project(
            &project,
            None,
            &BuildOptions {
                runtime: Some(runtime.clone()),
                ..BuildOptions::default()
            },
            &NullPlugin,
        )
        .expect("release build");
        assert_eq!(release.directory, project.join("dist/release"));

        // Dev lands in `dist/dev`, so it cannot clobber the release build.
        let dev = build_project(
            &project,
            None,
            &BuildOptions {
                runtime: Some(runtime),
                profile: Some(Profile::Dev),
                ..BuildOptions::default()
            },
            &NullPlugin,
        )
        .expect("dev build");
        assert_eq!(dev.directory, project.join("dist/dev"));

        std::fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn a_named_target_gets_its_own_folder_under_the_profile() {
        let project = temp_dir("profile-targets");
        write(
            &project.join("noctis.toml"),
            "[project]\nname = \"Game\"\n\n[[build]]\nname = \"level-one\"\nscene = \"scenes/main.scene.json\"\n",
        );
        write(
            &project.join("scenes/main.scene.json"),
            "{ \"name\": \"Main\", \"entities\": [] }",
        );

        let report = build_project(
            &project,
            Some("level-one"),
            &BuildOptions {
                runtime: Some(test_runtime()),
                ..BuildOptions::default()
            },
            &NullPlugin,
        )
        .expect("build");
        assert_eq!(report.directory, project.join("dist/release/level-one"));

        std::fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn a_dev_build_runs_the_project_in_place() {
        let project = temp_dir("dev-in-place").join("game");
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
        let out = temp_dir("dev-in-place-out");
        let report = build_project(
            &project,
            None,
            &BuildOptions {
                destination: Some(out.clone()),
                runtime: Some(test_runtime()),
                profile: Some(Profile::Dev),
                ..BuildOptions::default()
            },
            &NullPlugin,
        )
        .expect("dev build");

        // Nothing is copied: the folder holds the client and a manifest that
        // points back at the project, so its scripts are the live ones.
        assert!(report.files.is_empty(), "{:?}", report.files);
        assert!(!out.join("scripts/hello.js").exists());
        let root = project.to_string_lossy().replace('\\', "/");
        let manifest = std::fs::read_to_string(out.join("noctis.toml")).expect("manifest");
        assert!(manifest.contains(&format!("root = {root:?}")), "{manifest}");

        let opened = Project::open(&out).expect("open the dev folder");
        assert_eq!(opened.root(), Path::new(&root));
        assert_eq!(
            opened.default_scene_file(),
            Some(Path::new(&root).join("scenes/main.scene.json"))
        );

        std::fs::remove_dir_all(&project).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn a_build_names_the_artifact_after_the_project_directory() {
        // A display name with a space, in a directory called `showcase`.
        let project = temp_dir("artifact-name").join("showcase");
        write(
            &project.join("noctis.toml"),
            "[project]\nname = \"Noctis Showcase\"\n\n[project.settings]\ndefault_scene = \"scenes/main.scene.json\"\n",
        );
        write(
            &project.join("scenes/main.scene.json"),
            "{ \"name\": \"Main\", \"entities\": [] }",
        );

        let report = build_project(
            &project,
            None,
            &BuildOptions {
                runtime: Some(test_runtime()),
                ..BuildOptions::default()
            },
            &NullPlugin,
        )
        .expect("build");

        // The artifact is the directory name, never the display name; the
        // project's own build is the profile folder itself.
        assert_eq!(report.directory, project.join("dist/release"));
        assert_eq!(
            report.executable.file_name().and_then(|n| n.to_str()),
            Some(crate::export::executable_name("showcase").as_str())
        );

        std::fs::remove_dir_all(&project).ok();
    }
}
