//! Web Audio ownership, bounded worklet messages and asynchronous permission/control completion.
use super::*;
use js_sys::{Array, Float32Array, Object, Reflect};
use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::{Rc, Weak},
};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    AudioContext, AudioContextOptions, AudioContextState, AudioWorkletNode, AudioWorkletNodeOptions, MediaStream, MediaStreamAudioSourceNode,
    MediaStreamConstraints, MediaStreamTrack, MessageEvent, MessagePort,
};

/// Host-to-worklet tags are supplied to the bundled processor, never duplicated as JS constants.
#[derive(Clone, Copy)]
#[repr(u32)]
enum Command {
    Write = 1,
    Release = 2,
    Reset = 3,
    Stats = 4,
}
/// Worklet-to-host tags use the same supplied protocol table.
#[derive(Clone, Copy)]
#[repr(u32)]
enum EventCode {
    Consumed = 11,
    Capture = 12,
    Snapshot = 13,
    Reset = 14,
}

/// Exact local error ownership; JS exceptions are copied before any graph cleanup.
fn error(value: JsValue) -> AudioError {
    let message = value
        .as_string()
        .or_else(|| Reflect::get(&value, &"message".into()).ok()?.as_string())
        .unwrap_or_else(|| "browser audio operation failed".to_owned());
    AudioError::new(AUDIO_E_BACKEND, message)
}

/// The single revocation obligation for a registered worklet module URL.
struct WorkletUrl(String);

impl Drop for WorkletUrl {
    fn drop(&mut self) {
        let _ = web_sys::Url::revoke_object_url(&self.0);
    }
}

/// One typed cleanup record shared only while an opening future is outstanding.
struct OpeningResources {
    /// An allocated context must close immediately on request cancellation, even during addModule.
    context: Option<AudioContext>,
    /// Late microphone permission must stop every track even if the request was already dropped.
    media: Option<MediaStream>,
    /// Cancelling revokes the module URL even if its load promise has not settled.
    worklet_url: Option<WorkletUrl>,
}
impl OpeningResources {
    /// Release physical resources without borrowing any request or completion state across JS calls.
    fn close(self) {
        drop(self.worklet_url);
        stop_tracks(self.media.as_ref());
        if let Some(context) = self.context {
            close_context(&context);
        }
    }
}

/// Pending construction owns cleanup authority; its future holds only a weak completion destination.
pub(super) struct Pending {
    /// RefCell is required only for a browser event completion arriving between exclusive request polls.
    completion: Rc<RefCell<Option<Result<Stream, AudioError>>>>,
    /// Cancellation can close resources while the browser is still awaiting an asynchronous promise.
    resources: Rc<RefCell<Option<OpeningResources>>>,
}
impl Pending {
    /// Move a completed stream/failure out without retaining a mailbox borrow during publication.
    pub(super) fn poll(&mut self) -> Option<Result<Stream, AudioError>> {
        self.completion.borrow_mut().take()
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        let resources = self.resources.borrow_mut().take();
        if let Some(resources) = resources {
            resources.close();
        }
    }
}

/// Start asynchronous construction. No native state or COM object is retained by the future.
pub(super) fn open(direction: AudioDirection, config: AudioConfig) -> RequestState {
    let completion = Rc::new(RefCell::new(None));
    let weak = Rc::downgrade(&completion);
    let resources = Rc::new(RefCell::new(Some(OpeningResources {
        context: None,
        media: None,
        worklet_url: None,
    })));
    let cleanup = resources.clone();
    spawn_local(async move {
        let result = create(direction, config, &weak, &cleanup).await;
        let remaining = cleanup.borrow_mut().take();
        if result.is_err()
            && let Some(remaining) = remaining
        {
            remaining.close();
        }
        // Success transferred physical ownership into Stream; cleanup clones are disarmed.
        if let Some(completion) = weak.upgrade() {
            *completion.borrow_mut() = Some(result);
        }
        // A result with no recipient is dropped here, immediately closing any completed graph.
    });
    RequestState::Pending(Pending { completion, resources })
}

/// Browser stream authority. Only Open can still stop tracks or initiate context close.
enum GraphState {
    /// One open graph, with microphone ownership only for capture.
    Open(Option<MediaStream>),
    /// The first terminal failure, whose transition took physical cleanup authority.
    Failed(AudioError),
    /// The stream was explicitly closed or dropped and ignores later completions.
    Closed,
}

/// Main-realm mailbox shared with browser callbacks; PCM never exceeds fixed capacity.
struct Mailbox {
    /// Preallocated delivered capture samples; output PCM stays on the worklet thread.
    capture: VecDeque<f32>,
    /// Playback frames include transferred messages still in flight until consumption is acknowledged.
    playback: u32,
    /// Queue resets use generations so late messages cannot repopulate a cleared queue.
    epoch: u32,
    /// Latest observed monotonically increasing loss counters.
    dropped: u64,
    /// Latest observed playback starvation frames.
    silent: u64,
    /// At most one stats request is in flight, bounding idle status-message traffic.
    stats_pending: bool,
    /// A successful resume/suspend is adopted by the exclusive stream owner.
    control: Option<AudioState>,
    /// Reset acknowledgement permits a new generation to start only after older messages were delivered.
    reset: bool,
    /// Terminal state and physical cleanup authority for every callback and close path.
    graph: GraphState,
}

/// The stream owns this graph. Callbacks use weak access and cannot prolong its lifetime.
struct Shared {
    /// The context handle remains accessible to live operations and the first terminal transition.
    context: AudioContext,
    /// Browser callbacks and the exclusive Rust stream exchange state through short borrows.
    mailbox: RefCell<Mailbox>,
}
impl Shared {
    /// Claim the open graph once, then stop processing without a mailbox borrow across browser calls.
    fn fail(&self, failure: AudioError) {
        let media = {
            let mut mailbox = self.mailbox.borrow_mut();
            let GraphState::Open(media) = &mut mailbox.graph else { return };
            let media = media.take();
            mailbox.graph = GraphState::Failed(failure);
            mailbox.control = None;
            mailbox.reset = false;
            media
        };
        self.stop(media.as_ref());
    }

    /// Tear down physical audio; only the owner that took the open state can call this.
    fn stop(&self, media: Option<&MediaStream>) {
        stop_tracks(media);
        close_context(&self.context);
    }
}

/// Decoded worklet event; arbitrary JS data never escapes the wire boundary.
enum Message {
    /// Playback credit is returned only for frames actually consumed.
    Consumed(u32, u64),
    /// Transferred finite PCM and latest capture overrun count.
    Capture(Vec<f32>, u64),
    /// Loss counters from one explicitly requested snapshot.
    Snapshot(u64, u64),
    /// All older-generation messages precede this acknowledgement on the same port.
    Reset,
}

/// Decode nonnegative exact JS integers, bounded by the largest exactly representable count.
fn integer(value: JsValue) -> Result<u64, AudioError> {
    let value = value.as_f64().ok_or_else(|| error("invalid PCM integer".into()))?;
    if value.is_finite() && (0. ..=9_007_199_254_740_991.).contains(&value) && value.fract() == 0. {
        Ok(value as u64)
    } else {
        Err(error("invalid PCM integer".into()))
    }
}

/// Validate the complete message shape and current generation before accessing a transferred sample array.
fn decode(value: JsValue, epoch: u32, config: AudioConfig) -> Result<Option<Message>, AudioError> {
    if !Array::is_array(&value) {
        return Err(error("invalid PCM message".into()));
    }
    let array = Array::from(&value);
    if array.length() != 3 {
        return Err(error("invalid PCM message length".into()));
    }
    let tag = integer(array.get(0))?;
    let generation = integer(array.get(1))?;
    if generation > u32::MAX as u64 {
        return Err(error("invalid PCM generation".into()));
    }
    if generation as u32 != epoch {
        return Ok(None);
    }
    if tag == EventCode::Reset as u64 {
        return Ok(Some(Message::Reset));
    }
    let payload = array.get(2);
    if !Array::is_array(&payload) {
        return Err(error("invalid PCM payload".into()));
    }
    let payload = Array::from(&payload);
    if payload.length() != 2 {
        return Err(error("invalid PCM payload length".into()));
    }
    if tag == EventCode::Consumed as u64 {
        let count = integer(payload.get(0))?;
        if count > config.capacity_frames as u64 {
            return Err(error("invalid PCM consumption".into()));
        }
        Ok(Some(Message::Consumed(count as u32, integer(payload.get(1))?)))
    } else if tag == EventCode::Capture as u64 {
        let samples = payload.get(0).dyn_into::<Float32Array>().map_err(error)?;
        if samples.length() == 0 || samples.length() % config.channels.count() != 0 || samples.length() / config.channels.count() > config.capacity_frames {
            return Err(error("invalid PCM capture extent".into()));
        }
        // Copy and validate the complete packet before borrowing the main-realm mailbox again.
        let samples = samples.to_vec();
        if samples.iter().any(|sample| !sample.is_finite() || !(-1.0..=1.0).contains(sample)) {
            return Err(error("invalid normalized capture PCM".into()));
        }
        Ok(Some(Message::Capture(samples, integer(payload.get(1))?)))
    } else if tag == EventCode::Snapshot as u64 {
        Ok(Some(Message::Snapshot(integer(payload.get(0))?, integer(payload.get(1))?)))
    } else {
        Err(error("unknown PCM event".into()))
    }
}

/// Owns one private Web Audio graph and its registered event closures, independent of any canvas/window.
struct Browser {
    /// Immutable application-side PCM configuration.
    config: AudioConfig,
    /// Exclusive stream lifecycle, with explicit asynchronous intermediate phases.
    state: AudioState,
    /// Exactly one private worklet node; capture output is silent.
    node: Option<AudioWorkletNode>,
    /// A capture source is retained until close and never connected directly to speakers.
    source: Option<MediaStreamAudioSourceNode>,
    /// Main-realm event/promise mailbox, with short borrows that never span JS calls.
    shared: Rc<Shared>,
    /// Closure ownership lasts exactly as long as the node's message registration.
    message: Option<Closure<dyn FnMut(MessageEvent)>>,
    /// Processor failure registration is removed before releasing its Rust closure.
    processor_error: Option<Closure<dyn FnMut(web_sys::Event)>>,
}

/// Playback and capture have separate Rust/COM types even though their physical graph controls are identical.
pub(super) struct Playback(Browser);
/// Microphone queue cannot be converted by QI into a playback producer.
pub(super) struct Capture(Browser);

/// Observe browser API availability without creating a context or requesting microphone permission.
pub(super) fn capabilities() -> AudioCapabilities {
    let mut capabilities = AudioCapabilities::portable(AudioBackend::WebAudio);
    let global = js_sys::global();
    let playback = Reflect::get(&global, &"AudioContext".into()).is_ok_and(|value| value.is_function())
        && Reflect::get(&global, &"AudioWorkletNode".into()).is_ok_and(|value| value.is_function());
    let capture = playback && web_sys::window().is_some_and(|window| window.navigator().media_devices().is_ok());
    capabilities.playback = if playback {
        AudioSupport::Available {
            devices: AudioDeviceCount::Unknown,
            access: AudioAccess::BrowserUserActivation,
        }
    } else {
        AudioSupport::Unavailable
    };
    capabilities.capture = if capture {
        AudioSupport::Available {
            devices: AudioDeviceCount::Unknown,
            access: AudioAccess::BrowserMicrophone,
        }
    } else {
        AudioSupport::Unavailable
    };
    capabilities
}

/// Stop tracks synchronously, including after late permission resolution or opening cancellation.
fn stop_tracks(media: Option<&MediaStream>) {
    if let Some(media) = media {
        for track in media.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                track.stop();
            }
        }
    }
}
/// Drain close's promise so cleanup cannot create an unhandled rejection in the application.
fn close_context(context: &AudioContext) {
    if context.state() != AudioContextState::Closed
        && let Ok(promise) = context.close()
    {
        spawn_local(async move {
            let _ = JsFuture::from(promise).await;
        });
    }
}
/// Build a message with its explicit wire tag and generation, without shell/script interpolation.
fn command(tag: Command, epoch: u32, value: &JsValue) -> Array {
    Array::of3(&JsValue::from(tag as u32), &JsValue::from(epoch), value)
}

/// Create permission and worklet resources transactionally. Cancellation owns cleanup across every await.
async fn create(
    direction: AudioDirection,
    config: AudioConfig,
    destination: &Weak<RefCell<Option<Result<Stream, AudioError>>>>,
    cleanup: &Rc<RefCell<Option<OpeningResources>>>,
) -> Result<Stream, AudioError> {
    if destination.upgrade().is_none() {
        return Err(AUDIO_E_STATE.into());
    }
    let media = if direction == AudioDirection::Capture {
        let constraints = MediaStreamConstraints::new();
        constraints.set_audio_bool(true);
        constraints.set_video_bool(false);
        let devices = web_sys::window()
            .ok_or_else(|| error("no browser window".into()))?
            .navigator()
            .media_devices()
            .map_err(error)?;
        let value = JsFuture::from(devices.get_user_media_with_constraints(&constraints).map_err(error)?)
            .await
            .map_err(error)?;
        let media = value.dyn_into::<MediaStream>().map_err(error)?;
        for track in media.get_audio_tracks().iter() {
            if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                track.set_enabled(false);
            }
        }
        if destination.upgrade().is_none() {
            stop_tracks(Some(&media));
            return Err(AUDIO_E_STATE.into());
        }
        if let Some(resources) = cleanup.borrow_mut().as_mut() {
            resources.media = Some(media.clone());
        }
        Some(media)
    } else {
        None
    };
    let options = AudioContextOptions::new();
    options.set_sample_rate(config.sample_rate as f32);
    let context = AudioContext::new_with_context_options(&options).map_err(error)?;
    if let Some(resources) = cleanup.borrow_mut().as_mut() {
        resources.context = Some(context.clone());
    }
    if context.sample_rate() != config.sample_rate as f32 {
        return Err(error("browser changed the requested PCM rate".into()));
    }
    let mut samples = VecDeque::new();
    samples
        .try_reserve_exact(config.capacity_frames as usize * config.channels.count() as usize)
        .map_err(|failure| AudioError::new(AUDIO_E_LIMIT, failure.to_string()))?;
    let shared = Rc::new(Shared {
        context,
        mailbox: RefCell::new(Mailbox {
            capture: samples,
            playback: 0,
            epoch: 0,
            dropped: 0,
            silent: 0,
            stats_pending: false,
            control: None,
            reset: false,
            graph: GraphState::Open(media.clone()),
        }),
    });
    let mut browser = Browser {
        config,
        state: AudioState::Paused,
        node: None,
        source: None,
        shared,
        message: None,
        processor_error: None,
    };
    let context = &browser.shared.context;
    JsFuture::from(context.suspend().map_err(error)?).await.map_err(error)?;
    if destination.upgrade().is_none() {
        return Err(AUDIO_E_STATE.into());
    }
    let worklet = context.audio_worklet().map_err(error)?;
    let parts = Array::of1(&JsValue::from_str(include_str!("processor.js")));
    let properties = web_sys::BlobPropertyBag::new();
    properties.set_type("text/javascript");
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &properties).map_err(error)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(error)?;
    let registration = WorkletUrl(url.clone());
    {
        let mut pending = cleanup.borrow_mut();
        let Some(resources) = pending.as_mut() else {
            drop(pending);
            return Err(AUDIO_E_STATE.into());
        };
        // Construction registers exactly one module. The borrow ends before invoking JavaScript.
        resources.worklet_url = Some(registration);
    }
    let loaded = match worklet.add_module(&url) {
        Ok(promise) => JsFuture::from(promise).await.map(|_| ()).map_err(error),
        Err(value) => Err(error(value)),
    };
    // Cancellation may have already taken the registration. Drop a remaining owner outside the borrow.
    let registration = cleanup.borrow_mut().as_mut().and_then(|resources| resources.worklet_url.take());
    drop(registration);
    loaded?;
    if destination.upgrade().is_none() {
        return Err(AUDIO_E_STATE.into());
    }
    let options = AudioWorkletNodeOptions::new();
    options.set_number_of_inputs(u32::from(direction == AudioDirection::Capture));
    options.set_number_of_outputs(1);
    options.set_channel_count(config.channels.count());
    options.set_channel_count_mode(web_sys::ChannelCountMode::Explicit);
    options.set_output_channel_count(&Array::of1(&JsValue::from(config.channels.count())));
    let parameters = Object::new();
    Reflect::set(&parameters, &"channels".into(), &config.channels.count().into()).map_err(error)?;
    Reflect::set(&parameters, &"capacity".into(), &config.capacity_frames.into()).map_err(error)?;
    Reflect::set(&parameters, &"capture".into(), &(direction == AudioDirection::Capture).into()).map_err(error)?;
    let protocol = Array::new();
    for tag in [
        Command::Write as u32,
        Command::Release as u32,
        Command::Reset as u32,
        Command::Stats as u32,
        EventCode::Consumed as u32,
        EventCode::Capture as u32,
        EventCode::Snapshot as u32,
        EventCode::Reset as u32,
    ] {
        protocol.push(&tag.into());
    }
    Reflect::set(&parameters, &"protocol".into(), &protocol).map_err(error)?;
    options.set_processor_options(Some(&parameters));
    let node = AudioWorkletNode::new_with_options(context, "neocogi-pcm-v1", &options).map_err(error)?;
    browser.node = Some(node.clone());
    let weak = Rc::downgrade(&browser.shared);
    let message = Closure::new(move |event: MessageEvent| {
        let Some(shared) = weak.upgrade() else {
            return;
        };
        let epoch = {
            let mailbox = shared.mailbox.borrow();
            if !matches!(mailbox.graph, GraphState::Open(_)) {
                return;
            }
            mailbox.epoch
        };
        let decoded = decode(event.data(), epoch, config);
        let outcome = (|| -> Result<(), AudioError> {
            let Some(message) = decoded? else {
                return Ok(());
            };
            let mut shared = shared.mailbox.borrow_mut();
            if !matches!(shared.graph, GraphState::Open(_)) || shared.epoch != epoch {
                return Ok(());
            }
            match message {
                Message::Consumed(frames, silent) => {
                    if frames > shared.playback {
                        return Err(error("PCM playback credit exceeded admission".into()));
                    }
                    shared.playback -= frames;
                    shared.silent = shared.silent.max(silent);
                }
                Message::Capture(samples, dropped) => {
                    if shared.capture.len() + samples.len() > config.capacity_frames as usize * config.channels.count() as usize {
                        return Err(error("PCM capture credit exceeded capacity".into()));
                    }
                    shared.capture.extend(samples);
                    shared.dropped = shared.dropped.max(dropped);
                }
                Message::Snapshot(dropped, silent) => {
                    shared.dropped = shared.dropped.max(dropped);
                    shared.silent = shared.silent.max(silent);
                    shared.stats_pending = false;
                }
                Message::Reset => shared.reset = true,
            }
            Ok(())
        })();
        if let Err(failure) = outcome {
            shared.fail(failure);
        }
    });
    let port = node.port().map_err(error)?;
    port.set_onmessage(Some(message.as_ref().unchecked_ref()));
    port.start();
    browser.message = Some(message);
    let weak = Rc::downgrade(&browser.shared);
    let processor_error = Closure::new(move |_: web_sys::Event| {
        if let Some(shared) = weak.upgrade() {
            shared.fail(error("audio worklet processing failed".into()));
        }
    });
    node.set_onprocessorerror(Some(processor_error.as_ref().unchecked_ref()));
    // Deserialization failures also terminate the graph; they cannot silently stall queue credits.
    port.set_onmessageerror(Some(processor_error.as_ref().unchecked_ref()));
    browser.processor_error = Some(processor_error);
    if let Some(media) = &media {
        let source = context.create_media_stream_source(media).map_err(error)?;
        source.connect_with_audio_node(&node).map_err(error)?;
        browser.source = Some(source);
    }
    node.connect_with_audio_node(&context.destination()).map_err(error)?;
    Ok(if direction == AudioDirection::Playback {
        Stream::Playback(Playback(browser))
    } else {
        Stream::Capture(Capture(browser))
    })
}

impl Browser {
    /// Refresh asynchronous control and terminal failure before admitting another operation.
    fn refresh(&mut self) {
        let mut mailbox = self.shared.mailbox.borrow_mut();
        match &mailbox.graph {
            GraphState::Failed(failure) => self.state = AudioState::Failed(failure.status.code()),
            GraphState::Closed => self.state = AudioState::Closed,
            GraphState::Open(_) => {
                if let Some(state) = mailbox.control.take() {
                    self.state = state;
                }
                if std::mem::take(&mut mailbox.reset) && self.state == AudioState::Clearing {
                    self.state = AudioState::Paused;
                }
            }
        }
    }
    /// Writes/reads are available only in stable open phases; terminal and pending phases remain distinct.
    fn active(&mut self) -> Result<(), NcStatus> {
        self.refresh();
        match self.state {
            AudioState::Paused | AudioState::Running => Ok(()),
            AudioState::Starting | AudioState::Pausing | AudioState::Clearing => Err(AUDIO_E_PENDING),
            _ => Err(AUDIO_E_STATE),
        }
    }
    /// Acquire the private communication port without exposing it through any public capability.
    fn port(&self) -> Result<MessagePort, NcStatus> {
        self.node.as_ref().ok_or(AUDIO_E_STATE)?.port().map_err(|_| AUDIO_E_BACKEND)
    }
    /// Post a control message, latching and cleaning up any backend transmission failure.
    fn post(&mut self, tag: Command, payload: &JsValue) -> Result<(), NcStatus> {
        let epoch = self.shared.mailbox.borrow().epoch;
        self.port()?.post_message(&command(tag, epoch, payload)).map_err(|value| {
            let failure = error(value);
            let status = failure.status;
            self.shared.fail(failure);
            self.refresh();
            status
        })
    }
    /// Resume/suspend are serialized. Rejection retires the graph, while success is adopted on observation.
    fn control(&mut self, running: bool) -> Result<(), NcStatus> {
        self.active()?;
        let target = if running { AudioState::Running } else { AudioState::Paused };
        if self.state == target {
            return Ok(());
        }
        let context = &self.shared.context;
        let promise = if running { context.resume() } else { context.suspend() }.map_err(|value| {
            let failure = error(value);
            let status = failure.status;
            self.shared.fail(failure);
            self.refresh();
            status
        })?;
        let media = match &self.shared.mailbox.borrow().graph {
            GraphState::Open(media) => media.clone(),
            GraphState::Failed(_) | GraphState::Closed => None,
        };
        if let Some(media) = media {
            for track in media.get_audio_tracks().iter() {
                if let Ok(track) = track.dyn_into::<MediaStreamTrack>() {
                    track.set_enabled(running);
                }
            }
        }
        self.state = if running { AudioState::Starting } else { AudioState::Pausing };
        let weak = Rc::downgrade(&self.shared);
        spawn_local(async move {
            let result = JsFuture::from(promise).await;
            let Some(shared) = weak.upgrade() else { return };
            match result {
                Ok(_) => {
                    let mut mailbox = shared.mailbox.borrow_mut();
                    if matches!(mailbox.graph, GraphState::Open(_)) {
                        mailbox.control = Some(target);
                    }
                }
                Err(value) => shared.fail(error(value)),
            }
        });
        Ok(())
    }
    /// Close releases microphone authority synchronously; asynchronous context close is drained separately.
    fn close(&mut self) {
        if self.state == AudioState::Closed {
            return;
        }
        let previous = {
            let mut mailbox = self.shared.mailbox.borrow_mut();
            mailbox.capture.clear();
            mailbox.playback = 0;
            mailbox.control = None;
            mailbox.reset = false;
            std::mem::replace(&mut mailbox.graph, GraphState::Closed)
        };
        if let Some(node) = self.node.take() {
            if let Ok(port) = node.port() {
                port.set_onmessage(None);
                port.set_onmessageerror(None);
                port.close();
            }
            node.set_onprocessorerror(None);
            let _ = node.disconnect();
        }
        if let Some(source) = self.source.take() {
            let _ = source.disconnect();
        }
        if let GraphState::Open(media) = previous {
            self.shared.stop(media.as_ref());
        }
        self.message = None;
        self.processor_error = None;
        self.state = AudioState::Closed;
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        self.close();
    }
}

/// Only identical graph controls are generated; direction-specific PCM and credits remain explicit below.
macro_rules! controls {
    ($owner:ty, $capture:expr) => {
        impl Driver for $owner {
            fn config(&self) -> AudioConfig {
                self.0.config
            }
            fn info(&mut self) -> Result<AudioInfo, NcStatus> {
                self.0.refresh();
                let query = {
                    let shared = self.0.shared.mailbox.borrow();
                    !shared.stats_pending && matches!(shared.graph, GraphState::Open(_)) && self.0.state != AudioState::Clearing
                };
                if query {
                    self.0.post(Command::Stats, &JsValue::NULL)?;
                    self.0.shared.mailbox.borrow_mut().stats_pending = true;
                }
                let shared = self.0.shared.mailbox.borrow();
                Ok(AudioInfo {
                    config: self.0.config,
                    state: self.0.state,
                    queue: if $capture {
                        AudioQueueInfo::Capture {
                            queued_frames: shared.capture.len() as u32 / self.0.config.channels.count(),
                            dropped_frames: shared.dropped,
                        }
                    } else {
                        AudioQueueInfo::Playback {
                            queued_frames: shared.playback,
                            silent_frames: shared.silent,
                        }
                    },
                })
            }
            fn start(&mut self) -> Result<(), NcStatus> {
                self.0.control(true)
            }
            fn pause(&mut self) -> Result<(), NcStatus> {
                self.0.control(false)
            }
            fn clear(&mut self) -> Result<(), NcStatus> {
                self.0.active()?;
                if self.0.state != AudioState::Paused {
                    return Err(AUDIO_E_STATE);
                }
                let epoch = self.0.shared.mailbox.borrow().epoch.checked_add(1).ok_or(AUDIO_E_LIMIT)?;
                self.0
                    .port()?
                    .post_message(&command(Command::Reset, epoch, &JsValue::NULL))
                    .map_err(|_| AUDIO_E_BACKEND)?;
                {
                    let mut shared = self.0.shared.mailbox.borrow_mut();
                    shared.epoch = epoch;
                    shared.capture.clear();
                    shared.playback = 0;
                    shared.stats_pending = false;
                    shared.reset = false;
                }
                self.0.state = AudioState::Clearing;
                Ok(())
            }
            fn close(&mut self) {
                self.0.close();
            }
            fn failure(&self) -> Option<AudioError> {
                match &self.0.shared.mailbox.borrow().graph {
                    GraphState::Failed(failure) => Some(failure.clone()),
                    GraphState::Open(_) | GraphState::Closed => None,
                }
            }
        }
    };
}
controls!(Playback, false);
controls!(Capture, true);

impl Playback {
    /// Admission counts all outstanding frames, including transferred packets awaiting worklet processing.
    pub(super) fn write(&mut self, samples: &[f32]) -> Result<(), NcStatus> {
        self.0.active()?;
        let frames = samples.len() as u32 / self.0.config.channels.count();
        if frames == 0 {
            return Ok(());
        }
        if self.0.shared.mailbox.borrow().playback + frames > self.0.config.capacity_frames {
            return Err(AUDIO_E_LIMIT);
        }
        let samples = Float32Array::from(samples);
        let epoch = self.0.shared.mailbox.borrow().epoch;
        self.0
            .port()?
            .post_message_with_transferable(&command(Command::Write, epoch, &samples), &Array::of1(&samples.buffer()))
            .map_err(|value| {
                let failure = error(value);
                let status = failure.status;
                self.0.shared.fail(failure);
                self.0.refresh();
                status
            })?;
        self.0.shared.mailbox.borrow_mut().playback += frames;
        Ok(())
    }
}
impl Capture {
    /// Credits return only for complete consumed frames; no output is touched if posting them fails.
    pub(super) fn read(&mut self, output: &mut [f32]) -> Result<u32, NcStatus> {
        self.0.active()?;
        let channels = self.0.config.channels.count() as usize;
        let frames = (self.0.shared.mailbox.borrow().capture.len() / channels).min(output.len() / channels);
        if frames == 0 {
            return Ok(0);
        }
        self.0.post(Command::Release, &JsValue::from(frames as u32))?;
        let mut shared = self.0.shared.mailbox.borrow_mut();
        for sample in &mut output[..frames * channels] {
            *sample = shared.capture.pop_front().expect("counted capture frame");
        }
        Ok(frames as u32)
    }
}

/// Acceptance inspects actual retained browser tracks to prove dropping an untaken ready stream releases them.
#[cfg(feature = "test-support")]
pub(super) async fn verify_drop_cleanup() -> Result<(), JsValue> {
    let RequestState::Pending(mut pending) = open(AudioDirection::Capture, AudioConfig::stereo()) else {
        unreachable!()
    };
    for _ in 0..2000 {
        if let Some(result) = pending.poll() {
            let Stream::Capture(capture) = result.map_err(|failure| JsValue::from_str(&failure.to_string()))? else {
                unreachable!()
            };
            let tracks = match &capture.0.shared.mailbox.borrow().graph {
                GraphState::Open(Some(media)) => media.get_tracks(),
                _ => return Err("capture must own open microphone tracks".into()),
            };
            drop(capture);
            for track in tracks.iter() {
                if track.unchecked_into::<MediaStreamTrack>().ready_state() != web_sys::MediaStreamTrackState::Ended {
                    return Err("dropped capture retained microphone track".into());
                }
            }
            return Ok(());
        }
        super::testing::tick().await?;
    }
    Err("capture drop cleanup timed out".into())
}
