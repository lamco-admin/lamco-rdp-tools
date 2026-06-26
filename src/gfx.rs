//! CPU-based EGFX handler for headless RDP sessions.
//!
//! Implements the upstream `GraphicsPipelineHandler` callback trait to composite
//! decoded bitmap data into a shared framebuffer. Upstream `GraphicsPipelineClient`
//! handles surface lifecycle, codec dispatch, and frame acknowledgment; this handler
//! receives decoded RGBA via `on_bitmap_updated` and manages the pixel buffers needed
//! for `SolidFill`, `SurfaceToSurface`, and `RemoteFX` decode (which upstream doesn't handle).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use ironrdp_core::ReadCursor;
use ironrdp_egfx::{
    client::{BitmapUpdate, GraphicsPipelineHandler, Surface},
    pdu::{Codec1Type, GfxPdu, SolidFillPdu, SurfaceToSurfacePdu, WireToSurface1Pdu},
};
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_pdu::geometry::{InclusiveRectangle, Rectangle as _};
use ironrdp_session::{image::DecodedImage, rfx};
use tracing::{debug, trace, warn};

/// RGBA framebuffer for composited EGFX output.
///
/// Unlike `DecodedImage` (whose write methods are crate-private to ironrdp-session),
/// this buffer is fully owned and writable by rdpdo.
pub(crate) struct Framebuffer {
    width: u16,
    height: u16,
    data: Vec<u8>,
}

impl Framebuffer {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        let len = usize::from(width) * usize::from(height) * 4;
        Self {
            width,
            height,
            data: vec![0; len],
        }
    }

    pub(crate) fn width(&self) -> u16 {
        self.width
    }

    pub(crate) fn height(&self) -> u16 {
        self.height
    }

    pub(crate) fn data(&self) -> &[u8] {
        &self.data
    }

    fn stride(&self) -> usize {
        usize::from(self.width) * 4
    }

    /// Write RGBA pixels into a rectangular region (top-down row-major order).
    fn blit(&mut self, rect: &InclusiveRectangle, rgba: &[u8], src_stride: usize) {
        let dst_stride = self.stride();
        let w = usize::from(rect.width()) * 4;
        let h = usize::from(rect.height());
        for row in 0..h {
            let src_start = row * src_stride;
            let dst_start = (usize::from(rect.top) + row) * dst_stride + usize::from(rect.left) * 4;
            if src_start + w <= rgba.len() && dst_start + w <= self.data.len() {
                self.data[dst_start..dst_start + w]
                    .copy_from_slice(&rgba[src_start..src_start + w]);
            }
        }
    }
}

/// Per-surface pixel buffer.
struct SurfaceBuffer {
    width: u16,
    data: Vec<u8>,
}

impl SurfaceBuffer {
    fn new(width: u16, height: u16) -> Self {
        let len = usize::from(width) * usize::from(height) * 4;
        Self {
            width,
            data: vec![0; len],
        }
    }

    fn stride(&self) -> usize {
        usize::from(self.width) * 4
    }
}

/// Output position for a surface in the composited framebuffer.
struct OutputMapping {
    origin_x: u32,
    origin_y: u32,
}

/// Headless EGFX handler: composites decoded EGFX frames into a shared framebuffer.
///
/// Upstream handles Uncompressed and AVC420 decode internally, delivering decoded
/// RGBA via `on_bitmap_updated`. This handler also decodes `RemoteFX` via
/// `on_unhandled_pdu`, and handles `SolidFill` / `SurfaceToSurface` that upstream
/// passes through as raw PDU callbacks.
pub(crate) struct HeadlessGfxHandler {
    framebuffer: Arc<Mutex<Framebuffer>>,
    /// Scratch `DecodedImage` for RFX decode (`rfx::DecodingContext` requires it)
    rfx_scratch: DecodedImage,
    surfaces: BTreeMap<u16, SurfaceBuffer>,
    mappings: BTreeMap<u16, OutputMapping>,
    rfx_context: rfx::DecodingContext,
    has_content: Arc<AtomicBool>,
    /// Set when EGFX capabilities are confirmed (channel is active)
    egfx_negotiated: Arc<AtomicBool>,
    /// The server-confirmed EGFX capability tier (e.g. "V8.1 (AVC420)"), once the
    /// caps-confirm PDU arrives. Shared with the session for the `report` output.
    egfx_caps: Arc<Mutex<Option<String>>>,
    /// EGFX frame counter -- incremented on every `on_bitmap_updated` call.
    /// Shared with the session for perf metrics (`total_frames` in `perf` output).
    egfx_frame_count: Arc<AtomicU64>,
}

/// Human label for a confirmed EGFX capability set: version tier plus the
/// codec-relevant flags a report cares about.
fn egfx_cap_label(caps: &ironrdp_egfx::pdu::CapabilitySet) -> String {
    use ironrdp_egfx::pdu::{CapabilitiesV81Flags, CapabilitySet};

    match caps {
        CapabilitySet::V8 { .. } => "V8".to_owned(),
        CapabilitySet::V8_1 { flags } => {
            if flags.contains(CapabilitiesV81Flags::AVC420_ENABLED) {
                "V8.1 (AVC420)".to_owned()
            } else {
                "V8.1".to_owned()
            }
        }
        CapabilitySet::V10 { .. } => "V10 (AVC444)".to_owned(),
        CapabilitySet::V10_1 => "V10.1".to_owned(),
        CapabilitySet::V10_2 { .. } => "V10.2".to_owned(),
        CapabilitySet::V10_3 { .. } => "V10.3".to_owned(),
        CapabilitySet::V10_4 { .. } => "V10.4".to_owned(),
        CapabilitySet::V10_5 { .. } => "V10.5".to_owned(),
        CapabilitySet::V10_6 { .. } | CapabilitySet::V10_6Err { .. } => "V10.6".to_owned(),
        CapabilitySet::V10_7 { .. } => "V10.7".to_owned(),
    }
}

impl HeadlessGfxHandler {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        Self {
            framebuffer: Arc::new(Mutex::new(Framebuffer::new(width, height))),
            rfx_scratch: DecodedImage::new(PixelFormat::RgbA32, width, height),
            surfaces: BTreeMap::new(),
            mappings: BTreeMap::new(),
            rfx_context: rfx::DecodingContext::new(),
            has_content: Arc::new(AtomicBool::new(false)),
            egfx_negotiated: Arc::new(AtomicBool::new(false)),
            egfx_caps: Arc::new(Mutex::new(None)),
            egfx_frame_count: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn framebuffer(&self) -> Arc<Mutex<Framebuffer>> {
        Arc::clone(&self.framebuffer)
    }

    pub(crate) fn has_content(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.has_content)
    }

    pub(crate) fn frame_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.egfx_frame_count)
    }

    pub(crate) fn negotiated(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.egfx_negotiated)
    }

    pub(crate) fn egfx_caps(&self) -> Arc<Mutex<Option<String>>> {
        Arc::clone(&self.egfx_caps)
    }

    /// Copy a region from a surface buffer into the composited framebuffer
    /// at the surface's mapped output position.
    fn composite_surface_region(&self, surface_id: u16, region: &InclusiveRectangle) {
        let Some(mapping) = self.mappings.get(&surface_id) else {
            return;
        };
        let Some(surface) = self.surfaces.get(&surface_id) else {
            return;
        };

        let src_stride = surface.stride();
        let region_width = usize::from(region.width());
        let region_height = usize::from(region.height());
        let row_bytes = region_width * 4;

        // Extract the region from the surface into a contiguous buffer
        let mut region_data = vec![0u8; region_height * row_bytes];
        for row in 0..region_height {
            let src_row = usize::from(region.top) + row;
            let src_start = src_row * src_stride + usize::from(region.left) * 4;
            let dst_start = row * row_bytes;
            if src_start + row_bytes <= surface.data.len() {
                region_data[dst_start..dst_start + row_bytes]
                    .copy_from_slice(&surface.data[src_start..src_start + row_bytes]);
            }
        }

        // Write into framebuffer at the mapped output position
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            reason = "RDP desktop coordinates fit in u16 (max 32766)"
        )]
        let dest_rect = InclusiveRectangle {
            left: region.left + mapping.origin_x as u16,
            top: region.top + mapping.origin_y as u16,
            right: region.right + mapping.origin_x as u16,
            bottom: region.bottom + mapping.origin_y as u16,
        };

        let mut fb = self
            .framebuffer
            .lock()
            .expect("GFX framebuffer mutex poisoned");
        fb.blit(&dest_rect, &region_data, row_bytes);
    }

    /// Decode a `RemoteFX` `WireToSurface1` PDU. Upstream doesn't handle this codec,
    /// so it arrives via `on_unhandled_pdu`.
    fn decode_remotefx(&mut self, pdu: &WireToSurface1Pdu) {
        let Some(mapping) = self.mappings.get(&pdu.surface_id) else {
            trace!(
                surface_id = pdu.surface_id,
                "RFX: no output mapping, skipping"
            );
            return;
        };

        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            reason = "RDP desktop coordinates fit in u16 (max 32766)"
        )]
        let adjusted_rect = InclusiveRectangle {
            left: pdu.destination_rectangle.left + mapping.origin_x as u16,
            top: pdu.destination_rectangle.top + mapping.origin_y as u16,
            right: pdu.destination_rectangle.right + mapping.origin_x as u16,
            bottom: pdu.destination_rectangle.bottom + mapping.origin_y as u16,
        };

        let mut cursor = ReadCursor::new(&pdu.bitmap_data);

        // Decode into scratch DecodedImage (rfx::DecodingContext requires it)
        match self
            .rfx_context
            .decode(&mut self.rfx_scratch, &adjusted_rect, &mut cursor)
        {
            Ok((_frame_id, update_rect)) => {
                // Copy decoded region from scratch image to our framebuffer
                let data = self.rfx_scratch.data_for_rect(&update_rect);
                let mut fb = self
                    .framebuffer
                    .lock()
                    .expect("GFX framebuffer mutex poisoned");
                fb.blit(&update_rect, data, usize::from(update_rect.width()) * 4);
                self.has_content.store(true, Ordering::Relaxed);
                trace!(surface_id = pdu.surface_id, "RFX decode successful");
            }
            Err(e) => {
                warn!(surface_id = pdu.surface_id, error = %e, "RFX decode failed");
            }
        }
    }
}

impl GraphicsPipelineHandler for HeadlessGfxHandler {
    fn capabilities(&self) -> Vec<ironrdp_egfx::pdu::CapabilitySet> {
        // Only advertise what we can actually decode:
        // V8.1 (AVC420) and V8 (no AVC, for uncompressed/RFX fallback).
        // V10+ implies AVC444 which upstream GraphicsPipelineClient doesn't decode.
        vec![
            ironrdp_egfx::pdu::CapabilitySet::V8_1 {
                flags: ironrdp_egfx::pdu::CapabilitiesV81Flags::AVC420_ENABLED
                    | ironrdp_egfx::pdu::CapabilitiesV81Flags::SMALL_CACHE,
            },
            ironrdp_egfx::pdu::CapabilitySet::V8 {
                flags: ironrdp_egfx::pdu::CapabilitiesV8Flags::SMALL_CACHE,
            },
        ]
    }

    fn on_capabilities_confirmed(&mut self, caps: &ironrdp_egfx::pdu::CapabilitySet) {
        let label = egfx_cap_label(caps);
        debug!(caps = %label, "EGFX capabilities confirmed");
        *self.egfx_caps.lock().expect("EGFX caps mutex poisoned") = Some(label);
        self.egfx_negotiated.store(true, Ordering::Relaxed);
    }

    fn on_reset_graphics(&mut self, width: u32, height: u32) {
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            reason = "RDP desktop dimensions fit in u16 (max 32766)"
        )]
        let (w, h) = (width as u16, height as u16);
        debug!(w, h, "EGFX ResetGraphics");

        *self
            .framebuffer
            .lock()
            .expect("GFX framebuffer mutex poisoned") = Framebuffer::new(w, h);
        self.rfx_scratch = DecodedImage::new(PixelFormat::RgbA32, w, h);
        self.surfaces.clear();
        self.mappings.clear();
    }

    fn on_surface_created(&mut self, surface: &Surface) {
        debug!(
            surface_id = surface.id,
            width = surface.width,
            height = surface.height,
            "EGFX CreateSurface"
        );
        self.surfaces.insert(
            surface.id,
            SurfaceBuffer::new(surface.width, surface.height),
        );
    }

    fn on_surface_deleted(&mut self, surface_id: u16) {
        debug!(surface_id, "EGFX DeleteSurface");
        self.surfaces.remove(&surface_id);
        self.mappings.remove(&surface_id);
    }

    fn on_surface_mapped(&mut self, surface_id: u16, origin_x: u32, origin_y: u32) {
        debug!(surface_id, origin_x, origin_y, "EGFX MapSurface");
        self.mappings
            .insert(surface_id, OutputMapping { origin_x, origin_y });
    }

    fn on_bitmap_updated(&mut self, update: &BitmapUpdate) {
        debug!(
            surface_id = update.surface_id,
            width = update.destination_rectangle.width(),
            height = update.destination_rectangle.height(),
            data_len = update.data.len(),
            "EGFX on_bitmap_updated called"
        );
        // Write decoded RGBA into surface buffer
        if let Some(surface) = self.surfaces.get_mut(&update.surface_id) {
            let dest_width = usize::from(update.destination_rectangle.width());
            let src_stride = dest_width * 4;
            let dst_stride = surface.stride();
            let dest_height = usize::from(update.destination_rectangle.height());

            for row in 0..dest_height {
                let src_start = row * src_stride;
                let dst_start = (usize::from(update.destination_rectangle.top) + row) * dst_stride
                    + usize::from(update.destination_rectangle.left) * 4;
                if src_start + src_stride <= update.data.len()
                    && dst_start + src_stride <= surface.data.len()
                {
                    surface.data[dst_start..dst_start + src_stride]
                        .copy_from_slice(&update.data[src_start..src_start + src_stride]);
                }
            }
        }

        // Composite to framebuffer at mapped output position
        if let Some(mapping) = self.mappings.get(&update.surface_id) {
            #[expect(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                reason = "RDP desktop coordinates fit in u16 (max 32766)"
            )]
            let dest_rect = InclusiveRectangle {
                left: update.destination_rectangle.left + mapping.origin_x as u16,
                top: update.destination_rectangle.top + mapping.origin_y as u16,
                right: update.destination_rectangle.right + mapping.origin_x as u16,
                bottom: update.destination_rectangle.bottom + mapping.origin_y as u16,
            };
            let src_stride = usize::from(update.destination_rectangle.width()) * 4;
            let mut fb = self
                .framebuffer
                .lock()
                .expect("GFX framebuffer mutex poisoned");
            fb.blit(&dest_rect, &update.data, src_stride);
        }

        self.has_content.store(true, Ordering::Relaxed);
        self.egfx_frame_count.fetch_add(1, Ordering::Relaxed);
    }

    fn on_solid_fill(&mut self, pdu: &SolidFillPdu) {
        let Some(surface) = self.surfaces.get_mut(&pdu.surface_id) else {
            warn!(surface_id = pdu.surface_id, "SolidFill for unknown surface");
            return;
        };

        let pixel = [
            pdu.fill_pixel.r,
            pdu.fill_pixel.g,
            pdu.fill_pixel.b,
            pdu.fill_pixel.xa,
        ];
        let stride = surface.stride();

        for rect in &pdu.rectangles {
            // MS-RDPEGFX 2.2.1.4.1: right/bottom are one-past-end (exclusive).
            for row in usize::from(rect.top)..usize::from(rect.bottom) {
                for col in usize::from(rect.left)..usize::from(rect.right) {
                    let offset = row * stride + col * 4;
                    if offset + 4 <= surface.data.len() {
                        surface.data[offset..offset + 4].copy_from_slice(&pixel);
                    }
                }
            }
        }

        let surface_id = pdu.surface_id;
        for rect in &pdu.rectangles {
            let inclusive = InclusiveRectangle {
                left: rect.left,
                top: rect.top,
                right: rect.right.saturating_sub(1),
                bottom: rect.bottom.saturating_sub(1),
            };
            self.composite_surface_region(surface_id, &inclusive);
        }
    }

    fn on_surface_to_surface(&mut self, pdu: &SurfaceToSurfacePdu) {
        let src_rect = &pdu.source_rectangle;
        let w = usize::from(src_rect.width());
        let h = usize::from(src_rect.height());

        // Copy source region to temp buffer (avoids borrow conflicts)
        let temp = {
            let Some(surface) = self.surfaces.get(&pdu.source_surface_id) else {
                warn!(
                    surface_id = pdu.source_surface_id,
                    "SurfaceToSurface: unknown source"
                );
                return;
            };
            let stride = surface.stride();
            let mut buf = vec![0u8; w * h * 4];
            for row in 0..h {
                let src_start =
                    (usize::from(src_rect.top) + row) * stride + usize::from(src_rect.left) * 4;
                let tmp_start = row * w * 4;
                if src_start + w * 4 <= surface.data.len() {
                    buf[tmp_start..tmp_start + w * 4]
                        .copy_from_slice(&surface.data[src_start..src_start + w * 4]);
                }
            }
            buf
        };

        let Some(dest_surface) = self.surfaces.get_mut(&pdu.destination_surface_id) else {
            warn!(
                surface_id = pdu.destination_surface_id,
                "SurfaceToSurface: unknown dest"
            );
            return;
        };

        let dst_stride = dest_surface.stride();
        for dest_point in &pdu.destination_points {
            for row in 0..h {
                let src_start = row * w * 4;
                let dst_start =
                    (usize::from(dest_point.y) + row) * dst_stride + usize::from(dest_point.x) * 4;
                if dst_start + w * 4 <= dest_surface.data.len() && src_start + w * 4 <= temp.len() {
                    dest_surface.data[dst_start..dst_start + w * 4]
                        .copy_from_slice(&temp[src_start..src_start + w * 4]);
                }
            }
        }

        // Composite destination regions
        let dest_id = pdu.destination_surface_id;
        let src_w = pdu.source_rectangle.width();
        let src_h = pdu.source_rectangle.height();
        for dest_point in &pdu.destination_points {
            if src_w > 0 && src_h > 0 {
                let dest_rect = InclusiveRectangle {
                    left: dest_point.x,
                    top: dest_point.y,
                    right: dest_point.x + src_w - 1,
                    bottom: dest_point.y + src_h - 1,
                };
                self.composite_surface_region(dest_id, &dest_rect);
            }
        }
    }

    fn on_unhandled_pdu(&mut self, pdu: &GfxPdu) {
        // Handle RemoteFX — upstream doesn't decode it
        if let GfxPdu::WireToSurface1(wire_pdu) = pdu {
            if wire_pdu.codec_id == Codec1Type::RemoteFx {
                self.decode_remotefx(wire_pdu);
                return;
            }
            // Log any WireToSurface1 that reaches unhandled (e.g. AVC420 decode failure)
            warn!(
                codec = ?wire_pdu.codec_id,
                data_len = wire_pdu.bitmap_data.len(),
                "WireToSurface1 reached on_unhandled_pdu (decode may have failed)"
            );
            return;
        }
        debug!("Unhandled EGFX PDU: {:?}", std::mem::discriminant(pdu));
    }
}
