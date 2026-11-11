//! CPU-based EGFX handler for headless RDP sessions.
//!
//! Implements the upstream `GraphicsPipelineHandler` callback trait to composite
//! decoded bitmap data into a shared framebuffer. Upstream `GraphicsPipelineClient`
//! handles surface lifecycle, codec decode, and frame acknowledgment; this handler
//! receives decoded RGBA via `on_bitmap_updated` and keeps the per-surface pixel
//! buffers and the bitmap cache that `SolidFill`, `SurfaceToSurface`,
//! `SurfaceToCache` and `CacheToSurface` need.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use ironrdp_egfx::{
    cache::{SMALL_CACHE_BYTES, SMALL_CACHE_SLOTS},
    client::{BitmapUpdate, GraphicsPipelineHandler, Surface},
    pdu::{
        CacheToSurfacePdu, EvictCacheEntryPdu, GfxPdu, SolidFillPdu, SurfaceToCachePdu,
        SurfaceToSurfacePdu,
    },
};
use ironrdp_pdu::geometry::{InclusiveRectangle, Rectangle as _};
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
    /// Returns whether any row landed in the framebuffer.
    fn blit(&mut self, rect: &InclusiveRectangle, rgba: &[u8], src_stride: usize) -> bool {
        let dst_stride = self.stride();
        let w = usize::from(rect.width()) * 4;
        let h = usize::from(rect.height());
        let mut wrote = false;
        for row in 0..h {
            let src_start = row * src_stride;
            let dst_start = (usize::from(rect.top) + row) * dst_stride + usize::from(rect.left) * 4;
            if src_start + w <= rgba.len() && dst_start + w <= self.data.len() {
                self.data[dst_start..dst_start + w]
                    .copy_from_slice(&rgba[src_start..src_start + w]);
                wrote = true;
            }
        }
        wrote
    }
}

/// Per-surface pixel buffer.
struct SurfaceBuffer {
    width: u16,
    height: u16,
    data: Vec<u8>,
}

impl SurfaceBuffer {
    fn new(width: u16, height: u16) -> Self {
        let len = usize::from(width) * usize::from(height) * 4;
        Self {
            width,
            height,
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

/// A bitmap the server stored with `SurfaceToCache`: RGBA rows of `width` pixels.
struct CachedTile {
    width: u16,
    height: u16,
    data: Vec<u8>,
}

/// Headless EGFX handler: composites decoded EGFX frames into a shared framebuffer.
///
/// Upstream decodes the surface codecs (Uncompressed, AVC420 through our decoder,
/// `RemoteFX`, `RemoteFX` Progressive, `ClearCodec`, Planar, Alpha) and delivers RGBA via
/// `on_bitmap_updated`. This handler handles `SolidFill`, `SurfaceToSurface` and the
/// bitmap cache commands, which upstream applies only to its own compositor and passes
/// on as raw PDU callbacks.
pub(crate) struct HeadlessGfxHandler {
    framebuffer: Arc<Mutex<Framebuffer>>,
    surfaces: BTreeMap<u16, SurfaceBuffer>,
    mappings: BTreeMap<u16, OutputMapping>,
    /// Bitmap cache slots (one-based, MS-RDPEGFX 3.3.1.4) and the bytes they hold.
    cache: BTreeMap<u16, CachedTile>,
    cache_bytes: usize,
    has_content: Arc<AtomicBool>,
    /// Set when EGFX capabilities are confirmed (channel is active)
    egfx_negotiated: Arc<AtomicBool>,
    /// The server-confirmed EGFX capability tier (e.g. "V8.1 (AVC420)"), once the
    /// caps-confirm PDU arrives. Shared with the session for the `report` output.
    egfx_caps: Arc<Mutex<Option<String>>>,
    /// EGFX frame counter, incremented at each `EndFrame`. Counting bitmap updates
    /// instead would count Progressive tiles and AVC420 region rectangles, several
    /// per frame. Shared with the session for perf metrics (`total_frames` in
    /// `perf` output).
    egfx_frame_count: Arc<AtomicU64>,
    /// Whether an H.264 decoder loaded, the condition for advertising AVC420.
    h264_available: bool,
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
    pub(crate) fn new(width: u16, height: u16, h264_available: bool) -> Self {
        Self {
            framebuffer: Arc::new(Mutex::new(Framebuffer::new(width, height))),
            surfaces: BTreeMap::new(),
            mappings: BTreeMap::new(),
            cache: BTreeMap::new(),
            cache_bytes: 0,
            has_content: Arc::new(AtomicBool::new(false)),
            egfx_negotiated: Arc::new(AtomicBool::new(false)),
            egfx_caps: Arc::new(Mutex::new(None)),
            egfx_frame_count: Arc::new(AtomicU64::new(0)),
            h264_available,
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
        if fb.blit(&dest_rect, &region_data, row_bytes) {
            self.has_content.store(true, Ordering::Relaxed);
        }
    }
}

impl GraphicsPipelineHandler for HeadlessGfxHandler {
    fn capabilities(&self) -> Vec<ironrdp_egfx::pdu::CapabilitySet> {
        // Only advertise what we can actually decode: V8.1 and V8. V10+ implies AVC444,
        // which upstream GraphicsPipelineClient decodes only with an H.264 decoder that
        // returns YUV planes; Bt709H264Decoder returns RGBA.
        //
        // RDPDO_EGFX_CAPS=v8 advertises V8 alone, the client a server has to send
        // raw pixels to (EGFX 8.0 has neither H.264 nor RemoteFX Progressive).
        if std::env::var("RDPDO_EGFX_CAPS").is_ok_and(|v| v.eq_ignore_ascii_case("v8")) {
            return vec![ironrdp_egfx::pdu::CapabilitySet::V8 {
                flags: ironrdp_egfx::pdu::CapabilitiesV8Flags::SMALL_CACHE,
            }];
        }
        // AVC420 only with a decoder: Without one, upstream completes each H.264 frame
        // with nothing painted. The client drops an AVC420 set on its own, but it drops
        // the whole V8.1 set and leaves V8 alone, and servers that keep RemoteFX
        // Progressive for 8.1 clients then send raw pixels. V8.1 with SMALL_CACHE alone
        // is one of the flag combinations MS-RDPEGFX 2.2.3.2 lists.
        let mut v81_flags = ironrdp_egfx::pdu::CapabilitiesV81Flags::SMALL_CACHE;
        if self.h264_available {
            v81_flags |= ironrdp_egfx::pdu::CapabilitiesV81Flags::AVC420_ENABLED;
        }
        vec![
            ironrdp_egfx::pdu::CapabilitySet::V8_1 { flags: v81_flags },
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
        // The new framebuffer is blank until the server paints it again.
        self.has_content.store(false, Ordering::Relaxed);
        self.surfaces.clear();
        self.mappings.clear();
        // The bitmap cache stays: MS-RDPEGFX 3.3.5.14 resizes only the output buffer,
        // and the server may go on painting from slots it filled before the reset.
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
        // Progressive arrives one 64x64 tile per update, so this is per-tile chatter.
        trace!(
            surface_id = update.surface_id,
            codec = ?update.codec_id,
            width = update.destination_rectangle.width(),
            height = update.destination_rectangle.height(),
            data_len = update.data.len(),
            "EGFX bitmap update"
        );
        // The rectangle is exclusive (right/bottom one past the edge, MS-RDPEGFX
        // 2.2.1.2), and the inclusive one built below would underflow on an empty one.
        let dest = &update.destination_rectangle;
        if dest.width() == 0 || dest.height() == 0 {
            return;
        }
        let src_stride = usize::from(dest.width()) * 4;
        let expected_len = src_stride * usize::from(dest.height());
        if update.data.len() < expected_len {
            debug!(
                surface_id = update.surface_id,
                width = dest.width(),
                height = dest.height(),
                data_len = update.data.len(),
                expected_len,
                "EGFX bitmap update is shorter than its rectangle, painting only the rows it covers"
            );
        }

        // Write decoded RGBA into surface buffer
        if let Some(surface) = self.surfaces.get_mut(&update.surface_id) {
            let dst_stride = surface.stride();
            for row in 0..usize::from(dest.height()) {
                let src_start = row * src_stride;
                let dst_start =
                    (usize::from(dest.top) + row) * dst_stride + usize::from(dest.left) * 4;
                if src_start + src_stride <= update.data.len()
                    && dst_start + src_stride <= surface.data.len()
                {
                    surface.data[dst_start..dst_start + src_stride]
                        .copy_from_slice(&update.data[src_start..src_start + src_stride]);
                }
            }
        }

        // An unmapped surface isn't on screen: The update stays in the surface buffer
        // for a later SurfaceToSurface, and the framebuffer has no new content.
        let Some(mapping) = self.mappings.get(&update.surface_id) else {
            return;
        };

        // Composite to framebuffer at mapped output position. Framebuffer::blit takes
        // an inclusive rectangle, hence right/bottom minus one.
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            reason = "RDP desktop coordinates fit in u16 (max 32766)"
        )]
        let dest_rect = InclusiveRectangle {
            left: dest.left + mapping.origin_x as u16,
            top: dest.top + mapping.origin_y as u16,
            right: dest.right - 1 + mapping.origin_x as u16,
            bottom: dest.bottom - 1 + mapping.origin_y as u16,
        };
        let mut fb = self
            .framebuffer
            .lock()
            .expect("GFX framebuffer mutex poisoned");
        if fb.blit(&dest_rect, &update.data, src_stride) {
            self.has_content.store(true, Ordering::Relaxed);
        }
    }

    fn on_frame_complete(&mut self, _frame_id: u32) {
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
            // MS-RDPEGFX 2.2.1.2: right/bottom are one-past-end (exclusive).
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
            if src_rect.right > surface.width || src_rect.bottom > surface.height {
                debug!(
                    surface_id = pdu.source_surface_id,
                    left = src_rect.left,
                    top = src_rect.top,
                    right = src_rect.right,
                    bottom = src_rect.bottom,
                    surface_width = surface.width,
                    surface_height = surface.height,
                    "SurfaceToSurface source rectangle exceeds its surface, the part outside copies as transparent black"
                );
            }
            // Clamped to the surface, so a rectangle too wide doesn't read into the
            // next row; whatever lies outside stays zero.
            let copy_bytes = usize::from(
                src_rect
                    .right
                    .min(surface.width)
                    .saturating_sub(src_rect.left),
            ) * 4;
            let copy_rows = usize::from(
                src_rect
                    .bottom
                    .min(surface.height)
                    .saturating_sub(src_rect.top),
            );
            let stride = surface.stride();
            let mut buf = vec![0u8; w * h * 4];
            for row in 0..copy_rows {
                let src_start =
                    (usize::from(src_rect.top) + row) * stride + usize::from(src_rect.left) * 4;
                let tmp_start = row * w * 4;
                buf[tmp_start..tmp_start + copy_bytes]
                    .copy_from_slice(&surface.data[src_start..src_start + copy_bytes]);
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

    fn on_surface_to_cache(&mut self, pdu: &SurfaceToCachePdu) {
        // Every capability set rdpdo advertises carries SMALL_CACHE, so the cache is
        // the 16 MB one with slots 1 to 4096 (MS-RDPEGFX 3.3.1.4).
        if pdu.cache_slot == 0 || pdu.cache_slot > SMALL_CACHE_SLOTS {
            warn!(
                cache_slot = pdu.cache_slot,
                "SurfaceToCache: slot outside the bitmap cache"
            );
            return;
        }
        let Some(surface) = self.surfaces.get(&pdu.surface_id) else {
            warn!(
                surface_id = pdu.surface_id,
                "SurfaceToCache: unknown source"
            );
            return;
        };

        // Clamped to the surface, as SurfaceToSurface's source is, so a rectangle past
        // its edge neither reads into the next row nor sizes an oversized tile.
        let rect = &pdu.source_rectangle;
        let width = rect.right.min(surface.width).saturating_sub(rect.left);
        let height = rect.bottom.min(surface.height).saturating_sub(rect.top);
        if width == 0 || height == 0 {
            return;
        }
        let row_bytes = usize::from(width) * 4;
        let len = row_bytes * usize::from(height);
        let replaced = self
            .cache
            .get(&pdu.cache_slot)
            .map_or(0, |tile| tile.data.len());
        let cache_bytes = self.cache_bytes - replaced + len;
        if cache_bytes > SMALL_CACHE_BYTES {
            warn!(
                cache_slot = pdu.cache_slot,
                len,
                cache_bytes = self.cache_bytes,
                "SurfaceToCache would overfill the 16 MB bitmap cache, not cached"
            );
            return;
        }

        let stride = surface.stride();
        let mut data = Vec::with_capacity(len);
        for row in 0..usize::from(height) {
            let start = (usize::from(rect.top) + row) * stride + usize::from(rect.left) * 4;
            data.extend_from_slice(&surface.data[start..start + row_bytes]);
        }
        self.cache_bytes = cache_bytes;
        self.cache.insert(
            pdu.cache_slot,
            CachedTile {
                width,
                height,
                data,
            },
        );
    }

    fn on_cache_to_surface(&mut self, pdu: &CacheToSurfacePdu) {
        let Some(tile) = self.cache.get(&pdu.cache_slot) else {
            warn!(
                cache_slot = pdu.cache_slot,
                "CacheToSurface: empty cache slot"
            );
            return;
        };
        let Some(surface) = self.surfaces.get_mut(&pdu.surface_id) else {
            warn!(surface_id = pdu.surface_id, "CacheToSurface: unknown dest");
            return;
        };

        let tile_stride = usize::from(tile.width) * 4;
        let stride = surface.stride();
        let mut painted = Vec::with_capacity(pdu.destination_points.len());
        for point in &pdu.destination_points {
            // Clipped at the surface's edges, so a row never runs on into the next one.
            let width = tile.width.min(surface.width.saturating_sub(point.x));
            let height = tile.height.min(surface.height.saturating_sub(point.y));
            if width == 0 || height == 0 {
                continue;
            }
            let row_bytes = usize::from(width) * 4;
            for row in 0..usize::from(height) {
                let src = row * tile_stride;
                let dst = (usize::from(point.y) + row) * stride + usize::from(point.x) * 4;
                surface.data[dst..dst + row_bytes]
                    .copy_from_slice(&tile.data[src..src + row_bytes]);
            }
            painted.push(InclusiveRectangle {
                left: point.x,
                top: point.y,
                right: point.x + width - 1,
                bottom: point.y + height - 1,
            });
        }

        for region in &painted {
            self.composite_surface_region(pdu.surface_id, region);
        }
    }

    fn on_evict_cache_entry(&mut self, pdu: &EvictCacheEntryPdu) {
        if let Some(tile) = self.cache.remove(&pdu.cache_slot) {
            self.cache_bytes -= tile.data.len();
        }
    }

    fn on_unhandled_pdu(&mut self, pdu: &GfxPdu) {
        // Upstream forwards only AVC444, which it cannot decode with an RGBA-only H.264
        // decoder. We never advertise V10, so a server sending it is out of spec.
        if let GfxPdu::WireToSurface1(wire_pdu) = pdu {
            warn!(
                codec = ?wire_pdu.codec_id,
                data_len = wire_pdu.bitmap_data.len(),
                "WireToSurface1 reached on_unhandled_pdu, not painted"
            );
            return;
        }
        debug!("Unhandled EGFX PDU: {:?}", std::mem::discriminant(pdu));
    }
}

#[cfg(test)]
mod tests {
    use ironrdp_core::{decode, encode_vec};
    use ironrdp_dvc::DvcProcessor as _;
    use ironrdp_egfx::{
        client::GraphicsPipelineClient,
        decode::{DecodedFrame, DecoderError, DecoderResult, H264Decoder},
        pdu::{
            Avc420BitmapStream, CapabilitiesConfirmPdu, CapabilitiesV8Flags, CapabilitiesV81Flags,
            CapabilitySet, Codec1Type, Codec2Type, Color, CreateSurfacePdu, EndFramePdu,
            MapSurfaceToOutputPdu, PixelFormat, Point, QuantQuality, ResetGraphicsPdu,
            StartFramePdu, Timestamp, WireToSurface1Pdu, WireToSurface2Pdu,
        },
    };
    use ironrdp_graphics::{
        progressive::{COEFFICIENTS_PER_COMPONENT, encode_first_pass},
        zgfx,
    };
    use ironrdp_pdu::{
        codecs::rfx::{
            RfxRectangle,
            progressive::{
                ComponentCodecQuant, ProgressiveBlock, ProgressiveContextPdu,
                ProgressiveFrameBeginPdu, ProgressiveFrameEndPdu, ProgressiveRegion,
                ProgressiveSyncPdu, ProgressiveTile, TileSimple, encode_progressive_stream,
            },
        },
        geometry::ExclusiveRectangle,
    };

    use super::*;

    const WIDTH: u16 = 128;
    // Two tile rows, the second cut to 32 rows by the surface edge.
    const HEIGHT: u16 = 96;

    /// A first-pass Progressive frame over the whole surface. All-zero coefficients
    /// decode to mid grey, so every painted pixel is opaque.
    fn progressive_frame() -> Vec<u8> {
        let quant = ComponentCodecQuant::LOSSLESS;
        let mut coefficients = [0i16; COEFFICIENTS_PER_COMPONENT];
        let mut encoded = [0u8; 8192];
        let len = encode_first_pass(&mut coefficients, &mut encoded, &quant, &quant, false)
            .expect("encode tile component");
        let component = &encoded[..len];

        let tiles = (0..WIDTH.div_ceil(64))
            .flat_map(|x_idx| (0..HEIGHT.div_ceil(64)).map(move |y_idx| (x_idx, y_idx)))
            .map(|(x_idx, y_idx)| {
                ProgressiveTile::Simple(TileSimple {
                    quant_idx_y: 0,
                    quant_idx_cb: 0,
                    quant_idx_cr: 0,
                    x_idx,
                    y_idx,
                    flags: 0,
                    y_data: component,
                    cb_data: component,
                    cr_data: component,
                    tail_data: &[],
                })
            })
            .collect();

        encode_progressive_stream(&[
            ProgressiveBlock::Sync(ProgressiveSyncPdu),
            ProgressiveBlock::Context(ProgressiveContextPdu {
                context_id: 0,
                tile_size: 64,
                flags: 0,
            }),
            ProgressiveBlock::FrameBegin(ProgressiveFrameBeginPdu {
                frame_index: 0,
                region_count: 1,
            }),
            ProgressiveBlock::Region(ProgressiveRegion {
                tile_size: 64,
                rects: vec![RfxRectangle {
                    x: 0,
                    y: 0,
                    width: WIDTH,
                    height: HEIGHT,
                }],
                quant_vals: vec![quant],
                quant_prog_vals: Vec::new(),
                flags: 0,
                tiles,
            }),
            ProgressiveBlock::FrameEnd(ProgressiveFrameEndPdu),
        ])
        .expect("encode progressive stream")
    }

    /// An H.264 decoder that never produces a picture. Only its presence matters to
    /// what the client advertises.
    struct NoPictureDecoder;

    impl H264Decoder for NoPictureDecoder {
        fn decode(&mut self, _data: &[u8]) -> DecoderResult<DecodedFrame> {
            Err(DecoderError::msg("no picture"))
        }
    }

    /// The capability sets the client puts on the wire when the EGFX channel opens,
    /// after its own filtering of what the handler returns.
    fn advertised_capabilities(h264_decoder: Option<Box<dyn H264Decoder>>) -> Vec<CapabilitySet> {
        let handler = HeadlessGfxHandler::new(WIDTH, HEIGHT, h264_decoder.is_some());
        let mut client = GraphicsPipelineClient::new(Box::new(handler), h264_decoder);
        let messages = client.start(0).expect("start EGFX channel");
        let bytes = encode_vec(messages[0].as_ref()).expect("encode advertise PDU");
        let GfxPdu::CapabilitiesAdvertise(advertise) = decode(&bytes).expect("decode EGFX PDU")
        else {
            panic!("first EGFX message is not CapabilitiesAdvertise");
        };
        advertise
            .0
            .iter()
            .map(|set| {
                set.parsed()
                    .expect("decode capability set")
                    .expect("known capability version")
            })
            .collect()
    }

    /// One frame carrying `updates`, on surface 1, which covers the whole output and is
    /// mapped at its origin.
    fn frame_pdus(updates: Vec<GfxPdu>) -> Vec<GfxPdu> {
        let mut pdus = vec![
            GfxPdu::CapabilitiesConfirm(CapabilitiesConfirmPdu::from_typed(&CapabilitySet::V8_1 {
                flags: CapabilitiesV81Flags::SMALL_CACHE,
            })),
            GfxPdu::ResetGraphics(ResetGraphicsPdu {
                width: WIDTH.into(),
                height: HEIGHT.into(),
                monitors: Vec::new(),
            }),
            GfxPdu::CreateSurface(CreateSurfacePdu {
                surface_id: 1,
                width: WIDTH,
                height: HEIGHT,
                pixel_format: PixelFormat::XRgb,
            }),
            GfxPdu::MapSurfaceToOutput(MapSurfaceToOutputPdu {
                surface_id: 1,
                output_origin_x: 0,
                output_origin_y: 0,
            }),
            GfxPdu::StartFrame(StartFramePdu {
                timestamp: Timestamp {
                    milliseconds: 0,
                    seconds: 0,
                    minutes: 0,
                    hours: 0,
                },
                frame_id: 1,
            }),
        ];
        pdus.extend(updates);
        pdus.push(GfxPdu::EndFrame(EndFramePdu { frame_id: 1 }));
        pdus
    }

    fn unmapped(pdus: Vec<GfxPdu>) -> Vec<GfxPdu> {
        pdus.into_iter()
            .filter(|pdu| !matches!(pdu, GfxPdu::MapSurfaceToOutput(_)))
            .collect()
    }

    fn solid_fill(surface_id: u16, rect: ExclusiveRectangle, r: u8, g: u8, b: u8) -> GfxPdu {
        GfxPdu::SolidFill(SolidFillPdu {
            surface_id,
            fill_pixel: Color { b, g, r, xa: 0xFF },
            rectangles: vec![rect],
        })
    }

    fn rect(left: u16, top: u16, right: u16, bottom: u16) -> ExclusiveRectangle {
        ExclusiveRectangle {
            left,
            top,
            right,
            bottom,
        }
    }

    fn pixel(fb: &Framebuffer, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * usize::from(fb.width()) + x) * 4;
        fb.data()[offset..offset + 4]
            .try_into()
            .expect("four bytes")
    }

    fn feed(client: &mut GraphicsPipelineClient, pdus: &[GfxPdu]) {
        for pdu in pdus {
            let bytes = encode_vec(pdu).expect("encode EGFX PDU");
            client
                .process(0, &zgfx::wrap_uncompressed(&bytes))
                .expect("process EGFX PDU");
        }
    }

    fn progressive_update() -> GfxPdu {
        GfxPdu::WireToSurface2(WireToSurface2Pdu {
            surface_id: 1,
            codec_id: Codec2Type::RemoteFxProgressive,
            codec_context_id: 1,
            pixel_format: PixelFormat::XRgb,
            bitmap_data: progressive_frame(),
        })
    }

    #[test]
    fn progressive_frame_paints_every_framebuffer_pixel() {
        let handler = HeadlessGfxHandler::new(WIDTH, HEIGHT, false);
        let framebuffer = handler.framebuffer();
        let frames = handler.frame_counter();
        let has_content = handler.has_content();
        let mut client = GraphicsPipelineClient::new(Box::new(handler), None);

        feed(&mut client, &frame_pdus(vec![progressive_update()]));

        assert_eq!(client.dropped_updates(), 0);
        assert_eq!(frames.load(Ordering::Relaxed), 1);
        assert!(has_content.load(Ordering::Relaxed));
        // Rows 63 and 95 close a tile, the rows an exclusive rectangle blitted as an
        // inclusive one used to leave unpainted.
        let fb = framebuffer.lock().expect("framebuffer lock");
        let width = usize::from(WIDTH);
        for (index, pixel) in fb.data().as_chunks::<4>().0.iter().enumerate() {
            assert_eq!(
                pixel[3],
                0xFF,
                "pixel ({}, {}) unpainted",
                index % width,
                index / width
            );
        }
    }

    #[test]
    fn avc420_is_advertised_only_with_a_decoder() {
        let v8 = CapabilitySet::V8 {
            flags: CapabilitiesV8Flags::SMALL_CACHE,
        };
        assert_eq!(
            advertised_capabilities(None),
            [
                CapabilitySet::V8_1 {
                    flags: CapabilitiesV81Flags::SMALL_CACHE,
                },
                v8.clone(),
            ]
        );
        assert_eq!(
            advertised_capabilities(Some(Box::new(NoPictureDecoder))),
            [
                CapabilitySet::V8_1 {
                    flags: CapabilitiesV81Flags::AVC420_ENABLED | CapabilitiesV81Flags::SMALL_CACHE,
                },
                v8,
            ]
        );
    }

    #[test]
    fn frame_with_nothing_painted_is_not_content() {
        let handler = HeadlessGfxHandler::new(WIDTH, HEIGHT, false);
        let frames = handler.frame_counter();
        let has_content = handler.has_content();
        let mut client = GraphicsPipelineClient::new(Box::new(handler), None);

        // A server that sends H.264 anyway: With no decoder the client skips the
        // update and still completes the frame.
        let stream = Avc420BitmapStream {
            rectangles: vec![ExclusiveRectangle {
                left: 0,
                top: 0,
                right: WIDTH,
                bottom: HEIGHT,
            }],
            quant_qual_vals: vec![QuantQuality {
                quantization_parameter: 22,
                progressive: false,
                quality: 100,
            }],
            data: &[0, 0, 0, 1, 0x65],
        };
        let update = GfxPdu::WireToSurface1(WireToSurface1Pdu {
            surface_id: 1,
            codec_id: Codec1Type::Avc420,
            pixel_format: PixelFormat::XRgb,
            destination_rectangle: ExclusiveRectangle {
                left: 0,
                top: 0,
                right: WIDTH,
                bottom: HEIGHT,
            },
            bitmap_data: encode_vec(&stream).expect("encode AVC420 stream"),
        });
        feed(&mut client, &frame_pdus(vec![update]));

        assert_eq!(client.dropped_updates(), 0);
        assert_eq!(frames.load(Ordering::Relaxed), 1);
        assert!(!has_content.load(Ordering::Relaxed));
    }

    #[test]
    fn update_on_an_unmapped_surface_is_not_content() {
        let handler = HeadlessGfxHandler::new(WIDTH, HEIGHT, false);
        let frames = handler.frame_counter();
        let has_content = handler.has_content();
        let mut client = GraphicsPipelineClient::new(Box::new(handler), None);

        feed(
            &mut client,
            &unmapped(frame_pdus(vec![progressive_update()])),
        );

        assert_eq!(client.dropped_updates(), 0);
        assert_eq!(frames.load(Ordering::Relaxed), 1);
        assert!(!has_content.load(Ordering::Relaxed));
    }

    #[test]
    fn solid_fill_is_content_until_graphics_reset() {
        let handler = HeadlessGfxHandler::new(WIDTH, HEIGHT, false);
        let framebuffer = handler.framebuffer();
        let has_content = handler.has_content();
        let mut client = GraphicsPipelineClient::new(Box::new(handler), None);

        feed(
            &mut client,
            &frame_pdus(vec![solid_fill(
                1,
                rect(0, 0, WIDTH, HEIGHT),
                0x20,
                0x40,
                0x80,
            )]),
        );
        assert!(has_content.load(Ordering::Relaxed));
        assert_eq!(
            pixel(&framebuffer.lock().expect("framebuffer lock"), 5, 5),
            [0x20, 0x40, 0x80, 0xFF]
        );

        feed(
            &mut client,
            &[GfxPdu::ResetGraphics(ResetGraphicsPdu {
                width: WIDTH.into(),
                height: HEIGHT.into(),
                monitors: Vec::new(),
            })],
        );
        assert!(!has_content.load(Ordering::Relaxed));
    }

    #[test]
    fn surface_to_surface_source_past_the_edge_copies_no_wrapped_pixels() {
        let handler = HeadlessGfxHandler::new(WIDTH, HEIGHT, false);
        let framebuffer = handler.framebuffer();
        let mut client = GraphicsPipelineClient::new(Box::new(handler), None);

        // A 4x2 source surface, red over blue. Its source rectangle runs two pixels
        // past the right edge, where a copy by byte offset would read the blue row.
        feed(
            &mut client,
            &frame_pdus(vec![
                GfxPdu::CreateSurface(CreateSurfacePdu {
                    surface_id: 2,
                    width: 4,
                    height: 2,
                    pixel_format: PixelFormat::XRgb,
                }),
                solid_fill(2, rect(0, 0, 4, 1), 0xFF, 0, 0),
                solid_fill(2, rect(0, 1, 4, 2), 0, 0, 0xFF),
                GfxPdu::SurfaceToSurface(SurfaceToSurfacePdu {
                    source_surface_id: 2,
                    destination_surface_id: 1,
                    source_rectangle: rect(2, 0, 6, 1),
                    destination_points: vec![Point { x: 0, y: 0 }],
                }),
            ]),
        );

        let fb = framebuffer.lock().expect("framebuffer lock");
        assert_eq!(pixel(&fb, 0, 0), [0xFF, 0, 0, 0xFF]);
        assert_eq!(pixel(&fb, 1, 0), [0xFF, 0, 0, 0xFF]);
        assert_eq!(pixel(&fb, 2, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&fb, 3, 0), [0, 0, 0, 0]);
    }

    /// Fills an unmapped `width` x `height` surface 2 with `r` and caches all of it in
    /// slot 1.
    fn cached_tile(width: u16, height: u16, r: u8) -> Vec<GfxPdu> {
        vec![
            GfxPdu::CreateSurface(CreateSurfacePdu {
                surface_id: 2,
                width,
                height,
                pixel_format: PixelFormat::XRgb,
            }),
            solid_fill(2, rect(0, 0, width, height), r, 0, 0),
            GfxPdu::SurfaceToCache(SurfaceToCachePdu {
                surface_id: 2,
                cache_key: 0x1234,
                cache_slot: 1,
                source_rectangle: rect(0, 0, width, height),
            }),
        ]
    }

    fn cache_to_surface(points: &[(u16, u16)]) -> GfxPdu {
        GfxPdu::CacheToSurface(CacheToSurfacePdu {
            cache_slot: 1,
            surface_id: 1,
            destination_points: points.iter().map(|&(x, y)| Point { x, y }).collect(),
        })
    }

    #[test]
    fn cached_tile_paints_every_destination_point_after_a_graphics_reset() {
        let handler = HeadlessGfxHandler::new(WIDTH, HEIGHT, false);
        let framebuffer = handler.framebuffer();
        let has_content = handler.has_content();
        let mut client = GraphicsPipelineClient::new(Box::new(handler), None);

        feed(&mut client, &frame_pdus(cached_tile(8, 8, 0xC0)));
        // The next frame resets graphics before painting from the cache, which keeps
        // the slot. Its capabilities confirm is left out: That comes once per channel.
        let frame = frame_pdus(vec![cache_to_surface(&[(0, 0), (16, 16)])]);
        feed(&mut client, &frame[1..]);

        assert!(has_content.load(Ordering::Relaxed));
        let fb = framebuffer.lock().expect("framebuffer lock");
        for (x, y) in [(0, 0), (7, 7), (16, 16), (23, 23)] {
            assert_eq!(pixel(&fb, x, y), [0xC0, 0, 0, 0xFF], "pixel ({x}, {y})");
        }
        assert_eq!(pixel(&fb, 8, 8), [0, 0, 0, 0]);
    }

    #[test]
    fn cached_tile_past_the_edge_paints_no_wrapped_pixels() {
        let handler = HeadlessGfxHandler::new(WIDTH, HEIGHT, false);
        let framebuffer = handler.framebuffer();
        let mut client = GraphicsPipelineClient::new(Box::new(handler), None);

        // A 4x1 tile placed two pixels from the right edge, where a copy by byte offset
        // would put its last two pixels at the start of row 1.
        let mut updates = cached_tile(4, 1, 0xC0);
        updates.push(cache_to_surface(&[(WIDTH - 2, 0)]));
        feed(&mut client, &frame_pdus(updates));

        let fb = framebuffer.lock().expect("framebuffer lock");
        let right = usize::from(WIDTH) - 1;
        assert_eq!(pixel(&fb, right - 1, 0), [0xC0, 0, 0, 0xFF]);
        assert_eq!(pixel(&fb, right, 0), [0xC0, 0, 0, 0xFF]);
        assert_eq!(pixel(&fb, 0, 1), [0, 0, 0, 0]);
        assert_eq!(pixel(&fb, 1, 1), [0, 0, 0, 0]);
    }

    #[test]
    fn evicted_slot_paints_nothing() {
        let handler = HeadlessGfxHandler::new(WIDTH, HEIGHT, false);
        let framebuffer = handler.framebuffer();
        let has_content = handler.has_content();
        let mut client = GraphicsPipelineClient::new(Box::new(handler), None);

        let mut updates = cached_tile(8, 8, 0xC0);
        updates.push(GfxPdu::EvictCacheEntry(EvictCacheEntryPdu {
            cache_slot: 1,
        }));
        updates.push(cache_to_surface(&[(0, 0)]));
        feed(&mut client, &frame_pdus(updates));

        assert!(!has_content.load(Ordering::Relaxed));
        assert_eq!(
            pixel(&framebuffer.lock().expect("framebuffer lock"), 0, 0),
            [0, 0, 0, 0]
        );
    }
}
