//! Explicit acceptance diagnostics. No native API handle is returned or retained by a probe.
use super::*;
use neocogi_platform::gpu::native::NativeResources;

#[cfg(feature = "glow")]
pub use neocogi_platform::gpu::backends::glow::Backend as NativeBackend;
#[cfg(feature = "vulkan")]
pub use neocogi_platform::gpu::backends::vulkan::Backend as NativeBackend;
#[cfg(feature = "wgpu")]
pub use neocogi_platform::gpu::backends::wgpu::Backend as NativeBackend;
#[cfg(feature = "webgl")]
pub use neocogi_platform::gpu::backends::webgl::Backend as NativeBackend;

#[cfg(all(feature = "glow", not(target_arch = "wasm32")))]
pub(crate) mod calls;
/// Owned metadata for one measurement; never enters a renderer ABI.
pub struct Sample {
    /// Window/assets/UI/client construction, including CPU shader compilation.
    pub setup_ns: u128,
    /// Real retained painting into authenticated immutable blocks.
    pub record_ns: u128,
    /// Includes a separately owned procedural client contribution.
    pub mixed: bool,
    /// Actual admitted commands in painter order.
    pub commands: usize,
    /// Actual draws, excluding regional clears.
    pub draws: usize,
    /// Distinct logical vertex/index versions.
    pub buffers: usize,
    /// Distinct logical sampled image versions.
    pub textures: usize,
    /// Distinct logical pipelines; native links are counted separately.
    pub pipelines: usize,
}

/// Factory-created platform window paired with non-owning acceptance observations.
pub struct NativeWindow {
    /// Actual production window capability; dropping it ends input ownership normally.
    pub window: ComPtr<IPlatformWindowAbi>,
    /// Weak observations never extend device or physical context lifetime.
    pub probe: NativeProbe,
}

/// Concrete weak observer issued alongside a window, never recovered by downcasting COM.
#[derive(Clone)]
pub struct NativeProbe {
    /// Does not retain window event ownership or the device facade.
    device: Weak<Device>,
    /// Observes the physical graphics context while resources may still retain it.
    core: Weak<<NativeBackend as NativeResources>::Core>,
}

/// Constructs the production selected backend with an explicitly supplied shader compiler.
pub fn window(compiler: ComPtr<neocogi_platform::shader::abi::ShaderCompilerAbi>, title: &str, size: Size) -> Result<NativeWindow, RenderError> {
    let native = PlatformObject::new(compiler).window(title, size)?;
    let probe = NativeProbe {
        device: Rc::downgrade(&native.device),
        core: Rc::downgrade(&native.device.access()?.state().gpu.core()),
    };
    Ok(NativeWindow {
        window: WindowObject::from_value(native).query()?,
        probe,
    })
}
impl NativeProbe {
    /// Whether public windows, hosts or active renderer calls still retain the device facade.
    pub fn is_alive(&self) -> bool {
        self.device.strong_count() != 0
    }
    /// Whether immutable native allocations still retain their physical context.
    pub fn native_alive(&self) -> bool {
        self.core.strong_count() != 0
    }
    /// Captures ordinary owned logical recording access without extending facade/input ownership.
    pub fn logical(&self) -> Result<neocogi_platform::gpu::GpuDevice<NativeBackend>, NcStatus> {
        Ok(self.device.upgrade().ok_or(RENDER_E_EXPIRED)?.logical.clone())
    }
    /// Observes registration of a canonical image identity after a caller releases its owner.
    pub fn contains_image(&self, key: usize) -> bool {
        self.device.upgrade().is_some_and(|device| device.textures.borrow().contains_key(&key))
    }
    /// Returns a copy of completed native GL pixels; acceptance capture is compiled out by default.
    #[cfg(feature = "glow")]
    pub fn pixels(&self) -> Result<Vec<u8>, NcStatus> {
        let device = self.device.upgrade().ok_or(RENDER_E_EXPIRED)?;
        let access = device.access()?;
        Ok(access.state.as_ref().expect("admitted access owns native state").gpu.last_pixels().to_vec())
    }
    /// Observe canonical surface identity without retaining the public surface or its target metadata.
    pub fn contains_surface(&self, key: usize) -> bool {
        self.device.upgrade().is_some_and(|device| device.surfaces.borrow().contains_key(&key))
    }
    /// Waits for already issued native commands before an exact retirement-quota assertion.
    #[cfg(feature = "glow")]
    pub fn finish(&self) -> Result<(), RenderError> {
        use glow::HasContext;
        let device = self.device.upgrade().ok_or(RENDER_E_EXPIRED)?;
        let core = device.access()?.state().gpu.core();
        core.current().map_err(|message| device.failure(message))?;
        // SAFETY: The owned core selected its live context; no foreign handle or memory is accepted.
        unsafe {
            core.dispatch().finish();
        }
        Ok(())
    }
    /// Measures preparation/execution of owned authenticated snapshots without presentation/readback.
    #[cfg(all(feature = "glow", not(target_arch = "wasm32")))]
    pub fn measure(&self, blocks: &[Rc<neocogi_platform::gpu::record::Block<NativeBackend>>], sample: Sample) -> Result<(), NcStatus> {
        self.device.upgrade().ok_or(RENDER_E_EXPIRED)?.access()?.state().gpu.measure(blocks, sample);
        Ok(())
    }
}
#[cfg(target_arch = "wasm32")]
pub use crate::platform::input::tests::run_platform_input_tests;
