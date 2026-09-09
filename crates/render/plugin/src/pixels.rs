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
    pub(super) fn read(source: ComRef<'_, IAssetBundleAbi>) -> Result<Self, NcStatus> {
        let info = source.info()?;
        let count = info.rgba_bytes()?;
        let mut pixels = Vec::new();
        // Bounds are checked before allocation, and a failed foreign copy remains unpublished.
        pixels.try_reserve_exact(count).map_err(|_| neocogi_com::NC_E_OUT_OF_MEMORY)?;
        pixels.resize(count, 0);
        source.pixels(neocogi_com::NcCopyToBufferOffset::new(0, &mut pixels))?;
        Ok(Self { size: info.size, pixels })
    }

    /// Lends packed bytes to a backend without manufacturing a native atlas or color vector.
    pub(super) fn image(&self) -> neocogi_ui_runtime::ImageSource<'_> {
        // Backends complete upload synchronously and retain only their GPU texture afterwards.
        neocogi_ui_runtime::ImageSource {
            width: self.size.width,
            height: self.size.height,
            pixels: &self.pixels,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neocogi_com::NcCopyToBufferOffset;

    #[com_object(interfaces(IAssetBundle))]
    /// A pixel projection deliberately makes every UI metadata operation unavailable.
    struct PixelSource {
        /// Header dimensions exercised before any pixel copy.
        size: Size,
        /// Shared observation proves rejected headers do not reach foreign copy code.
        copies: Rc<Cell<u32>>,
        /// Simulates a recoverable transfer failure after successful header validation.
        fail_copy: bool,
    }
    impl IAssetBundle for PixelSource {
        /// Renderers need revision identity, not the UI's catalog namespace.
        fn catalog(&self) -> Result<ComPtr<IAssetCatalogAbi>, NcStatus> {
            Err(NC_E_INVALID_ARG)
        }
        /// Supplies bounded counts without requiring the consumer to fetch those tables.
        fn info(&self) -> Result<AssetInfo, NcStatus> {
            Ok(AssetInfo {
                schema: ASSET_SCHEMA_VERSION,
                size: self.size,
                fonts: 1,
                icons: 1,
                glyphs: 1,
                names: 1,
            })
        }
        /// Copies one known texel or propagates the requested source failure.
        fn pixels(&self, mut output: NcCopyToBufferOffset<'_, u8>) -> Result<(), NcStatus> {
            self.copies.set(self.copies.get() + 1);
            if self.fail_copy {
                return Err(neocogi_com::NC_E_LOAD_FAILED);
            }
            assert_eq!((output.offset(), output.len()), (0, 4));
            output.as_mut_slice().copy_from_slice(&[11, 22, 33, 44]);
            Ok(())
        }
        /// Font metrics must never be requested by the pixel uploader.
        fn fonts(&self, _: NcCopyToBufferOffset<'_, AssetFont>) -> Result<(), NcStatus> {
            Err(NC_E_INVALID_ARG)
        }
        /// Glyph positions belong to geometry preparation in the UI.
        fn glyphs(&self, _: NcCopyToBufferOffset<'_, AssetGlyph>) -> Result<(), NcStatus> {
            Err(NC_E_INVALID_ARG)
        }
        /// GPU atlas sampling receives finished UVs and needs no icon table.
        fn icons(&self, _: NcCopyToBufferOffset<'_, AssetIcon>) -> Result<(), NcStatus> {
            Err(NC_E_INVALID_ARG)
        }
        /// Resource names never enter the native GPU upload path.
        fn names(&self, _: NcCopyToBufferOffset<'_, u8>) -> Result<(), NcStatus> {
            Err(NC_E_INVALID_ARG)
        }
        /// Layout metrics are deliberately unavailable to this consumer.
        fn metrics(&self) -> Result<AssetMetrics, NcStatus> {
            Err(NC_E_INVALID_ARG)
        }
        /// Window policy is deliberately unavailable to this consumer.
        fn chrome(&self) -> Result<AssetChrome, NcStatus> {
            Err(NC_E_INVALID_ARG)
        }
        /// Compiled appearances are consumed by the UI before triangles reach the renderer.
        fn visuals(&self, _: AppearanceFamily, _: NcCopyToBufferOffset<'_, AssetVisual>) -> Result<(), NcStatus> {
            Err(NC_E_INVALID_ARG)
        }
    }

    /// Pixel transfer succeeds even when all font, theme and catalog operations reject calls.
    #[test]
    fn pixel_import_has_no_ui_metadata_dependency() {
        let copies = Rc::new(Cell::new(0));
        let source: ComPtr<IAssetBundleAbi> = PixelSource::from_value(PixelSource {
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
            let source: ComPtr<IAssetBundleAbi> = PixelSource::from_value(PixelSource {
                size,
                copies: copies.clone(),
                fail_copy: false,
            })
            .query()
            .unwrap();
            assert_eq!(AtlasPixels::read(source.as_com_ref()).err().unwrap(), NC_E_INVALID_ARG);
        }
        assert_eq!(copies.get(), 0);
        let source: ComPtr<IAssetBundleAbi> = PixelSource::from_value(PixelSource {
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
