//! Owned operation outcomes at the COM boundary and ordinary Rust error values.
//!
//! Native libraries exchange immutable status/text reports, never Rust String
//! layout or a shared last-error slot. Rust adapters finish reading that same
//! report before returning Err(RenderError); its text then outlives every COM
//! object involved. Creation reports exclusively own unpublished result handles.
use crate::render::abi::*;
use crate::render::platform::{IPlatformWindowAbi, IWindowCreationResult, IWindowCreationResultAbi, IWindowCreationResultVtblFor};
use neocogi_com::{com_object, ComBox, ComPtr, INcStringAbi, INcStringComPtrExt, NcStatus, NcString, NC_E_INVALID_ARG, NC_OK};

/// Maximum UTF-8 length and retained String capacity after failure normalization.
pub const MAX_RENDER_ERROR_BYTES: usize = 4096;

/// One failed operation's status and independently owned explanation.
///
/// Constructors and COM adapters bound text and reject successful failure codes.
/// An application may retain or aggregate these values without keeping a window,
/// driver, plugin report or previous shared diagnostic state alive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderError {
    /// Original failing code, preserved even when diagnostic export itself fails.
    pub status: NcStatus,
    /// UTF-8 explanation copied before the operation returns to its Rust caller.
    pub message: String,
}
impl RenderError {
    /// Normalizes status and bounds retained UTF-8 length/capacity before publication.
    /// Input conversion and invalid-status formatting may allocate before normalization.
    pub fn new(status: NcStatus, message: impl Into<String>) -> Self {
        let mut message = message.into();
        let status = if status.is_err() {
            status
        } else {
            message = format!("invalid successful error status {status}: {message}");
            NC_E_INVALID_ARG
        };
        let message = crate::diagnostic::bounded_message(message, MAX_RENDER_ERROR_BYTES);
        Self { status, message }
    }

    /// Adds the local operation name without consulting any other operation's state.
    pub fn context(self, operation: &str) -> Self {
        Self::new(self.status, format!("{operation}: {}", self.message))
    }
}
impl From<NcStatus> for RenderError {
    /// Transport/admission failures still carry their own status and readable fallback.
    fn from(status: NcStatus) -> Self {
        Self::new(status, status.to_string())
    }
}
impl std::fmt::Display for RenderError {
    /// Keep the machine-readable status and human-readable detail together in logs.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status, self.message)
    }
}
impl std::error::Error for RenderError {
    /// The status is a stable underlying error; no borrowed native error is retained.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.status)
    }
}

/// Reads one owned report, with no query of the renderer or any global error state.
/// Successful reports never read text. If text export fails or exceeds the cap,
/// preserve the original operation code and supply a bounded local explanation.
/// Derived creation clients dereference to this same generated base client.
pub fn check_outcome(report: &IRenderOutcomeClient, operation: &str) -> Result<(), RenderError> {
    let status = NcStatus::from_code(report.status().map_err(|status| RenderError::from(status).context(operation))?);
    if status.is_ok() {
        return Ok(());
    }
    let message = match report.message().and_then(|text| text.to_string_bounded(MAX_RENDER_ERROR_BYTES)) {
        Ok(message) if !message.is_empty() => message,
        Ok(_) => format!("{operation} failed: {status}"),
        Err(detail_status) => format!("{operation} failed: {status}; error text unavailable: {detail_status}"),
    };
    Err(RenderError::new(status, message))
}

/// Rust callers receive either an exclusive shader or the owned failure from that exact call.
impl IRenderResourcesClient {
    /// Compiles and imports a shader, copying failure detail before the report is released.
    /// Raw COM consumers use `create_shader_result` and own its one-shot report.
    pub fn create_shader(
        &self,
        stage: GpuShaderStage,
        code: neocogi_com::ComRef<'_, IGpuDataSourceAbi>,
        entry: neocogi_com::NcCopyFromBufferOffset<'_, u8>,
    ) -> Result<ComBox<IGpuShaderAbi>, RenderError> {
        let mut report = self
            .create_shader_result(stage, code, entry)
            .map_err(|status| RenderError::from(status).context("create shader"))?;
        check_outcome(&report, "create shader")?;
        report.take_shader().map_err(|status| RenderError::from(status).context("take compiled shader"))
    }
}

/// Provider-side exclusive result ownership. Only an explicit successful take
/// consumes the payload; a rejected outcome retains its own immutable failure.
enum Outcome<T> {
    /// Complete unpublished capability, owned until the result is taken or dropped.
    Ready(T),
    /// Normalized owned detail, with no device/renderer/report back-reference.
    Rejected(RenderError),
    /// Terminal state after exactly one successful capability transfer.
    Consumed,
}
impl<T> Outcome<T> {
    /// Validate public error values before exposing a COM success/failure discriminator.
    fn new(result: Result<T, RenderError>) -> Self {
        match result {
            Ok(value) => Self::Ready(value),
            Err(error) => Self::Rejected(RenderError::new(error.status, error.message)),
        }
    }
    /// The report is readable until consumption; failure statuses are successful getter outputs.
    fn status(&self) -> Result<i32, NcStatus> {
        match self {
            Self::Ready(_) => Ok(NC_OK.code()),
            Self::Rejected(error) => Ok(error.status.code()),
            Self::Consumed => Err(RENDER_E_STATE),
        }
    }
    /// Every returned text is an independent immutable snapshot of this operation alone.
    fn message(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        match self {
            Self::Ready(_) => Ok(NcString::from_utf8_str("")),
            Self::Rejected(error) => Ok(NcString::from_utf8_str(&error.message)),
            Self::Consumed => Err(RENDER_E_STATE),
        }
    }
    /// Transfer a ready capability once; invalid takes leave a rejected outcome readable.
    fn take(&mut self) -> Result<T, NcStatus> {
        match std::mem::replace(self, Self::Consumed) {
            Self::Ready(value) => Ok(value),
            Self::Rejected(error) => {
                let status = error.status;
                *self = Self::Rejected(error);
                Err(status)
            }
            Self::Consumed => Err(RENDER_E_STATE),
        }
    }
}

#[com_object(interfaces(IRenderOutcome))]
/// Immutable result for an operation with no success payload.
struct Completion {
    /// Success contains no device; failure contains only bounded owned error data.
    outcome: Outcome<()>,
}
impl IRenderOutcome for Completion {
    /// Exposes only this operation's discriminator.
    fn status(&self) -> Result<i32, NcStatus> {
        self.outcome.status()
    }
    /// Copies only this operation's immutable text.
    fn message(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.outcome.message()
    }
}
/// Publishes an owned completion report; providers may retain/reuse an immutable
/// success report to avoid allocating a new object for every successful frame.
pub fn operation_result(result: Result<(), RenderError>) -> ComPtr<IRenderOutcomeAbi> {
    Completion::from_value(Completion { outcome: Outcome::new(result) })
        .query()
        .expect("completion interface")
}

#[com_object(interfaces(IShaderCreationResult), ownership = exclusive)]
/// Exclusive carrier for one shader or one failure; dropping it drops an untaken shader.
struct ShaderCreation {
    /// Real resource ownership, never a borrowed facade or a numeric lookup key.
    outcome: Outcome<ComBox<IGpuShaderAbi>>,
}
impl IRenderOutcome for ShaderCreation {
    /// Return this result's fixed status while it is unconsumed.
    fn status(&self) -> Result<i32, NcStatus> {
        self.outcome.status()
    }
    /// Return independently owned text from this result only.
    fn message(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.outcome.message()
    }
}
impl IShaderCreationResult for ShaderCreation {
    /// Exclusive access permits ordinary owned mutation and one transfer without a RefCell.
    fn take_shader(&mut self) -> Result<ComBox<IGpuShaderAbi>, NcStatus> {
        self.outcome.take()
    }
}
/// Packages a shader result without discarding either its resource owner or failure detail.
pub fn shader_creation_result(result: Result<ComBox<IGpuShaderAbi>, RenderError>) -> ComBox<IShaderCreationResultAbi> {
    ShaderCreation::into_local_box(ShaderCreation { outcome: Outcome::new(result) })
        .into_interface()
        .unwrap_or_else(|(status, _)| panic!("shader result interface: {status}"))
}

#[com_object(interfaces(ITextureImportResult), ownership = exclusive)]
/// Exclusive carrier for a prepared logical texture import or an owned failure.
struct TextureImport {
    /// Untaken imported resources retain their own normal quota/native ownership.
    outcome: Outcome<ComBox<IGpuBufferAbi>>,
}
impl IRenderOutcome for TextureImport {
    /// Return only this operation's success/failure discriminator.
    fn status(&self) -> Result<i32, NcStatus> {
        self.outcome.status()
    }
    /// Export immutable text independently of later operations.
    fn message(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.outcome.message()
    }
}
impl ITextureImportResult for TextureImport {
    /// Transfer the exclusive imported resource once.
    fn take_texture(&mut self) -> Result<ComBox<IGpuBufferAbi>, NcStatus> {
        self.outcome.take()
    }
}
/// Packages a texture import without retaining a shared diagnostic slot.
pub fn texture_import_result(result: Result<ComBox<IGpuBufferAbi>, RenderError>) -> ComBox<ITextureImportResultAbi> {
    TextureImport::into_local_box(TextureImport { outcome: Outcome::new(result) })
        .into_interface()
        .unwrap_or_else(|(status, _)| panic!("texture result interface: {status}"))
}

#[com_object(interfaces(ITextureCreationResult), ownership = exclusive)]
/// One exclusive report owns an ordinary shareable native image until transfer.
struct TextureCreation {
    /// Untaken success retains pixels; rejection retains only bounded text/status.
    outcome: Outcome<ComPtr<ITextureAbi>>,
}
impl IRenderOutcome for TextureCreation {
    /// Reads this operation's unconsumed discriminator.
    fn status(&self) -> Result<i32, NcStatus> {
        self.outcome.status()
    }
    /// Exports immutable text owned by this operation.
    fn message(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.outcome.message()
    }
}
impl ITextureCreationResult for TextureCreation {
    /// Transfers the one image capability and consumes this report's success.
    fn take_texture(&mut self) -> Result<ComPtr<ITextureAbi>, NcStatus> {
        self.outcome.take()
    }
}
/// Publishes a native image or its original owned failure without a shared diagnostic slot.
pub fn texture_creation_result(result: Result<ComPtr<ITextureAbi>, RenderError>) -> ComBox<ITextureCreationResultAbi> {
    TextureCreation::into_local_box(TextureCreation { outcome: Outcome::new(result) })
        .into_interface()
        .unwrap_or_else(|(status, _)| panic!("native image result interface: {status}"))
}

#[com_object(interfaces(IWindowCreationResult), ownership = exclusive)]
/// Exclusive construction report; an untaken window is released with this report.
struct WindowCreation {
    /// The complete window or plain failure, with no partially constructed native state.
    outcome: Outcome<ComPtr<IPlatformWindowAbi>>,
}
impl IRenderOutcome for WindowCreation {
    /// Constructor failure detail belongs to this report, not the platform root.
    fn status(&self) -> Result<i32, NcStatus> {
        self.outcome.status()
    }
    /// The returned snapshot survives report/platform/window release.
    fn message(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
        self.outcome.message()
    }
}
impl IWindowCreationResult for WindowCreation {
    /// A completed window remains shareable after its one transfer out of the report.
    fn take_window(&mut self) -> Result<ComPtr<IPlatformWindowAbi>, NcStatus> {
        self.outcome.take()
    }
}
/// Packages native/browser window construction as an owned per-call result.
pub fn window_creation_result(result: Result<ComPtr<IPlatformWindowAbi>, RenderError>) -> ComBox<IWindowCreationResultAbi> {
    WindowCreation::into_local_box(WindowCreation { outcome: Outcome::new(result) })
        .into_interface()
        .unwrap_or_else(|(status, _)| panic!("window result interface: {status}"))
}

impl crate::render::platform::PlatformClient {
    /// Constructs one window or returns the failure and text belonging to that construction.
    pub fn create_window(&self, title: neocogi_com::NcCopyFromBufferOffset<'_, u8>, size: Size) -> Result<ComPtr<IPlatformWindowAbi>, RenderError> {
        let mut report = self
            .create_window_result(title, size)
            .map_err(|status| RenderError::from(status).context("create window"))?;
        check_outcome(&report, "create window")?;
        report.take_window().map_err(|status| RenderError::from(status).context("take window"))
    }
}
impl ITextureFactoryClient {
    /// Uploads an ordinary immutable image, retaining any native failure in the returned error.
    pub fn create_texture(&self, size: Size, pixels: neocogi_com::NcValueRef<'_, InitialGpuData>) -> Result<ComPtr<ITextureAbi>, RenderError> {
        let mut report = self
            .create_texture_result(size, pixels)
            .map_err(|status| RenderError::from(status).context("create native texture"))?;
        check_outcome(&report, "create native texture")?;
        report.take_texture().map_err(|status| RenderError::from(status).context("take native texture"))
    }
}
impl IRendererClient {
    /// Submits one frame and returns that submission's complete owned failure, if any.
    pub fn render(&self, info: RenderInfo, stream: neocogi_com::ComRef<'_, IRenderStreamAbi>) -> Result<(), RenderError> {
        let report = self.render_result(info, stream).map_err(|status| RenderError::from(status).context("render"))?;
        check_outcome(&report, "render")
    }
}
impl IRenderHostClient {
    /// Reclaims retired native resources, returning its own status and explanation.
    pub fn maintain(&self) -> Result<(), RenderError> {
        let report = self
            .maintain_result()
            .map_err(|status| RenderError::from(status).context("maintain renderer"))?;
        check_outcome(&report, "maintain renderer")
    }
    /// Imports an authenticated image into this group, or returns this import's failure.
    pub fn import_texture(
        &self,
        resources: neocogi_com::ComRef<'_, IRenderResourcesAbi>,
        image: neocogi_com::ComRef<'_, ITextureAbi>,
    ) -> Result<ComBox<IGpuBufferAbi>, RenderError> {
        let mut report = self
            .import_texture_result(resources, image)
            .map_err(|status| RenderError::from(status).context("import texture"))?;
        check_outcome(&report, "import texture")?;
        report
            .take_texture()
            .map_err(|status| RenderError::from(status).context("take imported texture"))
    }
    /// Prepares one imported image before recording, with owned native failure detail.
    pub fn prepare_texture(
        &self,
        resources: neocogi_com::ComRef<'_, IRenderResourcesAbi>,
        texture: neocogi_com::ComRef<'_, IGpuBufferAbi>,
    ) -> Result<(), RenderError> {
        let report = self
            .prepare_texture_result(resources, texture)
            .map_err(|status| RenderError::from(status).context("prepare texture"))?;
        check_outcome(&report, "prepare texture")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[com_object(interfaces(IGpuShader), ownership = exclusive)]
    /// Minimal real COM shader owner records exactly one destruction after transfer or abandonment.
    struct Shader {
        /// Ordinary driver identity keeps interface metadata concrete.
        identity: ComPtr<IRenderDriverIdentityAbi>,
        /// Observation only; no observer retains the resource itself.
        drops: Rc<Cell<usize>>,
    }
    impl IGpuShader for Shader {}
    impl IRenderDriverBound for Shader {
        /// Returns the immutable metadata identity, not a native owner.
        fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
            Ok(self.identity.clone())
        }
    }
    impl Drop for Shader {
        /// Records actual final object destruction.
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }
    /// Construct the actual exclusive COM capability used by result-transfer regressions.
    fn shader(drops: Rc<Cell<usize>>) -> ComBox<IGpuShaderAbi> {
        Shader::into_local_box(Shader {
            identity: crate::render::source::new_driver_identity(),
            drops,
        })
        .into_interface()
        .unwrap_or_else(|(status, _)| panic!("test shader interface: {status}"))
    }

    #[com_object(interfaces(IRenderOutcome))]
    /// Valid foreign-style report storage isolates error-text failure from the operation status.
    struct UnreadableFailure {
        /// Oversized successful text export versus an explicit failing getter.
        oversized: bool,
    }
    impl IRenderOutcome for UnreadableFailure {
        /// The original failed operation always retains its distinct backend status.
        fn status(&self) -> Result<i32, NcStatus> {
            Ok(RENDER_E_BACKEND.code())
        }
        /// Both forms are legal COM interactions with initialized storage.
        fn message(&self) -> Result<ComPtr<INcStringAbi>, NcStatus> {
            if self.oversized {
                Ok(NcString::from_utf8_str(&"x".repeat(MAX_RENDER_ERROR_BYTES + 1)))
            } else {
                Err(NC_E_INVALID_ARG)
            }
        }
    }
    /// A secondary getter/size failure must not replace the actual operation's code.
    #[test]
    fn unreadable_report_text_preserves_original_operation_status() {
        for oversized in [false, true] {
            let report: ComPtr<IRenderOutcomeAbi> = UnreadableFailure::from_value(UnreadableFailure { oversized }).query().unwrap();
            let error = check_outcome(&report, "native upload").unwrap_err();
            drop(report);
            assert_eq!(error.status, RENDER_E_BACKEND);
            assert!(error.message.contains("native upload"));
            assert!(error.message.contains("error text unavailable"));
            assert!(error.message.len() <= MAX_RENDER_ERROR_BYTES);
        }
    }

    /// Real COM reports preserve normalized failures after releasing oversized input storage.
    #[test]
    fn normalized_storage_and_com_exports_remain_bounded_and_owned() {
        for (input, expected) in [
            (String::new(), String::new()),
            ("short".to_owned(), "short".to_owned()),
            ("x".repeat(MAX_RENDER_ERROR_BYTES + 5), "x".repeat(MAX_RENDER_ERROR_BYTES)),
            (format!("{}🦀", "x".repeat(MAX_RENDER_ERROR_BYTES - 1)), "x".repeat(MAX_RENDER_ERROR_BYTES - 1)),
        ] {
            let mut storage = String::with_capacity(MAX_RENDER_ERROR_BYTES * 32);
            storage.push_str(&input);
            let normalized = RenderError::new(RENDER_E_BACKEND, storage);
            assert_eq!(normalized.message, expected);
            assert!(normalized.message.capacity() <= MAX_RENDER_ERROR_BYTES);
            let mut storage = String::with_capacity(MAX_RENDER_ERROR_BYTES * 32);
            storage.push_str(&input);
            // Exercise normalization at the provider boundary, including an ordinary Rust
            // error literal whose caller did not invoke the constructor first.
            let Outcome::Rejected(failure) = Outcome::<()>::new(Err(RenderError {
                status: RENDER_E_BACKEND,
                message: storage,
            })) else {
                panic!("expected retained provider failure");
            };
            assert_eq!(failure.message, expected);
            assert!(failure.message.capacity() <= MAX_RENDER_ERROR_BYTES);
            let report = operation_result(Err(failure));
            let snapshot = report.message().unwrap();
            let retained = check_outcome(&report, "bounded report").unwrap_err();
            assert_eq!(retained.status, RENDER_E_BACKEND);
            assert!(retained.message.capacity() <= MAX_RENDER_ERROR_BYTES);
            if !expected.is_empty() {
                assert_eq!(retained.message, expected);
            }
            drop(report);
            assert_eq!(snapshot.to_string_bounded(MAX_RENDER_ERROR_BYTES).unwrap(), expected);
        }
        let invalid = RenderError::new(NC_OK, "x".repeat(MAX_RENDER_ERROR_BYTES * 32));
        assert_eq!(invalid.status, NC_E_INVALID_ARG);
        assert!(invalid.message.starts_with("invalid successful error status"));
        assert!(invalid.message.capacity() <= MAX_RENDER_ERROR_BYTES);
    }

    /// Reports survive unrelated success, later failures and final report destruction as Rust errors.
    #[test]
    fn failures_are_owned_bounded_and_never_erased_by_another_result() {
        let report = operation_result(Err(RenderError::new(RENDER_E_BACKEND, "first failure")));
        let error = check_outcome(&report, "render").unwrap_err();
        let success = operation_result(Ok(()));
        check_outcome(&success, "render").unwrap();
        let later = operation_result(Err(RenderError::new(NC_E_INVALID_ARG, "later failure")));
        assert_eq!(check_outcome(&later, "render").unwrap_err().message, "later failure");
        drop((report, success, later));
        assert_eq!(error.status, RENDER_E_BACKEND);
        assert_eq!(error.message, "first failure");
        let unicode = RenderError::new(RENDER_E_BACKEND, "☃".repeat(MAX_RENDER_ERROR_BYTES));
        assert!(unicode.message.len() <= MAX_RENDER_ERROR_BYTES);
        assert!(unicode.message.chars().all(|ch| ch == '☃'));
        let invalid = operation_result(Err(RenderError {
            status: NC_OK,
            message: "bad error code".into(),
        }));
        assert_eq!(check_outcome(&invalid, "render").unwrap_err().status, NC_E_INVALID_ARG);
    }

    /// Ready resources transfer once; abandonment and consumption preserve exclusive ownership.
    #[test]
    fn creation_results_transfer_or_release_exactly_one_resource() {
        let drops = Rc::new(Cell::new(0));
        let mut result = shader_creation_result(Ok(shader(drops.clone())));
        check_outcome(&result, "create shader").unwrap();
        let owner = result.take_shader().unwrap();
        assert_eq!(result.take_shader().err(), Some(RENDER_E_STATE));
        drop(result);
        assert_eq!(drops.get(), 0);
        drop(owner);
        assert_eq!(drops.get(), 1);
        drop(shader_creation_result(Ok(shader(drops.clone()))));
        assert_eq!(drops.get(), 2);
        let mut rejected = shader_creation_result(Err(RenderError::new(NC_E_INVALID_ARG, "invalid source")));
        assert_eq!(rejected.take_shader().err(), Some(NC_E_INVALID_ARG));
        assert_eq!(check_outcome(&rejected, "create shader").unwrap_err().message, "invalid source");
    }
}
