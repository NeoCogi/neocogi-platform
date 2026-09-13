//! Rust validation conveniences layered over the generated platform ABI.

use crate::*;
use neocogi_com::{NC_E_INVALID_ARG, NcStatus};

impl Default for Point {
    /// Returns the coordinate-system origin.
    fn default() -> Self {
        Self { x: 0, y: 0 }
    }
}

impl KeyStroke {
    /// Rejects undefined keys, invalid scalar/function payloads, and unknown modifier bits.
    pub fn validate(self) -> Result<(), NcStatus> {
        // The copied key family determines the exact meaning of `value`; keeping
        // this validation beside the owning contract prevents platform and UI
        // implementations from drifting into different accepted domains.
        let value_valid = match self.key {
            KeyCode::Character => char::from_u32(self.value).is_some(),
            KeyCode::Function => (1..=24).contains(&self.value),
            key if key.raw() <= KeyCode::Super.raw() => self.value == 0,
            _ => false,
        };
        if value_valid && self.modifiers & !15 == 0 && (!self.repeat || self.pressed) {
            Ok(())
        } else {
            Err(NC_E_INVALID_ARG)
        }
    }
}

/// An operation is forbidden in the current platform phase or would reenter polling.
pub const PLATFORM_E_BUSY: NcStatus = NcStatus::from_code(-1001);

/// Native window, event-pump, or browser integration failed; diagnostics retain detail.
pub const PLATFORM_E_BACKEND: NcStatus = NcStatus::from_code(-1005);
