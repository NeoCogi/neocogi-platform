//! DOM construction and input implement the same platform contracts as the native SDL assembly.
use super::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;

impl PlatformObject {
    /// Allocates an independent canvas for each COM window; no two devices accidentally share
    /// one WebGL context. Applications style canvas elements using ordinary page CSS.
    pub(super) fn window(&self, title: &str, size: Size, atlas: ComRef<'_, IRgbaImageAbi>) -> Result<WindowObject, NcStatus> {
        let pixels = AtlasPixels::read(atlas)?;
        self.canvas(title, size, atlas.try_add_ref()?, pixels).map_err(|error| {
            *self.diagnostic.borrow_mut() = error;
            PLATFORM_E_BACKEND
        })
    }

    /// Owns partial DOM construction so listener or GPU failures leave no orphan canvas in the page.
    fn canvas(&self, title: &str, size: Size, atlas: ComPtr<IRgbaImageAbi>, pixels: AtlasPixels) -> Result<WindowObject, String> {
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
        let input = input::BrowserInputHost::new(&pending.canvas)?;
        let gpu = gpu::Gpu::from_canvas(pending.canvas.clone(), pixels.image())?;
        let window = gpu.window();
        let device = Rc::new(Device {
            render: RefCell::new(Some(RenderState {
                gpu,
                atlas,
                prepared: Prepared::default(),
            })),
            textures: RefCell::default(),
            scenes: RefCell::default(),
            retired: RefCell::default(),
            diagnostic: RefCell::default(),
            window,
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
    /// Returns only the separate delegable clipboard identity.
    fn clipboard(&self) -> Result<ComPtr<ITextClipboardAbi>, NcStatus> {
        clipboard::Clipboard::for_window(&self.device.window)
    }

    /// Synchronizes CSS/device scaling before returning the physical drawable allocation.
    fn size(&self) -> Result<Size, NcStatus> {
        resize(&self.device.window);
        Ok(Size {
            width: self.device.window.width() as i32,
            height: self.device.window.height() as i32,
        })
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
        self.input.synchronize(&self.device.window, area);
        Ok(())
    }
}

/// Updates the framebuffer only when physical dimensions change; unchanged sizing preserves contents.
fn resize(canvas: &HtmlCanvasElement) {
    let scale = web_sys::window().map(|window| window.device_pixel_ratio()).unwrap_or(1.0);
    let width = (f64::from(canvas.client_width()) * scale).round().clamp(1.0, f64::from(i32::MAX)) as u32;
    let height = (f64::from(canvas.client_height()) * scale).round().clamp(1.0, f64::from(i32::MAX)) as u32;
    if canvas.width() != width {
        canvas.set_width(width);
    }
    if canvas.height() != height {
        canvas.set_height(height);
    }
}

/// Preserves JavaScript construction details in the platform's owned diagnostic snapshot.
fn js_error(error: wasm_bindgen::JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}
