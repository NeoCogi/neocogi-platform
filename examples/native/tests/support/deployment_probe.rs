//! Test-only executable: standalone platform acquisition without constructing a GPU.
/// Loads the single selected platform library with no external compiler registration.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let launch = neocogi_platform_examples::LaunchArguments::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(24);
    });
    let _platform = neocogi_platform_examples::acquire_platform(launch.backend.as_deref()).unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(24);
    });
    // SAFETY: This executable owns the process-lived host, already initialized by acquisition.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let loaded: Vec<_> = neocogi_example_plugins::Backend::ALL
        .into_iter()
        .filter(|backend| {
            runtime
                .plugin_info(neocogi_com::guid_from_qualified_name(backend.plugin_name()))
                .unwrap()
                .is_some()
        })
        .collect();
    assert_eq!(loaded.len(), 1, "exactly one requested backend is activated");
    let name = loaded[0].plugin_name();
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
