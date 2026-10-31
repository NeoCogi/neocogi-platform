//! Narrow physical GL lifetime shared by native allocations, never by submission queues.
use neocogi_render_core::source::{DriverIdentity, new_driver_identity};
use std::{cell::RefCell, rc::Rc};

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
    context: web_sys::WebGl2RenderingContext,
    /// SDL window clones retain their video subsystem independently of the event pump.
    #[cfg(not(target_arch = "wasm32"))]
    window: sdl2::video::Window,
    /// Keeping native rendering alive does not keep DOM listeners or polling capabilities alive.
    #[cfg(target_arch = "wasm32")]
    _canvas: web_sys::HtmlCanvasElement,
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

    /// Constructs dispatch from the owned browser context, keeping its canvas but no listeners.
    #[cfg(target_arch = "wasm32")]
    pub fn from_canvas(canvas: web_sys::HtmlCanvasElement, context: web_sys::WebGl2RenderingContext) -> Result<Rc<Self>, String> {
        let core = Rc::new(Self {
            gl: Rc::new(glow::Context::from_webgl2_context(context.clone())),
            state: RefCell::new(State::Live(Physical { context, _canvas: canvas })),
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
                        if physical.context.is_context_lost() {
                            Err("WebGL context was lost; recreate the driver and its resources".into())
                        } else {
                            Ok(())
                        }
                    }
                }
            }
        };
        if let Err(reason) = &result {
            // No native resource may survive by borrowing an unrelated current context. Removing
            // the physical owner terminates this context's storage; GL allocations then skip deletes.
            let previous = self.state.replace(State::Failed(reason.clone()));
            drop(previous);
        }
        result
    }

    /// Actual context loss is terminal; no Ready backing may silently reuse a recreated context.
    pub(crate) fn lose(&self, reason: String) {
        let previous = self.state.replace(State::Failed(reason));
        drop(previous);
    }

    /// Late allocation destruction uses the same physical selection and terminal-loss policy.
    /// False means storage ended with its own context; never issue glDelete against the other one.
    pub(crate) fn for_drop(&self) -> bool {
        self.current().is_ok()
    }
}
