//! Runs independently generated COM calls against the packaged shader implementation.
use neocogi_com::{ComPtr, INcBytesComPtrExt, NcCopyFromBufferOffset, Runtime, NC_OK};
use neocogi_platform::render::compiler::CompilerObject;

/// Consumer-owned generated definitions negotiate by COM identity.
#[allow(dead_code, clippy::len_without_is_empty)]
mod client {
    include!(env!("IDL_NEOCOGI_RENDERER"));
}

/// Retained artifact exports stay usable after their report and compiler are released.
fn main() {
    // SAFETY: This executable owns the process-lived immutable host tables.
    let _runtime = unsafe { Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.unwrap();
    let root: ComPtr<client::ShaderCompilerAbi> =
        CompilerObject::from_value(CompilerObject::new(neocogi_platform::render::shader::ShaderTarget::Wgsl).unwrap())
            .query()
            .unwrap();
    let source = b"@vertex fn vs()->@builtin(position) vec4<f32>{return vec4<f32>(0.,0.,0.,1.);}";
    let report = root
        .compile_result(
            client::ShaderCompileRequest {
                stage: client::GpuShaderStage::Vertex,
                max_output_bytes: 4 * 1024 * 1024,
            },
            NcCopyFromBufferOffset::new(0, source),
            NcCopyFromBufferOffset::new(0, b"vs"),
        )
        .unwrap();
    assert_eq!(report.status().unwrap(), NC_OK.code());
    assert_eq!(report.target().unwrap(), client::ShaderTarget::Wgsl);
    assert_eq!(report.info().unwrap().target, client::ShaderTarget::Wgsl);
    // A consumer generated from the merged renderer schema interoperates with
    // ordinary SDK readers by typed QI, including backend format validation.
    use neocogi_platform::render::{
        abi,
        shader::{PreparedShader, ShaderTarget},
    };
    let canonical = report.query::<abi::IShaderCompilationAbi>().unwrap();
    assert!(matches!(
        PreparedShader::new(canonical.clone(), abi::GpuShaderStage::Vertex, ShaderTarget::SpirV, b"vs", 4 * 1024 * 1024),
        Err(neocogi_com::NC_E_INVALID_ARG)
    ));
    let prepared = PreparedShader::new(canonical, abi::GpuShaderStage::Vertex, ShaderTarget::Wgsl, b"vs", 4 * 1024 * 1024).unwrap();
    assert_eq!(prepared.copy().unwrap().code.target(), ShaderTarget::Wgsl);
    drop(prepared);
    let code = report.code().unwrap();
    drop(report);
    drop(root);
    assert_eq!(code.to_vec().unwrap(), source);
    assert_eq!(
        std::fs::read(std::path::Path::new(env!("NEOCOGI_NATIVE_OUTPUT_DIR")).join("asset.txt")).unwrap(),
        include_bytes!("../asset.txt")
    );
    println!("PASS: packaged sources, independent COM caller and retained shader exports");
}
