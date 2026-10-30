//! Complete generic-frame validation before native target acquisition.
use neocogi_render_core::abi::*;
use neocogi_com::{ComRef, NcStatus, WeakComBox, NC_E_INVALID_ARG, NC_E_OUT_OF_MEMORY};
use neocogi_render_gpu::record::Block;
use std::rc::Rc;

/// Trusted device assembly authenticates canonical block identities without calling client code.
pub trait Resources {
    /// Resolve one weak exclusive observer to immutable data, retaining its domain admission gate.
    fn view(&self, view: WeakComBox<IRenderViewBlockAbi>) -> Result<Rc<Block>, NcStatus>;
}

/// Reusable ordered block storage. Foreign callbacks finish before any native frame is acquired.
#[derive(Default)]
pub struct PreparedFrame {
    /// Each snapshot retains its original domain and immutable resource versions through preparation.
    blocks: Vec<Rc<Block>>,
}
impl PreparedFrame {
    /// Complete authenticated contributions in painter order.
    pub fn blocks(&self) -> &[Rc<Block>] {
        &self.blocks
    }

    /// Release CPU ownership outside native/device-state borrow guards, keeping vector capacity.
    pub fn clear(&mut self) {
        self.blocks.clear();
    }

    /// Validate the complete envelope and snapshot all blocks before native execution.
    /// On any failure, release every partially prepared contribution. Callers own this storage
    /// across foreign callbacks; this method rechecks all domain gates after the last callback.
    pub fn read<R: Resources>(&mut self, info: RenderInfo, stream: ComRef<'_, IRenderStreamAbi>, resources: &R) -> Result<(), NcStatus> {
        self.clear();
        let result = self.read_once(info, stream, resources);
        if result.is_err() {
            self.clear();
        }
        result
    }

    /// Count limits precede reservations and callbacks; roots cannot extend beyond the drawable.
    fn read_once<R: Resources>(&mut self, info: RenderInfo, stream: ComRef<'_, IRenderStreamAbi>, resources: &R) -> Result<(), NcStatus> {
        if info.invocation == 0 || info.size.width <= 0 || info.size.height <= 0 || info.block_count > MAX_RENDER_BLOCKS {
            return Err(NC_E_INVALID_ARG);
        }
        self.blocks.try_reserve(info.block_count as usize).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        let bounds = Rect {
            x: 0,
            y: 0,
            width: info.size.width,
            height: info.size.height,
        };
        for index in 0..info.block_count {
            let block = resources.view(stream.view(info.invocation, index)?)?;
            if !neocogi_render_gpu::scope::contains(bounds, block.root.target) {
                return Err(NC_E_INVALID_ARG);
            }
            self.blocks.push(block);
        }
        // Every foreign accessor has returned. A later callback may have closed an earlier
        // controller, so admission is a complete-frame decision, never a per-accessor cache.
        for block in &self.blocks {
            block.admit()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
