//! Concrete backend families and resource-owned native memoization, entirely inside Rust.
use crate::{
    abi::Size,
    command::UniformData,
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
    /// Initialized native storage for one immutable copied uniform payload.
    type Uniform: 'static;
    /// Renderer-owned attachment signature, not a shader compiler input or public selector.
    type PipelineKey: Clone + Eq;
    /// Exclusive concrete driver access; only this backend can construct its preparation capability.
    type Preparation<'a>
    where
        Self: 'a;

    /// Retained opaque identity minted exactly once for the actual compatible native core.
    fn identity(core: &Self::Core) -> &DriverIdentity;
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
    /// Creates an image or captures an authenticated import; import-use receipts remain independent.
    fn image(source: &TextureVersion<Self>, preparation: &mut Self::Preparation<'_>) -> Result<Rc<Self::Image>, String>;
    /// Creates immutable sampling state on the matching native core.
    fn sampler(source: &SamplerVersion<Self>, preparation: &mut Self::Preparation<'_>) -> Result<Rc<Self::Sampler>, String>;
    /// Builds one native target variant. Validation scopes must finish before returning success.
    fn pipeline(source: &PipelineVersion<Self>, key: &Self::PipelineKey, preparation: &mut Self::Preparation<'_>) -> Result<Rc<Self::Pipeline>, String>;
    /// Initializes the exact copied payload and retains its command receipt through native use.
    fn uniform(source: &UniformData<Self>, preparation: &mut Self::Preparation<'_>) -> Result<Rc<Self::Uniform>, String>;
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
