//! Shared S-expression admission cases exercised through compiler and renderer COM calls.
use neocogi_platform_contracts::render::abi::GpuShaderStage;

/// A valid selected entry followed by an independently forbidden declaration or entry.
pub struct Case {
    /// Rule identified in assertion output.
    pub name: &'static str,
    /// Whole module; both selected entries must reject it.
    pub source: String,
    /// Stage of the second entry, named `outside` when present.
    pub stage: GpuShaderStage,
}
/// Resource-free entry cannot itself cause rejection.
const SELECTED: &str = "(vertex selected () (returns vec4 (builtin position)) (return (vec4 0.0 0.0 0.0 1.0)))";
/// Whole-module admission includes inactive functions, declarations, and stage interfaces.
pub fn cases() -> Vec<Case> {
    [
        (
            "location sixteen",
            "(vertex outside ((p vec4 (location 16))) (returns vec4 (builtin position)) (return p))",
            GpuShaderStage::Vertex,
        ),
        (
            "signed input",
            "(vertex outside ((p ivec4 (location 0))) (returns vec4 (builtin position)) (return (vec4 p)))",
            GpuShaderStage::Vertex,
        ),
        (
            "signed output",
            "(struct V (p vec4 (builtin position)) (n i32 (location 0) (interpolate flat))) (vertex outside () (returns V) (return (V (vec4 1.0) 1)))",
            GpuShaderStage::Vertex,
        ),
        (
            "wrong stage uniform",
            "(struct U (v vec4)) (uniform u U fragment 0) (vertex outside () (returns vec4 (builtin position)) (return (. u v)))",
            GpuShaderStage::Vertex,
        ),
        (
            "oversized uniform",
            "(struct U (v (array vec4 1025))) (uniform u U vertex 0) (vertex outside () (returns vec4 (builtin position)) (return (index (. u v) 0)))",
            GpuShaderStage::Vertex,
        ),
        (
            "unused oversized declaration",
            "(struct U (v (array vec4 1025))) (uniform u U vertex 0)",
            GpuShaderStage::Vertex,
        ),
        ("non-structure uniform", "(uniform u vec4 vertex 0)", GpuShaderStage::Vertex),
        (
            "image load without pair",
            "(texture t fragment 0) (fragment outside () (returns vec4 (location 0)) (return (load t (ivec2 0))))",
            GpuShaderStage::Fragment,
        ),
        (
            "image query without pair",
            "(texture t fragment 0) (fragment outside () (returns vec4 (location 0)) (return (vec4 (vec2 (dimensions t)) 0.0 1.0)))",
            GpuShaderStage::Fragment,
        ),
        (
            "nonzero load level",
            "(texture t fragment 0) (fragment outside () (returns vec4 (location 0)) (return (+ (sample t (vec2 0.5)) (load t (ivec2 0) 1))))",
            GpuShaderStage::Fragment,
        ),
        (
            "dynamic load level",
            "(texture t fragment 0) (fragment outside ((p vec4 (builtin position))) (returns vec4 (location 0)) (return (load t (ivec2 0) (i32 (. p x)))))",
            GpuShaderStage::Fragment,
        ),
        (
            "mip count",
            "(texture t fragment 0) (fn unused () (returns u32) (return (textureNumLevels t)))",
            GpuShaderStage::Fragment,
        ),
        (
            "nonzero dimensions level",
            "(texture t fragment 0) (fn unused () (returns uvec2) (return (dimensions t 1)))",
            GpuShaderStage::Fragment,
        ),
        (
            "duplicate sampled slot",
            "(texture t fragment 0) (texture other fragment 0)",
            GpuShaderStage::Fragment,
        ),
        (
            "wrong stage image",
            "(texture t vertex 0) (fragment outside () (returns vec4 (location 0)) (return (sample t (vec2 0.5))))",
            GpuShaderStage::Fragment,
        ),
    ]
    .into_iter()
    .map(|(name, body, stage)| Case {
        name,
        source: format!("(module {SELECTED} {body})"),
        stage,
    })
    .collect()
}
/// Query use is valid when that entry samples the same implicitly paired texture.
pub fn valid_mixed_module() -> String {
    format!(
        "(module {SELECTED} (texture t fragment 0) (fragment outside () (returns vec4 (location 0)) (let size (vec2 (dimensions t))) (return (/ (sample t (vec2 0.5)) (. size x)))))"
    )
}
