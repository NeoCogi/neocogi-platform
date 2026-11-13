//! Structural cost regressions complement native pixels; no GPU-specific instruction count is assumed.
use neocogi_platform::gpu::testing::optimization;
use neocogi_platform_contracts::render::{abi::GpuShaderStage, shader::ShaderCode};
#[path = "support/compilation.rs"]
mod compilation;
use compilation::{compile, targets, validate};

/// Compiler optimizations must still produce independently validated code for every output.
#[test]
fn aggregate_corpus_validates_for_every_enabled_output() {
    for case in optimization::cases() {
        for target in targets() {
            for (stage, entry) in [(GpuShaderStage::Vertex, "vs"), (GpuShaderStage::Fragment, "fs")] {
                let shader = compile(target, stage, &case.source, entry);
                validate(&shader);
                assert_eq!(
                    shader.uniforms.len(),
                    usize::from(case.uniform && stage == GpuShaderStage::Fragment),
                    "{}",
                    case.name
                );
                if let Some(uniform) = shader.uniforms.first() {
                    assert_eq!(uniform.byte_len, optimization::UNIFORM_BYTES);
                }
            }
        }
    }
}

/// Constant failures must return zero without a runtime check or an indexed parent copy.
#[test]
fn constant_indices_do_not_generate_bounds_branches() {
    for index in ["0", "3u", "-1", "4", "4294967295u"] {
        let source = format!(
            "(module (struct Data (values (array vec4 4))) (uniform data Data vertex 0) (vertex vs () (returns vec4 (builtin position)) (return (index (. data values) {index}))))"
        );
        for target in targets() {
            let shader = compile(target, GpuShaderStage::Vertex, &source, "vs");
            validate(&shader);
            match shader.code {
                ShaderCode::Wgsl(text) | ShaderCode::GlslEs300(text) => assert!(!text.contains("if ("), "{text}"),
                #[cfg(feature = "vulkan")]
                ShaderCode::SpirV(words) => assert_eq!(spv::instructions(&words).iter().filter(|i| i.op == spirv::Op::ULessThan as u32).count(), 0),
                #[cfg(not(feature = "vulkan"))]
                ShaderCode::SpirV(_) => {}
            }
        }
    }
}

/// Expanding an aggregate helper must leave short-circuit RHS work conditional and preserve loop reevaluation.
#[test]
fn expanded_calls_keep_loop_conditions_and_short_circuiting_structured() {
    let source = r#"(module
      (struct Data (values (array vec4 4))) (uniform data Data fragment 0)
      (fn test ((values (array vec4 4)) (i i32)) (returns bool)
        (if (< i 0) (then (return false))) (return (> (. (index values i) x) 0.0)))
      (fragment fs () (returns vec4 (location 0))
        (var i i32 0) (var result f32 0.0)
        (while (&& (< i 4) (test (. data values) i))
          (set i (+ i 1)) (if (== i 1) (then (continue))) (set result (+ result 0.25)))
        (return (vec4 result 0.0 0.0 1.0))))"#;
    for target in targets() {
        validate(&compile(target, GpuShaderStage::Fragment, source, "fs"));
    }
}

/// Eager unused arguments must remain outside a helper's divergent branches.
fn eager_argument_source() -> &'static str {
    r#"(module
      (struct Data (values (array vec4 4))) (uniform data Data fragment 0)
      (fn pick ((values (array vec4 4)) (i i32) (unused f32)) (returns vec4)
        (if (< i 0) (then (return (vec4 0.0))))
        (return (index values i)))
      (fragment fs ((p vec4 (builtin position))) (returns vec4 (location 0))
        (return (pick (. data values) (- (i32 (. p x)) 1) (dpdx (. p x))))))"#
}

/// Deep aggregate call chains must fall back to valid ordinary calls at the expansion limit.
fn deep_helper_source() -> String {
    let mut source = String::from(
        "(module (struct Data (values (array vec4 4))) (uniform data Data fragment 0)
         (fn f0 ((values (array vec4 4))) (returns vec4) (return (index values 0)))",
    );
    for i in 1..12 {
        source.push_str(&format!("(fn f{i} ((values (array vec4 4))) (returns vec4) (return (f{} values)))", i - 1));
    }
    source.push_str("(fragment fs () (returns vec4 (location 0)) (return (f11 (. data values)))))");
    source
}

/// Both shared-pass boundaries exercise the real backend writers and independent validators.
#[test]
fn helper_expansion_preserves_eager_arguments_and_bounded_fallback() {
    for source in [eager_argument_source(), &deep_helper_source()] {
        for target in targets() {
            validate(&compile(target, GpuShaderStage::Fragment, source, "fs"));
        }
    }
}

/// WGSL uniformity is checked by the native consumer, including the eager derivative argument.
#[cfg(feature = "wgpu")]
#[test]
#[ignore = "requires a native WGPU adapter"]
fn helper_boundaries_pass_native_wgsl_validation() {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, _) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    for source in [eager_argument_source(), &deep_helper_source()] {
        let shader = compile(
            neocogi_platform_contracts::render::shader::ShaderTarget::Wgsl,
            GpuShaderStage::Fragment,
            source,
            "fs",
        );
        let ShaderCode::Wgsl(text) = shader.code else { unreachable!() };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("expanded aggregate helper boundary"),
            source: wgpu::ShaderSource::Wgsl(text.into()),
        });
        assert!(pollster::block_on(scope.pop()).is_none());
    }
}

#[cfg(feature = "vulkan")]
mod spv {
    use super::*;
    use spirv::{Op, StorageClass};
    use std::collections::{BTreeMap, BTreeSet};

    /// Borrowed instruction, decoded from the compiler's owned complete word artifact.
    pub struct Instruction<'a> {
        /// Numeric opcode, excluding word count.
        pub op: u32,
        /// Operands after the instruction header.
        pub args: &'a [u32],
    }
    /// Refuse malformed extents rather than letting a structural check silently skip words.
    pub fn instructions(words: &[u32]) -> Vec<Instruction<'_>> {
        assert_eq!(words[0], 0x07230203);
        let mut rest = &words[5..];
        let mut result = Vec::new();
        while !rest.is_empty() {
            let count = (rest[0] >> 16) as usize;
            assert!(count > 0 && count <= rest.len());
            result.push(Instruction {
                op: rest[0] & 0xffff,
                args: &rest[1..count],
            });
            rest = &rest[count..];
        }
        result
    }
    /// Count only real private aggregate variables, independent of generated names and IDs.
    fn aggregate_variables(words: &[u32]) -> usize {
        let code = instructions(words);
        let aggregate: BTreeSet<_> = code
            .iter()
            .filter(|i| i.op == Op::TypeArray as u32 || i.op == Op::TypeStruct as u32)
            .map(|i| i.args[0])
            .collect();
        let pointers: BTreeMap<_, _> = code.iter().filter(|i| i.op == Op::TypePointer as u32).map(|i| (i.args[0], i.args[2])).collect();
        code.iter()
            .filter(|i| i.op == Op::Variable as u32 && i.args[2] == StorageClass::Function as u32 && aggregate.contains(&pointers[&i.args[0]]))
            .count()
    }
    /// Nested arrays/structures, captured indices and helper branches must read uniform leaves directly.
    #[test]
    fn readonly_paths_never_materialize_aggregate_parents() {
        for case in optimization::cases().into_iter().filter(|case| {
            matches!(
                case.name,
                "uniform_nested_fields"
                    | "uniform_index_snapshot"
                    | "uniform_matrix_columns"
                    | "helper_array_branch"
                    | "helper_array_loop"
                    | "helper_short_circuit"
                    | "helper_loop_condition"
                    | "helper_nested_return"
            )
        }) {
            let shader = compile(
                neocogi_platform_contracts::render::shader::ShaderTarget::SpirV,
                GpuShaderStage::Fragment,
                &case.source,
                "fs",
            );
            validate(&shader);
            let ShaderCode::SpirV(words) = shader.code else { unreachable!() };
            assert_eq!(aggregate_variables(&words), 0, "{} copied an aggregate into private storage", case.name);
            let code = instructions(&words);
            let parents: BTreeSet<_> = code
                .iter()
                .filter(|i| i.op == Op::TypeArray as u32 || i.op == Op::TypeStruct as u32)
                .map(|i| i.args[0])
                .collect();
            assert!(
                code.iter().all(|i| i.op != Op::Load as u32 || !parents.contains(&i.args[0])),
                "{} loaded an entire aggregate",
                case.name
            );
        }
    }
    /// Two genuine snapshots own two arrays; repeated reads and an immutable alias add none.
    #[test]
    fn mutable_snapshots_and_owned_storage_are_not_duplicated_per_read() {
        for case in optimization::cases()
            .into_iter()
            .filter(|case| matches!(case.name, "private_snapshot_alias" | "returned_snapshot"))
        {
            let shader = compile(
                neocogi_platform_contracts::render::shader::ShaderTarget::SpirV,
                GpuShaderStage::Fragment,
                &case.source,
                "fs",
            );
            let ShaderCode::SpirV(words) = shader.code else { unreachable!() };
            assert_eq!(aggregate_variables(&words), 2, "{}", case.name);
        }
    }
    /// Increasing uniform palette capacity must not grow shader work or private storage.
    #[test]
    fn uniform_capacity_does_not_scale_projection_work() {
        let case = optimization::cases().into_iter().find(|case| case.name == "helper_array_branch").unwrap();
        let mut lengths = Vec::new();
        for count in [4, 16, 64] {
            let source = case.source.replace("array mat4 64", &format!("array mat4 {count}"));
            let shader = compile(
                neocogi_platform_contracts::render::shader::ShaderTarget::SpirV,
                GpuShaderStage::Fragment,
                &source,
                "fs",
            );
            validate(&shader);
            let ShaderCode::SpirV(words) = shader.code else { unreachable!() };
            assert_eq!(aggregate_variables(&words), 0);
            lengths.push(words.len());
        }
        assert!(
            lengths.iter().max().unwrap() - lengths.iter().min().unwrap() <= 16,
            "projection cost grew with capacity: {lengths:?}"
        );
    }
}
