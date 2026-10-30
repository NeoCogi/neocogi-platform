//
// Copyright 2022-Present (c) Raja Lehtihet & Wael El Oraiby
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are met:
//
// 1. Redistributions of source code must retain the above copyright notice,
// this list of conditions and the following disclaimer.
//
// 2. Redistributions in binary form must reproduce the above copyright notice,
// this list of conditions and the following disclaimer in the documentation
// and/or other materials provided with the distribution.
//
// 3. Neither the name of the copyright holder nor the names of its contributors
// may be used to endorse or promote products derived from this software without
// specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
// AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
// IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
// ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE
// LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
// CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
// SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
// INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
// CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
// ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
// POSSIBILITY OF SUCH DAMAGE.
//
// -----------------------------------------------------------------------------
//
//! Vulkan native targets, immutable images and generic command execution.
//!
//! Window and private offscreen passes share the generic executor. Preparatory uploads
//! and submitted blocks retain typed native owners through completion. GFX owns all UI
//! shaders, vertex layouts, atlas selection and batching.

use std::{collections::HashMap, rc::Rc};

use ash::{khr, vk, Entry};
use neocogi_render_core::prelude::*;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use sdl2::video::Window;

use neocogi_render_support::resource_guard::ResourceGuard;

mod generic;
mod offscreen;
mod pass;
pub use generic::PreparedBlock;
#[cfg(test)]
mod surface_tests;

type Result<T> = std::result::Result<T, String>;
type Surface = khr::surface::Instance;
type Swapchain = khr::swapchain::Device;

/// Native swapchain image and synchronization slot acquired before display-list execution.
struct AcquiredVulkanFrame {
    /// Frame-in-flight slot whose acquire semaphore was signaled for this image.
    frame: usize,
    /// Swapchain image that must either be submitted and presented or make the context fatal.
    image_index: u32,
    /// Whether acquisition reported that the swapchain should be rebuilt after presentation.
    suboptimal: bool,
}

/// Exactly one native acquisition or one unpublished UI texture is owned by an active frame.
enum FrameTarget {
    /// Native image and binary semaphores follow the established acquire/present state machine.
    Window(AcquiredVulkanFrame),
    /// Device images are selected without touching any swapchain synchronization slot.
    Texture {
        /// Fresh output whose previous pixels have never been published.
        output: TextureId,
    },
}

/// Lifecycle of the single swapchain image currently owned by the high-level frame boundary.
///
/// A successful acquire signals a binary semaphore and removes one image from the presentation
/// engine. If any later record, submit, or present step fails, neither that semaphore nor that image
/// can be blindly reused. The fatal state deliberately disables later frames instead of attempting
/// a partial synchronization repair whose correctness would depend on how far the driver progressed.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
enum AcquiredFrameState {
    /// No acquired image or signaled acquire semaphore is outstanding.
    #[default]
    Ready,
    /// One [`AcquiredVulkanFrame`] must complete through graphics submission and presentation.
    Acquired,
    /// A post-acquire failure abandoned synchronization state; this context cannot render again.
    Fatal,
}

impl AcquiredFrameState {
    /// Rejects acquisition unless every earlier image and semaphore completed normally.
    fn ensure_ready(self) -> Result<()> {
        match self {
            Self::Ready => Ok(()),
            Self::Acquired => Err(String::from("cannot acquire a second Vulkan frame while one is outstanding")),
            Self::Fatal => Err(String::from("cannot acquire a Vulkan frame after an earlier acquired frame failed")),
        }
    }

    /// Records that swapchain acquisition succeeded and its synchronization objects are now live.
    fn acquire_succeeded(&mut self) -> Result<()> {
        match self {
            Self::Ready => {
                // Only a ready context can publish a new acquired-frame token.
                *self = Self::Acquired;
                Ok(())
            }
            Self::Acquired => Err(String::from("cannot acquire a second Vulkan frame while one is outstanding")),
            Self::Fatal => Err(String::from("cannot acquire a Vulkan frame after an earlier acquired frame failed")),
        }
    }

    /// Commits a successful acquired frame or permanently latches any post-acquire failure.
    fn finish<T>(&mut self, result: Result<T>) -> Result<T> {
        debug_assert_eq!(*self, Self::Acquired, "only an acquired frame may be finalized");
        if result.is_ok() {
            // Submission and presentation consumed both the image and its acquire semaphore.
            *self = Self::Ready;
        } else {
            // The exact driver progress is intentionally irrelevant after failure: no later frame
            // may wait on, reset, or reuse synchronization objects from this abandoned transaction.
            *self = Self::Fatal;
        }
        result
    }

    /// Latches a panic or other non-Result escape from acquired-frame finalization.
    fn abandon(&mut self) {
        // A panic can occur after native commands have been recorded. Treat it exactly like an
        // explicit error so unwinding cannot make the same slot available again.
        if *self == Self::Acquired {
            *self = Self::Fatal;
        }
    }

    /// Returns whether later rendering must remain disabled.
    fn is_fatal(self) -> bool {
        self == Self::Fatal
    }
}

/// Vulkan presentation and texture ownership with one validated generic graphics executor.
/// Prepared commands never expose the device, render pass or command buffer to clients.
pub struct VulkanRenderer {
    /// Native immutable images shared by ordinary UI capabilities and generic bindings.
    textures: HashMap<TextureId, VulkanTexture>,
    /// Generic caches and completion owners drop before context-owned fences and presentation.
    generic: generic::Executor,
    /// Private UI-only submission state is destroyed before its logical device.
    offscreen: Option<offscreen::Offscreen>,
    /// Native Vulkan state owned exclusively by the backend.
    context: VulkanContext,
    /// Realized generic blocks in painter order, with no UI-specific commands.
    commands: Vec<PreparedBlock>,
    /// Color cleared at the beginning of the shared graphics render pass.
    clear_color: Color,
    /// Logical microui frame width used when scaling application rectangles.
    width: u32,
    /// Logical microui frame height used when scaling application rectangles.
    height: u32,
    /// Permanent high-level latch set after device loss or an abandoned acquired frame.
    disabled: bool,
}

impl VulkanRenderer {
    /// Creates native presentation and a private generic executor on the same device.
    pub fn new(window: &Window, width: u32, height: u32) -> Result<Self> {
        let context = VulkanContext::new(window, width, height)?;
        let generic = generic::Executor::new(context.core.clone())?;

        Ok(Self {
            textures: HashMap::new(),
            offscreen: None,
            context,
            commands: Vec::new(),
            generic,
            clear_color: color(0, 0, 0, 255),
            width,
            height,
            disabled: false,
        })
    }

    /// Recreates the swapchain when the SDL window size diverges from the cached Vulkan extent.
    fn ensure_swapchain_extent(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(()); // Minimized window; skip until it has a size.
        }

        // The SDL window size is the source of truth; the Vulkan context rebuilds the swapchain on
        // demand when the cached extent diverges from it.
        let extent = self.context.extent();
        if extent.width != width || extent.height != height {
            self.context.recreate_swapchain(width, height)?;
        }

        Ok(())
    }

    /// Compiles immutable resources before acquiring or clearing a presentation target.
    pub fn prepare_block(&mut self, block: &neocogi_render_gpu::record::Block, offscreen: bool) -> Result<PreparedBlock> {
        if self.disabled || self.context.is_unusable() || self.generic.unusable() {
            return Err("Vulkan renderer is unavailable after a native frame failure".into());
        }
        if offscreen {
            if !block.is_ui() {
                return Err("offscreen Vulkan surfaces accept private UI commands only".into());
            }
            if self.offscreen.is_none() {
                self.offscreen = Some(offscreen::Offscreen::new(&self.context.core)?);
            }
            return self.generic.prepare(
                block,
                self.offscreen.as_ref().expect("initialized above").render_pass,
                generic::Signature {
                    color: vk::Format::R8G8B8A8_UNORM,
                    depth: None,
                },
                &self.textures,
            );
        }
        self.generic.prepare(block, self.context.render_pass, self.context.signature(), &self.textures)
    }

    /// Trusted UI resource preparation never acquires or clears a rendering target.
    pub fn prepare_texture(&mut self, texture: Rc<neocogi_render_gpu::resource::TextureVersion>) -> Result<()> {
        if self.disabled || self.context.is_unusable() {
            return Err("Vulkan renderer is unavailable after a native frame failure".into());
        }
        self.generic.prepare_texture(texture, &self.textures)
    }

    /// Converts a packed clear color into the Vulkan clear value used for the render pass.
    fn color_to_vk_clear(color: Color) -> vk::ClearValue {
        let to_float = |c: u8| c as f32 / 255.0;
        vk::ClearValue {
            color: vk::ClearColorValue {
                float32: [to_float(color.r), to_float(color.g), to_float(color.b), to_float(color.a)],
            },
        }
    }
}

impl VulkanRenderer {
    /// Starts an ordered generic frame without acquiring presentation.
    fn begin(&mut self, width: i32, height: i32, clr: Color) -> Result<()> {
        // Realization precedes acquisition; native command recording happens at finish.
        self.width = width as u32;
        self.height = height as u32;
        self.clear_color = clr;
        self.commands.clear();
        if self.disabled || self.generic.unusable() {
            return Err(String::from("Vulkan renderer is disabled after an unrecoverable frame failure"));
        }
        self.generic.collect()?;

        Ok(())
    }

    /// Finalizes the frame by submitting the queued generic blocks to Vulkan.
    fn finish(&mut self, target: FrameTarget) -> Result<()> {
        match target {
            FrameTarget::Window(acquired) => self.finish_window(acquired),
            FrameTarget::Texture { output } => {
                let result = self.offscreen.as_mut().ok_or("offscreen execution was not initialized")?.render(
                    &self.generic,
                    &self.textures[&output],
                    Self::color_to_vk_clear(self.clear_color),
                    self.width,
                    self.height,
                    &self.commands,
                );
                self.disabled |= self.offscreen.as_ref().is_some_and(offscreen::Offscreen::unusable);
                if result.is_ok() {
                    self.textures.get_mut(&output).expect("output remains owned").image.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
                }
                self.commands.clear();
                result
            }
        }
    }

    /// Native finalization preserves the existing acquire, transfer, submit, and present protocol.
    fn finish_window(&mut self, acquired: AcquiredVulkanFrame) -> Result<()> {
        if self.disabled {
            self.commands.clear();
            return Err(String::from("Vulkan renderer is disabled after an unrecoverable frame failure"));
        }
        let mut commands = std::mem::take(&mut self.commands);
        // `draw_frame` consumes the queued UI/custom jobs and may drain `commands`; taking the vec
        // avoids reallocating a fresh command buffer every frame.
        let result = self.context.draw_frame(
            acquired,
            Self::color_to_vk_clear(self.clear_color),
            self.width,
            self.height,
            &mut commands,
            &mut self.generic,
        );
        // draw_frame latches post-acquire failure; preserve that state before returning the error.
        self.disabled |= self.context.is_unusable();
        commands.clear();
        self.commands = commands;
        result
    }

    /// Creates a backend-owned sampled texture and tracks it by `TextureId`.
    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> std::result::Result<(), TextureError> {
        // Reject malformed storage before map reservation, native allocation or submission.
        ImageSource {
            width: id.width(),
            height: id.height(),
            pixels,
        }
        .validate()?;
        // Texture creation is the public backend boundary, so convert Vulkan's string-based
        // operational failures into the renderer's typed texture error without changing the
        // file-wide `Result<T>` used by unrelated swapchain and resource internals.
        if self.disabled {
            return Err(TextureError::backend("Vulkan renderer is disabled after an unrecoverable frame failure"));
        }
        self.textures
            .try_reserve(1)
            .map_err(|err| TextureError::backend(format!("failed to reserve texture ownership entry: {err}")))?;
        // Dimensions and actual storage have now been admitted together.
        let size = neocogi_render_gpu::abi::Size { width: id.width(), height: id.height() };
        let native = self
            .generic
            .upload_image(size, vk::Format::R8G8B8A8_UNORM, pixels, None)
            .map_err(TextureError::backend)?;
        let texture = VulkanTexture {
            image: ImageResource {
                native: Some(native),
                extent: vk::Extent2D {
                    width: id.width() as u32,
                    height: id.height() as u32,
                },
                layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            },
            framebuffer: None,
        };
        // A replacement is committed only after the new image is complete. Destroying the old
        // image after insertion also keeps the map leak-free if an ID is retried unexpectedly.
        if let Some(mut previous) = self.textures.insert(id, texture) {
            previous.destroy(&self.context);
        }
        Ok(())
    }

    /// Destroys a backend-owned sampled texture if it is still tracked.
    fn destroy_texture(&mut self, id: TextureId) {
        if let Some(mut texture) = self.textures.remove(&id) {
            // Texture destruction is rare in the examples; a simple idle boundary guarantees no
            // submitted frame still samples the image before its view/memory are released.
            self.context.core.wait_idle();
            texture.destroy(&self.context);
        }
    }
}

impl Drop for VulkanRenderer {
    /// Destroys user images while the context/device they depend on are still alive.
    fn drop(&mut self) {
        // `VulkanContext::drop` also waits, but renderer-owned images must be released before the
        // context field itself is dropped. Establish the idle boundary here first.
        self.context.core.wait_idle();
        for (_, mut texture) in self.textures.drain() {
            texture.destroy(&self.context);
        }
    }
}

#[must_use = "call finish to submit the Vulkan frame and observe errors"]
/// Acquired Vulkan frame accepts only validated resources and bounded commands.
pub struct VulkanFrame<'a> {
    /// Backend is borrowed for the complete acquire-to-present transaction.
    backend: &'a mut VulkanRenderer,
    /// Explicit target is consumed exactly once; texture frames own no native acquisition.
    target: Option<FrameTarget>,
}

impl VulkanFrame<'_> {
    /// Preserves the painter position of a block prepared before target acquisition.
    pub fn execute_block(&mut self, block: &PreparedBlock) {
        self.backend.commands.push(block.clone());
    }
}

impl RendererFrame for VulkanFrame<'_> {
    /// Completes the acquired transaction; Drop still guards against any unwinding escape.
    fn finish(mut self) -> std::result::Result<(), FrameError> {
        let target = self.target.take().expect("an active Vulkan frame owns its explicit target");
        self.backend.finish(target).map_err(FrameError::new)
    }
}

impl Drop for VulkanFrame<'_> {
    fn drop(&mut self) {
        // Success has already restored Ready. Any other escape, including a panic after taking
        // the token in finish, must forbid reuse of the potentially signaled acquire semaphore.
        self.backend.context.abandon_acquired_frame();
        self.backend.disabled |= self.backend.context.is_unusable() || self.backend.offscreen.as_ref().is_some_and(offscreen::Offscreen::unusable);
        self.backend.commands.clear();
    }
}

impl RendererBackend for VulkanRenderer {
    type Frame<'a> = VulkanFrame<'a>;

    fn frame(&mut self, info: FrameInfo) -> std::result::Result<Self::Frame<'_>, FrameError> {
        let dimensions = info.dimensions();
        self.ensure_swapchain_extent(dimensions.width as u32, dimensions.height as u32)
            .map_err(FrameError::new)?;
        self.begin(dimensions.width, dimensions.height, info.clear()).map_err(FrameError::new)?;
        let acquired = self
            .context
            .acquire_frame(dimensions.width as u32, dimensions.height as u32)
            .map_err(FrameError::new)?;
        Ok(VulkanFrame {
            backend: self,
            target: Some(FrameTarget::Window(acquired)),
        })
    }

    /// Creates an image/framebuffer/descriptor transaction without acquiring a presentation image.
    fn create_render_texture(&mut self, id: TextureId) -> std::result::Result<(), TextureError> {
        if self.disabled || self.textures.contains_key(&id) {
            return Err(TextureError::backend("Vulkan render target is unavailable or already exists"));
        }
        self.textures.try_reserve(1).map_err(|error| TextureError::backend(error.to_string()))?;
        if self.offscreen.is_none() {
            self.offscreen = Some(offscreen::Offscreen::new(&self.context.core).map_err(TextureError::backend)?);
        }
        let device = self.context.device.clone();
        let image = self
            .context
            .create_image_with_usage(
                id.width() as u32,
                id.height() as u32,
                vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC,
            )
            .map_err(TextureError::backend)?
            .guarded(&device);
        let attachments = [image.get().view()];
        let info = vk::FramebufferCreateInfo::default()
            .render_pass(self.offscreen.as_ref().expect("initialized above").render_pass)
            .attachments(&attachments)
            .width(id.width() as u32)
            .height(id.height() as u32)
            .layers(1);
        // SAFETY: The attachment, render pass, and logical device remain owned by this transaction.
        let framebuffer = ResourceGuard::new(
            unsafe { device.create_framebuffer(&info, None) }.map_err(|error| TextureError::backend(format!("offscreen framebuffer: {error:?}")))?,
            |framebuffer| unsafe { device.destroy_framebuffer(framebuffer, None) },
        );
        self.textures.insert(
            id,
            VulkanTexture {
                image: image.into_inner(),
                framebuffer: Some(framebuffer.into_inner()),
            },
        );
        Ok(())
    }

    /// Opens a fresh immutable-output target; the plugin admits private UI blocks only.
    fn texture_frame(&mut self, info: FrameInfo, output: TextureId) -> std::result::Result<Self::Frame<'_>, FrameError> {
        let size = info.dimensions();
        if (output.width(), output.height()) != (size.width, size.height)
            || info.clear().a != 255
            || !self.textures.get(&output).is_some_and(|image| image.framebuffer.is_some())
        {
            return Err(FrameError::new("invalid Vulkan output, extent, or clear"));
        }
        self.begin(size.width, size.height, info.clear()).map_err(FrameError::new)?;
        Ok(VulkanFrame {
            backend: self,
            target: Some(FrameTarget::Texture { output }),
        })
    }

    fn create_texture(&mut self, id: TextureId, pixels: &[u8]) -> std::result::Result<(), TextureError> {
        // Preserve the typed texture failure returned by the inherent native upload helper.
        self.create_texture(id, pixels)
    }

    fn destroy_texture(&mut self, id: TextureId) {
        self.destroy_texture(id);
    }
}

impl VulkanRenderer {}

/// One device image and its authenticated sampled binding, optionally usable as an output.
struct VulkanTexture {
    /// Complete image/view/allocation ownership.
    image: ImageResource,
    /// Only render outputs have a framebuffer for the private UI-only render pass.
    framebuffer: Option<vk::Framebuffer>,
}

impl VulkanTexture {
    /// Releases the individually allocated descriptor before destroying its referenced image.
    fn destroy(&mut self, context: &VulkanContext) {
        if let Some(framebuffer) = self.framebuffer.take() {
            unsafe {
                context.device.destroy_framebuffer(framebuffer, None);
            }
        }
        self.image.destroy(&context.device);
    }
}

/// Host image metadata keeps one shared native owner; prepared graphics can retain that same image.
struct ImageResource {
    /// Present while live; explicit host retirement takes only this reference.
    native: Option<Rc<generic::Image>>,
    /// Pixel extent used when recording uploads.
    extent: vk::Extent2D,
    /// Last layout established by host transfer/render commands.
    layout: vk::ImageLayout,
}

impl ImageResource {
    /// Arms reverse-order destruction for a complete image/memory/view aggregate.
    fn guarded(self, device: &ash::Device) -> ResourceGuard<Self, impl FnOnce(Self) + use<>> {
        let device = device.clone();
        ResourceGuard::new(self, move |mut image| image.destroy(&device))
    }

    /// A view has exactly the same lifetime as its native image owner.
    fn view(&self) -> vk::ImageView {
        self.native.as_ref().expect("live host image").view
    }

    /// Descriptor/framebuffer references have ended; prepared aliases may still retain the allocation.
    fn destroy(&mut self, _device: &ash::Device) {
        self.native.take();
    }
}
/// Owns the native objects acquired before a full [`VulkanContext`] can exist.
///
/// `VulkanContext::drop` cannot help while instance/surface/device creation is only partially
/// complete. This bootstrap owner mirrors their dependency order and is disarmed only after the
/// last fallible discovery step succeeds.
struct VulkanBootstrap {
    /// Root Vulkan instance retained until the completed context takes ownership.
    instance: Option<ash::Instance>,
    /// Instance-level surface dispatch table required to destroy `surface`.
    surface_loader: Option<Surface>,
    /// Window surface owned after creation and nulled when committed or destroyed.
    surface: vk::SurfaceKHR,
    /// Logical device retained only after device creation succeeds.
    device: Option<ash::Device>,
}

impl VulkanBootstrap {
    /// Starts a partial native ownership chain with the already-created instance.
    fn new(instance: ash::Instance) -> Self {
        // Later stages remain absent/null so Drop can distinguish exactly which prefix exists.
        Self {
            instance: Some(instance),
            surface_loader: None,
            surface: vk::SurfaceKHR::null(),
            device: None,
        }
    }

    /// Borrows the instance required by surface and physical-device discovery.
    fn instance(&self) -> &ash::Instance {
        // Bootstrap construction installs the instance before any helper can borrow it.
        self.instance.as_ref().expect("bootstrap instance is present before commit")
    }

    /// Borrows the surface loader after it has been paired with the created surface.
    fn surface_loader(&self) -> &Surface {
        // Callers reach this helper only after installing both loader and surface during startup.
        self.surface_loader.as_ref().expect("bootstrap surface loader is installed with the surface")
    }

    /// Borrows the logical device during the final queue/bootstrap stages.
    fn device(&self) -> &ash::Device {
        // Device-dependent discovery runs only after logical-device creation stored this owner.
        self.device.as_ref().expect("bootstrap device is present after logical-device creation")
    }

    /// Transfers the completed native ownership chain into `VulkanContext`.
    fn commit(mut self) -> (ash::Instance, Surface, vk::SurfaceKHR, ash::Device) {
        // Null/take every field before Drop runs, preserving dependency order while disarming all
        // bootstrap cleanup paths in one infallible ownership transfer.
        let surface = std::mem::replace(&mut self.surface, vk::SurfaceKHR::null());
        (
            self.instance.take().expect("bootstrap instance cannot be committed twice"),
            self.surface_loader.take().expect("bootstrap surface loader cannot be committed twice"),
            surface,
            self.device.take().expect("bootstrap device cannot be committed twice"),
        )
    }
}

impl Drop for VulkanBootstrap {
    /// Tears down only the prefix that was acquired, in reverse dependency order.
    fn drop(&mut self) {
        unsafe {
            if let Some(device) = self.device.take() {
                device.destroy_device(None);
            }
            if self.surface != vk::SurfaceKHR::null() {
                if let Some(loader) = self.surface_loader.as_ref() {
                    loader.destroy_surface(self.surface, None);
                }
                self.surface = vk::SurfaceKHR::null();
            }
            if let Some(instance) = self.instance.take() {
                instance.destroy_instance(None);
            }
        }
    }
}

pub(crate) struct VulkanContext {
    // `VulkanContext` owns the actual Vulkan objects and implements the frame graph used by the
    // example renderer: acquire -> upload/record -> submit -> present -> recreate on demand.
    /// Cache-free native owner can outlive presentation through escaped prepared blocks.
    core: Rc<generic::Core>,
    surface_loader: Surface,
    surface: vk::SurfaceKHR,
    physical_device: vk::PhysicalDevice,
    device: ash::Device,
    queue_indices: QueueFamilyIndices,
    graphics_queue: vk::Queue,
    present_queue: vk::Queue,
    swapchain_loader: Swapchain,
    swapchain: vk::SwapchainKHR,
    swapchain_images: Vec<vk::Image>,
    swapchain_image_views: Vec<vk::ImageView>,
    swapchain_format: vk::Format,
    depth_format: vk::Format,
    extent: vk::Extent2D,
    render_pass: vk::RenderPass,
    framebuffers: Vec<vk::Framebuffer>,
    command_pool: vk::CommandPool,
    command_buffers: Vec<vk::CommandBuffer>,
    image_available_semaphores: Vec<vk::Semaphore>,
    /// One per swapchain image: a graphics fence cannot prove consumption by presentation.
    render_finished_semaphores: Vec<vk::Semaphore>,
    in_flight_fences: Vec<vk::Fence>,
    current_frame: usize,
    max_frames_in_flight: usize,
    /// Ownership state for the acquire semaphore and swapchain image handed to the active frame.
    acquired_frame_state: AcquiredFrameState,
    depth_images: Vec<ImageResource>,
    device_lost: bool,
    /// Shared SDL native-window/video owner, retained until after Vulkan surface destruction.
    ///
    /// `Drop` destroys GPU objects before Rust drops fields. This last field also preserves that
    /// order if additional owning fields are introduced; an SDL clone shares the existing window.
    _window: Window,
}

impl VulkanContext {
    /// Pipeline compatibility is independent of swapchain extent and descriptor generations.
    fn signature(&self) -> generic::Signature {
        generic::Signature {
            color: self.swapchain_format,
            depth: Some(self.depth_format),
        }
    }

    /// Creates the Vulkan instance/device state and the first set of swapchain-dependent resources.
    fn new(window: &Window, width: u32, height: u32) -> Result<Self> {
        // Retain before creating GPU locals: constructor failures unwind those before this owner,
        // and success transfers it into the context until after explicit surface destruction.
        let window = window.clone();
        match window.subsystem().current_video_driver() {
            "offscreen" | "dummy" => return Err("Vulkan requires an SDL driver with native window handles".to_owned()),
            _ => {}
        }
        // Context creation front-loads the permanent objects: instance/device/queues, the shared
        // command pool, and the initial swapchain-dependent resources.
        let entry = Entry::linked();
        let app_name = c"neocogi-ui-examples";
        let engine_name = c"neocogi-ui";

        let display_handle = window.display_handle().map_err(|err| format!("failed to get display handle: {err:?}"))?;
        let window_handle = window.window_handle().map_err(|err| format!("failed to get window handle: {err:?}"))?;
        let raw_display_handle = display_handle.into();
        let raw_window_handle = window_handle.into();

        let app_info = vk::ApplicationInfo::default()
            .application_name(app_name)
            .engine_name(engine_name)
            .api_version(vk::API_VERSION_1_1);

        let mut extension_names = ash_window_handle::enumerate_required_extensions(raw_display_handle)
            .map_err(|err| format!("enumerate_required_extensions failed: {err:?}"))?
            .to_vec();
        let surface_extension = khr::surface::NAME.as_ptr();
        if !extension_names.contains(&surface_extension) {
            extension_names.push(surface_extension);
        }

        let instance_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_extension_names(&extension_names);
        let instance = unsafe { entry.create_instance(&instance_info, None) }.map_err(|err| format!("create_instance failed: {err:?}"))?;
        let mut bootstrap = VulkanBootstrap::new(instance);

        let surface_loader = Surface::new(&entry, bootstrap.instance());
        bootstrap.surface_loader = Some(surface_loader);
        let surface = unsafe { ash_window_handle::create_surface(&entry, bootstrap.instance(), raw_display_handle, raw_window_handle, None) }
            .map_err(|err| format!("create_surface failed: {err:?}"))?;
        bootstrap.surface = surface;

        let (physical_device, queue_indices) = Self::select_physical_device(bootstrap.instance(), bootstrap.surface_loader(), surface)?;

        let device = Self::create_logical_device(bootstrap.instance(), physical_device, &queue_indices)?;
        bootstrap.device = Some(device);
        let graphics_queue = unsafe { bootstrap.device().get_device_queue(queue_indices.graphics_family, 0) };
        let present_queue = unsafe { bootstrap.device().get_device_queue(queue_indices.present_family, 0) };

        let depth_format = Self::find_depth_format(bootstrap.instance(), physical_device)?;
        let (instance, surface_loader, surface, device) = bootstrap.commit();
        let swapchain_loader = Swapchain::new(&instance, &device);
        let core = generic::Core::new(entry, instance.clone(), physical_device, device.clone(), queue_indices.graphics_family);

        let mut ctx = Self {
            core,
            surface_loader,
            surface,
            physical_device,
            device,
            queue_indices,
            graphics_queue,
            present_queue,
            swapchain_loader,
            swapchain: vk::SwapchainKHR::null(),
            swapchain_images: Vec::new(),
            swapchain_image_views: Vec::new(),
            swapchain_format: vk::Format::UNDEFINED,
            depth_format,
            extent: vk::Extent2D { width, height },
            render_pass: vk::RenderPass::null(),
            framebuffers: Vec::new(),
            command_pool: vk::CommandPool::null(),
            command_buffers: Vec::new(),
            image_available_semaphores: Vec::new(),
            render_finished_semaphores: Vec::new(),
            in_flight_fences: Vec::new(),
            current_frame: 0,
            max_frames_in_flight: 2,
            acquired_frame_state: AcquiredFrameState::Ready,
            depth_images: Vec::new(),
            device_lost: false,
            _window: window,
        };

        ctx.command_pool = ctx.create_command_pool()?;
        ctx.recreate_swapchain(width, height)?;
        ctx.create_sync_objects()?;

        Ok(ctx)
    }

    /// Picks the first physical device that exposes both graphics and present queue families.
    fn select_physical_device(instance: &ash::Instance, surface_loader: &Surface, surface: vk::SurfaceKHR) -> Result<(vk::PhysicalDevice, QueueFamilyIndices)> {
        let devices = unsafe { instance.enumerate_physical_devices() }.map_err(|err| format!("enumerate_physical_devices failed: {err:?}"))?;

        for device in devices {
            if let Some(indices) = Self::find_queue_families(instance, surface_loader, surface, device) {
                return Ok((device, indices));
            }
        }

        Err("no suitable Vulkan physical device found".into())
    }

    /// Finds queue families that can render and present to the supplied surface.
    fn find_queue_families(
        instance: &ash::Instance,
        surface_loader: &Surface,
        surface: vk::SurfaceKHR,
        device: vk::PhysicalDevice,
    ) -> Option<QueueFamilyIndices> {
        let queue_families = unsafe { instance.get_physical_device_queue_family_properties(device) };
        let mut graphics_family = None;
        let mut present_family = None;

        for (index, family) in queue_families.iter().enumerate() {
            if family.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                graphics_family = Some(index as u32);
            }

            let present_support = unsafe {
                surface_loader
                    .get_physical_device_surface_support(device, index as u32, surface)
                    .unwrap_or(false)
            };

            if present_support {
                present_family = Some(index as u32);
            }

            if graphics_family.is_some() && present_family.is_some() {
                break;
            }
        }

        match (graphics_family, present_family) {
            (Some(graphics_family), Some(present_family)) => Some(QueueFamilyIndices { graphics_family, present_family }),
            _ => None,
        }
    }

    /// Creates the logical device and queues needed by the example renderer.
    fn create_logical_device(instance: &ash::Instance, physical_device: vk::PhysicalDevice, indices: &QueueFamilyIndices) -> Result<ash::Device> {
        let unique_indices = if indices.graphics_family == indices.present_family {
            vec![indices.graphics_family]
        } else {
            vec![indices.graphics_family, indices.present_family]
        };

        let queue_priority = 1.0f32;
        let queue_infos: Vec<_> = unique_indices
            .iter()
            .map(|index| {
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(*index)
                    .queue_priorities(std::slice::from_ref(&queue_priority))
            })
            .collect();

        let device_extensions = [khr::swapchain::NAME.as_ptr()];

        let device_features = vk::PhysicalDeviceFeatures::default();
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_infos)
            .enabled_extension_names(&device_extensions)
            .enabled_features(&device_features);

        unsafe { instance.create_device(physical_device, &device_info, None) }.map_err(|err| format!("create_device failed: {err:?}"))
    }

    /// Rebuilds all swapchain-dependent resources for a new window extent.
    fn recreate_swapchain(&mut self, width: u32, height: u32) -> Result<()> {
        if self.device_lost {
            return Err("cannot recreate swapchain: device is lost".into());
        }
        if width == 0 || height == 0 {
            return Ok(());
        }

        unsafe {
            self.device.device_wait_idle().map_err(|err| self.handle_vk_error("device_wait_idle", err))?;
        }

        // Rebuild all resources tied to the window surface size or swapchain format. Permanent
        // objects like the device, queues, and command pool stay alive across this boundary.
        self.cleanup_swapchain();
        self.create_swapchain(width, height)?;
        self.create_present_semaphores()?;
        self.create_image_views()?;
        self.create_depth_images()?;
        self.render_pass = self.create_render_pass()?;
        self.framebuffers = self.create_framebuffers()?;
        self.allocate_command_buffers()?;

        Ok(())
    }

    /// Creates the Vulkan swapchain that backs presentation to the SDL window.
    fn create_swapchain(&mut self, width: u32, height: u32) -> Result<()> {
        let surface_caps = unsafe {
            self.surface_loader
                .get_physical_device_surface_capabilities(self.physical_device, self.surface)
                .map_err(|err| format!("get_surface_capabilities failed: {err:?}"))?
        };

        let formats = unsafe {
            self.surface_loader
                .get_physical_device_surface_formats(self.physical_device, self.surface)
                .map_err(|err| format!("get_surface_formats failed: {err:?}"))?
        };

        let present_modes = unsafe {
            self.surface_loader
                .get_physical_device_surface_present_modes(self.physical_device, self.surface)
                .map_err(|err| format!("get_surface_present_modes failed: {err:?}"))?
        };

        let surface_format = Self::choose_surface_format(&formats);
        let present_mode = Self::choose_present_mode(&present_modes);
        let extent = Self::choose_extent(&surface_caps, width, height);
        self.core
            .image_extent(surface_format.format, extent.width, extent.height, vk::ImageUsageFlags::COLOR_ATTACHMENT)?;
        let mut image_count = (surface_caps.min_image_count + 1).max(2);
        if surface_caps.max_image_count > 0 {
            image_count = image_count.min(surface_caps.max_image_count);
        }

        let queue_family_indices = [self.queue_indices.graphics_family, self.queue_indices.present_family];

        let mut create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(image_count)
            .image_format(surface_format.format)
            .image_color_space(surface_format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .pre_transform(surface_caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE);

        if self.queue_indices.graphics_family != self.queue_indices.present_family {
            create_info = create_info
                .image_sharing_mode(vk::SharingMode::CONCURRENT)
                .queue_family_indices(&queue_family_indices);
        }

        // Do not publish a swapchain until its image list can also be queried. The guard closes
        // the otherwise easy-to-miss leak when `get_swapchain_images` fails.
        let swapchain = ResourceGuard::new(
            unsafe { self.swapchain_loader.create_swapchain(&create_info, None) }.map_err(|err| format!("create_swapchain failed: {err:?}"))?,
            |swapchain| unsafe { self.swapchain_loader.destroy_swapchain(swapchain, None) },
        );
        let images = unsafe { self.swapchain_loader.get_swapchain_images(*swapchain.get()) }.map_err(|err| format!("get_swapchain_images failed: {err:?}"))?;
        let previous = std::mem::replace(&mut self.swapchain, swapchain.into_inner());
        if previous != vk::SwapchainKHR::null() {
            unsafe { self.swapchain_loader.destroy_swapchain(previous, None) };
        }
        self.swapchain_images = images;
        self.swapchain_format = surface_format.format;
        self.extent = extent;

        Ok(())
    }

    /// Creates image views for every swapchain image.
    fn create_image_views(&mut self) -> Result<()> {
        // Keep every newly created view guarded until the full image set succeeds; `collect` over
        // raw Vulkan handles would leak the successful prefix on the first error.
        let mut pending = Vec::with_capacity(self.swapchain_images.len());
        for &image in &self.swapchain_images {
            let components = vk::ComponentMapping {
                r: vk::ComponentSwizzle::IDENTITY,
                g: vk::ComponentSwizzle::IDENTITY,
                b: vk::ComponentSwizzle::IDENTITY,
                a: vk::ComponentSwizzle::IDENTITY,
            };
            let subresource_range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(self.swapchain_format)
                .components(components)
                .subresource_range(subresource_range);
            let view = unsafe { self.device.create_image_view(&view_info, None) }.map_err(|err| format!("create_image_view failed: {err:?}"))?;
            pending.push(ResourceGuard::new(view, |view| unsafe { self.device.destroy_image_view(view, None) }));
        }
        let views = pending.into_iter().map(|view| view.into_inner()).collect();
        for previous in std::mem::replace(&mut self.swapchain_image_views, views) {
            unsafe { self.device.destroy_image_view(previous, None) };
        }
        Ok(())
    }

    /// Builds the shared render pass used for UI and application graphics.
    fn create_render_pass(&self) -> Result<vk::RenderPass> {
        pass::create(
            &self.device,
            self.signature(),
            vk::ImageLayout::PRESENT_SRC_KHR,
            Some(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
        )
    }

    /// Creates one framebuffer per swapchain image view.
    fn create_framebuffers(&self) -> Result<Vec<vk::Framebuffer>> {
        let mut pending = Vec::with_capacity(self.swapchain_image_views.len());
        for (index, &view) in self.swapchain_image_views.iter().enumerate() {
            let depth_view = self.depth_images.get(index).map(|image| image.view()).unwrap_or(vk::ImageView::null());
            let attachments = [view, depth_view];
            let framebuffer_info = vk::FramebufferCreateInfo::default()
                .render_pass(self.render_pass)
                .attachments(&attachments)
                .width(self.extent.width)
                .height(self.extent.height)
                .layers(1);
            let framebuffer =
                unsafe { self.device.create_framebuffer(&framebuffer_info, None) }.map_err(|err| format!("create_framebuffer failed: {err:?}"))?;
            pending.push(ResourceGuard::new(framebuffer, |framebuffer| unsafe {
                self.device.destroy_framebuffer(framebuffer, None)
            }));
        }
        Ok(pending.into_iter().map(|framebuffer| framebuffer.into_inner()).collect())
    }

    /// Creates the shared command pool used for graphics and transient copy work.
    fn create_command_pool(&self) -> Result<vk::CommandPool> {
        let pool_info = vk::CommandPoolCreateInfo::default()
            .queue_family_index(self.queue_indices.graphics_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        unsafe { self.device.create_command_pool(&pool_info, None) }.map_err(|err| format!("create_command_pool failed: {err:?}"))
    }

    /// Each reusable graphics command buffer belongs to its waited frame-slot fence.
    fn allocate_command_buffers(&mut self) -> Result<()> {
        let alloc_info = vk::CommandBufferAllocateInfo::default()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(self.max_frames_in_flight as u32);
        // Allocate replacement buffers before releasing the currently valid set.
        let buffers = unsafe { self.device.allocate_command_buffers(&alloc_info) }.map_err(|err| format!("allocate_command_buffers failed: {err:?}"))?;
        let previous = std::mem::replace(&mut self.command_buffers, buffers);
        if !previous.is_empty() {
            unsafe { self.device.free_command_buffers(self.command_pool, &previous) };
        }
        Ok(())
    }

    /// Swapchain image ownership, rather than frame-slot graphics completion, protects present waits.
    fn create_present_semaphores(&mut self) -> Result<()> {
        let mut candidates = Vec::new();
        candidates.try_reserve_exact(self.swapchain_images.len()).map_err(|error| error.to_string())?;
        for _ in &self.swapchain_images {
            let raw = unsafe { self.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }
                .map_err(|error| format!("create presentation semaphore: {error:?}"))?;
            candidates.push(ResourceGuard::new(raw, |raw| unsafe { self.device.destroy_semaphore(raw, None) }));
        }
        self.render_finished_semaphores = candidates.into_iter().map(ResourceGuard::into_inner).collect();
        Ok(())
    }

    /// Per-frame fences protect command buffers, acquire waits and transfer completion.
    fn create_sync_objects(&mut self) -> Result<()> {
        let semaphore_info = vk::SemaphoreCreateInfo::default();
        let fence_info = vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);

        // Build a complete replacement set under guards. A failure in any frame slot destroys all
        // semaphores/fences created for earlier slots and preserves the installed set.
        let mut image_available = Vec::with_capacity(self.max_frames_in_flight);
        let mut fences = Vec::with_capacity(self.max_frames_in_flight);

        for _ in 0..self.max_frames_in_flight {
            unsafe {
                let image_available_handle = self
                    .device
                    .create_semaphore(&semaphore_info, None)
                    .map_err(|err| format!("create_semaphore failed: {err:?}"))?;
                let image_available_guard = ResourceGuard::new(image_available_handle, |semaphore| self.device.destroy_semaphore(semaphore, None));
                let fence_handle = self
                    .device
                    .create_fence(&fence_info, None)
                    .map_err(|err| format!("create_fence failed: {err:?}"))?;
                let fence_guard = ResourceGuard::new(fence_handle, |fence| self.device.destroy_fence(fence, None));

                image_available.push(image_available_guard);
                fences.push(fence_guard);
            }
        }

        let image_available = image_available.into_iter().map(|guard| guard.into_inner()).collect();
        let fences = fences.into_iter().map(|guard| guard.into_inner()).collect();
        for semaphore in std::mem::replace(&mut self.image_available_semaphores, image_available) {
            unsafe { self.device.destroy_semaphore(semaphore, None) };
        }
        for fence in std::mem::replace(&mut self.in_flight_fences, fences) {
            unsafe { self.device.destroy_fence(fence, None) };
        }

        Ok(())
    }

    /// Destroys resources that depend on the current swapchain and framebuffer set.
    fn cleanup_swapchain(&mut self) {
        unsafe {
            for semaphore in self.render_finished_semaphores.drain(..) {
                self.device.destroy_semaphore(semaphore, None);
            }
            // Only swapchain-dependent resources are destroyed here. The permanent command pool,
            // device, queues, and synchronization objects live until `Drop`.
            for &framebuffer in &self.framebuffers {
                self.device.destroy_framebuffer(framebuffer, None);
            }
            self.framebuffers.clear();

            if !self.command_buffers.is_empty() {
                self.device.free_command_buffers(self.command_pool, &self.command_buffers);
                self.command_buffers.clear();
            }

            for &view in &self.swapchain_image_views {
                self.device.destroy_image_view(view, None);
            }
            self.swapchain_image_views.clear();
            for mut depth in self.depth_images.drain(..) {
                depth.destroy(&self.device);
            }

            if self.render_pass != vk::RenderPass::null() {
                self.device.destroy_render_pass(self.render_pass, None);
                self.render_pass = vk::RenderPass::null();
            }

            if self.swapchain != vk::SwapchainKHR::null() {
                self.swapchain_loader.destroy_swapchain(self.swapchain, None);
                self.swapchain = vk::SwapchainKHR::null();
            }
        }
    }

    /// Acquires one swapchain image before higher-level display-list execution begins.
    fn acquire_frame(&mut self, width: u32, height: u32) -> Result<AcquiredVulkanFrame> {
        if self.device_lost {
            return Err("device is lost; VulkanContext is disabled".into());
        }
        // Never reuse a binary acquire semaphore or swapchain image left outstanding by an earlier
        // post-acquire failure. The fatal policy makes this check finite instead of waiting on an
        // unsignaled fence that was reset but never submitted.
        self.acquired_frame_state.ensure_ready()?;
        let frame = self.current_frame;
        let fence = self.in_flight_fences[frame];
        unsafe {
            self.device
                .wait_for_fences(&[fence], true, u64::MAX)
                .map_err(|err| self.handle_vk_error("wait_for_fences", err))?;
        }

        match unsafe {
            self.swapchain_loader
                .acquire_next_image(self.swapchain, u64::MAX, self.image_available_semaphores[frame], vk::Fence::null())
        } {
            Ok((image_index, suboptimal)) => {
                // Publish the ownership transition only after Vulkan has returned a real image and
                // signaled this frame slot's acquire semaphore.
                self.acquired_frame_state.acquire_succeeded()?;
                Ok(AcquiredVulkanFrame { frame, image_index, suboptimal })
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) | Err(vk::Result::SUBOPTIMAL_KHR) => {
                self.recreate_swapchain(width, height)?;
                Err(String::from("Vulkan swapchain changed during frame acquisition"))
            }
            Err(err) => Err(self.handle_vk_error("acquire_next_image", err)),
        }
    }

    /// Executes one complete Vulkan frame from an acquired image through submit and present.
    #[allow(clippy::too_many_arguments)] // Frame recording keeps Vulkan state explicit at the call boundary.
    fn draw_frame(
        &mut self,
        acquired: AcquiredVulkanFrame,
        clear_value: vk::ClearValue,
        width: u32,
        height: u32,
        commands: &mut Vec<PreparedBlock>,
        generic: &mut generic::Executor,
    ) -> Result<()> {
        // Every exit after successful acquisition passes through the lifecycle commit below. Any
        // error permanently latches the context before the caller can attempt another frame with a
        // signaled acquire semaphore, an unpresented image, or an unsignaled unsubmitted fence.
        let result = self.draw_acquired_frame(acquired, clear_value, width, height, commands, generic);

        // Success proves graphics submission and presentation consumed the acquired resources.
        // Every error latches Fatal, including record, transfer, fence-reset, submit, present, and
        // swapchain-rebuild failures.
        self.acquired_frame_state.finish(result)
    }

    /// Performs the fallible native work for one already-acquired swapchain image.
    #[allow(clippy::too_many_arguments)] // Frame recording keeps Vulkan state explicit at the call boundary.
    fn draw_acquired_frame(
        &mut self,
        acquired: AcquiredVulkanFrame,
        clear_value: vk::ClearValue,
        width: u32,
        height: u32,
        commands: &mut Vec<PreparedBlock>,
        generic: &mut generic::Executor,
    ) -> Result<()> {
        if self.device_lost {
            return Err("device is lost; VulkanContext is disabled".into());
        }
        // `draw_frame` receives the image acquired by `RendererBackend::frame`; by the time it
        // runs, the higher-level renderer has collected UI vertices plus ordered custom jobs.
        let AcquiredVulkanFrame { frame, image_index, suboptimal } = acquired;
        debug_assert_eq!(frame, self.current_frame);
        let fence = self.in_flight_fences[frame];

        generic.collect()?;
        let mut submitted = Vec::new();
        submitted.try_reserve_exact(commands.len()).map_err(|error| error.to_string())?;
        submitted.extend(commands.iter().cloned());

        // Reset per-frame upload cursors only after the frame fence signaled. Retired GPU buffers
        // are likewise only reclaimed once this frame slot is no longer in flight.

        let mut swapchain_needs_recreate = suboptimal;

        // UI drawing and custom rendering share one graphics command buffer per acquired image.
        let command_buffer = self.command_buffers[frame];
        self.record_command_buffer(command_buffer, image_index, clear_value, width, height, commands, generic)?;
        let wait_semaphores = [self.image_available_semaphores[frame]];
        // Include depth attachment work when reusing the acquired image's private depth backing.
        let wait_stages = [vk::PipelineStageFlags::ALL_GRAPHICS];
        let signal_semaphores = [self.render_finished_semaphores[image_index as usize]];

        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(&wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(std::slice::from_ref(&command_buffer))
            .signal_semaphores(&signal_semaphores);
        let submit_infos = [submit_info];

        unsafe {
            // Keep the fence signaled throughout every fallible CPU recording and transfer-submit
            // step. Reset it only when the graphics submission is fully assembled. If reset or
            // queue submission fails, the outer acquired-frame transaction becomes fatal and no
            // later frame can wait on or reuse this slot.
            self.device.reset_fences(&[fence]).map_err(|err| self.handle_vk_error("reset_fences", err))?;
            // Install native owners before the driver can observe this submission. An error
            // leaves them retained under the terminal acquired-frame state until idle/teardown.
            generic.submitted(fence, submitted);
            self.device
                .queue_submit(self.graphics_queue, &submit_infos, fence)
                .map_err(|err| self.handle_vk_error("queue_submit", err))?;
        }

        let swapchains = [self.swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&signal_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);

        let present_result = unsafe { self.swapchain_loader.queue_present(self.present_queue, &present_info) };
        match present_result {
            Ok(present_suboptimal) => {
                if present_suboptimal {
                    swapchain_needs_recreate = true;
                }
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                self.recreate_swapchain(width, height)?;
                self.current_frame = (self.current_frame + 1) % self.max_frames_in_flight;
                return Ok(());
            }
            Err(vk::Result::SUBOPTIMAL_KHR) => {
                self.recreate_swapchain(width, height)?;
                self.current_frame = (self.current_frame + 1) % self.max_frames_in_flight;
                return Ok(());
            }
            Err(err) => return Err(self.handle_vk_error("queue_present", err)),
        }

        if swapchain_needs_recreate {
            self.recreate_swapchain(width, height)?;
        }

        self.current_frame = (self.current_frame + 1) % self.max_frames_in_flight;

        Ok(())
    }

    /// Records the graphics command buffer by replaying queued UI and custom commands in order.
    #[allow(clippy::too_many_arguments)] // Command recording keeps Vulkan state explicit at the call boundary.
    fn record_command_buffer(
        &mut self,
        command_buffer: vk::CommandBuffer,
        image_index: u32,
        clear_value: vk::ClearValue,
        width: u32,
        height: u32,
        commands: &mut Vec<PreparedBlock>,
        generic: &mut generic::Executor,
    ) -> Result<()> {
        // GPU command recording is intentionally linear: begin render pass, replay the queued UI /
        // custom jobs in order, then end the pass. The `commands` vec is drained here so the
        // higher-level renderer can reuse its allocation next frame.
        for block in commands.iter() {
            generic.admit(block, self.signature())?;
        }
        let begin_info = vk::CommandBufferBeginInfo::default();
        unsafe {
            self.device
                .begin_command_buffer(command_buffer, &begin_info)
                .map_err(|err| format!("begin_command_buffer failed: {err:?}"))?;
        }

        let depth_clear = vk::ClearValue {
            depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 },
        };
        let clear_values = [clear_value, depth_clear];
        let render_pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(self.render_pass)
            .framebuffer(self.framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            })
            .clear_values(&clear_values);

        unsafe {
            self.device
                .cmd_begin_render_pass(command_buffer, &render_pass_info, vk::SubpassContents::INLINE);
        }

        let logical = neocogi_render_gpu::abi::Size {
            width: width as i32,
            height: height as i32,
        };
        let physical = neocogi_render_gpu::abi::Size {
            width: self.extent.width as i32,
            height: self.extent.height as i32,
        };
        for block in commands.drain(..) {
            generic.record(command_buffer, &block, logical, physical, self.signature())?;
        }

        unsafe {
            self.device.cmd_end_render_pass(command_buffer);
            self.device
                .end_command_buffer(command_buffer)
                .map_err(|err| format!("end_command_buffer failed: {err:?}"))?;
        }

        Ok(())
    }

    /// Chooses the preferred swapchain surface format, falling back to the first available one.
    fn choose_surface_format(available_formats: &[vk::SurfaceFormatKHR]) -> vk::SurfaceFormatKHR {
        available_formats
            .iter()
            .cloned()
            .find(|format| format.format == vk::Format::B8G8R8A8_UNORM)
            .unwrap_or_else(|| available_formats[0])
    }

    /// Finds a depth format supported for depth-stencil attachments on the selected device.
    fn find_depth_format(instance: &ash::Instance, physical_device: vk::PhysicalDevice) -> Result<vk::Format> {
        let candidates = [vk::Format::D32_SFLOAT, vk::Format::D32_SFLOAT_S8_UINT, vk::Format::D24_UNORM_S8_UINT];
        for &format in &candidates {
            let props = unsafe { instance.get_physical_device_format_properties(physical_device, format) };
            if props.optimal_tiling_features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT) {
                return Ok(format);
            }
        }
        Err("no supported depth format found".into())
    }

    /// Prefers mailbox presentation when available, otherwise falls back to FIFO.
    fn choose_present_mode(available_present_modes: &[vk::PresentModeKHR]) -> vk::PresentModeKHR {
        if available_present_modes.contains(&vk::PresentModeKHR::MAILBOX) {
            vk::PresentModeKHR::MAILBOX
        } else {
            vk::PresentModeKHR::FIFO
        }
    }

    /// Resolves the swapchain extent from surface caps and the requested window size.
    fn choose_extent(capabilities: &vk::SurfaceCapabilitiesKHR, width: u32, height: u32) -> vk::Extent2D {
        if capabilities.current_extent.width != u32::MAX {
            capabilities.current_extent
        } else {
            vk::Extent2D {
                width: width.clamp(capabilities.min_image_extent.width, capabilities.max_image_extent.width),
                height: height.clamp(capabilities.min_image_extent.height, capabilities.max_image_extent.height),
            }
        }
    }

    /// Converts a Vulkan error into a user-facing string and latches device-lost state.
    fn handle_vk_error(&mut self, op: &str, err: vk::Result) -> String {
        if err == vk::Result::ERROR_DEVICE_LOST {
            self.device_lost = true;
            format!("{op} failed: {err:?} (device lost)")
        } else {
            format!("{op} failed: {err:?}")
        }
    }

    /// Latches an acquired frame that escaped normal Result-based finalization.
    fn abandon_acquired_frame(&mut self) {
        // VulkanFrame's panic guard calls this before allowing another backend operation.
        self.acquired_frame_state.abandon();
    }

    /// Returns whether native rendering must remain disabled for the rest of this context's life.
    fn is_unusable(&self) -> bool {
        // Actual device loss and an abandoned acquired-frame transaction are both permanent here;
        // this example deliberately chooses a simple fatal policy instead of partial sync repair.
        self.device_lost || self.acquired_frame_state.is_fatal()
    }

    /// Returns the current swapchain extent.
    fn extent(&self) -> vk::Extent2D {
        self.extent
    }
    /// Shares guarded RGBA image ownership while keeping render and upload usage explicit.
    fn create_image_with_usage(&self, width: u32, height: u32, usage: vk::ImageUsageFlags) -> Result<ImageResource> {
        let size = neocogi_render_gpu::abi::Size {
            width: width as i32,
            height: height as i32,
        };
        let native = generic::Image::allocate(&self.core, size, vk::Format::R8G8B8A8_UNORM, usage, vk::ImageAspectFlags::COLOR)?;
        Ok(ImageResource {
            native: Some(native),
            extent: vk::Extent2D { width, height },
            layout: vk::ImageLayout::UNDEFINED,
        })
    }

    /// Recreates the per-swapchain-image depth attachments for the current extent.
    fn create_depth_images(&mut self) -> Result<()> {
        // A failed nth attachment must clean the first n-1 and leave the installed set alone.
        let mut attachments = Vec::with_capacity(self.swapchain_images.len());
        for _ in &self.swapchain_images {
            attachments.push(self.create_depth_attachment(self.extent)?.guarded(&self.device));
        }
        let attachments = attachments.into_iter().map(ResourceGuard::into_inner).collect();
        for mut previous in std::mem::replace(&mut self.depth_images, attachments) {
            previous.destroy(&self.device);
        }
        Ok(())
    }

    /// Creates one depth attachment image and transitions it into attachment layout.
    fn create_depth_attachment(&self, extent: vk::Extent2D) -> Result<ImageResource> {
        let aspect = if Self::has_stencil_component(self.depth_format) {
            vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL
        } else {
            vk::ImageAspectFlags::DEPTH
        };
        let size = neocogi_render_gpu::abi::Size {
            width: extent.width as i32,
            height: extent.height as i32,
        };
        let native = generic::Image::allocate(&self.core, size, self.depth_format, vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT, aspect)?;
        Ok(ImageResource {
            native: Some(native),
            extent,
            layout: vk::ImageLayout::UNDEFINED,
        })
    }

    /// Returns whether the chosen depth format also carries a stencil component.
    fn has_stencil_component(format: vk::Format) -> bool {
        matches!(format, vk::Format::D32_SFLOAT_S8_UINT | vk::Format::D24_UNORM_S8_UINT)
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            // The drop order mirrors creation order in reverse: wait for the device to go idle,
            // destroy per-frame sync/swapchain resources, then tear down the command pool, surface,
            // device, and finally the instance.
            self.core.wait_idle();

            for &fence in &self.in_flight_fences {
                self.device.destroy_fence(fence, None);
            }
            for &semaphore in &self.image_available_semaphores {
                self.device.destroy_semaphore(semaphore, None);
            }
            self.cleanup_swapchain();

            if self.command_pool != vk::CommandPool::null() {
                self.device.destroy_command_pool(self.command_pool, None);
            }

            self.surface_loader.destroy_surface(self.surface, None);
        }
    }
}

#[derive(Clone, Copy)]
struct QueueFamilyIndices {
    graphics_family: u32,
    present_family: u32,
}
impl VulkanContext {}

#[cfg(test)]
mod tests {

    use super::*;

    /// Verifies every post-acquire failure and panic permanently closes the frame lifecycle.
    #[test]
    fn acquired_frame_failure_is_fatal_while_success_returns_to_ready() {
        let mut state = AcquiredFrameState::Ready;
        state.ensure_ready().unwrap();
        state.acquire_succeeded().unwrap();
        assert_eq!(state, AcquiredFrameState::Acquired);

        let error = state.finish::<()>(Err(String::from("injected record failure"))).unwrap_err();
        assert_eq!(error, "injected record failure");
        assert!(state.is_fatal());
        assert!(state.ensure_ready().is_err(), "a failed acquired frame must reject all later acquisition");

        let mut successful = AcquiredFrameState::Ready;
        successful.acquire_succeeded().unwrap();
        successful.finish(Ok(())).unwrap();
        assert_eq!(successful, AcquiredFrameState::Ready);

        successful.acquire_succeeded().unwrap();
        successful.abandon();
        assert!(successful.is_fatal(), "panic-style abandonment must use the same fatal latch");
    }
}
