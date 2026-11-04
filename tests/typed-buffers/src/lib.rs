//! Real generated ABI exercise for an application-defined typed wrapper.
extern crate self as fixture_typed_buffers;

#[cfg(test)]
mod tests {
    use neocogi_com::*;
    use neocogi_render_core::{
        abi::*,
        buffer::{GpuElement, TypedGpuBuffer, TypedGpuBufferFactory, TypedGpuBufferDesc},
    };

    /// Independently generated application definitions import the driver's existing Rust ABI.
    #[allow(dead_code)]
    pub(crate) mod typed_contracts {
        include!(env!("IDL_TESTS_TYPED_BUFFER"));
    }
    use typed_contracts::{IPoints, IPointsAbi, IPointsVtblFor, Point};
    include!(env!("IDL_TYPED_BUFFER_TRAIT"));

    // SAFETY: Two initialized f32 fields have no padding, ownership or restricted bits;
    // this application's explicit pipeline uses their corresponding eight-byte layout.
    unsafe impl GpuElement for Point {}

    #[com_object(interfaces(IPoints), ownership = exclusive)]
    /// The wrapper owns the original authenticated driver object and no native backend state.
    struct Points {
        /// Exact object returned by the untyped resource factory.
        inner: TypedGpuBuffer<Point>,
    }
    impl IPoints for Points {}
    impl IRenderDriverBound for Points {
        fn driver_identity(&self) -> Result<ComPtr<IRenderDriverIdentityAbi>, NcStatus> {
            self.inner.as_gpu_buffer().driver_identity()
        }
    }
    impl IGpuBuffer for Points {
        fn info(&self) -> Result<GpuBufferInfo, NcStatus> {
            self.inner.as_gpu_buffer().info()
        }
        fn upload(&mut self, data: NcCopyFromBufferOffset<'_, u8>) -> Result<(), NcStatus> {
            self.inner.as_gpu_buffer().upload(data)
        }
        fn read(&self, output: NcOutputWriter<'_, u8>) -> Result<(), NcStatus> {
            self.inner.as_gpu_buffer().read(output.forward()?)
        }
    }
    impl TypedBufferContract for Points {
        fn element_count(&self) -> Result<u64, NcStatus> {
            Ok(self.inner.element_count())
        }
        fn element_stride(&self) -> Result<u64, NcStatus> {
            Ok(8)
        }
        fn underlying_buffer(&self) -> Result<WeakComBox<IGpuBufferAbi>, NcStatus> {
            Ok(self.inner.underlying_buffer())
        }
        fn upload_elements(&mut self, data: NcCopyFromBufferOffset<'_, Point>) -> Result<(), NcStatus> {
            self.inner.upload(data.offset(), data.as_slice())
        }
        fn read_elements(&self, mut output: NcOutputWriter<'_, Point>) -> Result<(), NcStatus> {
            // This application chooses a typed scratch vector for COM codec conversion.
            // It is caller-independent CPU storage, never a new driver/GPU buffer.
            let mut values = vec![Point { x: 0., y: 0. }; output.capacity()];
            self.inner.read(output.offset(), &mut values)?;
            output.extend_from_slice(&values)
        }
    }

    /// Generic inheritance preserves copied transfers and the independently registered original identity.
    #[test]
    fn application_specialization_transfers_real_storage_and_exposes_its_driver_owner() {
        let gpu = neocogi_render_gpu::testing::device(
            nc_shader_compiler::CompilerObject::from_value(nc_shader_compiler::CompilerObject)
                .query()
                .unwrap(),
        );
        let domain = gpu
            .create_client(
                Default::default(),
                gpu.surface_target(RenderSurfaceKind::Framebuffer).unwrap(),
                Default::default(),
            )
            .unwrap();
        let resources = domain.resources().unwrap();
        let buffer = TypedGpuBufferFactory::new(resources.clone())
            .create_dynamic::<Point>(TypedGpuBufferDesc::Vertex { element_count: 4 })
            .unwrap();
        let original = buffer.as_gpu_buffer().as_raw().object as usize;
        let mut wrapper = Points::into_box(Points { inner: buffer }).query::<IPointsAbi>().unwrap();
        assert_ne!(wrapper.as_raw().object as usize, original);
        assert!(matches!(
            gpu.buffer_snapshot(resources.as_com_ref(), wrapper.query_ref::<IGpuBufferAbi>().unwrap()),
            Err(RENDER_E_OWNER)
        ));
        let underlying = wrapper.underlying_buffer().unwrap();
        assert_eq!(underlying.as_raw().object as usize, original);
        assert_eq!(wrapper.element_count().unwrap(), 4);
        assert_eq!(wrapper.element_stride().unwrap(), 8);
        wrapper.upload_elements(NcCopyFromBufferOffset::new(2, &[Point { x: 3., y: 5. }])).unwrap();
        let mut output = NcOutputBuffer::new();
        wrapper.read_elements(output.output(2, 1)).unwrap();
        assert_eq!((output.as_slice()[0].x, output.as_slice()[0].y), (3., 5.));
        let mut bytes = NcOutputBuffer::new();
        underlying.read(bytes.output(16, 8)).unwrap();
        assert_eq!(bytes.as_slice(), &[3_f32.to_le_bytes(), 5_f32.to_le_bytes()].concat());
        assert_eq!(
            wrapper.upload_elements(NcCopyFromBufferOffset::new(4, &[Point { x: 1., y: 2. }])),
            Err(NC_E_OUT_OF_RANGE)
        );
        assert_eq!(wrapper.read_elements(output.output(u64::MAX, 1)), Err(NC_E_OUT_OF_RANGE));
        assert!(output.is_empty());
        wrapper.read_elements(output.output(2, 1)).unwrap();
        drop(wrapper);
        assert_eq!(output.as_slice()[0].y, 5.);
        assert!(underlying.info().is_err());
    }
}
