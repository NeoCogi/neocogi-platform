//! Final browser test artifact; native builds contain no browser provider dependencies.
#![deny(missing_docs)]

/// Executes the actual platform DOM/WebGL assertions under this artifact's single host.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn run_platform_input_tests() {
    let _runtime = neocogi_com_host::initialize_runtime();
    neocogi_platform::platform::testing::run_platform_input_tests().await;
}

/// Acquires the WebGL provider through the linked host and exercises its browser audio service.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn run_platform_audio_tests() -> Result<(), wasm_bindgen::JsValue> {
    let runtime = neocogi_com_host::initialize_runtime();
    let descriptor = neocogi_platform::platform::plugin_descriptor();
    // SAFETY: The descriptor and provider code remain linked for the lifetime of this Wasm instance.
    unsafe { runtime.register_plugin_descriptor(descriptor) }.map_err(|status| wasm_bindgen::JsValue::from_str(&status.to_string()))?;
    let platform = runtime
        .acquire_plugin_root(descriptor.plugin_id)
        .map_err(|status| wasm_bindgen::JsValue::from_str(&status.to_string()))?;
    neocogi_platform::audio::testing::run(platform).await
}
