//! Common compiler/validator qualification without a renderer or checkout-relative assets.
use neocogi_platform::render::compiler::Compiler;
use neocogi_platform_contracts::render::{
    abi::GpuShaderStage,
    shader::{ShaderTarget, Shader, ShaderCode, MAX_SHADER_ARTIFACT_BYTES},
};
use std::process::Command;
/// Enabled output configurations, independent of any native backend.
pub fn targets() -> Vec<ShaderTarget> {
    let mut t = Vec::new();
    if cfg!(feature = "shader-wgsl") {
        t.push(ShaderTarget::Wgsl);
    }
    if cfg!(feature = "shader-glsl") {
        t.push(ShaderTarget::GlslEs300);
    }
    if cfg!(feature = "shader-spirv") {
        t.push(ShaderTarget::SpirV);
    }
    t
}
/// Compile one complete artifact through the public bounded CPU compiler.
pub fn compile(target: ShaderTarget, stage: GpuShaderStage, source: &str, entry: &str) -> Shader {
    Compiler::new(target)
        .unwrap()
        .compile(stage, source, entry, MAX_SHADER_ARTIFACT_BYTES)
        .unwrap_or_else(|e| panic!("{e}"))
}
/// External validators are required when requested by qualification, and run whenever installed.
pub fn validate(shader: &Shader) {
    let directory = tempfile::tempdir().unwrap();
    let (tool, args, bytes) = match &shader.code {
        ShaderCode::GlslEs300(text) => (
            "glslangValidator",
            vec!["-S", if shader.stage == GpuShaderStage::Vertex { "vert" } else { "frag" }],
            text.as_bytes().to_vec(),
        ),
        ShaderCode::SpirV(words) => (
            "spirv-val",
            vec!["--target-env", "vulkan1.0"],
            words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        ),
        ShaderCode::Wgsl(_) => return,
    };
    let path = directory.path().join("shader");
    std::fs::write(&path, bytes).unwrap();
    let output = match Command::new(tool).args(args).arg(&path).output() {
        Ok(o) => o,
        Err(e) if std::env::var_os("NEOCOGI_SHADER_VALIDATORS").is_none() && e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("{tool} unavailable; set NEOCOGI_SHADER_VALIDATORS=1 to require validation");
            return;
        }
        Err(e) => panic!("{tool}: {e}"),
    };
    assert!(
        output.status.success(),
        "{tool}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
