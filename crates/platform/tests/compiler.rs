//! Independently generated COM calls verify format selection, ownership and exact quotas.
use neocogi_com::{ComPtr, INcBytesComPtrExt, INcStringComPtrExt, NcCopyFromBufferOffset, NC_OK, NC_E_INVALID_ARG, NC_E_NOINTERFACE, NC_E_OUT_OF_RANGE};
use neocogi_platform::{
    compiler::CompilerObject,
    render::abi::GpuShaderStage,
    shader::{self, PreparedShader, ShaderTarget},
};

/// A separately compiled projection negotiates by ABI identity, never a Rust object cast.
#[allow(dead_code, clippy::len_without_is_empty, reason = "independent len/get ABI caller")]
mod client {
    include!(env!("IDL_NEOCOGI_SHADER"));
}

const SOURCE: &str = "struct U { value:vec4<f32> } @group(1) @binding(0) var<uniform> u:U; @vertex fn vs(@location(0) p:vec2<f32>)->@builtin(position) vec4<f32> {return vec4<f32>(p,0.25,1.0)+u.value;}";

/// Build flags determine availability, while every individual request selects one representation.
fn targets() -> Vec<ShaderTarget> {
    let mut values = vec![ShaderTarget::Wgsl];
    if cfg!(feature = "shader-spirv") {
        values.push(ShaderTarget::SpirV);
    }
    if cfg!(feature = "shader-glsl") {
        values.push(ShaderTarget::GlslEs300);
    }
    values
}

/// Keeps independent generated request values distinct from the implementation's Rust types.
fn compile(stage: GpuShaderStage, target: ShaderTarget, source: &str, entry: &str, cap: u64) -> ComPtr<client::IShaderCompilationAbi> {
    let root: ComPtr<client::ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    root.compile_result(
        client::ShaderCompileRequest {
            stage,
            target: client::ShaderTarget::from_raw(target.raw()),
            max_output_bytes: cap,
        },
        NcCopyFromBufferOffset::new(0, source.as_bytes()),
        NcCopyFromBufferOffset::new(0, entry.as_bytes()),
    )
    .unwrap()
}

/// Only selected code is exported; independently retained code/items outlive report and root.
#[test]
fn exactly_one_owned_format_and_target_specific_names() {
    for target in targets() {
        let report = compile(GpuShaderStage::Vertex, target, SOURCE, "vs", 4194304);
        assert_eq!(report.status().unwrap(), NC_OK.code());
        assert_eq!(report.target().unwrap().raw(), target.raw());
        let info = report.info().unwrap();
        assert_eq!(info.target, report.target().unwrap());
        let inputs = report.inputs().unwrap();
        assert_eq!(inputs.len().unwrap(), 1);
        assert_eq!(inputs.get(1).unwrap_err(), NC_E_OUT_OF_RANGE);
        let location = inputs.get(0).unwrap();
        let uniforms = report.uniforms().unwrap();
        let uniform = uniforms.get(0).unwrap();
        assert_eq!(uniform.info().unwrap().byte_len, 16);
        assert_eq!(info.uniform_count, uniforms.len().unwrap());
        match target {
            ShaderTarget::Wgsl => {
                assert_eq!(report.glsl().unwrap_err(), NC_E_NOINTERFACE);
                assert_eq!(report.spirv().unwrap_err(), NC_E_NOINTERFACE);
                assert_eq!(uniform.glsl_block_name().unwrap_err(), NC_E_NOINTERFACE);
                let code = report.wgsl().unwrap();
                assert_eq!(info.code_byte_count, code.byte_len().unwrap());
                drop(report);
                assert_eq!(code.to_string().unwrap(), SOURCE);
            }
            ShaderTarget::SpirV => {
                assert_eq!(report.glsl().unwrap_err(), NC_E_NOINTERFACE);
                assert_eq!(report.wgsl().unwrap_err(), NC_E_NOINTERFACE);
                assert_eq!(uniform.glsl_block_name().unwrap_err(), NC_E_NOINTERFACE);
                let code = report.spirv().unwrap();
                assert_eq!(info.code_byte_count, code.len().unwrap());
                drop(report);
                assert_eq!(&code.to_vec().unwrap()[..4], &0x07230203u32.to_le_bytes());
            }
            ShaderTarget::GlslEs300 => {
                assert_eq!(report.wgsl().unwrap_err(), NC_E_NOINTERFACE);
                assert_eq!(report.spirv().unwrap_err(), NC_E_NOINTERFACE);
                let name = uniform.glsl_block_name().unwrap();
                let code = report.glsl().unwrap();
                assert_eq!(info.code_byte_count, code.byte_len().unwrap());
                drop(report);
                let text = code.to_string().unwrap();
                assert!(text.starts_with("#version 300 es"));
                assert!(text.contains(&name.to_string().unwrap()));
            }
            _ => unreachable!(),
        }
        drop((inputs, uniforms));
        assert_eq!(location.info().unwrap().location, 0);
        assert_eq!(uniform.info().unwrap().byte_len, 16);
    }
}

/// Every output uses the same whole-module admission, including unused forbidden declarations.
#[test]
fn complete_module_profile_admission_precedes_selected_exports() {
    for target in targets() {
        for case in shader::testing::cases() {
            let module = naga::front::wgsl::parse_str(&case.source).unwrap();
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty())
                .validate(&module)
                .unwrap();
            for (stage, entry) in [(GpuShaderStage::Vertex, "selected"), (case.stage, "outside")] {
                let report = compile(stage, target, &case.source, entry, 4194304);
                assert_eq!(report.status().unwrap(), NC_E_INVALID_ARG.code(), "{} / {entry} / {target:?}", case.name);
                assert_eq!(report.wgsl().unwrap_err(), NC_E_INVALID_ARG);
                assert_eq!(report.spirv().unwrap_err(), NC_E_INVALID_ARG);
                assert_eq!(report.glsl().unwrap_err(), NC_E_INVALID_ARG);
            }
        }
        let source = shader::testing::valid_mixed_module();
        for (stage, entry, count) in [(GpuShaderStage::Vertex, "selected", 0), (GpuShaderStage::Fragment, "outside", 1)] {
            let report = compile(stage, target, &source, entry, 4194304);
            assert_eq!(report.status().unwrap(), NC_OK.code(), "{}", report.diagnostic().unwrap().to_string().unwrap());
            assert_eq!(report.sampled_textures().unwrap().len().unwrap(), count);
        }
    }
}

/// Target mismatch is rejected before code import; exact caps charge entry plus selected bytes only.
#[test]
fn exact_selected_payload_limit_and_bounded_snapshot() {
    for target in targets() {
        let report = compile(GpuShaderStage::Vertex, target, SOURCE, "vs", 4194304);
        let info = report.info().unwrap();
        let report = report.query::<shader::abi::IShaderCompilationAbi>().unwrap();
        let other = if target == ShaderTarget::Wgsl {
            ShaderTarget::SpirV
        } else {
            ShaderTarget::Wgsl
        };
        assert!(matches!(
            PreparedShader::new(report.clone(), GpuShaderStage::Vertex, other, b"vs", 4194304),
            Err(NC_E_INVALID_ARG)
        ));
        let prepared = PreparedShader::new(report, GpuShaderStage::Vertex, target, b"vs", 4194304).unwrap();
        let size = prepared.byte_len();
        assert_eq!(size as u64, info.entry_point_byte_count + info.code_byte_count);
        let artifact = prepared.copy().unwrap();
        assert_eq!(artifact.byte_len(), size);
        assert_eq!(artifact.code.target(), target);
        assert_eq!(
            compile(GpuShaderStage::Vertex, target, SOURCE, "vs", size as u64).status().unwrap(),
            NC_OK.code()
        );
        assert_eq!(
            compile(GpuShaderStage::Vertex, target, SOURCE, "vs", size as u64 - 1).status().unwrap(),
            NC_E_OUT_OF_RANGE.code()
        );
    }
}

/// Disabled known targets reject explicitly; invalid wire enum values never enter provider code.
#[test]
fn unavailable_targets_and_unknown_wire_tags_reject() {
    for target in [ShaderTarget::SpirV, ShaderTarget::GlslEs300] {
        if !targets().contains(&target) {
            let report = compile(GpuShaderStage::Vertex, target, SOURCE, "vs", 4194304);
            assert_eq!(report.status().unwrap(), shader::SHADER_E_TARGET_UNAVAILABLE.code());
            assert_eq!(report.wgsl().unwrap_err(), shader::SHADER_E_TARGET_UNAVAILABLE);
            assert_eq!(report.target().unwrap_err(), shader::SHADER_E_TARGET_UNAVAILABLE);
        }
    }
    let root: ComPtr<client::ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
    let result = root.compile_result(
        client::ShaderCompileRequest {
            stage: GpuShaderStage::Vertex,
            target: client::ShaderTarget::from_raw(255),
            max_output_bytes: 4194304,
        },
        NcCopyFromBufferOffset::new(0, SOURCE.as_bytes()),
        NcCopyFromBufferOffset::new(0, b"vs"),
    );
    assert!(matches!(result, Err(neocogi_com::NC_E_INVALID_TAG)));
}

/// Owned failures remain independent of later calls and compiler lifetime for every target.
#[test]
fn rejection_is_complete_and_diagnostics_are_per_call() {
    for target in targets() {
        for (source, entry, cap, expected) in [
            ("invalid", "vs", 4194304, NC_E_INVALID_ARG),
            (SOURCE, "missing", 4194304, NC_E_INVALID_ARG),
            (SOURCE, "vs", 1, NC_E_OUT_OF_RANGE),
        ] {
            let report = compile(GpuShaderStage::Vertex, target, source, entry, cap);
            assert_eq!(report.status().unwrap(), expected.code());
            assert_eq!(report.wgsl().unwrap_err(), expected);
            assert_eq!(report.spirv().unwrap_err(), expected);
            assert_eq!(report.glsl().unwrap_err(), expected);
            let diagnostic = report.diagnostic().unwrap();
            drop(report);
            assert!(!diagnostic.to_string_bounded(4096).unwrap().is_empty());
        }
        let root: ComPtr<shader::abi::ShaderCompilerAbi> = CompilerObject::from_value(CompilerObject).query().unwrap();
        let request = shader::abi::ShaderCompileRequest {
            stage: GpuShaderStage::Vertex,
            target,
            max_output_bytes: 4194304,
        };
        let error = root
            .compile(request, NcCopyFromBufferOffset::new(0, b"invalid"), NcCopyFromBufferOffset::new(0, b"vs"))
            .unwrap_err();
        assert_eq!(error.status, NC_E_INVALID_ARG);
        let text = error.message.clone();
        drop(
            root.compile(
                request,
                NcCopyFromBufferOffset::new(0, SOURCE.as_bytes()),
                NcCopyFromBufferOffset::new(0, b"vs"),
            )
            .unwrap(),
        );
        drop(root);
        assert_eq!(error.message, text);
    }
}
