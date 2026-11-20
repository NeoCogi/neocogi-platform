//! DOM construction and input implement the same platform contracts as the native SDL assembly.
use super::*;
use wasm_bindgen::JsCast;
use web_sys::{CssStyleDeclaration, HtmlCanvasElement};

impl PlatformObject {
    /// Allocates an independent canvas for each COM window; no two devices accidentally share
    /// one WebGL context. Applications style canvas elements using ordinary page CSS.
    pub(super) fn window(&self, title: &str, size: Size) -> Result<WindowObject, RenderError> {
        self.canvas(title, size).map_err(|error| RenderError::new(PLATFORM_E_BACKEND, error))
    }

    /// Owns partial DOM construction so listener or GPU failures leave no orphan canvas in the page.
    fn canvas(&self, title: &str, size: Size) -> Result<WindowObject, String> {
        let document = web_sys::window().and_then(|window| window.document()).ok_or("browser document unavailable")?;
        let canvas = document
            .create_element("canvas")
            .map_err(js_error)?
            .dyn_into::<HtmlCanvasElement>()
            .map_err(|_| "created element is not a canvas")?;
        canvas.set_attribute("aria-label", title).map_err(js_error)?;
        canvas.set_width(size.width as u32);
        canvas.set_height(size.height as u32);
        canvas.style().set_css_text("display:block;width:100%;height:100%;outline:none");
        document.body().ok_or("browser body unavailable")?.append_child(&canvas).map_err(js_error)?;
        let mut pending = CanvasConstruction { canvas, committed: false };
        resize(&pending.canvas);
        let gpu = gpu::Gpu::from_canvas(pending.canvas.clone())?;
        let drawable = gpu.core();
        let input = input::BrowserInputHost::new(&pending.canvas, drawable.clone())?;
        let window = gpu.window();
        let device = Rc::new(Device {
            success: operation_result(Ok(())),
            logical: neocogi_platform::gpu::GpuDevice::new(gpu.core()).map_err(|status| RenderError::from(status).to_string())?,
            render: RefCell::new(Some(RenderState { gpu, prepared: Prepared::default() })),
            textures: Rc::default(),
            surfaces: RefCell::default(),
            window,
            drawable,
        });
        let window = WindowObject { device, input, polling: Cell::new(false) };
        let _ = window.device.window.focus();
        // The complete COM object now owns DOM removal; disarm partial-construction cleanup.
        pending.committed = true;
        Ok(window)
    }
}

/// Removes a canvas if any fallible initialization step exits before COM publication.
struct CanvasConstruction {
    /// Newly appended element has not yet transferred lifecycle ownership to WindowObject.
    canvas: HtmlCanvasElement,
    /// Successful publication transfers DOM removal to the COM window.
    committed: bool,
}
impl Drop for CanvasConstruction {
    /// DOM removal is independent of GPU/resource destruction and does not submit a frame.
    fn drop(&mut self) {
        if !self.committed {
            self.canvas.remove();
        }
    }
}

impl Drop for WindowObject {
    /// Retained GPU resources keep their context alive, but the closed public window leaves the page.
    fn drop(&mut self) {
        self.device.window.remove();
    }
}

impl IPlatformWindow for WindowObject {
    /// Retaining graphics assembly does not retain this canvas object's DOM registrations.
    fn graphics_host(&self) -> Result<ComPtr<IRenderHostAbi>, NcStatus> {
        GraphicsHost::for_device(self.device.clone())
    }

    /// Returns only the separate delegable clipboard identity.
    fn clipboard(&self) -> Result<ComPtr<ITextClipboardAbi>, NcStatus> {
        clipboard::Clipboard::browser()
    }

    /// Synchronizes CSS/device scaling before returning the physical drawable allocation.
    fn size(&self) -> Result<Size, NcStatus> {
        self.device.synchronized_drawable_size().map_err(|_| PLATFORM_E_BACKEND)
    }
    /// Borrows one destination during delivery and rejects callback-driven recursive polling.
    fn poll(&self, destination: ComRef<'_, IInputSinkAbi>) -> Result<PollStatus, NcStatus> {
        if self.polling.replace(true) {
            return Err(PLATFORM_E_BUSY);
        }
        let _polling = Polling { active: &self.polling };
        Ok(if self.input.poll(destination)? { PollStatus::Event } else { PollStatus::Idle })
    }
    /// Keeps DOM composition active at committed editable focus without stealing external page focus.
    fn text_input(&self, area: TextInputArea) -> Result<(), NcStatus> {
        let area = match area {
            TextInputArea::None => None,
            TextInputArea::Area(area) if area.width >= 0 && area.height >= 0 => Some(area),
            _ => return Err(NC_E_INVALID_ARG),
        };
        self.input.synchronize(&self.device.window, area)
    }
}

/// Visible canvas content measured in fractional viewport CSS pixels.
pub(super) struct CanvasContent {
    /// Viewport location of the top-left content edge.
    pub(super) origin: [f64; 2],
    /// Displayed content size after axis-aligned translation and positive scaling.
    pub(super) extent: [f64; 2],
}

impl CanvasContent {
    /// Observes the layout and viewport boxes without changing the canvas backing.
    pub(super) fn measure(canvas: &HtmlCanvasElement) -> Option<Self> {
        let bounds = canvas.get_bounding_client_rect();
        let mut content = Self {
            origin: [bounds.left(), bounds.top()],
            extent: [bounds.width(), bounds.height()],
        };
        if content.extent == [0.0, 0.0] {
            return Some(content);
        }
        let style = web_sys::window()?.get_computed_style(canvas).ok()??;
        let border_box = style.get_property_value("box-sizing").ok()? == "border-box";
        for (axis, [size, start_border, start_padding, end_border, end_padding]) in [
            ["width", "border-left-width", "padding-left", "border-right-width", "padding-right"],
            ["height", "border-top-width", "padding-top", "border-bottom-width", "padding-bottom"],
        ]
        .into_iter()
        .enumerate()
        {
            let start = css_pixels(&style, start_border)? + css_pixels(&style, start_padding)?;
            let end = css_pixels(&style, end_border)? + css_pixels(&style, end_padding)?;
            let used = css_pixels(&style, size)?;
            let layout_content = if border_box { (used - start - end).max(0.0) } else { used };
            let layout_border = start + layout_content + end;
            // The border rectangle includes ancestor scaling. Apply its axis ratio to the insets
            // and content size together so they share the pointer and textarea's viewport space.
            let scale = if layout_border > 0.0 { content.extent[axis] / layout_border } else { 0.0 };
            content.origin[axis] += start * scale;
            content.extent[axis] = layout_content * scale;
        }
        Some(content)
    }
}

/// Reads one resolved, nonnegative layout length in CSS pixels.
fn css_pixels(style: &CssStyleDeclaration, property: &str) -> Option<f64> {
    let value = style.get_property_value(property).ok()?;
    let pixels = value.strip_suffix("px")?.parse::<f64>().ok()?;
    (pixels.is_finite() && pixels >= 0.0).then_some(pixels)
}

/// Requests a backing from the displayed content and device scale. The WebGL
/// context independently reports the drawing-buffer size it could allocate.
pub(super) fn resize(canvas: &HtmlCanvasElement) {
    let Some(content) = CanvasContent::measure(canvas) else { return };
    let scale = web_sys::window().map(|window| window.device_pixel_ratio()).unwrap_or(1.0);
    let [width, height] = content.extent.map(|extent| (extent * scale).round().clamp(1.0, f64::from(i32::MAX)) as u32);
    if canvas.width() != width {
        canvas.set_width(width);
    }
    if canvas.height() != height {
        canvas.set_height(height);
    }
}

/// Preserves JavaScript construction details in the returned owned construction error.
fn js_error(error: wasm_bindgen::JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}
