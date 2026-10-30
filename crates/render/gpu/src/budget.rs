//! Device-free accounting follows actual backing lifetime rather than public handles.
use crate::abi::RenderClientLimits;
use neocogi_com::{NC_E_OUT_OF_RANGE, NcStatus};
use std::{cell::Cell, rc::Rc};

/// Mutable counters shared with destruction paths, without a renderer/device back-reference.
pub(crate) struct Account {
    /// Host-selected limits, immutable for this domain.
    pub(crate) limits: RenderClientLimits,
    /// Public resource owners currently alive.
    resources: Cell<u32>,
    /// Admitted immutable versions, including retained old versions.
    bytes: Cell<u64>,
}

impl Account {
    /// Validates finite host limits before allocating any public capability.
    pub(crate) fn new(limits: RenderClientLimits) -> Result<Rc<Self>, NcStatus> {
        if limits.resources == 0
            || limits.resources > 65_536
            || limits.live_bytes == 0
            || limits.live_bytes > 1024 * 1024 * 1024
            || limits.pending_bytes == 0
            || limits.pending_bytes > 64 * 1024 * 1024
            || limits.frame_draws == 0
            || limits.frame_indices == 0
            || limits.live_views == 0
            || limits.stack_depth == 0
            || limits.stack_depth > 64
        {
            return Err(NC_E_OUT_OF_RANGE);
        }
        Ok(Rc::new(Self {
            limits,
            resources: Cell::new(0),
            bytes: Cell::new(0),
        }))
    }

    /// Reserves owner count before foreign bulk reads; rollback is automatic.
    pub(crate) fn resource(self: &Rc<Self>) -> Result<ResourceCharge, NcStatus> {
        let next = self
            .resources
            .get()
            .checked_add(1)
            .filter(|n| *n <= self.limits.resources)
            .ok_or(NC_E_OUT_OF_RANGE)?;
        self.resources.set(next);
        Ok(ResourceCharge(self.clone()))
    }

    /// Reserves a complete immutable allocation before copying source bytes.
    pub(crate) fn bytes(self: &Rc<Self>, count: usize) -> Result<ByteCharge, NcStatus> {
        let count = u64::try_from(count).map_err(|_| NC_E_OUT_OF_RANGE)?;
        let next = self
            .bytes
            .get()
            .checked_add(count)
            .filter(|n| *n <= self.limits.live_bytes)
            .ok_or(NC_E_OUT_OF_RANGE)?;
        self.bytes.set(next);
        Ok(ByteCharge { account: self.clone(), count })
    }

    /// Current version bytes, useful for host diagnostics and lifetime regressions.
    pub(crate) fn live_bytes(&self) -> u64 {
        self.bytes.get()
    }
}

/// One live public owner; weak observations do not retain its resource count.
pub(crate) struct ResourceCharge(Rc<Account>);
impl Drop for ResourceCharge {
    /// Owner destruction happens after checked COM calls finish.
    fn drop(&mut self) {
        self.0.resources.set(self.0.resources.get() - 1);
    }
}

/// Bytes retained by an immutable version, independent of public resource ownership.
pub(crate) struct ByteCharge {
    /// Contains counters only; holding this token cannot pin a device or UI tree.
    account: Rc<Account>,
    /// Exact logical version charge.
    count: u64,
}
impl Drop for ByteCharge {
    /// Refund only when the actual last version owner releases this token.
    fn drop(&mut self) {
        self.account.bytes.set(self.account.bytes.get() - self.count);
    }
}
