//! Native GPU checks for fallible texture construction, without an SDL window or surface.

use super::*;

/// Runs the actual WGPU validation layer; requires any native adapter, including a software one.
#[test]
#[ignore = "requires a native WGPU adapter; run cargo test -p neocogi-render-wgpu -- --ignored"]
fn texture_upload_failures_are_returned_without_poisoning_later_uploads() {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap();
    let (_pipeline, layout, sampler) = WgpuRenderer::<()>::create_pipeline(&device, wgpu::TextureFormat::Rgba8Unorm);
    let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("texture regression uniforms"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });

    // An outer scope catches any validation error the helper accidentally lets escape. A real
    // existing texture remains owned while each invalid replacement candidate is rejected.
    let outer = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let live = WgpuRenderer::<()>::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, 1, 1, Some(&[255; 4])).unwrap();
    let too_wide = device.limits().max_texture_dimension_2d + 1;
    let dimensions = WgpuRenderer::<()>::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, too_wide, 1, None);
    assert!(dimensions.err().unwrap().contains("dimension limit"));
    let zero = WgpuRenderer::<()>::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, 0, 1, None);
    assert!(zero.err().unwrap().contains("non-zero"));

    // Deliberately undersized bytes exercise asynchronous-shaped WGPU error scopes rather than
    // just the helper's dimension precheck. This must return Err instead of reporting success.
    let invalid = WgpuRenderer::<()>::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, 1, 1, Some(&[0; 3]));
    assert!(invalid.err().unwrap().contains("texture creation or upload failed"));
    let replacement = WgpuRenderer::<()>::create_gpu_texture(&device, &queue, &layout, &sampler, &uniforms, 1, 1, Some(&[17; 4])).unwrap();
    assert!(pollster::block_on(outer.pop()).is_none(), "helper scopes must own their diagnostics");
    drop((replacement, live));
}
