//! Test-only executable: real plugin acquisition/failure paths without constructing a GPU.
#[cfg(not(target_arch = "wasm32"))]
use neocogi_com::{ComPtr, INcStringComPtrExt, NcStatus};

/// A valid owned plugin with the right identity but deliberately incompatible root contract.
#[cfg(not(target_arch = "wasm32"))]
mod wrong {
    use super::*;
    use neocogi_render_core::abi::{IRenderDiagnostics, IRenderDiagnosticsAbi};
    #[neocogi_com::com_object(interfaces(neocogi_render_core::abi::IRenderDiagnostics))]
    /// Valid COM allocation advertising diagnostics rather than the compiler contract.
    struct Root;
    impl IRenderDiagnostics for Root {
        fn diagnostic(&self) -> Result<ComPtr<neocogi_com::INcStringAbi>, NcStatus> {
            Ok(neocogi_com::NcString::from_utf8_str("wrong compiler root"))
        }
    }
    #[neocogi_com::plugin(name = "neocogi.shader.compiler")]
    /// Deliberately wrong exact root under the required compiler plugin identity.
    fn acquire_root(_runtime: neocogi_com::Runtime) -> Result<ComPtr<IRenderDiagnosticsAbi>, NcStatus> {
        Root::from_value(Root).query()
    }
}

/// Exercises one isolated loader/acquisition outcome without creating a window.
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    // SAFETY: This test executable owns its final process-lived host.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let scenario = std::env::args().nth(1).unwrap_or_else(|| "valid".into());
    match scenario.as_str() {
        "valid" => {
            let platform = neocogi_platform_examples::acquire_platform().unwrap();
            let text = platform.diagnostic().unwrap().to_string().unwrap();
            assert!(text.is_empty(), "root acquisition must not initialize a failed native device");
            // Independent expectation catches a stale library after cached backend switches.
            let name = if cfg!(feature = "vulkan") {
                "neocogi.platform.vulkan"
            } else if cfg!(feature = "wgpu") {
                "neocogi.platform.wgpu"
            } else {
                "neocogi.platform.glow"
            };
            assert!(
                runtime.plugin_info(neocogi_com::guid_from_qualified_name(name)).unwrap().is_some(),
                "selected backend artifact must be restored before acquisition"
            );
            println!("Dynamic platform root acquired through the actual shared libraries: {name}.");
        }
        "missing-compiler" | "wrong-compiler" => {
            if scenario == "wrong-compiler" {
                // SAFETY: This deliberately incompatible descriptor is statically mapped.
                unsafe { runtime.register_plugin_descriptor(wrong::plugin_descriptor()) }.unwrap();
            }
            let directory = std::env::current_exe().unwrap().parent().unwrap().join("lib");
            // SAFETY: A freshly staged trusted real platform library; missing dependency
            // loading is deliberately fail-fatal in the SDK and runs only in a child.
            let id = unsafe { runtime.load_plugin_library(directory.join(neocogi_example_plugins::LIBRARIES[1])) };
            let result = runtime.acquire_plugin_root::<neocogi_render_core::platform::PlatformAbi>(id);
            assert!(result.is_err(), "wrong compiler root must reject before native construction");
            eprintln!("Wrong compiler contract rejected before native construction.");
            std::process::exit(23);
        }
        _ => panic!("unknown deployment fixture scenario"),
    }
}
/// This deployment fixture has no browser entry behavior.
#[cfg(target_arch = "wasm32")]
fn main() {}
