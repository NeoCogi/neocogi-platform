//! Isolated process validates actual SDL construction failure without a display dependency.
#![cfg(not(target_arch = "wasm32"))]
use neocogi_com::{ComPtr, NcCopyFromBufferOffset, NC_E_INVALID_ARG};
use neocogi_platform::render::{
    abi::Size,
    platform::{PlatformAbi, PLATFORM_E_BACKEND},
};

/// The failed constructor's owned text survives a later request and final platform release.
#[test]
fn native_window_construction_error_is_owned_by_its_call() {
    const CHILD: &str = "NEOCOGI_OWNED_WINDOW_ERROR_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "native_window_construction_error_is_owned_by_its_call", "--nocapture"])
            .env(CHILD, "1")
            .env("SDL_VIDEODRIVER", "neocogi-unavailable-video-driver")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let platform: ComPtr<PlatformAbi> = neocogi_platform::platform::PlatformObject::from_value(neocogi_platform::platform::PlatformObject::new())
        .query()
        .unwrap();
    let error = platform
        .create_window(NcCopyFromBufferOffset::new(0, b"failure"), Size { width: 8, height: 8 })
        .unwrap_err();
    assert_eq!(error.status, PLATFORM_E_BACKEND);
    assert!(error.message.contains("neocogi-unavailable-video-driver"), "{}", error.message);
    let invalid = platform
        .create_window(NcCopyFromBufferOffset::new(0, b"invalid"), Size { width: 0, height: 8 })
        .unwrap_err();
    assert_eq!(invalid.status, NC_E_INVALID_ARG);
    drop(platform);
    assert!(error.message.contains("neocogi-unavailable-video-driver"));
    assert!(invalid.message.contains("create window"));
}
