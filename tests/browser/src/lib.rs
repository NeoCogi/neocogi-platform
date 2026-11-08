//! Final browser test artifact; native builds contain no browser provider dependencies.
#![deny(missing_docs)]

/// Executes the actual platform DOM/WebGL assertions under this artifact's single host.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn run_platform_input_tests() {
    // SAFETY: This final artifact owns the immutable process-lived host and keeps
    // its code/service tables mapped throughout every tested COM call.
    let _runtime = unsafe { neocogi_com::Runtime::__from_host_api(neocogi_com_host::initialize_process()) }.expect("compatible browser test host");
    neocogi_platform::platform::testing::run_platform_input_tests();
}
