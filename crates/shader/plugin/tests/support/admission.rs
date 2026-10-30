//! Shared WGSL inputs reach independent compiler COM and GPU resource-factory callers.
use neocogi_render_core::abi::GpuShaderStage;

/// Naga-valid module containing one ordinary selected vertex and an invalid portable entry/declaration.
pub(super) struct Case {
    /// Identifies the independently exercised rule in assertion output.
    pub name: &'static str,
    /// Complete original module; both selection attempts must reject it.
    pub source: String,
    /// Stage of the second entry, named `outside`.
    pub stage: GpuShaderStage,
}

/// Ordinary entry has no resources or locations and cannot itself cause the rejection.
const SELECTED: &str = "@vertex fn selected()->@builtin(position) vec4<f32>{return vec4<f32>(0.,0.,0.,1.);}";

/// Isolated inactive-entry and whole-declaration profile violations.
pub(super) fn cases() -> Vec<Case> {
    [
        ("location sixteen", "@vertex fn outside(@location(16) p:vec4<f32>)->@builtin(position) vec4<f32>{return p;}", GpuShaderStage::Vertex),
        ("signed input", "@vertex fn outside(@location(0) p:vec4<i32>)->@builtin(position) vec4<f32>{return vec4<f32>(p);}", GpuShaderStage::Vertex),
        ("signed output", "struct V{@builtin(position) p:vec4<f32>,@location(0) @interpolate(flat) n:i32} @vertex fn outside()->V{return V(vec4(0.,0.,0.,1.),1);}", GpuShaderStage::Vertex),
        ("wrong stage uniform", "struct U{v:vec4<f32>} @group(3) @binding(0) var<uniform> u:U; @vertex fn outside()->@builtin(position) vec4<f32>{return u.v;}", GpuShaderStage::Vertex),
        ("oversized uniform", "struct U{v:array<vec4<f32>,1025>} @group(1) @binding(0) var<uniform> u:U; @vertex fn outside()->@builtin(position) vec4<f32>{return u.v[0];}", GpuShaderStage::Vertex),
        ("unused oversized declaration", "struct U{v:array<vec4<f32>,1025>} @group(1) @binding(0) var<uniform> u:U; @vertex fn outside()->@builtin(position) vec4<f32>{return vec4(1.);}", GpuShaderStage::Vertex),
        ("non-structure uniform", "@group(1) @binding(0) var<uniform> u:vec4<f32>; @vertex fn outside()->@builtin(position) vec4<f32>{return u;}", GpuShaderStage::Vertex),
        ("image load without pair", "@group(2) @binding(0) var t:texture_2d<f32>; @group(2) @binding(1) var s:sampler; @fragment fn outside()->@location(0) vec4<f32>{return textureLoad(t,vec2<i32>(0),0);}", GpuShaderStage::Fragment),
        ("image query without pair", "@group(2) @binding(0) var t:texture_2d<f32>; @fragment fn outside()->@location(0) vec4<f32>{return vec4(vec2<f32>(textureDimensions(t)),0.,1.);}", GpuShaderStage::Fragment),
        ("wrong sampler slot", "@group(2) @binding(0) var t:texture_2d<f32>; @group(2) @binding(3) var s:sampler; @fragment fn outside()->@location(0) vec4<f32>{return textureSample(t,s,vec2(.5));}", GpuShaderStage::Fragment),
        ("wrong stage image", "@group(0) @binding(0) var t:texture_2d<f32>; @group(0) @binding(1) var s:sampler; @fragment fn outside()->@location(0) vec4<f32>{return textureSample(t,s,vec2(.5));}", GpuShaderStage::Fragment),
    ].into_iter().map(|(name, body, stage)| Case { name, source: format!("{SELECTED} {body}"), stage }).collect()
}

/// Query use is legal when the entry also samples the same correctly paired image.
pub(super) fn valid_mixed_module() -> String {
    format!("{SELECTED} @group(2) @binding(0) var t:texture_2d<f32>; @group(2) @binding(1) var s:sampler; @fragment fn outside()->@location(0) vec4<f32>{{let size=vec2<f32>(textureDimensions(t,0)); return textureSample(t,s,vec2(.5))/size.x;}}")
}
