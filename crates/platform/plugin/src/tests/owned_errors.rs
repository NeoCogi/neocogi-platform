//! Real native failures keep their explanations across unrelated work and final device release.
use super::*;

/// A failed native allocation returns text from that allocation, and success
/// reports retain no device/window. Run against each selected native backend.
#[test]
#[ignore = "requires a native SDL display; Glow permits SDL_VIDEODRIVER=offscreen"]
fn native_operation_errors_and_success_reports_outlive_their_device() {
    let _session = native_session();
    let platform = PlatformObject::new(
        nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
            .query()
            .unwrap(),
    );
    let native = platform.window("owned native errors", SIZE).unwrap();
    let device = Rc::downgrade(&native.device);
    let window: ComPtr<IPlatformWindowAbi> = WindowObject::from_value(native).query().unwrap();
    let factory: ComPtr<ITextureFactoryAbi> = window.query().unwrap();
    // Four million bytes fit the shared 64 MiB image cap, but the width exceeds
    // the tested native devices' maximum texture dimension. The backend supplies
    // the concrete reason, rather than a generic logical admission error.
    let failure = factory
        .create_texture(
            Size { width: 1_000_000, height: 1 },
            neocogi_render_core::source::initial(None).borrow().unwrap(),
        )
        .unwrap_err();
    assert_eq!(failure.status, RENDER_E_BACKEND);
    assert!(failure.message.contains("create native texture"));
    assert!(!failure.message.ends_with(&RENDER_E_BACKEND.to_string()));
    let text = failure.message.clone();
    drop(
        factory
            .create_texture(Size { width: 1, height: 1 }, neocogi_render_core::source::initial(None).borrow().unwrap())
            .unwrap(),
    );
    let renderer: ComPtr<IRendererAbi> = window.query().unwrap();
    let empty: ComPtr<IRenderStreamAbi> = Empty::from_value(Empty).query().unwrap();
    let other = renderer
        .render(
            RenderInfo {
                size: Size { width: 1, height: 1 },
                ..info()
            },
            empty.as_com_ref(),
        )
        .unwrap_err();
    assert_eq!(other.status, NC_E_INVALID_ARG);
    assert!(other.message.contains("drawable size"));
    let host = window.graphics_host().unwrap();
    let success = host.maintain_result().unwrap();
    let later_success = host.maintain_result().unwrap();
    assert_eq!(success.as_raw().object, later_success.as_raw().object);
    drop((host, renderer, factory, window, platform));
    assert!(device.upgrade().is_none(), "success reports must not retain the device");
    check_outcome(&success, "maintain").unwrap();
    check_outcome(&later_success, "maintain").unwrap();
    assert_eq!(failure.message, text);
    assert!(other.message.contains("drawable size"));
}
