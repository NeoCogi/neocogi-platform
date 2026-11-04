//! Concrete backend families and resource-owned native memoization, entirely inside Rust.
use crate::{
    abi::Size,
    resource::{BufferVersion, TextureVersion, SamplerVersion, PipelineVersion},
};
use neocogi_render_core::source::DriverIdentity;
use std::{cell::RefCell, rc::Rc};

/// Statically selected native resource family. No backend type or Rust callback crosses COM.
/// Implementors keep hardware lifetime in Core, completion/channel state in the existing driver,
/// and retain only native owners plus opaque receipts in submitted work and retirement packets.
pub trait NativeResources: Sized + 'static {
    /// Hardware/context lifetime only; no renderer, resource map, channel endpoint or submitted work.
    type Core: 'static;
    /// Initialized native vertex/index allocation, independent of its source version.
    type Buffer: 'static;
    /// Shared initialized native image, with no public COM owner or source-version back-reference.
    type Image: 'static;
    /// Immutable native sampling state and its lifetime/accounting owners.
    type Sampler: 'static;
    /// Linked target-compatible native pipeline; never owns its parent version.
    type Pipeline: 'static;
    /// Renderer-owned attachment signature, not a shader compiler input or public selector.
    type PipelineKey: Clone + Eq;
    /// Exclusive concrete driver access; only this backend can construct its preparation capability.
    type Preparation<'a>
    where
        Self: 'a;

    /// Retained opaque identity minted exactly once for the actual compatible native core.
    fn identity(core: &Self::Core) -> &DriverIdentity;
    /// Rejects terminal physical-device/context loss before CPU transfers.
    fn storage_available(core: &Self::Core) -> Result<(), String>;
    /// Actual native core selected by this exclusive preparation transaction.
    fn core<'a>(preparation: &'a Self::Preparation<'_>) -> &'a Rc<Self::Core>;
    /// Checks authoritative native health before either creation or Ready reuse.
    fn check(preparation: &mut Self::Preparation<'_>) -> Result<(), String>;
    /// The image's actual allocation owner; a caller-supplied token cannot substitute for it.
    fn image_core(image: &Self::Image) -> &Rc<Self::Core>;
    /// Immutable dimensions of that same admitted native allocation.
    fn image_size(image: &Self::Image) -> Size;
    /// Creates/initializes a candidate, retaining actual unpublished owners for any issued uploads.
    fn buffer(source: &BufferVersion<Self>, preparation: &mut Self::Preparation<'_>) -> Result<Rc<Self::Buffer>, String>;
    /// Refreshes the same idle allocation after a published upload. No older
    /// logical capture or native prepared/completion owner may still access it.
    fn update_buffer(source: &BufferVersion<Self>, native: &Rc<Self::Buffer>, preparation: &mut Self::Preparation<'_>) -> Result<(), String>;
    /// Creates an image or captures an authenticated import; import-use receipts remain independent.
    fn image(source: &TextureVersion<Self>, preparation: &mut Self::Preparation<'_>) -> Result<Rc<Self::Image>, String>;
    /// Refreshes existing idle image storage; imports and immutable data never call this.
    fn update_image(source: &TextureVersion<Self>, native: &Rc<Self::Image>, preparation: &mut Self::Preparation<'_>) -> Result<(), String>;
    /// Creates immutable sampling state on the matching native core.
    fn sampler(source: &SamplerVersion<Self>, preparation: &mut Self::Preparation<'_>) -> Result<Rc<Self::Sampler>, String>;
    /// Builds one native target variant. Validation scopes must finish before returning success.
    fn pipeline(source: &PipelineVersion<Self>, key: &Self::PipelineKey, preparation: &mut Self::Preparation<'_>) -> Result<Rc<Self::Pipeline>, String>;
}

/// Resource-local native state. Only immutable payload versions may contain this memoization.
enum State<T> {
    /// No successful realization exists; a failed candidate leaves this state retryable.
    Unprepared,
    /// Published only after native creation, initialization admission and scoped validation succeed.
    Ready(Rc<T>),
}

/// The one shared-mutation boundary in an otherwise immutable version.
/// Fields and publication are private; external Rust readers cannot replace captured backings.
pub(crate) struct NativeSlot<T> {
    /// Short borrows inspect/publish a successful candidate, never span native construction.
    state: RefCell<State<T>>,
}
impl<T> Default for NativeSlot<T> {
    fn default() -> Self {
        Self { state: RefCell::new(State::Unprepared) }
    }
}
impl<T> NativeSlot<T> {
    /// Last-owner physical proof is separate from the logical slot's Rc ownership.
    pub(crate) fn idle(&self) -> bool {
        match self.state.try_borrow() {
            Ok(state) => match &*state {
                State::Unprepared => true,
                State::Ready(value) => Rc::strong_count(value) == 1 && Rc::weak_count(value) == 0,
            },
            Err(_) => false,
        }
    }

    /// A completed authoring write refreshes capacity without replacing its handle.
    /// New slots need no refresh because their first realization reads current bytes.
    pub(crate) fn refresh(&self, update: impl FnOnce(&Rc<T>) -> Result<(), String>) -> Result<(), String> {
        let state = self.state.try_borrow().map_err(|_| "native resource is busy")?;
        match &*state {
            State::Unprepared => Ok(()),
            State::Ready(value) if Rc::strong_count(value) == 1 && Rc::weak_count(value) == 0 => update(value),
            State::Ready(_) => Err("native resource still has prepared or submitted owners".into()),
        }
    }
    /// Backend-owned constructors run outside the cell borrow under exclusive preparation.
    pub(crate) fn realize(&self, create: impl FnOnce() -> Result<Rc<T>, String>) -> Result<Rc<T>, String> {
        match &*self.state.try_borrow().map_err(|_| "native resource is busy")? {
            State::Ready(value) => return Ok(value.clone()),
            State::Unprepared => {}
        }
        let value = create()?;
        *self.state.try_borrow_mut().map_err(|_| "native resource is busy")? = State::Ready(value.clone());
        Ok(value)
    }
}

/// Same backend does not imply the same device/context. Check held owners before any slot read.
pub(crate) fn check_core<B: NativeResources>(core: &Rc<B::Core>, preparation: &mut B::Preparation<'_>) -> Result<(), String> {
    if !Rc::ptr_eq(core, B::core(preparation)) {
        return Err("resource belongs to an incompatible driver instance".into());
    }
    B::check(preparation)
}
