//! Exercised on each qualified toolchain against the archived v3 provider.
use neocogi_com_sdk::ComPtr;

extern crate self as local_projection;

/// These modules match the local locators selected by the build script.
#[allow(dead_code, clippy::len_without_is_empty)]
mod graphics {
    include!(env!("IDL_NEOCOGI_RENDERER"));
}

/// The audio projection used across the generated Platform method signature.
#[allow(dead_code, clippy::len_without_is_empty)]
mod sound {
    include!(env!("IDL_NEOCOGI_AUDIO"));
}

/// The Platform v3 root and its cross-namespace service methods.
#[allow(dead_code, clippy::len_without_is_empty)]
mod host {
    include!(env!("IDL_NEOCOGI_PLATFORM"));
}

/// Loads only the trusted library supplied by the archive qualifier, then
/// acquires the generated root and decodes data through its local audio ABI.
fn main() {
    let library = std::path::PathBuf::from(std::env::args_os().nth(1).expect("qualified platform library"));
    let runtime = neocogi_com_host::initialize_runtime();
    // SAFETY: The archive qualifier passes the library it built from the inspected shim.
    let id = unsafe { runtime.load_plugin_library(library) };
    assert_eq!(id, neocogi_com_sdk::guid_from_qualified_name("neocogi.platform.glow"));
    let platform = runtime.acquire_plugin_root::<host::PlatformAbi>(id).unwrap();
    let audio: ComPtr<sound::IAudioCapabilitiesAbi> = platform.audio_capabilities().unwrap();
    assert_eq!(audio.info().unwrap().backend, sound::AudioBackend::Sdl);
    let mut supported = neocogi_com_sdk::NcOutputBuffer::<sound::PcmType>::new();
    audio.pcm_types(supported.output(0, audio.pcm_type_count().unwrap() as usize)).unwrap();
    assert!(supported.as_slice().contains(&sound::PcmType::FixedPoint(32, 16)));
    assert!(supported.as_slice().contains(&sound::PcmType::UnsignedInteger(8)));
}
