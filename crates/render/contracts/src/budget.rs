//! Per-frame intake limits precede owned copies and expensive primitive expansion.
use neocogi_com::{NcStatus, NC_E_OUT_OF_RANGE};

/// Maximum submitted drawing operations, including fully clipped and empty operations.
pub const MAX_DRAW_COMMANDS: usize = 65_536;
/// Maximum caller-owned bytes inspected or copied into one recorded frame.
pub const MAX_DRAW_INPUT_BYTES: usize = 64 * 1024 * 1024;
/// Maximum estimated primitive work; polygon triangulation charges quadratic point work.
pub const MAX_DRAW_WORK: usize = 16 * 1024 * 1024;
/// Maximum points in one polygon, bounding the tessellator's quadratic worst case.
pub const MAX_POLYGON_POINTS: usize = 4096;

/// Exclusive, allocation-free intake accounting, independent of final clipped output limits.
#[derive(Clone, Copy, Default)]
pub struct FrameBudget {
    /// Submitted operations, charged even when visibility eliminates all output.
    commands: usize,
    /// Total bytes accepted for scanning or retained storage.
    bytes: usize,
    /// Estimated units of expansion or triangulation work accepted so far.
    work: usize,
}
impl FrameBudget {
    /// Starts an empty frame without allocating or retaining resources.
    pub const fn new() -> Self {
        Self { commands: 0, bytes: 0, work: 0 }
    }

    /// Checks every limit before changing accounting; a rejected request reserves nothing.
    pub fn charge(&mut self, bytes: usize, work: usize) -> Result<(), NcStatus> {
        let commands = self
            .commands
            .checked_add(1)
            .filter(|value| *value <= MAX_DRAW_COMMANDS)
            .ok_or(NC_E_OUT_OF_RANGE)?;
        let bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|value| *value <= MAX_DRAW_INPUT_BYTES)
            .ok_or(NC_E_OUT_OF_RANGE)?;
        let work = self.work.checked_add(work).filter(|value| *value <= MAX_DRAW_WORK).ok_or(NC_E_OUT_OF_RANGE)?;
        self.commands = commands;
        self.bytes = bytes;
        self.work = work;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rejected dimensions and arithmetic overflow do not consume otherwise available capacity.
    #[test]
    fn accounting_is_atomic_and_bounds_empty_operations() {
        let mut budget = FrameBudget::new();
        assert_eq!(budget.charge(usize::MAX, 0), Err(NC_E_OUT_OF_RANGE));
        assert_eq!(budget.charge(0, usize::MAX), Err(NC_E_OUT_OF_RANGE));
        budget.charge(MAX_DRAW_INPUT_BYTES, MAX_DRAW_WORK).unwrap();
        assert_eq!(budget.charge(1, 0), Err(NC_E_OUT_OF_RANGE));
        assert_eq!(budget.charge(0, 1), Err(NC_E_OUT_OF_RANGE));
        for _ in 1..MAX_DRAW_COMMANDS {
            budget.charge(0, 0).unwrap();
        }
        assert_eq!(budget.charge(0, 0), Err(NC_E_OUT_OF_RANGE));
    }
}
