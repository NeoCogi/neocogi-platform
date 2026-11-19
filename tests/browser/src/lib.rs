//! Final browser test artifact; native builds contain no browser provider dependencies.
#![deny(missing_docs)]

/// Executes the actual platform DOM/WebGL assertions under this artifact's single host.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn run_platform_input_tests() {
    let _runtime = neocogi_com_host::initialize_runtime();
    neocogi_platform::platform::testing::run_platform_input_tests().await;
}

/// Exercises actual AudioWorklet processing, typed COM, capture permissions and microphone teardown.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn run_platform_audio_tests() -> Result<(), wasm_bindgen::JsValue> {
    let _runtime = neocogi_com_host::initialize_runtime();
    neocogi_platform::audio::testing::run().await
}
