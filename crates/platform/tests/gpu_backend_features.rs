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
        let directory = tempfile::tempdir().unwrap();
        let package = Path::new(env!("CARGO_MANIFEST_DIR"));
        std::fs::create_dir(directory.path().join("src")).unwrap();
        std::fs::write(directory.path().join("src/lib.rs"), "//! Feature-resolution fixture.\n").unwrap();
        let dependency_path = serde_json::to_string(package.to_str().unwrap()).unwrap();
        let features = serde_json::to_string(features).unwrap();
        let manifest = format!(
            "[workspace]\n[package]\nname='gpu-feature-probe'\nversion='0.0.0'\nedition='2024'\n[dependencies]\nneocogi-platform={{path={dependency_path},default-features=false,features={features}}}\n"
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

/// Recording/admission and virtual fixtures must not require a graphics loader, SDL or browser API.
#[test]
fn cpu_only_and_virtual_fixtures_do_not_enable_native_backends() {
    for features in [vec![], vec!["test-support"]] {
        let graph = Graph::resolve(&features, &host());
        for package in ["sdl2", "ash", "wgpu", "glow", "web-sys"] {
            assert!(!graph.contains(package), "{features:?} pulled in {package}");
        }
    }
}

/// Audio services select only target audio dependencies, including host-side schema generation.
#[test]
fn audio_only_has_no_renderer_or_shader_compiler_dependency() {
    let native = Graph::resolve(&["audio"], &host());
    assert!(native.contains("sdl2"));
    let browser = Graph::resolve(&["audio"], "wasm32-unknown-unknown");
    assert!(browser.contains("web-sys"));
    assert!(!browser.contains("sdl2"));
    for graph in [native, browser] {
        for forbidden in ["naga", "glow", "ash", "wgpu"] {
            assert!(!graph.contains(forbidden), "audio enabled {forbidden}");
        }
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

/// Backends may coexist in the GPU library; WGPU counters remain an explicit test-only choice.
#[test]
fn native_features_are_additive_and_wgpu_observation_is_opt_in() {
    let native = host();
    let combined = Graph::resolve(&["glow", "vulkan", "wgpu"], &native);
    for package in ["glow", "ash", "wgpu", "sdl2"] {
        assert!(combined.contains(package), "combined graph lacks {package}");
    }
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

/// Standalone compiler outputs use the in-house writers and never a graphics implementation.
#[test]
fn compiler_output_features_are_minimal_and_independent_of_graphics() {
    let native = host();
    for output in ["shader-wgsl", "shader-spirv", "shader-glsl"] {
        let graph = Graph::resolve(&["shader-compiler", output], &native);
        for dependency in ["naga", "naga-types", "sdl2", "ash", "wgpu", "glow", "web-sys"] {
            assert!(!graph.contains(dependency), "{output} enabled {dependency}");
        }
        assert_eq!(graph.contains("spirv"), output == "shader-spirv");
        let disabled = Graph::resolve(&[output], &native);
        assert!(!disabled.packages["neocogi-platform"].contains("shader-compiler"));
    }
    for (backend, output) in [
        ("glow", "shader-glsl"),
        ("webgl", "shader-glsl"),
        ("vulkan", "shader-spirv"),
        ("wgpu", "shader-wgsl"),
    ] {
        let target = if backend == "webgl" { "wasm32-unknown-unknown" } else { &native };
        let graph = Graph::resolve(&[backend], target);
        assert!(graph.packages["neocogi-platform"].contains(output));
        assert!(graph.packages["neocogi-platform"].contains("shader-compiler"));
    }
}
