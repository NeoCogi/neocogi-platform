//! Structural cost regressions complement native pixels; no GPU-specific instruction count is assumed.
use neocogi_com_sdk::{ComPtr, ComStringExt, NcCopyFromBufferOffset, NC_E_INVALID_ARG, NC_OK};
use neocogi_platform::gpu::testing::optimization;
use neocogi_platform::render::compiler::{CompileError, Compiler, Factory};
use neocogi_platform_contracts::render::{
    abi::{GpuShaderStage, IShaderCompilationAbi, ShaderCompileRequest, ShaderCompilerFactoryAbi},
    shader::{PreparedShader, Shader, ShaderCode, ShaderTarget, MAX_SHADER_ARTIFACT_BYTES},
};
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

/// WGSL emits one helper per selected shift shape and charges that helper to the artifact quota.
#[test]
fn wgsl_shift_helpers_follow_the_entry_and_quota() {
    if !targets().contains(&ShaderTarget::Wgsl) {
        return;
    }
    let source = r#"(module
      (fn nc_shift_left_u32_1 ((count u32)) (returns u32) (return (<< 1u count)))
      (vertex vs () (returns vec4 (builtin position)) (return (vec4 (f32 (>> -8 1u)))))
      (fragment fs () (returns vec4 (location 0)) (return (vec4 (f32 (+ (nc_shift_left_u32_1 32u) (<< 1u 33u)))))))"#;
    let stage = GpuShaderStage::Fragment;
    let full = compile(ShaderTarget::Wgsl, stage, source, "fs");
    let text = full.code.wgsl().unwrap();
    assert_eq!(text.lines().filter(|line| line.starts_with("fn nc_shift_")).count(), 1, "{text}");
    let compiler = Compiler::new(ShaderTarget::Wgsl).unwrap();
    assert!(compiler.compile(stage, source, "fs", full.byte_len()).is_ok());
    assert!(matches!(compiler.compile(stage, source, "fs", full.byte_len() - 1), Err(CompileError::Limit)));
}

/// Every emitted remainder lowering is charged to its selected artifact's byte limit.
#[test]
fn signed_remainder_charges_the_selected_artifact_quota() {
    for case in optimization::cases().into_iter().filter(|case| case.name.ends_with("signed_remainder")) {
        for target in targets() {
            let stage = GpuShaderStage::Fragment;
            let shader = compile(target, stage, &case.source, "fs");
            let compiler = Compiler::new(target).unwrap();
            assert!(compiler.compile(stage, &case.source, "fs", shader.byte_len()).is_ok());
            assert!(matches!(
                compiler.compile(stage, &case.source, "fs", shader.byte_len() - 1),
                Err(CompileError::Limit)
            ));
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

/// RHS evaluation categories with distinct emitted operations.
#[derive(Clone, Copy, Debug)]
enum StoreValue {
    /// Direct screen-space derivative in the assigned value.
    Derivative,
    /// Ordinary call to a helper with a transitively derivative-dependent result.
    Helper,
    /// Implicit-derivative sample from a reflected texture and sampler pair.
    Sample,
}

/// One source store and the expected shape of its native write path.
#[derive(Clone, Copy, Debug)]
enum StoreTarget {
    /// A dynamically indexed slot alternates between zero and the upper bound.
    UpperBound,
    /// A dynamically indexed slot alternates between minus one and zero.
    Negative,
    /// A statically valid path writes directly.
    Direct,
    /// Helper expansion leaves a proven in-bounds or invalid constant.
    Folded(i32),
    /// The first index completes before expansion of the second index.
    Nested,
    /// The same later expansion runs even if the first index is always invalid.
    InvalidAncestor,
}

impl StoreTarget {
    /// A proven invalid projection skips the leaf access entirely.
    fn writes(self) -> bool {
        match self {
            Self::InvalidAncestor => false,
            Self::Folded(index) => index == 0,
            _ => true,
        }
    }

    /// A runtime branch is needed when the complete path is dynamically bounded.
    fn guarded(self) -> bool {
        matches!(self, Self::UpperBound | Self::Negative | Self::Nested)
    }
}

/// Generates identical source for the independent validators, COM ordering and admission controls.
fn store_source(value: StoreValue, target: StoreTarget, conditional: bool) -> String {
    let (value_declarations, rhs) = match value {
        StoreValue::Derivative => ("", "(vec4 (dpdx (. p x)))"),
        StoreValue::Helper => (
            "(fn slope ((x f32)) (returns f32) (return (dpdx x)))
             (fn shade ((x f32)) (returns vec4) (return (vec4 (slope x))))",
            "(shade (. p x))",
        ),
        StoreValue::Sample => ("(texture tex fragment 0)", "(sample tex (. p xy))"),
    };
    let nested = matches!(target, StoreTarget::Nested | StoreTarget::InvalidAncestor);
    let path_declarations = if let StoreTarget::Folded(index) = target {
        format!("(fn pick ((unused (array vec4 1)) (x f32)) (returns i32) (let width (fwidth x)) (return {index}))")
    } else if nested {
        "(struct Row (values (array vec4 1)))
         (fn later ((unused (array vec4 1)) (x f32)) (returns i32)
           (let width (fwidth x))
           (return (i32 width)))"
            .into()
    } else {
        String::new()
    };
    let mut locals = "(var slots (array vec4 1))";
    let path = match target {
        StoreTarget::UpperBound => "(index slots (& (i32 (. p x)) 1))".into(),
        StoreTarget::Negative => "(index slots (- (& (i32 (. p x)) 1) 1))".into(),
        StoreTarget::Direct => "(index slots 0)".into(),
        StoreTarget::Folded(_) => "(index slots (pick slots (. p x)))".into(),
        StoreTarget::Nested | StoreTarget::InvalidAncestor => {
            locals = "(var slots (array Row 1)) (var arg (array vec4 1))";
            let first = if matches!(target, StoreTarget::Nested) {
                "(i32 (dpdy (. p y)))"
            } else {
                "-1"
            };
            format!("(index (. (index slots {first}) values) (later arg (. p x)))")
        }
    };
    let mut statement = format!("(set {path} {rhs})");
    if conditional {
        statement = format!("(if (> (. p x) 0.0) (then {statement}))");
    }
    format!(
        "(module {value_declarations} {path_declarations}
      (fragment fs ((p vec4 (builtin position))) (returns vec4 (location 0))
        {locals} {statement} (return (vec4 0.0))))"
    )
}

/// Includes the concrete fast path, two dynamic failure boundaries and a statically failed ancestor.
const STORE_CASES: [(StoreValue, StoreTarget); 8] = [
    (StoreValue::Derivative, StoreTarget::UpperBound),
    (StoreValue::Derivative, StoreTarget::Direct),
    (StoreValue::Derivative, StoreTarget::Folded(0)),
    (StoreValue::Derivative, StoreTarget::Folded(-1)),
    (StoreValue::Derivative, StoreTarget::Nested),
    (StoreValue::Derivative, StoreTarget::InvalidAncestor),
    (StoreValue::Helper, StoreTarget::Negative),
    (StoreValue::Sample, StoreTarget::UpperBound),
];

/// Exercises the generated application boundary and preserves the COM result on source failure.
fn compile_store(target: ShaderTarget, source: &str) -> ComPtr<IShaderCompilationAbi> {
    let factory: ComPtr<ShaderCompilerFactoryAbi> = Factory::from_value(Factory).query().unwrap();
    factory
        .create(target)
        .unwrap()
        .compile_result(
            ShaderCompileRequest {
                stage: GpuShaderStage::Fragment,
                max_output_bytes: MAX_SHADER_ARTIFACT_BYTES as u64,
            },
            NcCopyFromBufferOffset::new(0, source.as_bytes()),
            NcCopyFromBufferOffset::new(0, b"fs"),
        )
        .unwrap()
}

/// Copies one successful fragment artifact through the generated COM result getters.
fn store_artifact(target: ShaderTarget, source: &str) -> Shader {
    let report = compile_store(target, source);
    assert_eq!(report.status().unwrap(), NC_OK.code(), "{}", report.diagnostic().unwrap().to_string().unwrap());
    PreparedShader::new(report, GpuShaderStage::Fragment, target, b"fs", MAX_SHADER_ARTIFACT_BYTES)
        .unwrap()
        .copy()
        .unwrap()
}

/// The operation consuming the sampled texture or derivative must precede the index guard.
/// For nested paths, the expanded helper prefix must follow the first index's conversion.
fn assert_text_store_order(text: &str, gl: bool, value: StoreValue, target: StoreTarget) {
    let marker = match (value, gl) {
        (StoreValue::Derivative, true) => "dFdx(",
        (StoreValue::Derivative, false) => "dpdx(",
        (StoreValue::Helper, _) => "nc_f1(",
        (StoreValue::Sample, true) => "texture(",
        (StoreValue::Sample, false) => "textureSample(",
    };
    let rhs = text.rfind(marker).unwrap_or_else(|| panic!("{text}"));
    assert_eq!(text.matches(marker).count(), 1 + usize::from(matches!(value, StoreValue::Helper)), "{text}");
    let guard = text.find("if (");
    assert_eq!(guard.is_some(), target.guarded(), "{text}");
    // GLSL array declarations also contain `] = `. A native store has a parenthesized root.
    let leaf_write = text.lines().find(|line| line.contains("(v1)[") && line.contains("] = "));
    assert_eq!(leaf_write.is_some(), target.writes(), "{text}");
    if let Some(guard) = guard {
        assert!(rhs < guard, "{text}");
        assert!(text[guard..].lines().next().unwrap().contains(leaf_write.unwrap().trim()), "{text}");
    }
    if matches!(target, StoreTarget::Nested | StoreTarget::InvalidAncestor) {
        let prefix = text.find("fwidth(").unwrap();
        let conversion = if gl { "int(" } else { "i32(" };
        assert_eq!(text.matches("fwidth(").count(), 1, "{text}");
        assert!(rhs < prefix && text[prefix..].contains(conversion), "{text}");
        if let Some(guard) = guard {
            let first_index = text.find(conversion).unwrap();
            assert!(rhs < first_index && first_index < prefix && prefix < guard, "{text}");
        }
    }
    if matches!(target, StoreTarget::Folded(_)) {
        let prefix = text.find("fwidth(").unwrap();
        assert_eq!(text.matches("fwidth(").count(), 1, "{text}");
        assert!(rhs < prefix, "{text}");
        if let Some(write) = leaf_write {
            assert!(prefix < text.find(write).unwrap(), "{text}");
        }
    }
}

/// Validate complete artifacts and assert which instructions run before the generated store branch.
#[test]
fn indexed_stores_prepare_all_operands_in_source_order() {
    for target in targets() {
        for (value, path) in STORE_CASES {
            let shader = store_artifact(target, &store_source(value, path, false));
            validate(&shader);
            match shader.code {
                ShaderCode::Wgsl(text) => assert_text_store_order(&text, false, value, path),
                ShaderCode::GlslEs300(text) => assert_text_store_order(&text, true, value, path),
                #[cfg(feature = "vulkan")]
                ShaderCode::SpirV(words) => spv::assert_store_order(&words, value, path),
                #[cfg(not(feature = "vulkan"))]
                ShaderCode::SpirV(_) => unreachable!(),
            }
            let rejected = compile_store(target, &store_source(value, path, true));
            assert_eq!(rejected.status().unwrap(), NC_E_INVALID_ARG.code());
            assert!(rejected.diagnostic().unwrap().to_string().unwrap().contains("unconditional control flow"));
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

/// Create every module in a real native WGPU device's validation scope.
#[cfg(feature = "wgpu")]
fn validate_native_wgsl(shaders: impl IntoIterator<Item = Shader>) {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, _) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    for shader in shaders {
        let ShaderCode::Wgsl(text) = shader.code else { unreachable!() };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader optimization validation"),
            source: wgpu::ShaderSource::Wgsl(text.into()),
        });
        let error = pollster::block_on(scope.pop());
        assert!(error.is_none(), "{error:?}");
    }
}

/// WGSL uniformity is checked by the native consumer, including the eager derivative argument.
#[cfg(feature = "wgpu")]
#[test]
#[ignore = "requires a native WGPU adapter"]
fn helper_boundaries_pass_native_wgsl_validation() {
    validate_native_wgsl(
        [eager_argument_source(), &deep_helper_source()]
            .into_iter()
            .map(|source| compile(ShaderTarget::Wgsl, GpuShaderStage::Fragment, source, "fs")),
    );
}

/// Text ordering is independently validated by actual WGPU module creation for every store shape.
#[cfg(feature = "wgpu")]
#[test]
#[ignore = "requires a native WGPU adapter"]
fn indexed_stores_pass_native_wgsl_validation() {
    validate_native_wgsl(
        STORE_CASES
            .into_iter()
            .map(|(value, path)| store_artifact(ShaderTarget::Wgsl, &store_source(value, path, false))),
    );
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

    /// Require every eager op before the outer selection and the native pointer inside its true block.
    pub(super) fn assert_store_order(words: &[u32], value: StoreValue, target: StoreTarget) {
        let code = instructions(words);
        let entry = code.iter().find(|instruction| instruction.op == Op::EntryPoint as u32).unwrap().args[1];
        let function = |id| {
            let start = code
                .iter()
                .position(|instruction| instruction.op == Op::Function as u32 && instruction.args[1] == id)
                .unwrap();
            let len = code[start..].iter().position(|instruction| instruction.op == Op::FunctionEnd as u32).unwrap();
            &code[start..start + len]
        };
        // The native entry wrapper calls the selected source function. Check there so a call
        // inside a transitively derivative-dependent helper cannot satisfy the RHS assertion.
        let source = function(entry)
            .iter()
            .find(|instruction| instruction.op == Op::FunctionCall as u32)
            .unwrap()
            .args[2];
        let code = function(source);
        let position = |op: Op| code.iter().position(|instruction| instruction.op == op as u32);
        let rhs_op = match value {
            StoreValue::Derivative => Op::DPdx,
            StoreValue::Helper => Op::FunctionCall,
            StoreValue::Sample => Op::ImageSampleImplicitLod,
        };
        assert_eq!(code.iter().filter(|instruction| instruction.op == rhs_op as u32).count(), 1);
        let mut rhs = position(rhs_op).unwrap();
        if matches!(value, StoreValue::Derivative) {
            assert!(rhs < position(Op::CompositeConstruct).unwrap());
            rhs = position(Op::CompositeConstruct).unwrap();
        }
        let guard = position(Op::BranchConditional);
        let pointer = position(Op::AccessChain);
        assert_eq!(guard.is_some(), target.guarded());
        if !target.writes() {
            assert!(pointer.is_none() && guard.is_none());
        } else {
            let pointer = pointer.unwrap();
            assert!(rhs < pointer);
            assert_eq!(code[pointer + 1].op, Op::Store as u32);
            assert_eq!(code[pointer + 1].args[0], code[pointer].args[1]);
            assert_eq!(code[pointer + 1].args[1], code[rhs].args[1]);
            if let Some(guard) = guard {
                assert!(rhs < guard && guard < pointer);
                assert_eq!(code[guard + 1].op, Op::Label as u32);
                assert_eq!(code[guard + 1].args[0], code[guard].args[1]);
                let merge = code[guard].args[2];
                let end = code
                    .iter()
                    .position(|instruction| instruction.op == Op::Label as u32 && instruction.args[0] == merge)
                    .unwrap();
                assert!(pointer + 1 < end);
            }
        }
        if matches!(target, StoreTarget::Nested | StoreTarget::InvalidAncestor) {
            let prefix = position(Op::Fwidth).unwrap();
            let convert = |instruction: &Instruction<'_>| instruction.op == Op::ConvertFToS as u32;
            let last_index = code.iter().rposition(convert).unwrap();
            assert!(rhs < prefix && prefix < last_index);
            if let Some(guard) = guard {
                let first_derivative = position(Op::DPdy).unwrap();
                let first_index = code.iter().position(convert).unwrap();
                assert!(rhs < first_derivative && first_derivative < first_index && first_index < prefix && last_index < guard);
            }
        }
        if matches!(target, StoreTarget::Folded(_)) {
            let prefix = position(Op::Fwidth).unwrap();
            assert!(rhs < prefix);
            if let Some(pointer) = pointer {
                assert!(prefix < pointer);
            }
        }
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
