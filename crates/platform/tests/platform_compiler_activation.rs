//! Platform activation has no external compiler dependency and needs no native subsystem.
#![cfg(not(target_arch = "wasm32"))]
use neocogi_platform::render::platform::PlatformAbi;

/// Registering only the platform creates owned roots without adding a compiler plugin to the catalog.
#[test]
fn renderer_owns_compilation_without_a_separate_compiler_plugin() {
    const CHILD: &str = "NEOCOGI_OWNED_COMPILER_ACTIVATION";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "renderer_owns_compilation_without_a_separate_compiler_plugin", "--nocapture"])
            .env(CHILD, "1")
            .env("SDL_VIDEODRIVER", "deliberately-invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // SAFETY: This executable owns the immutable process-lived host and linked descriptor graph.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let descriptor = neocogi_platform::platform::plugin_descriptor();
    // SAFETY: All descriptor metadata and code remain statically mapped for process lifetime.
    unsafe { runtime.register_plugin_descriptor(descriptor) }.unwrap();
    let first = runtime.acquire_plugin_root::<PlatformAbi>(descriptor.plugin_id).unwrap();
    let second = runtime.acquire_plugin_root::<PlatformAbi>(descriptor.plugin_id).unwrap();
    drop((first, second));
    assert!(
        runtime
            .plugin_info(neocogi_com::guid_from_qualified_name("neocogi.shader.compiler"))
            .unwrap()
            .is_none()
    );
}
