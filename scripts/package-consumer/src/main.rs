//! Runs independently generated COM calls against the packaged shader implementation.
use neocogi_com::{ComPtr, INcBytesComPtrExt, INcStringComPtrExt, NcCopyFromBufferOffset, Runtime, NC_OK};
use neocogi_render_core::abi::GpuShaderStage;
use neocogi_render_support as _;

/// Consumer-owned generated definitions negotiate by COM identity.
#[allow(dead_code, clippy::len_without_is_empty)]
mod client {
    include!(env!("IDL_NEOCOGI_SHADER"));
}

/// A real macro expansion from the packaged procedural macro, enabled only on nightly.
#[cfg(feature = "staging")]
const _: () = neocogi_native_layout::stage_resources!("asset.txt" => "asset.txt");

/// Retained artifact exports stay usable after their report and compiler are released.
fn main() {
    // SAFETY: This executable owns the process-lived immutable host tables.
    let runtime = unsafe { Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    // SAFETY: The descriptor graph and implementation are linked for process lifetime.
    unsafe { runtime.register_plugin_descriptor(compiler::plugin_descriptor()) }.unwrap();
    let root: ComPtr<client::ShaderCompilerAbi> = runtime.acquire_plugin_root(compiler::plugin_descriptor().plugin_id).unwrap();
    let source = b"@vertex fn vs()->@builtin(position) vec4<f32>{return vec4<f32>(0.,0.,0.,1.);}";
    let report = root
        .compile(
            client::ShaderCompileRequest {
                stage: GpuShaderStage::Vertex,
                max_output_bytes: 4 * 1024 * 1024,
            },
            NcCopyFromBufferOffset::new(0, source),
            NcCopyFromBufferOffset::new(0, b"vs"),
        )
        .unwrap();
    assert_eq!(report.status().unwrap(), NC_OK.code());
    let spirv = report.spirv().unwrap();
    let wgsl = report.wgsl().unwrap();
    let glsl = report.glsl().unwrap();
    drop(report);
    drop(root);
    assert_eq!(&spirv.to_vec().unwrap()[..4], &0x07230203u32.to_le_bytes());
    assert_eq!(wgsl.to_string().unwrap().as_bytes(), source);
    assert!(glsl.to_string().unwrap().contains("#version 300 es"));
    #[cfg(feature = "staging")]
    assert_eq!(
        std::fs::read(std::path::Path::new(env!("NEOCOGI_NATIVE_OUTPUT_DIR")).join("asset.txt")).unwrap(),
        b"packaged staging fixture\n"
    );
    println!("PASS: packaged sources, independent COM caller and retained shader exports");
}
