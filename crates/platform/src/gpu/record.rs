//! Host-owned revocable recording with weak client facades and immutable generic blocks.
//!
//! The shared state cell exists because host expiry and an externally retained
//! exclusive view must retire the same payload. Mutations take the payload out,
//! work on owned data and reinstall only while the private lease remains live.
use crate::gpu::{
    abi::*,
    budget::{Account, PendingCharge, ViewCharge},
    command::*,
    resource::{self, *},
    scope::ViewStack,
};
use neocogi_com::{com_object, ComBox, ComPtr, ComRef, NcStatus, WeakLocalBox, NC_E_INVALID_ARG, NC_E_OUT_OF_MEMORY, NC_E_OUT_OF_RANGE};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::{Rc, Weak},
};

/// Immutable host-owned block; domain provenance also exists for clear-only/empty work.
pub struct Block<B: crate::gpu::native::NativeResources> {
    /// Captured final visible root, independent of later caller envelopes.
    pub(crate) root: RenderViewRoot,
    /// Ordered generic commands, never callbacks or mutable public resources.
    pub(crate) commands: Vec<Command<B>>,
    /// Keeps admission/native lifetime authority outside device-owned caches.
    domain: Rc<Domain<B>>,
    /// Capacity of the original bounded recorder, retained with its transferred data.
    _base: PendingCharge,
}
impl<B: crate::gpu::native::NativeResources> Block<B> {
    /// Observes the immutable target captured from the resource group's trusted creation path.
    /// This metadata exposes no attachment handle or permission to submit to another target.
    pub fn target(&self) -> &Rc<crate::gpu::target::SurfaceTarget<B>> {
        &self.domain.target
    }

    /// Rechecks group closure and exact target identity after foreign readers finish.
    /// Image submission must use this check: matching device, kind or dimensions are insufficient.
    /// Returns `NC_E_DROPPED` for a closed group or `RENDER_E_OWNER` for another selected target.
    pub fn admit_target(&self, target: &Rc<crate::gpu::target::SurfaceTarget<B>>) -> Result<(), NcStatus> {
        self.domain.open()?;
        if Rc::ptr_eq(&self.domain.target, target) {
            Ok(())
        } else {
            Err(RENDER_E_OWNER)
        }
    }

    /// Framebuffer-view commands join the enclosing window frame without acquiring another target.
    /// Call after device-authenticated stream preparation; this rejects image groups and closure.
    /// Distinct framebuffer views are allowed because they refer to this device's current frame.
    pub fn admit_framebuffer(&self) -> Result<(), NcStatus> {
        self.domain.open()?;
        if self.domain.target.kind() == RenderSurfaceKind::Framebuffer {
            Ok(())
        } else {
            Err(RENDER_E_OWNER)
        }
    }

    /// Retains original command capacity in native/in-flight representations without device authority.
    /// The receipts share accounting only; they cannot submit, observe or reopen a domain.
    pub fn retain_capacity(&self) -> Vec<CommandCharge> {
        std::iter::once(CommandCharge { _charge: self._base.clone() })
            .chain(self.commands.iter().map(Command::retain_capacity))
            .collect()
    }

    /// Native executors validate driver ownership even for clear-only blocks with no resources.
    pub fn admit_on(&self, core: &Rc<B::Core>) -> Result<(), NcStatus> {
        if !Rc::ptr_eq(&self.domain.device.inner.core, core) {
            return Err(RENDER_E_OWNER);
        }
        self.domain.open()
    }

    /// Final local admission check after every foreign stream callback has completed.
    pub fn admit(&self) -> Result<(), NcStatus> {
        self.domain.open()
    }
}

#[com_object(interfaces(IRenderViewBlock), ownership = exclusive)]
/// Canonical marker whose checked local projection recovers one immutable block.
pub(crate) struct BlockObject<B: crate::gpu::native::NativeResources> {
    /// Sealed data can outlive public ownership during detached preparation.
    block: Rc<Block<B>>,
}
impl<B: crate::gpu::native::NativeResources> IRenderViewBlock for BlockObject<B> {}

impl<B: crate::gpu::native::NativeResources> GpuDevice<B> {
    /// Creates a narrow trusted broker; it exposes no renderer/device/controller getter.
    pub fn broker(&self) -> ComPtr<IRenderViewBrokerAbi> {
        Broker::from_value(Broker {
            device: self.clone(),
            active: Rc::new(Cell::new(false)),
        })
        .query()
        .expect("broker declares its exact interface")
    }

    /// Authenticates canonical block identity and snapshots it before native execution.
    pub fn block_snapshot(&self, block: ComRef<'_, IRenderViewBlockAbi>) -> Result<Rc<Block<B>>, NcStatus> {
        let observer = resource::observe(&self.inner.blocks, block.as_raw().object as usize)?;
        observer.with(|block| {
            block.block.admit()?;
            Ok(block.block.clone())
        })?
    }

    /// Returns live copied payload bytes and retained public view values for host diagnostics.
    pub fn recording_usage(&self, factory: ComRef<'_, IRenderResourcesAbi>) -> Result<(u64, u32), NcStatus> {
        let domain = self.domain(factory)?;
        Ok((domain.account.pending_bytes(), domain.account.live_views()))
    }
}

/// One account's cumulative attempts; retaining the account makes its map key generation-safe.
struct Work {
    /// Counters only, never native device or UI ownership.
    account: Rc<Account>,
    /// Draws and regional clear attempts, including automatic view initialization.
    draws: u32,
    /// Vertex/index work including instance multiplication.
    elements: u64,
}

/// Private CPU traversal lifetime; finished frames cannot reactivate or abort a later frame.
struct Frame<B: crate::gpu::native::NativeResources> {
    /// Logical device for provenance and operation reentry checks.
    device: GpuDevice<B>,
    /// False is terminal; successful finish and abort both close new view admission.
    alive: Cell<bool>,
    /// Per-installed-broker active flag, cleared exactly once by this frame.
    broker_active: Rc<Cell<bool>>,
    /// Shared quotas span all sessions and widgets in this CPU frame.
    work: RefCell<HashMap<usize, Work>>,
    /// Weak links retire pending sessions on CPU errors without a GPU submit.
    sessions: RefCell<Vec<Weak<SessionCore<B>>>>,
}
impl<B: crate::gpu::native::NativeResources> Frame<B> {
    /// Reserves attempts before validation/scanning; cancel and reset never refund them.
    fn charge(&self, account: &Rc<Account>, elements: u64) -> Result<(), NcStatus> {
        if !self.alive.get() {
            return Err(RENDER_E_EXPIRED);
        }
        let key = Rc::as_ptr(account) as usize;
        let mut work = self.work.try_borrow_mut().map_err(|_| RENDER_E_BUSY)?;
        work.try_reserve(1).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        let entry = work.entry(key).or_insert_with(|| Work {
            account: account.clone(),
            draws: 0,
            elements: 0,
        });
        let draws = entry.draws.checked_add(1).ok_or(NC_E_OUT_OF_RANGE)?;
        let total = entry.elements.checked_add(elements).ok_or(NC_E_OUT_OF_RANGE)?;
        // Record exhaustion too; another session cannot retry with a reset counter.
        entry.draws = draws;
        entry.elements = total;
        if draws > entry.account.limits.frame_draws || total > entry.account.limits.frame_indices {
            return Err(NC_E_OUT_OF_RANGE);
        }
        Ok(())
    }

    /// Closes the broker lease once and expires only this frame's remaining sessions.
    fn stop(&self) {
        if !self.alive.replace(false) {
            return;
        }
        self.broker_active.set(false);
        let sessions = std::mem::take(&mut *self.sessions.borrow_mut());
        for session in sessions {
            if let Some(session) = session.upgrade() {
                session.expire();
            }
        }
    }
}

#[com_object(interfaces(IRenderViewBroker))]
/// Shareable trusted entry; clients receive neither this object nor the frame owner.
struct Broker<B: crate::gpu::native::NativeResources> {
    /// Shared logical renderer, without retaining a window or application traversal owner.
    device: GpuDevice<B>,
    /// Only one active CPU traversal per installed broker.
    active: Rc<Cell<bool>>,
}
impl<B: crate::gpu::native::NativeResources> IRenderViewBroker for Broker<B> {
    /// Starts fresh cumulative counters without resetting live-byte or retained-shell charges.
    fn begin_frame(&self) -> Result<ComBox<IRenderRecordingFrameAbi>, NcStatus> {
        if self.device.inner.busy.get() || self.active.replace(true) {
            return Err(RENDER_E_BUSY);
        }
        let frame = Rc::new(Frame {
            device: self.device.clone(),
            alive: Cell::new(true),
            broker_active: self.active.clone(),
            work: RefCell::new(HashMap::new()),
            sessions: RefCell::new(Vec::new()),
        });
        FrameOwner::into_box(FrameOwner { frame }).query().map_err(|(status, _)| status)
    }
}

#[com_object(interfaces(IRenderRecordingFrame), ownership = exclusive)]
/// Trusted owner of one CPU recording traversal; Drop aborts without submitting GPU commands.
struct FrameOwner<B: crate::gpu::native::NativeResources> {
    /// Sessions refer to this frame weakly and cannot extend its active lifetime.
    frame: Rc<Frame<B>>,
}
impl<B: crate::gpu::native::NativeResources> IRenderRecordingFrame for FrameOwner<B> {
    /// Authenticates factory/root and allocates one renderer-owned pending slot.
    fn begin_view(&mut self, root: RenderViewRoot, factory: ComRef<'_, IRenderResourcesAbi>) -> Result<ComBox<IRenderViewSessionAbi>, NcStatus> {
        if !self.frame.alive.get() {
            return Err(RENDER_E_EXPIRED);
        }
        if self.frame.device.inner.busy.get() {
            return Err(RENDER_E_BUSY);
        }
        let domain = self.frame.device.domain(factory)?;
        // Only an explicitly requested regional depth clear consumes draw/work quota here.
        if domain.permissions.initialize_depth {
            self.frame.charge(&domain.account, 3)?;
        }
        let recorder = Recorder::new(root, &domain)?;
        let core = Rc::new(SessionCore {
            domain: domain.clone(),
            frame: Rc::downgrade(&self.frame),
            root,
            state: RefCell::new(State::Recording(recorder)),
            issued: Cell::new(false),
        });
        {
            let mut sessions = self.frame.sessions.borrow_mut();
            sessions.retain(|session| session.strong_count() > 0);
            sessions.try_reserve(1).map_err(|_| NC_E_OUT_OF_MEMORY)?;
            sessions.push(Rc::downgrade(&core));
        }
        {
            let mut sessions = domain.sessions.borrow_mut();
            sessions.retain(|session| session.strong_count() > 0);
            sessions.try_reserve(1).map_err(|_| NC_E_OUT_OF_MEMORY)?;
            sessions.push(Rc::downgrade(&core));
        }
        SessionOwner::into_box(SessionOwner { core }).query().map_err(|(status, _)| status)
    }

    /// Only harvested/cancelled sessions may finish; already transferred blocks remain valid.
    fn finish(&mut self) -> Result<(), NcStatus> {
        if !self.frame.alive.get() {
            return Err(RENDER_E_EXPIRED);
        }
        let sessions: Vec<_> = self.frame.sessions.borrow().iter().filter_map(Weak::upgrade).collect();
        for session in sessions {
            if !matches!(*session.state.borrow(), State::Cancelled | State::Expired) {
                return Err(RENDER_E_STATE);
            }
        }
        self.frame.stop();
        Ok(())
    }
    /// Error cleanup remains legal after finish; it cannot affect a newer frame.
    fn abort(&mut self) -> Result<(), NcStatus> {
        self.frame.stop();
        Ok(())
    }
}
impl<B: crate::gpu::native::NativeResources> Drop for FrameOwner<B> {
    /// Restores broker admission when a recording traversal exits without native submission.
    fn drop(&mut self) {
        self.frame.stop();
    }
}

/// One authoritative slot with exhaustive terminal states; only Recording owns mutable work.
// The slot already has one stable Rc allocation. Keeping its bounded recorder inline avoids a
// second allocation on every view; terminal slots release all payload despite their fixed size.
#[allow(clippy::large_enum_variant)]
enum State<B: crate::gpu::native::NativeResources> {
    /// Ordinary owned recording data is installed between calls.
    Recording(Recorder<B>),
    /// Payload is temporarily owned by a client method; host expiry may supersede it.
    Mutating,
    /// First error only; payload/capacity have already retired.
    Rejected(NcStatus),
    /// Complete immutable block awaiting a once-only host transfer.
    Committed(Rc<Block<B>>),
    /// Deliberate abandonment contributes no block.
    Cancelled,
    /// Host return/closure/transfer permanently revoked this slot.
    Expired,
}

/// Renderer-private revocation bridge; only host owners keep it alive.
pub(crate) struct SessionCore<B: crate::gpu::native::NativeResources> {
    /// Strong admission/lifetime authority exists while the trusted host session exists.
    domain: Rc<Domain<B>>,
    /// An escaped client view cannot keep a CPU frame active.
    frame: Weak<Frame<B>>,
    /// Immutable committed-layout root.
    root: RenderViewRoot,
    /// Shared solely because both host teardown and exclusive client calls retire one payload.
    state: RefCell<State<B>>,
    /// Client owner can be issued at most once, including after it drops.
    issued: Cell<bool>,
}
impl<B: crate::gpu::native::NativeResources> SessionCore<B> {
    /// Source of every client method's authority, independent of public invocation numbers.
    fn check(&self) -> Result<Rc<Frame<B>>, NcStatus> {
        let frame = self.frame.upgrade().filter(|frame| frame.alive.get()).ok_or(RENDER_E_EXPIRED)?;
        self.domain.open()?;
        if self.domain.device.inner.busy.get() {
            return Err(RENDER_E_BUSY);
        }
        Ok(frame)
    }
    /// Moves payload ownership out before any releases; retained client shells keep no data.
    pub(crate) fn expire(&self) {
        let previous = std::mem::replace(&mut *self.state.borrow_mut(), State::Expired);
        drop(previous);
    }
    /// Deliberately abandons only healthy recording; terminal outcomes retain their meaning.
    /// Release payload after the state guard so resource destruction may safely reenter.
    fn cancel(&self) {
        let previous = {
            let mut state = self.state.borrow_mut();
            if !matches!(*state, State::Recording(_)) {
                return;
            }
            std::mem::replace(&mut *state, State::Cancelled)
        };
        drop(previous);
    }
}

#[com_object(interfaces(IRenderViewSession), ownership = exclusive)]
/// Host control is a different allocation from its issued client view.
struct SessionOwner<B: crate::gpu::native::NativeResources> {
    /// Single pending payload slot.
    core: Rc<SessionCore<B>>,
}
impl<B: crate::gpu::native::NativeResources> IRenderViewSession for SessionOwner<B> {
    /// Transfers an exclusive facade with a device-free shell receipt and weak payload link.
    fn view(&mut self) -> Result<ComBox<IRenderViewAbi>, NcStatus> {
        self.core.check()?;
        if self.core.issued.get() || !matches!(*self.core.state.borrow(), State::Recording(_)) {
            return Err(RENDER_E_STATE);
        }
        let charge = self.core.domain.account.view()?;
        self.core.issued.set(true);
        View::into_box(View {
            core: Rc::downgrade(&self.core),
            _shell: charge,
        })
        .query()
        .map_err(|(status, _)| status)
    }
    /// Host inspection never revives an expired payload or queries a foreign object.
    fn state(&self) -> Result<RenderViewState, NcStatus> {
        Ok(match &*self.core.state.try_borrow().map_err(|_| RENDER_E_BUSY)? {
            State::Recording(_) => RenderViewState::Recording,
            State::Mutating => return Err(RENDER_E_BUSY),
            State::Rejected(_) => RenderViewState::Rejected,
            State::Committed(_) => RenderViewState::Committed,
            State::Cancelled => RenderViewState::Cancelled,
            State::Expired => RenderViewState::Expired,
        })
    }
    /// Removes the committed contribution once; public view methods remain permanently expired.
    fn take_block(&mut self) -> Result<ComBox<IRenderViewBlockAbi>, NcStatus> {
        self.core.check()?;
        let previous = std::mem::replace(&mut *self.core.state.borrow_mut(), State::Expired);
        match previous {
            State::Committed(block) => resource::publish(&self.core.domain.device.inner.blocks, BlockObject::into_local_box(BlockObject { block })),
            other => {
                *self.core.state.borrow_mut() = other;
                Err(RENDER_E_STATE)
            }
        }
    }
    /// Legal on any state; releases unmoved payload/capacity outside the state guard.
    fn expire(&mut self) -> Result<(), NcStatus> {
        self.core.expire();
        Ok(())
    }
}
impl<B: crate::gpu::native::NativeResources> Drop for SessionOwner<B> {
    /// Host teardown wins even if the client retained its exclusive facade.
    fn drop(&mut self) {
        self.core.expire();
    }
}

/// One weakly observed vertex binding; no public owner is promoted by binding.
struct BoundVertex<B: crate::gpu::native::NativeResources> {
    /// Stage-independent vertex stream slot.
    slot: u32,
    /// Checked weak construction proof.
    object: WeakLocalBox<BufferObject<B>>,
    /// Byte base for subsequent draw-time versions.
    offset: u64,
}
/// One weakly observed index binding with an explicit element interpretation.
struct BoundIndex<B: crate::gpu::native::NativeResources> {
    /// Checked weak construction proof.
    object: WeakLocalBox<BufferObject<B>>,
    /// Unsigned element width.
    format: GpuIndexFormat,
    /// Byte base for subsequent draw-time versions.
    offset: u64,
}
/// One weakly observed texture/sampler pair.
struct BoundTexture<B: crate::gpu::native::NativeResources> {
    /// Consuming shader stage.
    stage: GpuShaderStage,
    /// Stage-local sampled pair slot.
    slot: u32,
    /// Checked weak sampled texture construction proof.
    texture: WeakLocalBox<BufferObject<B>>,
    /// Checked weak sampler construction proof.
    sampler: WeakLocalBox<SamplerObject<B>>,
}

/// Uniform buffer observation does not retain its public owner before a draw.
struct BoundUniform<B: crate::gpu::native::NativeResources> {
    /// Consuming programmable stage and stage-local slot.
    stage: GpuShaderStage,
    /// Validated stage-local uniform index.
    slot: u32,
    /// Actual registered buffer; wrappers pass their underlying driver resource.
    object: WeakLocalBox<BufferObject<B>>,
    /// Aligned fixed-allocation byte base.
    offset: usize,
    /// Exact reflected shader byte extent.
    len: usize,
}

/// Ordinary owned mutable recording state, detached from the shared slot during operations.
struct Recorder<B: crate::gpu::native::NativeResources> {
    /// Checked single viewport/clip stack.
    stack: ViewStack,
    /// Pipeline binding is observed weakly until each draw captures it.
    pipeline: Option<WeakLocalBox<PipelineObject<B>>>,
    /// At most eight explicitly bound streams.
    vertices: Vec<BoundVertex<B>>,
    /// Optional index stream; non-indexed draws ignore it.
    index: Option<BoundIndex<B>>,
    /// At most four sampled slots per stage.
    textures: Vec<BoundTexture<B>>,
    /// At most four weak uniform-buffer bindings per stage.
    uniforms: Vec<BoundUniform<B>>,
    /// Commands already contain immutable resource versions and flattened bounds.
    commands: Vec<Command<B>>,
    /// Set only by a balanced successful finish.
    sealed: bool,
    /// Bounded stack/binding metadata allocation.
    base: PendingCharge,
}
impl<B: crate::gpu::native::NativeResources> Recorder<B> {
    /// Allocates bounded state and inserts a regional depth-one clear only when explicitly requested.
    /// The frame charges that initial clear before construction; this function owns its pending-byte
    /// receipt. Color-only groups start with no commands and consume no phantom depth-clear work.
    fn new(root: RenderViewRoot, domain: &Rc<Domain<B>>) -> Result<Self, NcStatus> {
        let bytes = std::mem::size_of::<Self>()
            + domain.account.limits.stack_depth as usize * std::mem::size_of::<crate::gpu::scope::ViewState>()
            + 8 * (std::mem::size_of::<BoundVertex<B>>() + std::mem::size_of::<BoundTexture<B>>() + std::mem::size_of::<BoundUniform<B>>());
        let base = domain.account.pending(bytes)?;
        let stack = ViewStack::new(root, domain.account.limits.stack_depth)?;
        let mut result = Self {
            stack,
            pipeline: None,
            vertices: Vec::new(),
            index: None,
            textures: Vec::new(),
            uniforms: Vec::new(),
            commands: Vec::new(),
            sealed: false,
            base,
        };
        result.vertices.try_reserve_exact(8).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        result.textures.try_reserve_exact(8).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        result.uniforms.try_reserve_exact(8).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        if domain.permissions.initialize_depth {
            let charge = CommandCharge {
                _charge: domain.account.pending(std::mem::size_of::<Command<B>>())?,
            };
            result.commands.try_reserve_exact(1).map_err(|_| NC_E_OUT_OF_MEMORY)?;
            result.commands.push(Command::ClearDepth { depth: 1.0, clip: root.target, charge });
        }
        Ok(result)
    }

    /// Discards transient weak bindings/stack while moving exactly one immutable payload.
    fn into_block(self, domain: Rc<Domain<B>>) -> Block<B> {
        Block {
            root: self.stack.root(),
            commands: self.commands,
            domain,
            _base: self.base,
        }
    }

    /// Captures all active required bindings and validates ranges together at draw time.
    fn draw(&mut self, core: &SessionCore<B>, range: DrawRange) -> Result<(), NcStatus> {
        let frame = core.check()?;
        let (count, instances) = match range {
            DrawRange::Vertices { count, instances, .. } | DrawRange::Indices { count, instances, .. } => (count, instances),
        };
        // Zero-instance indexed calls still scan their selected indices. Charge
        // that CPU work too instead of multiplying it down to zero.
        frame.charge(
            &core.domain.account,
            u64::from(count).checked_mul(u64::from(instances.max(1))).ok_or(NC_E_OUT_OF_RANGE)?,
        )?;
        if count > i32::MAX as u32 || instances > i32::MAX as u32 {
            return Err(NC_E_OUT_OF_RANGE);
        }
        let pipeline = self.pipeline.as_ref().ok_or(RENDER_E_STATE)?.with(|pipeline| {
            same_domain(&core.domain, &pipeline.domain)?;
            Ok::<_, NcStatus>(pipeline.current.clone())
        })??;
        let mut index = None;
        let elements = match range {
            DrawRange::Vertices { first, count, .. } => u64::from(first.checked_add(count).filter(|end| *end <= i32::MAX as u32).ok_or(NC_E_OUT_OF_RANGE)?),
            DrawRange::Indices { first, count, .. } => {
                let bound = self.index.as_ref().ok_or(RENDER_E_STATE)?;
                let buffer = bound.object.with(|buffer| {
                    same_domain(&core.domain, &buffer.domain)?;
                    buffer.linear().cloned()
                })??;
                if buffer.desc.role() != GpuBufferRole::Index {
                    return Err(NC_E_INVALID_ARG);
                }
                let size = index_size(bound.format)?;
                let start = bound.offset.checked_add(u64::from(first) * size).ok_or(NC_E_OUT_OF_RANGE)?;
                let end = start
                    .checked_add(u64::from(count) * size)
                    .filter(|end| *end <= buffer.bytes.len() as u64)
                    .ok_or(NC_E_OUT_OF_RANGE)?;
                let mut max = 0u64;
                for offset in (start as usize..end as usize).step_by(size as usize) {
                    let mut bytes = [0; 4];
                    buffer.bytes.read(offset, &mut bytes[..size as usize])?;
                    let value = if size == 2 {
                        u32::from(u16::from_le_bytes([bytes[0], bytes[1]]))
                    } else {
                        u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
                    };
                    // WebGL 2 always treats the all-ones element as restart.
                    // The baseline excludes restart, so never silently drop it.
                    if (size == 2 && value == u32::from(u16::MAX)) || value == u32::MAX {
                        return Err(NC_E_INVALID_ARG);
                    }
                    max = max.max(u64::from(value) + 1);
                }
                index = Some(IndexBinding {
                    buffer,
                    format: bound.format,
                    offset: bound.offset,
                });
                max
            }
        };
        let charge = core.domain.account.pending(
            std::mem::size_of::<Draw<B>>()
                + pipeline.vertex_buffers.len() * std::mem::size_of::<VertexBinding<B>>()
                + pipeline.bindings.len() * (std::mem::size_of::<SampledBinding<B>>() + std::mem::size_of::<UniformBinding<B>>()),
        )?;
        let mut streams = Vec::new();
        let mut textures = Vec::new();
        let mut uniforms = Vec::new();
        streams.try_reserve_exact(pipeline.vertex_buffers.len()).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        textures.try_reserve_exact(pipeline.bindings.len()).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        uniforms.try_reserve_exact(pipeline.bindings.len()).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        for layout in &pipeline.vertex_buffers {
            let bound = self.vertices.iter().find(|bound| bound.slot == layout.slot).ok_or(RENDER_E_STATE)?;
            let buffer = bound.object.with(|buffer| {
                same_domain(&core.domain, &buffer.domain)?;
                buffer.linear().cloned()
            })??;
            if buffer.desc.role() != GpuBufferRole::Vertex || bound.offset % 4 != 0 {
                return Err(NC_E_INVALID_ARG);
            }
            let element_count = if count == 0 || instances == 0 {
                0
            } else if layout.step == GpuStepMode::Instance {
                u64::from(instances)
            } else {
                elements
            };
            let footprint = pipeline
                .attributes
                .iter()
                .filter(|attribute| attribute.slot == layout.slot)
                .try_fold(0, |largest, attribute| {
                    crate::gpu::pipeline::vertex_format(attribute.format).map(|(size, _, _)| largest.max(u64::from(attribute.offset) + u64::from(size)))
                })?;
            let need = if element_count == 0 {
                0
            } else {
                (element_count - 1)
                    .checked_mul(u64::from(layout.stride))
                    .and_then(|offset| offset.checked_add(footprint))
                    .ok_or(NC_E_OUT_OF_RANGE)?
            };
            if bound.offset.checked_add(need).filter(|end| *end <= buffer.bytes.len() as u64).is_none() {
                return Err(NC_E_OUT_OF_RANGE);
            }
            streams.push(VertexBinding {
                slot: layout.slot,
                buffer,
                offset: bound.offset,
            });
        }
        for binding in &pipeline.bindings {
            match binding.kind {
                GpuBindingKind::Uniform(byte_len) => {
                    let bound = self
                        .uniforms
                        .iter()
                        .find(|bound| bound.stage == binding.stage && bound.slot == binding.slot)
                        .ok_or(RENDER_E_STATE)?;
                    if bound.len != byte_len as usize {
                        return Err(NC_E_INVALID_ARG);
                    }
                    let buffer = bound.object.with(|buffer| {
                        same_domain(&core.domain, &buffer.domain)?;
                        buffer.linear().cloned()
                    })??;
                    uniforms.push(UniformBinding {
                        stage: bound.stage,
                        slot: bound.slot,
                        data: Rc::new(UniformData {
                            buffer,
                            offset: bound.offset,
                            len: bound.len,
                        }),
                    });
                }
                GpuBindingKind::SampledTexture => {
                    let bound = self
                        .textures
                        .iter()
                        .find(|bound| bound.stage == binding.stage && bound.slot == binding.slot)
                        .ok_or(RENDER_E_STATE)?;
                    let texture = bound.texture.with(|texture| {
                        same_domain(&core.domain, &texture.domain)?;
                        texture.texture().cloned()
                    })??;
                    let sampler = bound.sampler.with(|sampler| {
                        same_domain(&core.domain, &sampler.domain)?;
                        Ok::<_, NcStatus>(sampler.current.clone())
                    })??;
                    textures.push(SampledBinding {
                        stage: bound.stage,
                        slot: bound.slot,
                        texture,
                        sampler,
                    });
                }
            }
        }
        let state = self.stack.current();
        if count == 0 || instances == 0 || state.clip.width == 0 || state.clip.height == 0 {
            return Ok(());
        }
        self.commands.try_reserve_exact(1).map_err(|_| NC_E_OUT_OF_MEMORY)?;
        self.commands.push(Command::Draw(Draw {
            pipeline,
            vertex_buffers: streams,
            index,
            textures,
            uniforms,
            range,
            viewport: self.stack.target(state.viewport)?,
            clip: self.stack.target(state.clip)?,
            _charge: charge,
        }));
        Ok(())
    }
}

#[com_object(interfaces(IRenderView), ownership = exclusive)]
/// Client facade cannot keep a session, domain, native renderer or command buffer alive.
/// Its destruction refunds only the shell; the host session owns and judges unfinished work.
struct View<B: crate::gpu::native::NativeResources> {
    /// Private lease identity cannot be rebound or forged through a public number.
    core: Weak<SessionCore<B>>,
    /// Small retained value charge survives expiry without retaining device authority.
    _shell: ViewCharge,
}
impl<B: crate::gpu::native::NativeResources> View<B> {
    /// Detaches work, validates and reinstalls only if host expiry did not supersede the call.
    fn mutate<T>(&mut self, call: impl FnOnce(&SessionCore<B>, &mut Recorder<B>) -> Result<T, NcStatus>) -> Result<T, NcStatus> {
        let core = self.core.upgrade().ok_or(RENDER_E_EXPIRED)?;
        core.check()?;
        let previous = std::mem::replace(&mut *core.state.borrow_mut(), State::Mutating);
        let mut recorder = match previous {
            State::Recording(recorder) => recorder,
            other => {
                let status = match &other {
                    State::Rejected(status) => *status,
                    State::Expired => RENDER_E_EXPIRED,
                    State::Mutating => RENDER_E_BUSY,
                    _ => RENDER_E_STATE,
                };
                *core.state.borrow_mut() = other;
                return Err(status);
            }
        };
        let result = call(&core, &mut recorder);
        let live = core.check();
        let mut state = core.state.borrow_mut();
        if !matches!(*state, State::Mutating) || live.is_err() {
            let previous = std::mem::replace(&mut *state, State::Expired);
            drop(state);
            drop(previous);
            drop(recorder);
            return Err(live.err().unwrap_or(RENDER_E_EXPIRED));
        }
        match result {
            Err(status) => {
                *state = State::Rejected(status);
                drop(state);
                drop(recorder);
                Err(status)
            }
            Ok(value) => {
                drop(state);
                let next = if recorder.sealed {
                    State::Committed(Rc::new(recorder.into_block(core.domain.clone())))
                } else {
                    State::Recording(recorder)
                };
                *core.state.borrow_mut() = next;
                Ok(value)
            }
        }
    }
}
impl<B: crate::gpu::native::NativeResources> IRenderView for View<B> {
    /// Information is meaningful only for this still-live invocation.
    fn info(&self) -> Result<RenderViewInfo, NcStatus> {
        let core = self.core.upgrade().ok_or(RENDER_E_EXPIRED)?;
        core.check()?;
        match &*core.state.try_borrow().map_err(|_| RENDER_E_BUSY)? {
            State::Recording(recorder) => Ok(recorder.stack.info()),
            State::Committed(_) => Ok(RenderViewInfo {
                client: core.root.client,
                viewport: Rect {
                    x: 0,
                    y: 0,
                    width: core.root.client.width,
                    height: core.root.client.height,
                },
                clip: Rect {
                    x: 0,
                    y: 0,
                    width: core.root.client.width,
                    height: core.root.client.height,
                },
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: core.root.client.width,
                    height: core.root.client.height,
                },
            }),
            State::Rejected(status) => Err(*status),
            State::Mutating => Err(RENDER_E_BUSY),
            State::Cancelled => Err(RENDER_E_STATE),
            State::Expired => Err(RENDER_E_EXPIRED),
        }
    }
    /// View mapping is constrained independently from clipped visibility.
    fn push_view(&mut self, local: Rect) -> Result<RenderViewInfo, NcStatus> {
        self.mutate(|_, recorder| recorder.stack.push_view(local))
    }
    /// Broad/negative-origin clips intersect host authority using checked wide edges.
    fn push_clip(&mut self, local: Rect) -> Result<(), NcStatus> {
        self.mutate(|_, recorder| recorder.stack.push_clip(local))
    }
    /// Root underflow rejects the view before any native operation.
    fn pop(&mut self) -> Result<(), NcStatus> {
        self.mutate(|_, recorder| recorder.stack.pop())
    }
    /// Retains bindings, commands, attachment depth and all attempted-work charges.
    fn reset(&mut self) -> Result<(), NcStatus> {
        self.mutate(|_, recorder| {
            recorder.stack.reset();
            Ok(())
        })
    }
    /// Binding checks liveness/domain now and observes it again at each consuming draw.
    fn bind_pipeline(&mut self, pipeline: ComRef<'_, IGpuPipelineAbi>) -> Result<(), NcStatus> {
        self.mutate(|core, recorder| {
            let observer = resource::observe(&core.domain.device.inner.pipelines, pipeline.as_raw().object as usize)?;
            observer.with(|pipeline| same_domain(&core.domain, &pipeline.domain))??;
            recorder.pipeline = Some(observer);
            Ok(())
        })
    }
    /// Unused bindings do not become retained owners or implicit shader inputs.
    fn bind_vertex_buffer(&mut self, slot: u32, buffer: ComRef<'_, IGpuBufferAbi>, offset: u64) -> Result<(), NcStatus> {
        self.mutate(|core, recorder| {
            if slot >= 8 || !offset.is_multiple_of(4) {
                return Err(NC_E_INVALID_ARG);
            }
            let object = resource::observe(&core.domain.device.inner.buffers, buffer.as_raw().object as usize)?;
            object.with(|buffer| {
                same_domain(&core.domain, &buffer.domain)?;
                let current = buffer.linear()?;
                if current.desc.role() != GpuBufferRole::Vertex || offset > current.bytes.len() as u64 {
                    return Err(NC_E_INVALID_ARG);
                }
                Ok(())
            })??;
            let next = BoundVertex { slot, object, offset };
            if let Some(bound) = recorder.vertices.iter_mut().find(|bound| bound.slot == slot) {
                *bound = next;
            } else {
                recorder.vertices.push(next);
            }
            Ok(())
        })
    }
    /// Native index role and element alignment are fixed before any draw can inspect data.
    fn bind_index_buffer(&mut self, buffer: ComRef<'_, IGpuBufferAbi>, format: GpuIndexFormat, offset: u64) -> Result<(), NcStatus> {
        self.mutate(|core, recorder| {
            if !offset.is_multiple_of(index_size(format)?) {
                return Err(NC_E_INVALID_ARG);
            }
            let object = resource::observe(&core.domain.device.inner.buffers, buffer.as_raw().object as usize)?;
            object.with(|buffer| {
                same_domain(&core.domain, &buffer.domain)?;
                let current = buffer.linear()?;
                if current.desc.role() != GpuBufferRole::Index || offset > current.bytes.len() as u64 {
                    return Err(NC_E_INVALID_ARG);
                }
                Ok(())
            })??;
            recorder.index = Some(BoundIndex { object, format, offset });
            Ok(())
        })
    }
    /// Sampling observes two distinct same-domain resources without retaining their public owners.
    fn bind_texture(
        &mut self,
        stage: GpuShaderStage,
        slot: u32,
        texture: ComRef<'_, IGpuBufferAbi>,
        sampler: ComRef<'_, IGpuSamplerAbi>,
    ) -> Result<(), NcStatus> {
        self.mutate(|core, recorder| {
            stage_slot(stage, slot)?;
            let texture = resource::observe(&core.domain.device.inner.buffers, texture.as_raw().object as usize)?;
            let sampler = resource::observe(&core.domain.device.inner.samplers, sampler.as_raw().object as usize)?;
            texture.with(|texture| {
                same_domain(&core.domain, &texture.domain)?;
                texture.texture().map(|_| ())
            })??;
            sampler.with(|sampler| same_domain(&core.domain, &sampler.domain))??;
            let next = BoundTexture { stage, slot, texture, sampler };
            if let Some(bound) = recorder.textures.iter_mut().find(|bound| bound.stage == stage && bound.slot == slot) {
                *bound = next;
            } else {
                recorder.textures.push(next);
            }
            Ok(())
        })
    }
    /// Weak uniform binding is resolved at each draw; mapped resources cannot be captured.
    fn bind_uniform_buffer(&mut self, stage: GpuShaderStage, slot: u32, buffer: ComRef<'_, IGpuBufferAbi>, offset: u64, byte_len: u64) -> Result<(), NcStatus> {
        self.mutate(|core, recorder| {
            stage_slot(stage, slot)?;
            if byte_len == 0 || byte_len > 16 * 1024 || !offset.is_multiple_of(256) {
                return Err(NC_E_INVALID_ARG);
            }
            let object = resource::observe(&core.domain.device.inner.buffers, buffer.as_raw().object as usize)?;
            object.with(|buffer| {
                same_domain(&core.domain, &buffer.domain)?;
                let current = buffer.linear()?;
                if current.desc.role() != GpuBufferRole::Uniform || offset.checked_add(byte_len).filter(|end| *end <= current.bytes.len() as u64).is_none() {
                    return Err(NC_E_INVALID_ARG);
                }
                Ok(())
            })??;
            let next = BoundUniform {
                stage,
                slot,
                object,
                offset: offset as usize,
                len: byte_len as usize,
            };
            if let Some(bound) = recorder.uniforms.iter_mut().find(|bound| bound.stage == stage && bound.slot == slot) {
                *bound = next;
            } else {
                recorder.uniforms.push(next);
            }
            Ok(())
        })
    }
    /// Captures versions at this ordered call, rather than later whole-frame preparation.
    fn draw(&mut self, vertices: u32, instances: u32, first_vertex: u32) -> Result<(), NcStatus> {
        self.mutate(|core, recorder| {
            recorder.draw(
                core,
                DrawRange::Vertices {
                    first: first_vertex,
                    count: vertices,
                    instances,
                },
            )
        })
    }
    /// Validates selected index values against every independently bound attribute stream.
    fn draw_indexed(&mut self, indices: u32, instances: u32, first_index: u32) -> Result<(), NcStatus> {
        self.mutate(|core, recorder| {
            recorder.draw(
                core,
                DrawRange::Indices {
                    first: first_index,
                    count: indices,
                    instances,
                },
            )
        })
    }
    /// Checks explicit color-clear permission independently of the pipeline's blend/write masks.
    /// Attempted work remains charged on denial, which rejects the session without appending a command.
    fn clear_color(&mut self, rgba: u32) -> Result<(), NcStatus> {
        self.mutate(|core, recorder| {
            core.check()?.charge(&core.domain.account, 3)?;
            if !core.domain.permissions.clear_color {
                return Err(RENDER_E_PERMISSION);
            }
            let charge = CommandCharge {
                _charge: core.domain.account.pending(std::mem::size_of::<Command<B>>())?,
            };
            recorder.commands.try_reserve_exact(1).map_err(|_| NC_E_OUT_OF_MEMORY)?;
            recorder.commands.push(Command::ClearColor {
                rgba,
                clip: recorder.stack.target(recorder.stack.current().clip)?,
                charge,
            });
            Ok(())
        })
    }
    /// Explicit normalized values are preserved; only automatic initialization is constant one.
    /// Clear permission is separate from depth testing/writing. Denial rejects the session, while
    /// allowed calls still validate finite `[0, 1]` values and remain within the effective view clip.
    fn clear_depth(&mut self, depth: f32) -> Result<(), NcStatus> {
        self.mutate(|core, recorder| {
            core.check()?.charge(&core.domain.account, 3)?;
            if !core.domain.permissions.clear_depth {
                return Err(RENDER_E_PERMISSION);
            }
            if !depth.is_finite() || !(0.0..=1.0).contains(&depth) {
                return Err(NC_E_INVALID_ARG);
            }
            let charge = CommandCharge {
                _charge: core.domain.account.pending(std::mem::size_of::<Command<B>>())?,
            };
            recorder.commands.try_reserve_exact(1).map_err(|_| NC_E_OUT_OF_MEMORY)?;
            recorder.commands.push(Command::ClearDepth {
                depth,
                clip: recorder.stack.target(recorder.stack.current().clip)?,
                charge,
            });
            Ok(())
        })
    }
    /// Sealing does not submit, present, rebind attachments or change the host's reserved submission order.
    fn finish(&mut self) -> Result<(), NcStatus> {
        self.mutate(|_, recorder| {
            if recorder.stack.depth() != 1 {
                return Err(RENDER_E_STATE);
            }
            recorder.sealed = true;
            Ok(())
        })
    }
    /// Cancels healthy pending work; an earlier rejection or commit is preserved.
    fn cancel(&mut self) -> Result<(), NcStatus> {
        if let Some(core) = self.core.upgrade() {
            core.cancel();
        }
        Ok(())
    }
}
/// Exact admitted unsigned element width; unknown open enum values never reach a driver.
fn index_size(format: GpuIndexFormat) -> Result<u64, NcStatus> {
    match format {
        GpuIndexFormat::U16 => Ok(2),
        GpuIndexFormat::U32 => Ok(4),
        _ => Err(NC_E_INVALID_ARG),
    }
}

/// Uniform and sampled slots share finite stage-local namespaces.
fn stage_slot(stage: GpuShaderStage, slot: u32) -> Result<(), NcStatus> {
    if !matches!(stage, GpuShaderStage::Vertex | GpuShaderStage::Fragment) || slot >= 4 {
        Err(NC_E_INVALID_ARG)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;

impl<B: crate::gpu::native::NativeResources> Block<B> {
    /// Copies the admitted root; later caller envelopes cannot change its bounds.
    pub fn root(&self) -> RenderViewRoot {
        self.root
    }
    /// Borrows recorded operations without exposing replacement or mutable access.
    pub fn commands(&self) -> &[Command<B>] {
        &self.commands
    }
}
