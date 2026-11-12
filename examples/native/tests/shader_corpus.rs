//! Qualify the exact production shader inventory and the general aggregate workloads.
use neocogi_platform_examples::{sdk, shaders};
use sdk::render::{
    abi::GpuShaderStage,
    compiler::{Compiler, CompileError},
    shader::{ShaderCode, ShaderTarget, MAX_SHADER_ARTIFACT_BYTES},
};
use std::{collections::BTreeMap, path::Path, process::Command};

/// A complete backend artifact must survive its independent validator when installed or required.
fn validate(code: &ShaderCode, stage: GpuShaderStage) {
    let directory = tempfile::tempdir().unwrap();
    let (tool, args, bytes) = match code {
        ShaderCode::Wgsl(_) => return,
        ShaderCode::GlslEs300(text) => (
            "glslangValidator",
            vec!["-S", if stage == GpuShaderStage::Vertex { "vert" } else { "frag" }],
            text.as_bytes().to_vec(),
        ),
        ShaderCode::SpirV(words) => (
            "spirv-val",
            vec!["--target-env", "vulkan1.0"],
            words.iter().flat_map(|word| word.to_le_bytes()).collect(),
        ),
    };
    let file = directory.path().join("shader");
    std::fs::write(&file, bytes).unwrap();
    match Command::new(tool).args(args).arg(file).output() {
        Ok(output) => assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && std::env::var_os("NEOCOGI_SHADER_VALIDATORS").is_none() => {}
        Err(error) => panic!("{tool}: {error}"),
    }
}

/// SPIR-V 1.0 declarations are inspected without relying on generated symbol names.
/// These workloads require only small owned arrays; a palette-sized Function
/// variable reveals a materialization regression even when a GPU is unavailable.
fn largest_private_array(words: &[u32]) -> u32 {
    const TYPE_ARRAY: u32 = 28;
    const TYPE_POINTER: u32 = 32;
    const CONSTANT: u32 = 43;
    const VARIABLE: u32 = 59;
    const FUNCTION: u32 = 7;
    let mut arrays = BTreeMap::new();
    let mut pointers = BTreeMap::new();
    let mut constants = BTreeMap::new();
    let mut variables = Vec::new();
    let mut remaining = &words[5..];
    while !remaining.is_empty() {
        let count = (remaining[0] >> 16) as usize;
        assert!(count > 0 && count <= remaining.len());
        let args = &remaining[1..count];
        match remaining[0] & 0xffff {
            TYPE_ARRAY => {
                arrays.insert(args[0], args[2]);
            }
            TYPE_POINTER => {
                pointers.insert(args[0], args[2]);
            }
            CONSTANT if args.len() == 3 => {
                constants.insert(args[1], args[2]);
            }
            VARIABLE if args[2] == FUNCTION => variables.push(args[0]),
            _ => {}
        }
        remaining = &remaining[count..];
    }
    variables
        .iter()
        .filter_map(|pointer| arrays.get(&pointers[pointer]))
        .map(|count| constants[count])
        .max()
        .unwrap_or(0)
}

/// Exporting artifacts is opt-in and owned by the qualification command, outside Cargo targets.
fn export(directory: &Path, name: &str, stage: &str, target: &str, code: &ShaderCode) {
    let bytes = match code {
        ShaderCode::Wgsl(text) | ShaderCode::GlslEs300(text) => text.as_bytes().to_vec(),
        ShaderCode::SpirV(words) => words.iter().flat_map(|word| word.to_le_bytes()).collect(),
    };
    std::fs::write(directory.join(format!("{name}.{stage}.{target}")), bytes).unwrap();
}

/// Every shipped stage plus every aggregate workload uses its production source constructor.
#[test]
fn all_shader_sources_validate_and_keep_bounded_private_storage() {
    let mut sources = vec![
        ("triangle", shaders::TRIANGLE.to_owned()),
        ("mesh", shaders::mesh()),
        ("skybox", shaders::skybox()),
    ];
    sources.extend(sdk::gpu::testing::optimization::cases().into_iter().map(|case| (case.name, case.source)));
    let output = std::env::var_os("NEOCOGI_SHADER_ARTIFACT_DIR").map(std::path::PathBuf::from);
    if let Some(directory) = &output {
        assert!(directory.is_absolute());
        std::fs::create_dir_all(directory).unwrap();
    }
    let mut report = Vec::new();
    let mut enabled = 0;
    for (target, label) in [(ShaderTarget::Wgsl, "wgsl"), (ShaderTarget::GlslEs300, "glsl"), (ShaderTarget::SpirV, "spv")] {
        let compiler = match Compiler::new(target) {
            Ok(compiler) => compiler,
            Err(CompileError::Unavailable) => continue,
            Err(error) => panic!("{error}"),
        };
        enabled += 1;
        for (name, source) in &sources {
            for (stage, entry, suffix) in [(GpuShaderStage::Vertex, "vs", "vert"), (GpuShaderStage::Fragment, "fs", "frag")] {
                let shader = compiler
                    .compile(stage, source, entry, MAX_SHADER_ARTIFACT_BYTES)
                    .unwrap_or_else(|error| panic!("{name}.{suffix}.{label}: {error}"));
                validate(&shader.code, stage);
                let private = if let ShaderCode::SpirV(words) = &shader.code {
                    largest_private_array(words)
                } else {
                    0
                };
                assert!(private <= 16, "{name}.{suffix} copied a {private}-element aggregate to private storage");
                if let Some(directory) = &output {
                    export(directory, name, suffix, label, &shader.code);
                }
                report.push(serde_json::json!({"name":name,"stage":suffix,"target":label,"bytes":shader.byte_len(),"largest_private_array":private}));
            }
        }
    }
    assert!(enabled > 0, "selected native backend must provide its built-in compiler output");
    if let Some(directory) = output {
        std::fs::write(directory.join("corpus.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
