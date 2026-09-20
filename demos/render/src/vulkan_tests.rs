//! Native allocation/lifetime checks exercise the real upload path without a window or GPU draws.

use super::*;
use crate::MeshSubmission;
use rs_math3d::{Vec2f, Vec3f};

/// Minimal Vulkan owner; tests drop every uploaded mesh before dropping this device.
struct TestDevice {
    /// Parent instance must outlive its logical device.
    instance: ash::Instance,
    /// Device used for actual coherent vertex/index allocation and upload.
    device: ash::Device,
    /// Memory properties belonging to the selected physical device.
    memory_properties: vk::PhysicalDeviceMemoryProperties,
}

impl TestDevice {
    /// Opens a graphics-capable device without surfaces, swapchains, or optional extensions.
    fn new() -> Self {
        let entry = ash::Entry::linked();
        // SAFETY: All create-info arrays remain live for their synchronous creation calls.
        let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default(), None) }.unwrap();
        let (physical, family) = unsafe { instance.enumerate_physical_devices() }
            .unwrap()
            .into_iter()
            .find_map(|physical| {
                unsafe { instance.get_physical_device_queue_family_properties(physical) }
                    .iter()
                    .position(|queue| queue.queue_count > 0 && queue.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                    .map(|family| (physical, family as u32))
            })
            .expect("a graphics-capable Vulkan device is required");
        let priorities = [1.0];
        let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
        let device = unsafe { instance.create_device(physical, &vk::DeviceCreateInfo::default().queue_create_infos(&queues), None) }.unwrap();
        let memory_properties = unsafe { instance.get_physical_device_memory_properties(physical) };
        Self { instance, device, memory_properties }
    }

    /// Simulates preparation after one of two slots has completed; no GPU commands are submitted.
    fn prepare(&self, renderer: &mut DemoRenderer, slot: usize, commands: &[&DemoCommand]) {
        renderer.prepare_meshes(&self.device, &self.memory_properties, slot, 2, commands).unwrap();
    }
}

impl Drop for TestDevice {
    /// No commands are submitted by these tests, and local declaration order drops buffers first.
    fn drop(&mut self) {
        // SAFETY: Every child allocation has been released and no work can still reference it.
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Each call returns distinct immutable allocations, even when the triangle values are identical.
fn triangle() -> MeshBuffers {
    let vertices = [(-0.5, -0.5), (0.5, -0.5), (0., 0.5)]
        .map(|(x, y)| MeshVertex {
            position: Vec3f::new(x, y, 0.).into(),
            normal: Vec3f::new(0., 0., 1.).into(),
            uv: Vec2f::new(0., 0.).into(),
        })
        .to_vec();
    MeshBuffers::from_vecs(vertices, vec![0, 1, 2])
}

/// Builds an ordinary scene snapshot with geometry independent of its copied transforms.
fn command(mesh: &MeshBuffers) -> DemoCommand {
    DemoCommand::DrawMesh(MeshSubmission {
        mesh: mesh.clone(),
        pvm: Mat4f::identity(),
        view_model: Mat4f::identity(),
    })
}

/// One upload survives slot changes, hiding and reordering, then retires after the final slot.
#[test]
#[ignore = "requires a Vulkan device; run explicitly with --include-ignored"]
fn immutable_mesh_uploads_are_shared_and_retired_after_all_frame_references() {
    let gpu = TestDevice::new();
    let mut renderer = DemoRenderer::new();
    let first = triangle();
    let first_command = command(&first);
    gpu.prepare(&mut renderer, 0, &[&first_command, &first_command]);
    assert_eq!(renderer.meshes.len(), 1, "duplicate draws upload only one vertex/index pair");
    let first_upload = Rc::downgrade(&renderer.meshes[0]);
    let vertex = renderer.meshes[0].vertex.buffer;
    let index = renderer.meshes[0].index.buffer;
    assert_eq!(renderer.meshes[0].index_count, 3);
    assert!(Rc::ptr_eq(
        renderer.prepared_frames[0][0].as_ref().unwrap(),
        renderer.prepared_frames[0][1].as_ref().unwrap()
    ));

    // Matrix edits and alternating synchronized slots must reuse the exact native buffers.
    let mut moved = command(&first);
    let DemoCommand::DrawMesh(submission) = &mut moved;
    submission.pvm.col[3].x = 0.25;
    for slot in [1, 0, 1, 0] {
        gpu.prepare(&mut renderer, slot, &[&moved]);
        let prepared = renderer.prepared_frames[slot][0].as_ref().unwrap();
        assert_eq!(Rc::as_ptr(prepared), first_upload.as_ptr());
        assert_eq!(prepared.vertex.buffer, vertex);
        assert_eq!(prepared.index.buffer, index);
    }

    // Complete both slots without drawing the mesh. The retained scene still owns its upload.
    gpu.prepare(&mut renderer, 1, &[]);
    gpu.prepare(&mut renderer, 0, &[]);
    assert!(first_upload.upgrade().is_some());
    gpu.prepare(&mut renderer, 1, &[&first_command]);
    assert_eq!(Rc::as_ptr(renderer.prepared_frames[1][0].as_ref().unwrap()), first_upload.as_ptr());

    // New immutable arrays require another upload; reordering cannot confuse the two identities.
    let second = triangle();
    let second_command = command(&second);
    gpu.prepare(&mut renderer, 0, &[&second_command, &first_command]);
    assert_eq!(renderer.meshes.len(), 2);
    let second_upload = Rc::downgrade(renderer.prepared_frames[0][0].as_ref().unwrap());
    assert_ne!(first_upload.as_ptr(), second_upload.as_ptr());
    gpu.prepare(&mut renderer, 1, &[&first_command, &second_command]);
    assert_eq!(Rc::as_ptr(renderer.prepared_frames[1][0].as_ref().unwrap()), first_upload.as_ptr());
    assert_eq!(Rc::as_ptr(renderer.prepared_frames[1][1].as_ref().unwrap()), second_upload.as_ptr());

    // Removing every CPU owner evicts the first mesh, but the other submitted slot protects it.
    drop((first, first_command, moved));
    gpu.prepare(&mut renderer, 0, &[&second_command]);
    assert_eq!(renderer.meshes.len(), 1);
    assert!(first_upload.upgrade().is_some(), "slot 1 must retain the retired upload");
    gpu.prepare(&mut renderer, 1, &[&second_command]);
    assert!(first_upload.upgrade().is_none(), "the completed final slot releases the retired buffers");

    // Empty meshes preserve command positions and allocate nothing while previous GPU reads retire.
    drop((second, second_command));
    let empty = command(&MeshBuffers::from_vecs(Vec::new(), Vec::new()));
    gpu.prepare(&mut renderer, 0, &[&empty]);
    assert!(renderer.meshes.is_empty());
    assert!(renderer.prepared_frames[0][0].is_none());
    assert!(second_upload.upgrade().is_some());
    gpu.prepare(&mut renderer, 1, &[]);
    assert!(second_upload.upgrade().is_none());
}
