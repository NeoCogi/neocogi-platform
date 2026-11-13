//! Reproducible native shader qualification, outside product startup and build scripts.
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

/// Exactly the build-selected native platform implementations.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum Backend {
    /// GLSL ES 3.00 through GLES3.
    Glow,
    /// Direct SPIR-V through Vulkan.
    Vulkan,
    /// WGSL through WGPU's checked native translation.
    Wgpu,
}
impl Backend {
    /// Shared Cargo feature and optional runtime descriptor assertion.
    fn name(self) -> &'static str {
        match self {
            Self::Glow => "glow",
            Self::Vulkan => "vulkan",
            Self::Wgpu => "wgpu",
        }
    }
}
/// One explicitly invoked qualification owns all generated reports and snapshots.
pub(crate) struct Options {
    /// Backend whose one normal cdylib is built.
    pub backend: Backend,
    /// Explicit frontend opt-in, forwarded to both the shim and example source selection.
    pub shader_python: bool,
    /// Fresh directory retaining artifacts, raw diagnostics and machine-readable measurements.
    pub output: PathBuf,
    /// Trusted earlier deployment, left untouched apart from ordinary application cache reuse.
    pub reference: Option<PathBuf>,
    /// Fixed number of frames, with a fixed animation pose in the glTF workloads.
    pub frames: u32,
    /// Repetitions used for medians, alternating reference and candidate.
    pub runs: u32,
    /// Configured acceptable candidate/reference elapsed-time ratio.
    pub max_slowdown: f64,
    /// Explicit request for Linux per-process perf counters.
    pub perf: bool,
    /// Explicit request for required Intel machine-shader cost checks.
    pub intel_stats: bool,
}
/// One requested finite render workload, shared by native timing and driver diagnostics.
const APPLICATIONS: [&str; 3] = ["triangle", "gltf_damaged_helmet", "gltf_brainstem"];
/// A stalled shader or driver must not leave an unbounded process tree behind.
const DEADLINE: Duration = Duration::from_secs(120);

/// One actual child-process run; FPS intentionally includes startup and teardown.
#[derive(Serialize)]
struct Sample {
    /// Candidate or reference deployment.
    variant: String,
    /// Real elapsed seconds covering the complete process lifecycle.
    seconds: f64,
    /// Requested frames divided by complete elapsed seconds, never labeled GPU-only FPS.
    fps_including_startup: f64,
}
/// A native Intel compilation variant; diagnostics remain available in the adjacent raw log.
#[derive(Debug, Serialize)]
struct MachineShader {
    /// Vertex/fragment when the installed driver labels its dump, otherwise unknown.
    stage: String,
    /// Compiled SIMD width.
    simd: u32,
    /// Native instruction count, independent of SPIR-V byte count.
    instructions: u32,
    /// Register spill operations reported by the compiler.
    spills: u32,
    /// Register fill operations reported by the compiler.
    fills: u32,
}
/// Complete result for one shipped program, including every captured shader variant.
#[derive(Serialize)]
struct Application {
    /// Stable executable name.
    name: String,
    /// Alternating raw measurements, retained rather than only publishing a favorable sample.
    samples: Vec<Sample>,
    /// Median complete candidate elapsed seconds.
    candidate_seconds: f64,
    /// Median reference elapsed seconds when requested.
    reference_seconds: Option<f64>,
    /// Every native shader compiled for the candidate application, when requested.
    shaders: Vec<MachineShader>,
}
/// Durable, versioned qualification report; failures still retain measured evidence.
#[derive(Serialize)]
struct Report {
    /// Schema discriminator for external regression tooling.
    version: u32,
    /// Build-selected backend under test.
    backend: String,
    /// Whether the optional Python parser and its example/corpus sources were included.
    shader_python: bool,
    /// Actual Rust compiler used with the repository's rebuilt-std min-size profile.
    rustc: String,
    /// Rendered frames for each measured process, after an unmeasured warmup.
    frames: u32,
    /// Number of repeated samples per variant.
    runs: u32,
    /// Complete platform cdylib bytes, including its built-in compiler.
    library_bytes: u64,
    /// Concrete measurements and shader compilation costs for every application.
    applications: Vec<Application>,
}

/// Read only actual Cargo compiler-artifact executable receipts; no hash/profile guesses.
fn executables(bytes: &[u8]) -> Result<std::collections::BTreeMap<String, PathBuf>> {
    let mut result = std::collections::BTreeMap::new();
    for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let message: serde_json::Value = serde_json::from_slice(line)?;
        if message["reason"] == "compiler-artifact"
            && let Some(path) = message["executable"].as_str()
        {
            let name = message["target"]["name"].as_str().context("Cargo executable has no name")?;
            result.insert(name.to_owned(), PathBuf::from(path));
        }
    }
    Ok(result)
}
/// Copy regular owned staging inputs; ordinary resource staging already established their layout.
fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source).with_context(|| format!("missing assets at {}; run cargo tools fetch-gltf-examples first", source.display()))? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let to = target.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else {
            ensure!(kind.is_file(), "staging input is not regular: {}", entry.path().display());
            fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}
/// Build/validate before timing, using the same normal library assembly as end users.
fn prepare(options: &Options, output: &Path) -> Result<(PathBuf, u64)> {
    use crate::process::{cargo, capture, run};
    let corpus = output.join("corpus");
    let backend_features = format!("{}{}", options.backend.name(), if options.shader_python { ",shader-python" } else { "" });
    let application_features = if options.shader_python { "dynamic,shader-python" } else { "dynamic" };
    run(cargo(None)
        .args(["test", "--locked", "-p", "neocogi-platform-examples", "--no-default-features", "--features"])
        .arg(format!(
            "static,{},test-support,provider/shader-wgsl,provider/shader-glsl,provider/shader-spirv",
            backend_features
        ))
        .args(["--test", "shader_corpus"])
        .env("NEOCOGI_SHADER_ARTIFACT_DIR", &corpus)
        .env("NEOCOGI_SHADER_VALIDATORS", "1"))?;
    let library = crate::packages::platform_artifact(&capture(
        cargo(None)
            .args([
                "build",
                "--locked",
                "--profile",
                "min-size",
                "-p",
                "neocogi-platform-dylib",
                "--no-default-features",
                "--features",
            ])
            .arg(&backend_features)
            .args(["--lib", "--message-format=json"])
            .env_remove("NEOCOGI_NATIVE_OUTPUT_DIR"),
    )?)?;
    let bytes = fs::metadata(&library)?.len();
    if matches!(options.backend, Backend::Vulkan) {
        ensure!(bytes < 524288, "complete Vulkan platform exceeds its byte cap: {bytes}");
    }
    let built = executables(&capture(
        cargo(None)
            .args([
                "build",
                "--locked",
                "--profile",
                "min-size",
                "-p",
                "neocogi-platform-examples",
                "--no-default-features",
                "--features",
            ])
            .arg(application_features)
            .args(["--bins", "--message-format=json"])
            .env_remove("NEOCOGI_NATIVE_OUTPUT_DIR"),
    )?)?;
    let candidate = output.join("candidate");
    fs::create_dir_all(candidate.join("lib"))?;
    fs::copy(&library, candidate.join("lib").join(library.file_name().context("library filename")?))?;
    for name in APPLICATIONS {
        let binary = built.get(name).with_context(|| format!("Cargo did not build {name}"))?;
        fs::copy(binary, candidate.join(binary.file_name().context("executable filename")?))?;
    }
    let binary = built.get("gltf_brainstem").context("missing BrainStem")?;
    copy_tree(&binary.parent().context("binary directory")?.join("assets"), &candidate.join("assets"))?;
    // A checked source-keyed cache avoids rebaking after each clean output tree;
    // the viewer still validates it normally before reuse.
    let cache = crate::root().join("examples/native/gltf/assets/Cannon_Exterior.ibl");
    if cache.is_file() {
        fs::copy(cache, candidate.join("assets/gltf/Cannon_Exterior.ibl"))?;
    }
    Ok((candidate, bytes))
}
/// Correct application arguments, with runtime backend verification on both compared deployments.
fn arguments(command: &mut Command, name: &str, backend: Backend, frames: u32) {
    command.args(["--backend", backend.name()]);
    if name == "triangle" {
        command.arg(frames.to_string());
    } else {
        command.args(["--frames", &frames.to_string(), "--time", "1"]);
    }
}
/// Return the exact executable in a trusted prepared deployment, without searching PATH.
fn executable(root: &Path, name: &str) -> PathBuf {
    root.join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
}
/// Capture a finite process tree and require proof that every requested frame actually rendered.
fn execute(options: &Options, root: &Path, name: &str, frames: u32, log: &Path, perf: bool, diagnostic: bool) -> Result<f64> {
    let mut command = if perf {
        ensure!(cfg!(target_os = "linux"), "--perf requires Linux perf");
        let mut perf = Command::new("perf");
        perf.args(["stat", "-e", "task-clock,context-switches", "-o"])
            .arg(log.with_extension("perf.txt"))
            .arg("--")
            .arg(executable(root, name));
        perf
    } else {
        Command::new(executable(root, name))
    };
    arguments(&mut command, name, options.backend, frames);
    command.current_dir(root).env_remove("INTEL_DEBUG").env_remove("MESA_SHADER_CACHE_DISABLE");
    if diagnostic {
        command.env("INTEL_DEBUG", "vs,fs").env("MESA_SHADER_CACHE_DISABLE", "true");
    }
    let start = Instant::now();
    let result = crate::process::timed_output(&mut command, DEADLINE)?;
    let elapsed = start.elapsed().as_secs_f64();
    fs::write(log, [result.stdout.as_slice(), result.stderr.as_slice()].concat())?;
    ensure!(result.status.success(), "{name} failed; see {}", log.display());
    ensure!(
        String::from_utf8_lossy(&result.stdout).contains(&format!("Rendered {frames} ")),
        "{name} stopped before all requested frames; see {}",
        log.display()
    );
    Ok(elapsed)
}
/// Installed Intel drivers report every SIMD compilation variant on one summary line.
fn machine_shaders(text: &str) -> Vec<MachineShader> {
    let mut result = Vec::new();
    let mut stage = "unknown";
    for line in text.lines() {
        if line.starts_with("NIR (") && line.contains("vertex shader") {
            stage = "vertex";
        }
        if line.starts_with("NIR (") && line.contains("fragment shader") {
            stage = "fragment";
        }
        let Some((simd, rest)) = line.strip_prefix("SIMD").and_then(|value| value.split_once(" shader: ")) else {
            continue;
        };
        let Some((instructions, _)) = rest.split_once(" instructions.") else {
            continue;
        };
        let Some(spills) = rest.split_once(" spills:fills").and_then(|(before, _)| before.split_whitespace().last()) else {
            continue;
        };
        let Some((spills, fills)) = spills.split_once(':') else { continue };
        if let (Ok(simd), Ok(instructions), Ok(spills), Ok(fills)) = (simd.parse(), instructions.parse(), spills.parse(), fills.parse()) {
            result.push(MachineShader {
                stage: stage.into(),
                simd,
                instructions,
                spills,
                fills,
            });
        }
    }
    result
}
/// Median avoids choosing only the best run and is stable for even and odd counts.
fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    }
}
/// Perform complete qualification and retain a report before returning any cost-regression failure.
pub(crate) fn run(options: Options) -> Result<()> {
    ensure!(
        options.frames >= 30 && options.runs > 0 && options.runs <= 20,
        "use at least 30 frames and 1..20 runs"
    );
    ensure!(
        options.max_slowdown.is_finite() && options.max_slowdown > 0.0,
        "max-slowdown must be finite and positive"
    );
    let output = if options.output.is_absolute() {
        options.output.clone()
    } else {
        std::env::current_dir()?.join(&options.output)
    };
    ensure!(
        !output.exists() || fs::read_dir(&output)?.next().is_none(),
        "report output must be new or empty: {}",
        output.display()
    );
    let metadata: serde_json::Value = serde_json::from_slice(&crate::process::capture(crate::process::cargo(None).args([
        "metadata",
        "--no-deps",
        "--format-version=1",
    ]))?)?;
    let target = Path::new(metadata["target_directory"].as_str().context("Cargo target directory")?);
    fs::create_dir_all(&output)?;
    let output = output.canonicalize()?;
    ensure!(!output.starts_with(target.canonicalize()?), "keep qualification reports outside Cargo targets");
    let reference = options.reference.as_ref().map(|path| path.canonicalize()).transpose()?;
    let (candidate, bytes) = prepare(&options, &output)?;
    let rustc = String::from_utf8(crate::process::capture(Command::new("rustc").arg("--version"))?)?
        .trim()
        .to_owned();
    let mut report = Report {
        version: 1,
        backend: options.backend.name().into(),
        shader_python: options.shader_python,
        rustc,
        frames: options.frames,
        runs: options.runs,
        library_bytes: bytes,
        applications: Vec::new(),
    };
    let mut failures = Vec::new();
    for name in APPLICATIONS {
        let variants: Vec<_> = reference
            .iter()
            .map(|root| ("reference", root.as_path()))
            .chain(std::iter::once(("candidate", candidate.as_path())))
            .collect();
        for (variant, root) in &variants {
            execute(&options, root, name, 3, &output.join(format!("{name}-{variant}-warmup.log")), false, false)?;
        }
        let mut samples = Vec::new();
        for run in 0..options.runs {
            for (variant, root) in &variants {
                let seconds = execute(
                    &options,
                    root,
                    name,
                    options.frames,
                    &output.join(format!("{name}-{variant}-{run}.log")),
                    options.perf,
                    false,
                )?;
                samples.push(Sample {
                    variant: (*variant).into(),
                    seconds,
                    fps_including_startup: f64::from(options.frames) / seconds,
                });
            }
        }
        let current = median(
            samples
                .iter()
                .filter(|sample| sample.variant == "candidate")
                .map(|sample| sample.seconds)
                .collect(),
        );
        let previous = reference.as_ref().map(|_| {
            median(
                samples
                    .iter()
                    .filter(|sample| sample.variant == "reference")
                    .map(|sample| sample.seconds)
                    .collect(),
            )
        });
        if previous.is_some_and(|previous| current > previous * options.max_slowdown) {
            failures.push(format!("{name}: candidate {current:.3}s exceeds reference tolerance"));
        }
        let mut shaders = Vec::new();
        if options.intel_stats {
            let log = output.join(format!("{name}-intel.log"));
            execute(&options, &candidate, name, 1, &log, false, true)?;
            shaders = machine_shaders(&fs::read_to_string(log)?);
            if shaders.is_empty() {
                failures.push(format!("{name}: selected driver supplied no Intel shader diagnostics"));
            }
            for shader in &shaders {
                if shader.spills > 0 || shader.fills > 0 || shader.instructions > 4096 {
                    failures.push(format!("{name}: excessive native shader cost: {shader:?}"));
                }
            }
        }
        println!(
            "{name}: {:.1} FPS including startup{}",
            f64::from(options.frames) / current,
            previous.map_or(String::new(), |previous| format!(", reference {:.1}", f64::from(options.frames) / previous))
        );
        report.applications.push(Application {
            name: name.into(),
            samples,
            candidate_seconds: current,
            reference_seconds: previous,
            shaders,
        });
    }
    fs::write(output.join("report.json"), serde_json::to_vec_pretty(&report)?)?;
    ensure!(
        failures.is_empty(),
        "shader qualification failed: {} (see {})",
        failures.join("; "),
        output.join("report.json").display()
    );
    println!(
        "PASS: all production shaders, three native applications and requested cost checks; {}",
        output.join("report.json").display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Missing/partial diagnostics cannot accidentally become zero-cost successful shaders.
    #[test]
    fn intel_costs_keep_spills_and_every_simd_variant() {
        let text = "NIR (SSA form) for vertex shader:\nSIMD8 shader: 20215 instructions. 0 loops. 128837 cycles. 4181:298 spills:fills, 36 sends\nNIR (final form) for fragment shader:\nSIMD16 shader: 1234 instructions. 0 loops. 2 cycles. 0:0 spills:fills, 5 sends\nSIMD32 shader: partial diagnostics\n";
        let shaders = machine_shaders(text);
        assert_eq!(shaders.len(), 2);
        assert_eq!(shaders[0].spills, 4181);
        assert_eq!(shaders[0].fills, 298);
        assert_eq!(shaders[1].stage, "fragment");
        assert!(machine_shaders("Rendered 3 frames").is_empty());
    }
    /// Both sample-count parities use the central measured values rather than a favorable extreme.
    #[test]
    fn median_handles_measured_order_and_outliers() {
        assert_eq!(median(vec![9.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(vec![9.0, 1.0, 2.0, 3.0]), 2.5);
    }
}
