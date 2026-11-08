//! Standalone triangle bootstrap; recording lives in the shared example-owned application module.
#[cfg(not(target_arch = "wasm32"))]
#[path = "../../triangle.rs"]
mod triangle;

/// Acquires one selected platform root and window, then runs the ordinary triangle drawing loop.
#[cfg(not(target_arch = "wasm32"))]
fn main() -> Result<(), neocogi_platform_examples::sdk::render::abi::RenderError> {
    use neocogi_com::NcCopyFromBufferOffset;
    let launch = neocogi_platform_examples::LaunchArguments::parse(std::env::args().skip(1))?;
    if launch.arguments == ["--help"] {
        println!("triangle: [--backend NAME] [FRAME_COUNT]");
        return Ok(());
    }
    if launch.arguments.len() > 1 {
        return Err(neocogi_com::NC_E_INVALID_ARG.into());
    }
    let frames = launch
        .arguments
        .first()
        .map(|value| value.parse::<u32>())
        .transpose()
        .map_err(|_| neocogi_com::NC_E_INVALID_ARG)?
        .unwrap_or(3);
    let platform = neocogi_platform_examples::acquire_platform(launch.backend.as_deref())?;
    let window = platform.create_window(NcCopyFromBufferOffset::new(0, b"NeoCogi Platform triangle"), triangle::INITIAL_SIZE)?;
    triangle::render_frames(window, frames)?;
    println!("Rendered {frames} platform frames.");
    Ok(())
}

/// Browser builds have no native example entry point.
#[cfg(target_arch = "wasm32")]
fn main() {}
