//! Rust validation conveniences layered over the generated platform ABI.

use crate::render::platform::*;
use neocogi_com::{NC_E_INVALID_ARG, NcStatus};

impl Default for Point {
    /// Returns the coordinate-system origin.
    fn default() -> Self {
        Self { x: 0, y: 0 }
    }
}

impl KeyStroke {
    /// Rejects invalid scalar/function payloads and unknown modifier bits.
    pub fn validate(self) -> Result<(), NcStatus> {
        // The enum carries only the applicable payload; value validation stays
        // beside the owning contract so producers and consumers share a domain.
        let value_valid = match self.key {
            KeyCode::Character(scalar) => char::from_u32(scalar).is_some(),
            KeyCode::Function(number) => (1..=24).contains(&number),
            KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Enter
            | KeyCode::Escape
            | KeyCode::Space
            | KeyCode::Tab
            | KeyCode::Insert
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::ArrowUp
            | KeyCode::ArrowDown
            | KeyCode::ArrowLeft
            | KeyCode::ArrowRight
            | KeyCode::Alt
            | KeyCode::Control
            | KeyCode::Shift
            | KeyCode::Super => true,
        };
        if value_valid && self.modifiers & !15 == 0 {
            Ok(())
        } else {
            Err(NC_E_INVALID_ARG)
        }
    }
}

/// An operation is forbidden in the current platform phase or would reenter polling.
pub const PLATFORM_E_BUSY: NcStatus = NcStatus::from_code(-1001);

/// Native window, event-pump, or browser integration failed; errors retain detail.
pub const PLATFORM_E_BACKEND: NcStatus = NcStatus::from_code(-1005);

/// Maximum UTF-8 bytes transferred by one clipboard request or successful read.
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
