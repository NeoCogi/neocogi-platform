//! Separate processes make missing/wrong/valid provider cases independent of the
//! process-lived SDK catalog. No SDL initialization is required for activation.
#![cfg(not(target_arch = "wasm32"))]
use neocogi_com::{ComPtr, NcStatus};
use neocogi_render_core::platform::PlatformAbi;
/// Wrong root uses valid metadata and storage, but advertises another exact contract.
mod wrong {
    use super::*;
    use neocogi_render_core::abi::{IRenderDiagnostics, IRenderDiagnosticsAbi};
    #[neocogi_com::com_object(interfaces(neocogi_render_core::abi::IRenderDiagnostics))]
    struct Root;
    impl IRenderDiagnostics for Root {
        fn diagnostic(&self) -> Result<ComPtr<neocogi_com::INcStringAbi>, NcStatus> {
            Ok(neocogi_com::NcString::from_utf8_str("wrong root"))
        }
    }
    #[neocogi_com::plugin(name = "neocogi.shader.compiler")]
    fn acquire_root(_runtime: neocogi_com::Runtime) -> Result<ComPtr<IRenderDiagnosticsAbi>, NcStatus> {
        Root::from_value(Root).query()
    }
}
/// Metadata registration rejects a missing hard dependency; exact typed acquisition
/// rejects an incompatible root; valid compiler activation needs no window/context.
#[test]
fn compiler_dependency_and_root_are_required_before_native_construction() {
    let key = "NEOCOGI_COMPILER_ACTIVATION_CASE";
    let Ok(case) = std::env::var(key) else {
        for case in ["missing", "wrong", "valid"] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "compiler_dependency_and_root_are_required_before_native_construction", "--nocapture"])
                .env(key, case)
                .env("SDL_VIDEODRIVER", "deliberately-invalid")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{case}: {}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
        return;
    };
    // SAFETY: this test executable owns the process-lived host API and linked descriptors.
    let runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let expected_name = if cfg!(feature = "vulkan") {
        "neocogi.platform.vulkan"
    } else if cfg!(feature = "wgpu") {
        "neocogi.platform.wgpu"
    } else {
        "neocogi.platform.glow"
    };
    let descriptor = nc_platform::plugin_descriptor();
    assert_eq!(descriptor.plugin_id, neocogi_com::guid_from_qualified_name(expected_name));
    if case == "missing" {
        assert_eq!(
            unsafe { runtime.register_plugin_descriptor(nc_platform::plugin_descriptor()) },
            Err(neocogi_com::NC_E_NOT_FOUND)
        );
        return;
    }
    let provider = if case == "wrong" {
        wrong::plugin_descriptor()
    } else {
        nc_shader_compiler::plugin_descriptor()
    };
    // SAFETY: descriptor strings, signatures and code are statically linked for process lifetime.
    unsafe {
        runtime.register_plugin_descriptor(provider).unwrap();
        runtime.register_plugin_descriptor(nc_platform::plugin_descriptor()).unwrap();
    }
    // The descriptor owns the selected identity; consumers need no parallel selection table.
    let platform = runtime.acquire_plugin_root::<PlatformAbi>(descriptor.plugin_id);
    if case == "wrong" {
        assert!(platform.is_err());
    } else {
        assert!(platform.is_ok());
        let legacy = expected_name.replace("neocogi.platform.", "neocogi.render.");
        assert!(runtime.plugin_info(neocogi_com::guid_from_qualified_name(&legacy)).unwrap().is_none());
    }
}
