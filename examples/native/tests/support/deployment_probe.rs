//! Test-only executable: standalone platform acquisition without constructing a GPU.
/// Loads the single selected platform library with no external compiler registration.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let _platform = neocogi_platform_examples::acquire_platform().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(24);
    });
    // SAFETY: This executable owns the process-lived host, already initialized by acquisition.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let name = if cfg!(feature = "vulkan") {
        "neocogi.platform.vulkan"
    } else if cfg!(feature = "wgpu") {
        "neocogi.platform.wgpu"
    } else {
        "neocogi.platform.glow"
    };
    assert!(runtime.plugin_info(neocogi_com::guid_from_qualified_name(name)).unwrap().is_some());
    assert!(
        runtime
            .plugin_info(neocogi_com::guid_from_qualified_name("neocogi.shader.compiler"))
            .unwrap()
            .is_none()
    );
    println!("Dynamic platform root acquired through its single shared library: {name}.");
}
/// This deployment fixture has no browser entry behavior.
#[cfg(target_arch = "wasm32")]
fn main() {}
