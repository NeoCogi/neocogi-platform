//! Bounded atlas pixels copied for upload without importing UI resource metadata.
use super::*;

/// Temporary CPU storage borrowed only during one synchronous GPU upload.
pub(super) struct AtlasPixels {
    /// Checked positive dimensions from the bundle header.
    size: Size,
    /// Exactly width × height × 4 initialized RGBA bytes.
    pixels: Vec<u8>,
}

impl AtlasPixels {
    /// Reads only allocation bounds and pixels; fonts, names and appearances belong to the UI.
    pub(super) fn read(source: ComRef<'_, IRgbaImageAbi>) -> Result<Self, NcStatus> {
        let size = source.size()?;
        let count = neocogi_renderer_contracts::rgba_bytes(size)?;
        let mut output = neocogi_com::NcOutputBuffer::new();
        // Header bounds precede reservation; short copies cannot reach a GPU upload.
        source.pixels(output.output(0, count))?;
        if output.len() != count { return Err(neocogi_com::NC_E_INVALID_ARG); }
        let pixels = output.into_vec();
        Ok(Self { size, pixels })
    }

    /// Lends packed bytes to a backend without manufacturing a native atlas or color vector.
    pub(super) fn image(&self) -> neocogi_render_core::ImageSource<'_> {
        // Backends complete upload synchronously and retain only their GPU texture afterwards.
        neocogi_render_core::ImageSource {
            width: self.size.width,
            height: self.size.height,
            pixels: &self.pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neocogi_com::NcOutputWriter;

    #[com_object(interfaces(IRgbaImage))]
    /// A renderer-only image proves that upload has no dependency on UI metadata contracts.
    struct PixelSource {
        /// Header dimensions exercised before any pixel copy.
        size: Size,
        /// Shared observation proves rejected headers do not reach foreign copy code.
        copies: Rc<Cell<u32>>,
        /// Simulates a recoverable transfer failure after successful header validation.
        fail_copy: bool,
    }
    impl IRgbaImage for PixelSource {
        /// Supplies only dimensions needed to bound the packed allocation.
        fn size(&self) -> Result<Size, NcStatus> {
            Ok(self.size)
        }
        /// Copies one known texel or propagates the requested source failure.
        fn pixels(&self, mut output: NcOutputWriter<'_, u8>) -> Result<(), NcStatus> {
            self.copies.set(self.copies.get() + 1);
            if self.fail_copy {
                return Err(neocogi_com::NC_E_LOAD_FAILED);
            }
            assert_eq!((output.offset(), output.capacity()), (0, 4));
            output.extend_from_slice(&[11, 22, 33, 44])?;
            Ok(())
        }
    }

    /// Pixel transfer succeeds with no font, theme, catalog, or UI interface at all.
    #[test]
    fn pixel_import_has_no_ui_metadata_dependency() {
        let copies = Rc::new(Cell::new(0));
        let source: ComPtr<IRgbaImageAbi> = PixelSource::from_value(PixelSource {
            size: Size { width: 1, height: 1 },
            copies: copies.clone(),
            fail_copy: false,
        })
        .query()
        .unwrap();
        let pixels = AtlasPixels::read(source.as_com_ref()).unwrap();
        assert_eq!((pixels.image().width, pixels.image().height), (1, 1));
        assert_eq!(pixels.image().pixels, &[11, 22, 33, 44]);
        assert_eq!(copies.get(), 1);
    }

    /// Invalid dimensions fail before a foreign copy, and copy errors keep their exact status.
    #[test]
    fn pixel_import_bounds_allocations_and_preserves_transfer_errors() {
        let copies = Rc::new(Cell::new(0));
        for size in [Size { width: 0, height: 1 }, Size { width: -1, height: 1 }, Size { width: 4096, height: 4097 }] {
            let source: ComPtr<IRgbaImageAbi> = PixelSource::from_value(PixelSource {
                size,
                copies: copies.clone(),
                fail_copy: false,
            })
            .query()
            .unwrap();
            assert_eq!(AtlasPixels::read(source.as_com_ref()).err().unwrap(), NC_E_INVALID_ARG);
        }
        assert_eq!(copies.get(), 0);
        let source: ComPtr<IRgbaImageAbi> = PixelSource::from_value(PixelSource {
            size: Size { width: 1, height: 1 },
            copies: copies.clone(),
            fail_copy: true,
        })
        .query()
        .unwrap();
        assert_eq!(AtlasPixels::read(source.as_com_ref()).err().unwrap(), neocogi_com::NC_E_LOAD_FAILED);
        assert_eq!(copies.get(), 1);
    }
}
