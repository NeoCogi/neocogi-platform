//! CPU-only compiler driver: compile TARGET STAGE ENTRY INPUT OUTPUT.
use neocogi_platform_contracts::render::{
    abi::GpuShaderStage,
    shader::{ShaderTarget, ShaderCode, MAX_SHADER_ARTIFACT_BYTES},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = std::env::args().skip(1).collect::<Vec<_>>();
    if a.len() != 5 {
        return Err("usage: compile wgsl|glsl|spirv vertex|fragment ENTRY INPUT OUTPUT".into());
    }
    let target = match a[0].as_str() {
        "wgsl" => ShaderTarget::Wgsl,
        "glsl" => ShaderTarget::GlslEs300,
        "spirv" => ShaderTarget::SpirV,
        _ => return Err("unknown output target".into()),
    };
    let stage = match a[1].as_str() {
        "vertex" => GpuShaderStage::Vertex,
        "fragment" => GpuShaderStage::Fragment,
        _ => return Err("unknown stage".into()),
    };
    let shader =
        neocogi_platform::render::compiler::Compiler::new(target)?.compile(stage, &std::fs::read_to_string(&a[3])?, &a[2], MAX_SHADER_ARTIFACT_BYTES)?;
    let bytes = match shader.code {
        ShaderCode::Wgsl(s) | ShaderCode::GlslEs300(s) => s.into_bytes(),
        ShaderCode::SpirV(words) => words.into_iter().flat_map(u32::to_le_bytes).collect(),
    };
    std::fs::write(&a[4], bytes)?;
    Ok(())
}
