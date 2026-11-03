//! Construction-proven drawing targets shared by independently owned resource groups.
//!
//! A target records identity and attachment capabilities, not a native framebuffer.
//! Framebuffer views are realized by their enclosing window frame. Image surfaces
//! allocate a fresh unpublished color destination for each independent submission.
use crate::{abi::*, native::NativeResources};
use neocogi_com::{NcStatus, NC_E_INVALID_ARG};
use std::rc::Rc;

/// Immutable identity for the surface selected when a resource group is created.
/// Fields are private so a caller cannot change target identity or device ownership.
/// Sharing this `Rc` selects the same image target; equal descriptors select nothing.
/// Groups retain this metadata after the public surface is released, without keeping
/// its renderer, completed-image slot or canonical COM registration alive.
pub struct SurfaceTarget<B: NativeResources> {
    /// Native device/context lifetime, without retaining a renderer facade or window interface.
    pub(crate) core: Rc<B::Core>,
    /// Framebuffer view or independently submitted image, validated before publication.
    kind: RenderSurfaceKind,
}

impl<B: NativeResources> SurfaceTarget<B> {
    /// Creates trusted target metadata; native attachment allocation belongs to frame execution.
    /// Rejects unknown wire-enum values before publishing an identity.
    pub(crate) fn new(core: Rc<B::Core>, kind: RenderSurfaceKind) -> Result<Rc<Self>, NcStatus> {
        match kind {
            RenderSurfaceKind::Framebuffer | RenderSurfaceKind::Image => Ok(Rc::new(Self { core, kind })),
            _ => Err(NC_E_INVALID_ARG),
        }
    }

    /// Returns the validated kind without granting native target mutation or presentation authority.
    pub fn kind(&self) -> RenderSurfaceKind {
        self.kind
    }

    /// Reports an attachment capability, independent of a group's permission to use it.
    /// Current framebuffer views inherit window depth; independent baseline images have color only.
    pub fn has_depth(&self) -> bool {
        self.kind == RenderSurfaceKind::Framebuffer
    }

    /// Rejects contradictory creation settings with `NC_E_INVALID_ARG` before group allocation.
    /// An initial depth clear requires clear permission; every depth operation requires an attachment.
    /// These checks grant no operations: immutable permission fields still gate recording/pipelines.
    pub(crate) fn validate(&self, permissions: RenderClientPermissions) -> Result<(), NcStatus> {
        if permissions.initialize_depth && !permissions.clear_depth {
            return Err(NC_E_INVALID_ARG);
        }
        if !self.has_depth() && (permissions.read_depth || permissions.write_depth || permissions.clear_depth || permissions.initialize_depth) {
            return Err(NC_E_INVALID_ARG);
        }
        Ok(())
    }
}
