//! Runs independently generated COM calls against the packaged shader implementation.
use neocogi_com::{ComPtr, INcStringComPtrExt, NcCopyFromBufferOffset, Runtime, NC_OK};
use neocogi_platform::render::abi::GpuShaderStage;
use neocogi_platform::compiler;

/// Consumer-owned generated definitions negotiate by COM identity.
#[allow(dead_code, clippy::len_without_is_empty)]
mod client {
    include!(env!("IDL_NEOCOGI_SHADER"));
}

/// Retained artifact exports stay usable after their report and compiler are released.
fn main() {
    // SAFETY: This executable owns the process-lived immutable host tables.
    let runtime = unsafe { Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    // SAFETY: The descriptor graph and implementation are linked for process lifetime.
    unsafe { runtime.register_plugin_descriptor(compiler::plugin_descriptor()) }.unwrap();
    let root: ComPtr<client::ShaderCompilerAbi> = runtime.acquire_plugin_root(compiler::plugin_descriptor().plugin_id).unwrap();
    let source = b"@vertex fn vs()->@builtin(position) vec4<f32>{return vec4<f32>(0.,0.,0.,1.);}";
    let report = root
        .compile_result(
            client::ShaderCompileRequest {
                stage: GpuShaderStage::Vertex,
                target: client::ShaderTarget::Wgsl,
                max_output_bytes: 4 * 1024 * 1024,
            },
            NcCopyFromBufferOffset::new(0, source),
            NcCopyFromBufferOffset::new(0, b"vs"),
        )
        .unwrap();
    assert_eq!(report.status().unwrap(), NC_OK.code());
    assert_eq!(report.spirv().unwrap_err(), neocogi_com::NC_E_NOINTERFACE);
    let wgsl = report.wgsl().unwrap();
    assert_eq!(report.glsl().unwrap_err(), neocogi_com::NC_E_NOINTERFACE);
    drop(report);
    drop(root);
    assert_eq!(wgsl.to_string().unwrap().as_bytes(), source);
    assert_eq!(
        std::fs::read(std::path::Path::new(env!("NEOCOGI_NATIVE_OUTPUT_DIR")).join("asset.txt")).unwrap(),
        include_bytes!("../asset.txt")
    );
    println!("PASS: packaged sources, independent COM caller and retained shader exports");
}
