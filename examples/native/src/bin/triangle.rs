//! Standalone triangle bootstrap; recording lives in the shared example-owned application module.
#[cfg(not(target_arch = "wasm32"))]
#[path = "../../triangle.rs"]
mod triangle;

/// Acquires one selected platform root and window, then runs the ordinary triangle drawing loop.
#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), neocogi_com::NcStatus> {
    use neocogi_com::NcCopyFromBufferOffset;
    let platform = neocogi_platform_examples::acquire_platform()?;
    let window = platform.create_window(NcCopyFromBufferOffset::new(0, b"NeoCogi Platform triangle"), triangle::INITIAL_SIZE)?;
    let frames = std::env::args()
        .nth(1)
        .map(|value| value.parse::<u32>())
        .transpose()
        .map_err(|_| neocogi_com::NC_E_INVALID_ARG)?
        .unwrap_or(3);
    triangle::render_frames(window, frames)?;
    println!("Rendered {frames} platform frames.");
    Ok(())
}

/// Browser builds have no native example entry point.
#[cfg(target_arch = "wasm32")]
fn main() {}
