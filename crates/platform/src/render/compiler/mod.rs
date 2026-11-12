//! Small, bounded S-expression graphics compiler with independent backend writers.
//! The platform COM plugin exposes this built-in service in static and shared-library builds.
#![deny(missing_docs)]

mod access;
mod ir;
mod objects;
mod optimize;
#[cfg(feature = "shader-spirv")]
mod spv;
mod syntax;
mod text;
pub use objects::CompilerObject;
pub use objects::Factory;

use neocogi_platform_contracts::render::{
    abi::GpuShaderStage,
    shader::{Shader, ShaderTarget, Uniform, SampledTexture, MAX_SHADER_BYTES, MAX_SHADER_ARTIFACT_BYTES},
};

#[cfg(any(test, feature = "shader-wgsl", feature = "shader-glsl", feature = "shader-spirv"))]
use neocogi_platform_contracts::render::shader::ShaderCode;

/// Source error with a byte offset retained through parsing and semantic validation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    /// UTF-8 byte offset at the beginning of the rejected form.
    pub offset: usize,
    /// Human-readable explanation without shared mutable error state.
    pub message: String,
}
impl Diagnostic {
    /// Formats a one-based line/column and a bounded excerpt with a caret.
    pub fn render(&self, source: &str) -> String {
        let offset = self.offset.min(source.len());
        let offset = (0..=offset).rev().find(|&i| source.is_char_boundary(i)).unwrap_or(0);
        let prefix = &source[..offset];
        let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
        let start = prefix.rfind('\n').map_or(0, |i| i + 1);
        let column = source[start..offset].chars().count() + 1;
        let excerpt = source[start..].lines().next().unwrap_or("").chars().take(160).collect::<String>();
        format!(
            "{line}:{column}: {}\n{excerpt}\n{}^",
            self.message,
            " ".repeat(column.saturating_sub(1).min(160))
        )
    }
}
/// Compilation failures distinguish invalid inputs, target availability, quota, and allocation.
#[derive(Debug)]
pub enum CompileError {
    /// Syntax, type, interface, or portability error, with its source location.
    Invalid(String),
    /// Selected output plus entry name exceeded the supplied or hard cap.
    Limit,
    /// Selected output is absent from this build.
    Unavailable,
    /// A bounded output allocation failed.
    Allocation,
}
impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) => f.write_str(s),
            Self::Limit => f.write_str("shader artifact byte budget exceeded"),
            Self::Unavailable => f.write_str("shader compiler output is not enabled in this build"),
            Self::Allocation => f.write_str("shader artifact allocation failed"),
        }
    }
}
impl std::error::Error for CompileError {}

/// Stateless CPU compiler configured once for one output format.
pub struct Compiler {
    /// Immutable output selection; requests cannot override it.
    target: ShaderTarget,
}
impl Compiler {
    /// Selects an enabled writer; requests cannot change the instance's target.
    pub fn new(target: ShaderTarget) -> Result<Self, CompileError> {
        let enabled = match target {
            ShaderTarget::Wgsl => cfg!(any(test, feature = "shader-wgsl")),
            ShaderTarget::GlslEs300 => cfg!(feature = "shader-glsl"),
            ShaderTarget::SpirV => cfg!(feature = "shader-spirv"),
            _ => return Err(CompileError::Invalid("unknown shader target".into())),
        };
        if !enabled {
            return Err(CompileError::Unavailable);
        }
        Ok(Self { target })
    }
    /// Validates every declaration, then emits only the selected entry and reachable helpers/resources.
    /// The cap covers emitted bytes plus the entry name; failures never export partial code.
    // With no output enabled, construction always rejects. Keep the public method
    // available while permitting the statically divergent emission match below.
    #[cfg_attr(
        not(any(test, feature = "shader-wgsl", feature = "shader-glsl", feature = "shader-spirv")),
        allow(unreachable_code, unused_variables)
    )]
    pub fn compile(&self, stage: GpuShaderStage, source: &str, entry: &str, budget: usize) -> Result<Shader, CompileError> {
        if !matches!(stage, GpuShaderStage::Vertex | GpuShaderStage::Fragment)
            || source.is_empty()
            || source.len() > MAX_SHADER_BYTES
            || entry.is_empty()
            || entry.len() > 128
        {
            return Err(CompileError::Invalid("shader source, stage, or entry exceeds the graphics profile".into()));
        }
        let available = budget.min(MAX_SHADER_ARTIFACT_BYTES).checked_sub(entry.len()).ok_or(CompileError::Limit)?;
        let root = syntax::parse(source).map_err(|d| CompileError::Invalid(d.render(source)))?;
        let mut module = ir::Module::validate(&root).map_err(|d| CompileError::Invalid(d.render(source)))?;
        optimize::run(&mut module);
        let index = module
            .functions
            .iter()
            .position(|f| f.name == entry && f.stage == Some(stage))
            .ok_or_else(|| CompileError::Invalid("requested graphics entry point does not exist for this stage".into()))?;
        let function = &module.functions[index];
        let inputs = module.io(&function.params, stage, true).map_err(|d| CompileError::Invalid(d.render(source)))?.0;
        let outputs = module
            .io(std::slice::from_ref(&function.result), stage, false)
            .map_err(|d| CompileError::Invalid(d.render(source)))?
            .0;
        let mut uniforms = Vec::new();
        let mut textures = Vec::new();
        for &i in &function.resources {
            let resource = &module.resources[i];
            if resource.ty == ir::Ty::Texture {
                textures.push(SampledTexture {
                    slot: resource.slot,
                    gl_name: (self.target == ShaderTarget::GlslEs300).then(|| text::texture_name(resource)),
                });
            } else {
                uniforms.push(Uniform {
                    slot: resource.slot,
                    byte_len: module.layout(&resource.ty).1,
                    gl_block: (self.target == ShaderTarget::GlslEs300).then(|| text::block_name(resource)),
                });
            }
        }
        uniforms.sort_by_key(|r| r.slot);
        textures.sort_by_key(|r| r.slot);
        let code = match self.target {
            #[cfg(any(test, feature = "shader-wgsl"))]
            ShaderTarget::Wgsl => ShaderCode::Wgsl(text::emit(&module, index, false, available)?),
            #[cfg(feature = "shader-glsl")]
            ShaderTarget::GlslEs300 => ShaderCode::GlslEs300(text::emit(&module, index, true, available)?),
            #[cfg(feature = "shader-spirv")]
            ShaderTarget::SpirV => ShaderCode::SpirV(spv::emit(&module, index, available)?),
            _ => return Err(CompileError::Unavailable),
        };
        Ok(Shader {
            stage,
            entry: entry.into(),
            code,
            inputs,
            outputs,
            uniforms,
            textures,
        })
    }
}
