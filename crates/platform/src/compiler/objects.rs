//! Owned report graph: reports own exports, collections own items, items own only
//! their own metadata/name. There are no back edges or whole-code item captures.
use crate::compiler::compile::{Compiler, CompileError};
use neocogi_com::{
    com_object, ComPtr, NcStatus, NcString, NcBytes, NcCopyFromBufferOffset, INcStringAbi, INcBytesAbi, NC_E_INVALID_ARG, NC_E_OUT_OF_RANGE, NC_E_OUT_OF_MEMORY,
};
use neocogi_platform::shader::{abi::*, Shader, MAX_DIAGNOSTIC_BYTES};

/// CPU-only root; no global diagnostic or session state.
#[com_object(interfaces(neocogi_platform::shader::abi::ShaderCompiler))]
pub struct CompilerObject;
impl ShaderCompiler for CompilerObject {
    fn compile_result(
        &self,
        request: ShaderCompileRequest,
        source: NcCopyFromBufferOffset<'_, u8>,
        entry: NcCopyFromBufferOffset<'_, u8>,
    ) -> Result<ComPtr<IShaderCompilationAbi>, NcStatus> {
        let result = (|| {
            if source.offset() != 0 || entry.offset() != 0 {
                return Err(CompileError::Invalid("nonzero input offset".into()));
            }
            let source = std::str::from_utf8(source.as_slice()).map_err(|_| CompileError::Invalid("shader is not UTF-8".into()))?;
            let entry = std::str::from_utf8(entry.as_slice()).map_err(|_| CompileError::Invalid("entry is not UTF-8".into()))?;
            Compiler::compile_bounded(request.stage, source, entry, usize::try_from(request.max_output_bytes).unwrap_or(usize::MAX))
        })();
        let outcome = match result {
            Ok(shader) => Outcome::Succeeded(Exports::new(shader)?),
            Err(error) => {
                let status = match error {
                    CompileError::Invalid(_) => NC_E_INVALID_ARG,
                    CompileError::Limit => NC_E_OUT_OF_RANGE,
                    CompileError::Allocation => NC_E_OUT_OF_MEMORY,
                };
                let mut detail = error.to_string();
                let mut end = detail.len().min(MAX_DIAGNOSTIC_BYTES);
                while !detail.is_char_boundary(end) {
                    end -= 1;
                }
                detail.truncate(end);
                Outcome::Rejected {
                    status,
                    diagnostic: NcString::from_string(detail),
                }
            }
        };
        Report::from_value(Report { outcome }).query()
    }
}

/// A successful graph is complete; rejection cannot accidentally export partial code.
enum Outcome {
    /// All translation and admission steps finished.
    Succeeded(Exports),
    /// A typed compiler failure and bounded per-call text.
    Rejected { status: NcStatus, diagnostic: ComPtr<INcStringAbi> },
}
/// Independent immutable code exports and typed collections, each stored once.
struct Exports {
    /// Optional count shortcut derived from the same data.
    info: ShaderInfo,
    /// Selected entry name.
    entry: ComPtr<INcStringAbi>,
    /// Validated canonical source.
    wgsl: ComPtr<INcStringAbi>,
    /// ES300 code with fixed coordinate semantics.
    glsl: ComPtr<INcStringAbi>,
    /// Guarded little-endian words.
    spirv: ComPtr<INcBytesAbi>,
    /// Location input items.
    inputs: ComPtr<IShaderLocationsAbi>,
    /// Location output items.
    outputs: ComPtr<IShaderLocationsAbi>,
    /// Active uniform items.
    uniforms: ComPtr<IShaderUniformsAbi>,
    /// Active paired textures.
    textures: ComPtr<IShaderSampledTexturesAbi>,
    /// Success has an owned empty diagnostic.
    diagnostic: ComPtr<INcStringAbi>,
}
impl Exports {
    /// Moves plain outputs into independent SDK-owned exports before publishing the report.
    fn new(shader: Shader) -> Result<Self, NcStatus> {
        let info = ShaderInfo {
            stage: shader.stage,
            input_count: shader.inputs.len() as u32,
            output_count: shader.outputs.len() as u32,
            uniform_count: shader.uniforms.len() as u32,
            sampled_texture_count: shader.textures.len() as u32,
            entry_point_byte_count: shader.entry.len() as u64,
            wgsl_byte_count: shader.wgsl.len() as u64,
            spirv_word_count: shader.spirv.len() as u64,
            glsl_byte_count: shader.glsl.len() as u64,
        };
        let locations = |values: Vec<neocogi_platform::shader::IoLocation>, direction| -> Result<ComPtr<IShaderLocationsAbi>, NcStatus> {
            let items = values
                .into_iter()
                .map(|value| {
                    Location::from_value(Location {
                        info: ShaderLocationInfo {
                            stage: shader.stage,
                            direction,
                            location: value.location,
                            scalar: value.shape.kind,
                            components: value.shape.components,
                            interpolation: value.interpolation,
                            sampling: value.sampling,
                        },
                    })
                    .query()
                })
                .collect::<Result<_, _>>()?;
            Locations::from_value(Locations { items }).query()
        };
        let inputs = locations(shader.inputs, ShaderDirection::Input)?;
        let outputs = locations(shader.outputs, ShaderDirection::Output)?;
        let uniforms = Uniforms::from_value(Uniforms {
            items: shader
                .uniforms
                .into_iter()
                .map(|v| {
                    Uniform::from_value(Uniform {
                        info: ShaderUniformInfo {
                            stage: shader.stage,
                            slot: v.slot,
                            byte_len: v.byte_len,
                        },
                        name: NcString::from_string(v.gl_block),
                    })
                    .query()
                })
                .collect::<Result<_, _>>()?,
        })
        .query()?;
        let textures = Textures::from_value(Textures {
            items: shader
                .textures
                .into_iter()
                .map(|v| {
                    Texture::from_value(Texture {
                        info: ShaderSampledTextureInfo { stage: shader.stage, slot: v.slot },
                        name: NcString::from_string(v.gl_name),
                    })
                    .query()
                })
                .collect::<Result<_, _>>()?,
        })
        .query()?;
        let mut spirv = Vec::new();
        spirv.try_reserve_exact(shader.spirv.len() * 4).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        for word in shader.spirv {
            spirv.extend_from_slice(&word.to_le_bytes());
        }
        Ok(Self {
            info,
            entry: NcString::from_string(shader.entry),
            wgsl: NcString::from_string(shader.wgsl),
            glsl: NcString::from_string(shader.glsl),
            spirv: NcBytes::from_vec(spirv),
            inputs,
            outputs,
            uniforms,
            textures,
            diagnostic: NcString::from_utf8_str(""),
        })
    }
}
/// One immutable report, with exhaustive access to success versus rejection.
#[com_object(interfaces(neocogi_platform::shader::abi::IShaderCompilation))]
struct Report {
    /// Complete immutable outcome.
    outcome: Outcome,
}
impl Report {
    /// Preserve the original rejection status for every artifact getter.
    fn exports(&self) -> Result<&Exports, NcStatus> {
        match &self.outcome {
            Outcome::Succeeded(v) => Ok(v),
            Outcome::Rejected { status, .. } => Err(*status),
        }
    }
}
impl IShaderCompilation for Report {
    fn status(&self) -> Result<i32, NcStatus> {
        Ok(match &self.outcome {
            Outcome::Succeeded(_) => neocogi_com::NC_OK,
            Outcome::Rejected { status, .. } => *status,
        }
        .code())
    }
    fn diagnostic(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(match &self.outcome {
            Outcome::Succeeded(v) => v.diagnostic.clone(),
            Outcome::Rejected { diagnostic, .. } => diagnostic.clone(),
        })
    }
    fn info(&self) -> Result<ShaderInfo, NcStatus> {
        Ok(self.exports()?.info)
    }
    fn entry_point(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(self.exports()?.entry.clone())
    }
    fn wgsl(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(self.exports()?.wgsl.clone())
    }
    fn spirv(&self) -> Result<ComPtr<INcBytesAbi>, NcStatus> {
        Ok(self.exports()?.spirv.clone())
    }
    fn glsl(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(self.exports()?.glsl.clone())
    }
    fn inputs(&self) -> Result<ComPtr<IShaderLocationsAbi>, NcStatus> {
        Ok(self.exports()?.inputs.clone())
    }
    fn outputs(&self) -> Result<ComPtr<IShaderLocationsAbi>, NcStatus> {
        Ok(self.exports()?.outputs.clone())
    }
    fn uniforms(&self) -> Result<ComPtr<IShaderUniformsAbi>, NcStatus> {
        Ok(self.exports()?.uniforms.clone())
    }
    fn sampled_textures(&self) -> Result<ComPtr<IShaderSampledTexturesAbi>, NcStatus> {
        Ok(self.exports()?.textures.clone())
    }
}
/// Independent numeric location item.
#[com_object(interfaces(neocogi_platform::shader::abi::IShaderLocation))]
struct Location {
    /// No parent owner.
    info: ShaderLocationInfo,
}
impl IShaderLocation for Location {
    fn info(&self) -> Result<ShaderLocationInfo, NcStatus> {
        Ok(self.info)
    }
}
/// Independent uniform item and native name.
#[com_object(interfaces(neocogi_platform::shader::abi::IShaderUniform))]
struct Uniform {
    /// Copied layout.
    info: ShaderUniformInfo,
    /// Native block identifier.
    name: ComPtr<INcStringAbi>,
}
impl IShaderUniform for Uniform {
    fn info(&self) -> Result<ShaderUniformInfo, NcStatus> {
        Ok(self.info)
    }
    fn glsl_block_name(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(self.name.clone())
    }
}
/// Independent sampled-pair item and native name.
#[com_object(interfaces(neocogi_platform::shader::abi::IShaderSampledTexture))]
struct Texture {
    /// Copied slot.
    info: ShaderSampledTextureInfo,
    /// Native combined sampler identifier.
    name: ComPtr<INcStringAbi>,
}
impl IShaderSampledTexture for Texture {
    fn info(&self) -> Result<ShaderSampledTextureInfo, NcStatus> {
        Ok(self.info)
    }
    fn glsl_name(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        Ok(self.name.clone())
    }
}
/// Generates only storage/retention mechanics for three explicitly named contracts.
macro_rules! collection {
    ($object:ident,$contract:path,$trait:ident,$item:ty) => {
        /// Immutable collection, retaining only its own independent items.
        #[com_object(interfaces($contract))]
        struct $object {
            /// Bounded items in stable index order.
            items: Vec<ComPtr<$item>>,
        }
        impl $trait for $object {
            fn len(&self) -> Result<u32, NcStatus> {
                Ok(self.items.len() as u32)
            }
            fn get(&self, index: u32) -> Result<ComPtr<$item>, NcStatus> {
                self.items.get(index as usize).cloned().ok_or(NC_E_OUT_OF_RANGE)
            }
        }
    };
}
collection!(Locations, neocogi_platform::shader::abi::IShaderLocations, IShaderLocations, IShaderLocationAbi);
collection!(Uniforms, neocogi_platform::shader::abi::IShaderUniforms, IShaderUniforms, IShaderUniformAbi);
collection!(
    Textures,
    neocogi_platform::shader::abi::IShaderSampledTextures,
    IShaderSampledTextures,
    IShaderSampledTextureAbi
);
