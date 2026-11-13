//! Actual resolved consumer graphs enforce optional backends and target dependency isolation.
//!
//! Cargo tree resolution performs no nested build and creates no Cargo target artifacts. Each
//! temporary consumer is outside this workspace so platform defaults cannot unify its features.
#![cfg(not(target_arch = "wasm32"))]
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::Command,
};

/// Names and resolved feature sets of packages reachable from one selected consumer root.
struct Graph {
    /// Reachable packages only, excluding unrelated packages in the lockfile.
    packages: HashMap<String, HashSet<String>>,
}

impl Graph {
    /// Resolves a real external path consumer offline, using this repository's exact lockfile.
    fn resolve(features: &[&str], target: &str) -> Self {
        Self::resolve_features(features, target, false)
    }

    /// Resolves the ordinary default SDK, without suppressing its Glow renderer.
    fn defaults(target: &str) -> Self {
        Self::resolve_features(&[], target, true)
    }

    /// The fixture controls defaults independently of explicit backend/frontend selection.
    fn resolve_features(features: &[&str], target: &str, defaults: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let package = Path::new(env!("CARGO_MANIFEST_DIR"));
        std::fs::create_dir(directory.path().join("src")).unwrap();
        std::fs::write(directory.path().join("src/lib.rs"), "//! Feature-resolution fixture.\n").unwrap();
        let dependency_path = serde_json::to_string(package.to_str().unwrap()).unwrap();
        let features = serde_json::to_string(features).unwrap();
        let manifest = format!(
            "[workspace]\n[package]\nname='gpu-feature-probe'\nversion='0.0.0'\nedition='2024'\n[dependencies]\nneocogi-platform={{path={dependency_path},default-features={defaults},features={features}}}\n"
        );
        std::fs::write(directory.path().join("Cargo.toml"), manifest).unwrap();
        // A published package carries its own lockfile; a checkout uses its
        // enclosing workspace lockfile. Neither case requires a fixed number
        // of repository parent directories.
        let lockfile = package
            .ancestors()
            .map(|parent| parent.join("Cargo.lock"))
            .find(|path| path.is_file())
            .expect("package or workspace Cargo.lock");
        std::fs::copy(lockfile, directory.path().join("Cargo.lock")).unwrap();
        let output = Command::new(env!("CARGO"))
            .current_dir(directory.path())
            .args([
                "tree",
                "--offline",
                "--target",
                target,
                "--edges",
                "normal,build",
                "--prefix",
                "none",
                "--format",
                "{p}|{f}",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        // Cargo metadata's resolve graph can include weak-feature-only edges that are not built.
        // The selected Cargo tree is authoritative for whether a native dependency is activated.
        let tree = String::from_utf8(output.stdout).unwrap();
        let mut packages: HashMap<String, HashSet<String>> = HashMap::new();
        for line in tree.lines() {
            let Some((package, features)) = line.split_once('|') else {
                continue;
            };
            let name = package.split_whitespace().next().unwrap().to_owned();
            packages
                .entry(name)
                .or_default()
                .extend(features.trim_end_matches(" (*)").split(',').filter(|s| !s.is_empty()).map(str::to_owned));
        }
        Self { packages }
    }

    /// Checks a dependency selected by the resolved graph, rather than manifest text.
    fn contains(&self, package: &str) -> bool {
        self.packages.contains_key(package)
    }
}

/// Uses rustc's actual host triple rather than assuming the developer or CI architecture.
fn host() -> String {
    let output = Command::new("rustc").arg("-vV").output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("host: ").map(str::to_owned))
        .unwrap()
}

/// The ordinary SDK contains window/audio services and chooses Glow without explicit features.
#[test]
fn default_platform_selects_glow_and_native_audio() {
    let graph = Graph::defaults(&host());
    assert!(graph.packages["neocogi-platform"].contains("glow"));
    assert!(graph.contains("glow") && graph.contains("sdl2"));
    for unwanted in ["ash", "wgpu", "web-sys"] {
        assert!(!graph.contains(unwanted), "default platform enabled {unwanted}");
    }
}

/// GL and Vulkan select their own native dependencies; browser GL never pulls in SDL or Vulkan.
#[test]
fn selected_backends_resolve_only_their_required_target_dependencies() {
    let native = host();
    let gl = Graph::resolve(&["glow"], &native);
    assert!(gl.contains("glow") && gl.contains("sdl2"));
    assert!(!gl.contains("ash") && !gl.contains("wgpu") && !gl.contains("web-sys"));
    let vk = Graph::resolve(&["vulkan"], &native);
    assert!(vk.contains("ash") && vk.contains("sdl2"));
    assert!(!vk.contains("glow") && !vk.contains("wgpu"));
    let browser = Graph::resolve(&["webgl"], "wasm32-unknown-unknown");
    assert!(browser.contains("glow") && browser.contains("web-sys"));
    assert!(!browser.contains("sdl2") && !browser.contains("ash") && !browser.contains("wgpu"));
}

/// An explicit renderer switch drops default Glow; WGPU counters remain a test-only choice.
#[test]
fn renderer_switches_and_wgpu_observation_are_explicit() {
    let native = host();
    let vulkan = Graph::resolve(&["vulkan"], &native);
    assert!(!vulkan.packages["neocogi-platform"].contains("glow"));
    assert!(vulkan.contains("sdl2") && vulkan.contains("ash"));
    let wgpu = Graph::resolve(&["wgpu"], &native);
    assert!(!wgpu.packages["neocogi-platform"].contains("glow"));
    assert!(!wgpu.packages["wgpu"].contains("counters"));
    let observed = Graph::resolve(&["wgpu", "test-support"], &native);
    assert!(observed.packages["wgpu"].contains("counters"));
}

/// Each enabled module supplies a distinct concrete implementation of the same resource contract.
#[test]
fn enabled_modules_implement_the_typed_resource_contract() {
    #[cfg(any(feature = "glow", feature = "vulkan", feature = "wgpu", feature = "webgl", feature = "test-support"))]
    fn backend<B: neocogi_platform::gpu::native::NativeResources>() {}
    #[cfg(feature = "glow")]
    backend::<neocogi_platform::gpu::backends::glow::Backend>();
    #[cfg(all(feature = "vulkan", not(target_arch = "wasm32")))]
    backend::<neocogi_platform::gpu::backends::vulkan::Backend>();
    #[cfg(all(feature = "wgpu", not(target_arch = "wasm32")))]
    backend::<neocogi_platform::gpu::backends::wgpu::Backend>();
    #[cfg(feature = "webgl")]
    backend::<neocogi_platform::gpu::backends::webgl::Backend>();
    // Keep the generic check instantiated for CPU-only runs as well.
    #[cfg(feature = "test-support")]
    backend::<neocogi_platform::gpu::testing::Backend>();
}

/// The compiler is builtin; renderer selection alone owns the writer and SPIR-V dependency.
#[test]
fn compiler_outputs_follow_renderer_features() {
    let native = host();
    for backend in ["glow", "webgl", "vulkan", "wgpu"] {
        let target = if backend == "webgl" { "wasm32-unknown-unknown" } else { &native };
        let graph = Graph::resolve(&[backend], target);
        // WGPU may retain spirv through Naga's native translation stack.
        // Exact writer availability is exercised by shader_targets in isolated builds.
        if backend != "wgpu" {
            assert_eq!(graph.contains("spirv"), backend == "vulkan");
        }
        assert!(!graph.packages["neocogi-platform"].contains("shader-python"));
    }
}

/// The sole shader Cargo feature is the optional Python frontend, with no separate compiler/writer flags.
#[test]
fn services_are_builtin_and_only_python_is_a_shader_feature() {
    let output = Command::new(env!("CARGO"))
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(["metadata", "--offline", "--no-deps", "--format-version=1"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let package = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "neocogi-platform")
        .unwrap();
    assert_eq!(package["features"]["default"], serde_json::json!(["glow"]));
    for retired in ["audio", "platform"] {
        assert!(package["features"].get(retired).is_none(), "{retired} must be builtin");
    }
    let names: Vec<_> = package["features"]
        .as_object()
        .unwrap()
        .keys()
        .filter(|name| name.starts_with("shader-"))
        .map(String::as_str)
        .collect();
    assert_eq!(names, ["shader-python"]);
}

/// No renderer or virtual test fixture silently opts into the Python source parser.
#[test]
fn python_frontend_is_only_enabled_by_explicit_opt_in() {
    for features in [vec!["glow"], vec!["glow", "test-support"], vec!["vulkan"], vec!["wgpu"]] {
        let graph = Graph::resolve(&features, &host());
        assert!(!graph.packages["neocogi-platform"].contains("shader-python"), "{features:?} enabled Python");
    }
    let plain = Graph::defaults(&host());
    let graph = Graph::resolve_features(&["shader-python"], &host(), true);
    assert!(graph.packages["neocogi-platform"].contains("shader-python"));
    assert_eq!(graph.packages.keys().collect::<HashSet<_>>(), plain.packages.keys().collect::<HashSet<_>>());
}
