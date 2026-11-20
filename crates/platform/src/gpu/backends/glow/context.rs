//! Narrow physical GL lifetime shared by native allocations, never by submission queues.
use glow::HasContext;
use neocogi_platform::render::source::{DriverIdentity, new_driver_identity};
use std::{cell::RefCell, rc::Rc};
#[cfg(target_arch = "wasm32")]
use std::cell::Cell;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, closure::Closure};
#[cfg(target_arch = "wasm32")]
use web_sys::{Event, HtmlCanvasElement, WebGl2RenderingContext};

#[cfg(target_arch = "wasm32")]
const CONTEXT_LOST: u32 = WebGl2RenderingContext::CONTEXT_LOST_WEBGL;
#[cfg(not(target_arch = "wasm32"))]
const CONTEXT_LOST: u32 = glow::CONTEXT_LOST;
#[cfg(target_arch = "wasm32")]
const WEBGL_LOSS_REASON: &str = "WebGL context was lost; recreate the driver and its resources";

/// Actual context/device lifetime, independent of renderer state, input routing and cleanup transport.
/// Allocations may retain this core after every renderer facade has been released.
pub struct Core {
    /// Dispatch is kept beside its real physical lifetime; it is not itself the lifetime guard.
    gl: Rc<glow::Context>,
    /// Shared allocations can encounter terminal native failure from Drop through &self.
    /// Interior mutability covers that authoritative lifetime transition only; no borrow crosses COM.
    state: RefCell<State>,
    /// Comparison metadata has no reverse edge to this core or any rendering capability.
    identity: DriverIdentity,
}

/// Physical context availability has a terminal transition; restoration needs a new core/identity.
enum State {
    /// Owns the physical context and the window/canvas it uses.
    Live(Physical),
    /// Native context is unavailable; old handles must never be used against another context.
    Failed(String),
}

/// Field order releases the physical context before its window and underlying video lifetime.
struct Physical {
    /// SDL owns a real native context, not merely loader function pointers.
    #[cfg(not(target_arch = "wasm32"))]
    context: sdl2::video::GLContext,
    /// The browser owns context loss/restoration; the core treats first loss as terminal.
    #[cfg(target_arch = "wasm32")]
    context: WebGl2RenderingContext,
    /// SDL window clones retain their video subsystem independently of the event pump.
    #[cfg(not(target_arch = "wasm32"))]
    window: sdl2::video::Window,
    /// Only the physical generation's loss registration survives the window's input teardown.
    #[cfg(target_arch = "wasm32")]
    loss: CanvasLoss,
}

/// Removable registration on the context's actual canvas. The callback owns only its latch.
#[cfg(target_arch = "wasm32")]
struct CanvasLoss {
    /// Retains the actual canvas for the physical generation and unregistration.
    canvas: HtmlCanvasElement,
    /// Shared with the DOM callback so a loss remains visible after the browser clears its flag.
    observed: Rc<Cell<bool>>,
    /// Browser callback retained for exactly the lifetime of the registration.
    callback: Closure<dyn FnMut(Event)>,
}

#[cfg(target_arch = "wasm32")]
impl CanvasLoss {
    const EVENT: &str = "webglcontextlost";

    /// Latches real loss without changing the page's restoration policy or borrowing Core.
    fn register(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let observed = Rc::new(Cell::new(false));
        let signal = observed.clone();
        // The callback only sets a boolean, so no partial state can escape unwinding.
        let callback = Closure::<dyn FnMut(Event)>::own_assert_unwind_safe(move |event: Event| {
            if event.is_trusted() {
                signal.set(true);
            }
        });
        canvas
            .add_event_listener_with_callback_and_bool(Self::EVENT, callback.as_ref().unchecked_ref(), true)
            .map_err(|error| format!("register WebGL context loss: {error:?}"))?;
        Ok(Self { canvas, observed, callback })
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for CanvasLoss {
    /// Unregister before freeing the callback, including on terminal failure and partial assembly.
    fn drop(&mut self) {
        let _ = self
            .canvas
            .remove_event_listener_with_callback_and_bool(Self::EVENT, self.callback.as_ref().unchecked_ref(), true);
    }
}

impl Core {
    /// Transfers the real SDL lifetime beside dispatch resolved for that context.
    ///
    /// # Safety
    /// `gl` must have been loaded for this live context. The caller transfers the unique context
    /// owner and must not destroy/migrate it through raw SDL handles or share its resource namespace
    /// with untracked contexts. Native calls stay on the owner thread.
    #[cfg(not(target_arch = "wasm32"))]
    pub unsafe fn from_sdl(gl: Rc<glow::Context>, window: sdl2::video::Window, context: sdl2::video::GLContext) -> Result<Rc<Self>, String> {
        let core = Rc::new(Self {
            gl,
            state: RefCell::new(State::Live(Physical { context, window })),
            identity: new_driver_identity(),
        });
        core.current()?;
        Ok(core)
    }

    /// Constructs dispatch and observes terminal loss on the exact canvas owned by the context.
    #[cfg(target_arch = "wasm32")]
    pub fn from_canvas(canvas: HtmlCanvasElement, context: WebGl2RenderingContext) -> Result<Rc<Self>, String> {
        if !context.canvas().is_some_and(|owner| js_sys::Object::is(owner.as_ref(), canvas.as_ref())) {
            return Err("WebGL context does not belong to the selected canvas".into());
        }
        if context.is_context_lost() {
            return Err(WEBGL_LOSS_REASON.into());
        }
        let loss = CanvasLoss::register(canvas)?;
        let core = Rc::new(Self {
            gl: Rc::new(glow::Context::from_webgl2_context(context.clone())),
            state: RefCell::new(State::Live(Physical { context, loss })),
            identity: new_driver_identity(),
        });
        core.current()?;
        Ok(core)
    }

    /// Read-only dispatch for trusted native execution; allocation lifetime stays with this Core.
    pub fn dispatch(&self) -> &Rc<glow::Context> {
        &self.gl
    }

    /// Stable retained metadata. Holding the returned token cannot retain this core.
    pub fn identity(&self) -> &DriverIdentity {
        &self.identity
    }

    /// Observes the actual WebGL drawing buffer without depending on the renderer's
    /// submission state or changing requested canvas attributes.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn drawable_size(&self) -> Result<neocogi_platform::render::abi::Size, String> {
        self.current()?;
        let State::Live(physical) = &*self.state.borrow() else {
            unreachable!("current verified a live context")
        };
        Ok(neocogi_platform::render::abi::Size {
            width: physical.context.drawing_buffer_width(),
            height: physical.context.drawing_buffer_height(),
        })
    }

    /// Selects/checks the actual owning context before each supported native execution phase.
    /// A failure permanently tears down this core's context; later Ready handles cannot be reused.
    pub fn current(&self) -> Result<(), String> {
        let result: Result<(), String> = {
            let state = self.state.borrow();
            match &*state {
                State::Failed(reason) => return Err(reason.clone()),
                State::Live(physical) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        physical.window.gl_make_current(&physical.context)
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        if physical.loss.observed.get() || physical.context.is_context_lost() {
                            Err(WEBGL_LOSS_REASON.into())
                        } else {
                            Ok(())
                        }
                    }
                }
            }
        };
        result.map_err(|reason| self.lose(reason))
    }

    /// Reads a native diagnostic after operations on this selected context and latches actual loss.
    pub(crate) fn check_error(&self, operation: &str) -> Result<(), String> {
        if let State::Failed(reason) = &*self.state.borrow() {
            return Err(reason.clone());
        }
        // SAFETY: Only the owner of this context's active execution phase calls this method.
        let error = unsafe { self.gl.get_error() };
        match error {
            glow::NO_ERROR => Ok(()),
            CONTEXT_LOST => Err(self.lose(format!("{operation} failed with context loss (0x{error:04x})"))),
            _ => Err(format!("{operation} failed with error 0x{error:04x}")),
        }
    }

    /// Preserves the first terminal cause and releases the old physical owner outside the state borrow.
    fn lose(&self, reason: String) -> String {
        if let State::Failed(first) = &*self.state.borrow() {
            return first.clone();
        }
        let previous = self.state.replace(State::Failed(reason.clone()));
        drop(previous);
        reason
    }

    /// Late allocation destruction uses the same physical selection and terminal-loss policy.
    /// False means storage ended with its own context; never issue glDelete against the other one.
    pub(in crate::gpu::backends::glow) fn for_drop(&self) -> bool {
        self.current().is_ok()
    }
}
