//! Actual final libraries, dependency failures and executable-relative relocation.
#![cfg(not(target_arch = "wasm32"))]
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// Isolated relocated application; no runtime path points back to target or the checkout.
struct Deployment {
    /// Owns copied executables/libraries/assets through all child process outcomes.
    _directory: tempfile::TempDir,
    /// Application directory renamed after copying, so initial paths become invalid.
    root: PathBuf,
    /// Runtime backend selected for this isolated fixture.
    backend: neocogi_example_plugins::Backend,
    /// Original Cargo deployment used only for copying fixtures, never passed to child code.
    profile: PathBuf,
}
impl Deployment {
    /// Copies Cargo-built final artifacts and all real example executables.
    fn new(assets: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let before = directory.path().join("before");
        fs::create_dir_all(before.join("lib")).unwrap();
        let binaries = [
            env!("CARGO_BIN_EXE_deployment_probe"),
            env!("CARGO_BIN_EXE_triangle"),
            env!("CARGO_BIN_EXE_gltf_damaged_helmet"),
            env!("CARGO_BIN_EXE_gltf_brainstem"),
        ];
        for source in binaries {
            fs::copy(source, before.join(Path::new(source).file_name().unwrap())).unwrap();
        }
        let profile = std::env::var_os("NEOCOGI_NATIVE_OUTPUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(binaries[0]).parent().unwrap().to_path_buf());
        let requested = std::env::var("NEOCOGI_TEST_BACKEND").ok();
        let backend = neocogi_example_plugins::select_backend(requested.as_deref(), &profile.join("lib")).unwrap();
        let filename = neocogi_example_plugins::library_filename(backend);
        fs::copy(profile.join("lib").join(filename), before.join("lib").join(filename)).unwrap();
        if assets {
            let source = profile.join("assets/gltf");
            fs::create_dir_all(before.join("assets/gltf")).unwrap();
            for filename in [
                "DamagedHelmet.glb",
                "BrainStem.glb",
                "Cannon_Exterior.hdr",
                "DamagedHelmet-LICENSE.md",
                "BrainStem-LICENSE.md",
                "LicenseRef-Poser-EULA.txt",
                "Cannon_Exterior-LICENSE.md",
                "Cannon_Exterior-CC0.txt",
            ] {
                fs::copy(source.join(filename), before.join("assets/gltf").join(filename)).expect("fetch example assets before native deployment acceptance");
            }
            // Reuse only a source-keyed validated cache; the renderer validates it
            // normally. This keeps adapter qualification from repeating CPU baking.
            let cache = Path::new(env!("CARGO_MANIFEST_DIR")).join("gltf/assets/Cannon_Exterior.ibl");
            if cache.is_file() {
                fs::copy(cache, before.join("assets/gltf/Cannon_Exterior.ibl")).unwrap();
            }
        }
        let root = directory.path().join("relocated");
        fs::rename(before, &root).unwrap();
        Self {
            _directory: directory,
            root,
            backend,
            profile,
        }
    }
    /// Launches directly with a neutral CWD and no Cargo-supplied loader search path.
    fn run(&self, binary: &str, args: &[&str], native: bool) -> Output {
        let mut command = Command::new(self.root.join(format!("{binary}{}", std::env::consts::EXE_SUFFIX)));
        command
            .args(args)
            .current_dir(self._directory.path())
            .env_remove("LD_LIBRARY_PATH")
            .env_remove("DYLD_LIBRARY_PATH")
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH");
        if !native {
            command.env("SDL_VIDEODRIVER", "deliberately-invalid");
        }
        command.output().unwrap()
    }
}

/// One unchanged executable loads distinct backends using runtime arguments after relocation.
#[test]
#[ignore = "requires two separately built platform libraries; no graphics device needed"]
fn one_dynamic_executable_selects_multiple_backends_and_rejects_mislabeled_libraries() {
    use neocogi_example_plugins::{Backend, library_filename};
    let app = Deployment::new(false);
    let available: Vec<_> = Backend::ALL
        .into_iter()
        .filter(|backend| app.profile.join("lib").join(library_filename(*backend)).is_file())
        .collect();
    assert!(available.len() >= 2, "build two platform variants before runtime selection acceptance");
    for backend in &available {
        let filename = library_filename(*backend);
        fs::copy(app.profile.join("lib").join(filename), app.root.join("lib").join(filename)).unwrap();
    }
    let executable = app.root.join(format!("deployment_probe{}", std::env::consts::EXE_SUFFIX));
    let original = fs::read(&executable).unwrap();
    let ambiguous = app.run("deployment_probe", &[], false);
    assert_eq!(ambiguous.status.code(), Some(24));
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("Multiple platform libraries"));
    for binary in ["gltf_brainstem", "gltf_damaged_helmet"] {
        let result = app.run(binary, &["--frames", "1"], false);
        assert_eq!(result.status.code(), Some(1));
        let message = String::from_utf8_lossy(&result.stderr);
        assert!(message.contains("--backend"), "{message}");
        assert!(!String::from_utf8_lossy(&result.stdout).contains("Preparing HDR"));
    }
    for backend in &available {
        let selected = app.run("deployment_probe", &["--backend", backend.name()], false);
        assert!(selected.status.success(), "{}", String::from_utf8_lossy(&selected.stderr));
        assert!(String::from_utf8_lossy(&selected.stdout).contains(backend.plugin_name()));
        assert_eq!(fs::read(&executable).unwrap(), original, "runtime choices cannot alter the executable");
    }
    let unknown = app.run("deployment_probe", &["--backend", "metal"], false);
    assert_eq!(unknown.status.code(), Some(24));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("Unknown backend"));
    // A valid library's descriptor must still belong to the requested backend.
    // Fresh child processes keep process-lived catalog registration independent.
    let wrong = available[0];
    let requested = available[1];
    let path = app.root.join("lib").join(library_filename(requested));
    let held = app.root.join("held-library");
    fs::rename(&path, &held).unwrap();
    fs::copy(app.root.join("lib").join(library_filename(wrong)), &path).unwrap();
    let mislabeled = app.run("deployment_probe", &["--backend", requested.name()], false);
    assert_eq!(mislabeled.status.code(), Some(24));
    assert!(String::from_utf8_lossy(&mislabeled.stderr).contains("does not identify the requested"));
    fs::remove_file(path).unwrap();
    fs::rename(held, app.root.join("lib").join(library_filename(requested))).unwrap();
}

/// A single real platform library must activate with no compiler registration or extra library.
#[test]
fn relocated_platform_owns_compilation_and_missing_library_reports_setup() {
    let app = Deployment::new(false);
    let valid = app.run("deployment_probe", &["valid"], false);
    assert!(valid.status.success(), "{}", String::from_utf8_lossy(&valid.stderr));
    assert!(String::from_utf8_lossy(&valid.stdout).contains("Dynamic platform root acquired"));
    for filename in [neocogi_example_plugins::library_filename(app.backend)] {
        let source = app.root.join("lib").join(filename);
        let held = app.root.join(filename);
        fs::rename(&source, &held).unwrap();
        let result = app.run("deployment_probe", &["--backend", app.backend.name()], false);
        assert_eq!(result.status.code(), Some(24), "missing {filename} must return an owned setup error");
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains(filename));
        assert!(error.contains("cargo build --profile"));
        let binary = Path::new(env!("CARGO_BIN_EXE_deployment_probe"));
        let profile = binary.parent().unwrap().file_name().unwrap().to_str().unwrap();
        let profile = if profile == "debug" { "dev" } else { profile };
        assert!(error.contains(&format!("--profile {profile}")), "{error}");
        for binary in ["gltf_damaged_helmet", "gltf_brainstem"] {
            let missing = app.run(binary, &["--frames", "1", "--backend", app.backend.name()], false);
            assert_eq!(missing.status.code(), Some(1), "missing deployment must be a normal application error");
            let error = String::from_utf8_lossy(&missing.stderr);
            assert!(error.contains(filename) && error.contains("cargo build --profile"), "{binary}: {error}");
            assert!(!error.contains("Preparing HDR") && !String::from_utf8_lossy(&missing.stdout).contains("Preparing HDR"));
        }
        fs::rename(held, source).unwrap();
    }
    for binary in ["gltf_damaged_helmet", "gltf_brainstem"] {
        assert!(app.run(binary, &["--help"], false).status.success(), "help needs no GPU or artwork");
    }
}

/// The same public render loop draws models through separately loaded shader/platform libraries.
#[test]
#[ignore = "requires fetched glTF/HDR assets and a native display/driver; GL permits SDL offscreen"]
fn relocated_dynamic_examples_render_triangle_helmet_and_animated_brainstem() {
    let app = Deployment::new(true);
    for (binary, args) in [
        ("triangle", vec!["3"]),
        ("gltf_damaged_helmet", vec!["--frames", "3"]),
        ("gltf_brainstem", vec!["--frames", "3", "--time", "1"]),
    ] {
        let output = app.run(binary, &args, true);
        assert!(
            output.status.success(),
            "{binary}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("Rendered 3"), "{binary}");
    }
}
