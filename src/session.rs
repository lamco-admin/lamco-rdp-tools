use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use ironrdp_cliprdr::{
    CliprdrClient,
    pdu::{
        ClipboardFileAttributes, ClipboardFormat, ClipboardFormatId, ClipboardFormatName,
        FileContentsFlags, FileContentsRequest, FileContentsResponse, FileDescriptor,
        FormatDataResponse, PackedFileList,
    },
};
use ironrdp_connector::connection_activation::ConnectionActivationState;
use ironrdp_core::{IntoOwned, WriteBuf};
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_pdu::input::fast_path::FastPathInputEvent;
use ironrdp_session::{ActiveStage, ActiveStageOutput, fast_path, image::DecodedImage};
use ironrdp_tokio::{FramedWrite, single_sequence_step_read, split_tokio_framed};
use tracing::{debug, info, trace, warn};

use crate::{
    clipboard::ClipboardState,
    connection::{ConnectResult, ErasedStream},
    input::{self, InputInjector},
    report::PerformanceReport,
};

pub(crate) struct HeadlessSession {
    active_stage: ActiveStage,
    /// Slow-path framebuffer (bitmap updates without EGFX)
    image: DecodedImage,
    /// EGFX composited framebuffer (populated by `HeadlessGfxHandler`)
    egfx_framebuffer: Arc<Mutex<crate::gfx::Framebuffer>>,
    egfx_has_content: Arc<AtomicBool>,
    egfx_negotiated: Arc<AtomicBool>,
    egfx_caps: Arc<Mutex<Option<String>>>,
    egfx_frame_count: Arc<AtomicU64>,
    /// Server capabilities observed while connecting, for the `info`/`report` output.
    observed: crate::connection::ObservedCapabilities,
    reader: ironrdp_tokio::TokioFramed<tokio::io::ReadHalf<ErasedStream>>,
    writer: ironrdp_tokio::TokioFramed<tokio::io::WriteHalf<ErasedStream>>,
    input_injector: InputInjector,
    clipboard_state: Arc<Mutex<ClipboardState>>,
    audio_state: Arc<Mutex<crate::audio::AudioState>>,
    metrics: SessionMetrics,
    peer_disconnected: bool,
    server_addr: String,
    /// Calibration correction offsets applied to mouse coordinates
    calibration_offset: Option<(f64, f64)>,
}

struct SessionMetrics {
    session_start: Instant,
    first_frame: Option<Instant>,
    frame_times: Vec<Instant>,
    graphics_updates: u64,
    bytes_received: u64,
    bytes_sent: u64,
}

impl SessionMetrics {
    fn new() -> Self {
        Self {
            session_start: Instant::now(),
            first_frame: None,
            frame_times: Vec::new(),
            graphics_updates: 0,
            bytes_received: 0,
            bytes_sent: 0,
        }
    }

    fn record_frame(&mut self) {
        let now = Instant::now();
        if self.first_frame.is_none() {
            self.first_frame = Some(now);
        }
        self.frame_times.push(now);
        self.graphics_updates += 1;
    }

    fn record_bytes_received(&mut self, n: u64) {
        self.bytes_received += n;
    }

    fn record_bytes_sent(&mut self, n: u64) {
        self.bytes_sent += n;
    }

    fn report(&self) -> PerformanceReport {
        let time_to_first_frame_ms = self
            .first_frame
            .map(|t| t.duration_since(self.session_start).as_millis() as u64);

        let elapsed_secs = self.session_start.elapsed().as_secs_f64();

        let avg_fps = if self.frame_times.len() > 1 {
            let first = self.frame_times[0];
            let last = self.frame_times[self.frame_times.len() - 1];
            let span = last.duration_since(first).as_secs_f64();
            if span > 0.0 {
                #[expect(clippy::cast_precision_loss)]
                Some((self.frame_times.len() as f64 - 1.0) / span)
            } else {
                None
            }
        } else {
            None
        };

        #[expect(clippy::cast_precision_loss)]
        let bandwidth_kbps = if elapsed_secs > 0.1 {
            Some((self.bytes_received as f64 * 8.0) / (elapsed_secs * 1000.0))
        } else {
            None
        };

        let (p50, p99) = frame_interval_percentiles(&self.frame_times);

        PerformanceReport {
            time_to_first_frame_ms,
            total_frames: self.graphics_updates,
            avg_fps,
            bandwidth_kbps,
            frame_interval_p50_ms: p50,
            frame_interval_p99_ms: p99,
            bytes_received: self.bytes_received,
            bytes_sent: self.bytes_sent,
        }
    }
}

fn frame_interval_percentiles(times: &[Instant]) -> (Option<f64>, Option<f64>) {
    if times.len() < 2 {
        return (None, None);
    }

    let mut intervals: Vec<f64> = times
        .windows(2)
        .map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1000.0)
        .collect();

    intervals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let p50 = percentile_sorted(&intervals, 50.0);
    let p99 = percentile_sorted(&intervals, 99.0);

    (Some(p50), Some(p99))
}

fn percentile_sorted(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    #[expect(clippy::cast_precision_loss)]
    let idx = (pct / 100.0 * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

impl HeadlessSession {
    pub(crate) fn from_connect_result(result: ConnectResult) -> Self {
        let ConnectResult {
            connection_result,
            framed,
            egfx_framebuffer,
            egfx_has_content,
            egfx_negotiated,
            egfx_caps,
            egfx_frame_count,
            clipboard_state,
            audio_state,
            observed,
        } = result;

        let image = DecodedImage::new(
            PixelFormat::RgbA32,
            connection_result.desktop_size.width,
            connection_result.desktop_size.height,
        );

        let active_stage = ActiveStage::new(connection_result);
        let (reader, writer) = split_tokio_framed(framed);

        Self {
            active_stage,
            image,
            egfx_framebuffer,
            egfx_has_content,
            egfx_negotiated,
            egfx_caps,
            egfx_frame_count,
            observed,
            reader,
            writer,
            input_injector: InputInjector::new(),
            clipboard_state,
            audio_state,
            metrics: SessionMetrics::new(),
            peer_disconnected: false,
            server_addr: String::new(),
            calibration_offset: None,
        }
    }

    pub(crate) fn set_server_addr(&mut self, addr: &str) {
        addr.clone_into(&mut self.server_addr);
    }

    pub(crate) fn server_address(&self) -> &str {
        &self.server_addr
    }

    pub(crate) fn set_calibration_offset(&mut self, offset_x: f64, offset_y: f64) {
        self.calibration_offset = Some((offset_x, offset_y));
    }

    /// Apply calibration correction to coordinates if a profile is loaded.
    pub(crate) fn calibrate_position(&self, x: u16, y: u16) -> (u16, u16) {
        if let Some((ox, oy)) = self.calibration_offset {
            let cx = (f64::from(x) - ox).round().max(0.0) as u16;
            let cy = (f64::from(y) - oy).round().max(0.0) as u16;
            (cx, cy)
        } else {
            (x, y)
        }
    }

    /// Process incoming frames until the specified duration elapses or a terminal event occurs.
    pub(crate) async fn run_for(&mut self, duration: Duration) -> Result<()> {
        let deadline = Instant::now() + duration;

        loop {
            if Instant::now() >= deadline {
                info!(
                    frames = self.metrics.graphics_updates,
                    "Session duration reached"
                );
                break;
            }

            let remaining = deadline.saturating_duration_since(Instant::now());

            tokio::select! {
                frame = self.reader.read_pdu() => {
                    let result = match frame {
                        Ok((action, payload)) => {
                            self.metrics.record_bytes_received(payload.len() as u64);
                            trace!(?action, len = payload.len());
                            self.process_pdu(action, &payload)
                        }
                        Err(e) => {
                            info!(frames = self.metrics.graphics_updates, "Peer disconnected: {e}");
                            self.peer_disconnected = true;
                            break;
                        }
                    };
                    self.dispatch_outputs(result?).await?;
                }
                () = tokio::time::sleep(remaining) => {
                    break;
                }
            }
        }

        Ok(())
    }

    /// Process incoming frames until at least one graphics update arrives, or timeout.
    pub(crate) async fn wait_for_frame(&mut self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        // Count both legacy bitmap updates and EGFX decoded frames. EGFX frames
        // arrive via the DVC graphics pipeline and increment `egfx_frame_count`,
        // not `graphics_updates`, so keying on the bitmap counter alone misses
        // them entirely (an AVC420 server then always reports no initial frame).
        let initial_count = self.frame_count();

        loop {
            if self.frame_count() > initial_count {
                return Ok(true);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }

            tokio::select! {
                frame = self.reader.read_pdu() => {
                    let result = match frame {
                        Ok((action, payload)) => {
                            self.metrics.record_bytes_received(payload.len() as u64);
                            self.process_pdu(action, &payload)
                        }
                        Err(e) => {
                            debug!("Peer disconnected while waiting for frame: {e}");
                            self.peer_disconnected = true;
                            return Ok(false);
                        }
                    };
                    self.dispatch_outputs(result?).await?;
                }
                () = tokio::time::sleep(remaining) => {
                    return Ok(false);
                }
            }
        }
    }

    /// Process one PDU through the active stage, tolerating the benign "data for a
    /// DVC channel we did not open" error. Some servers (xrdp, lamco-rdp-server)
    /// advertise ECHO and `FreeRDP` input dynamic channels that we decline, then send
    /// data on them anyway; that PDU must be skipped, not end the session. Whether it
    /// arrives before capture completes is a timing race, so the only robust handling
    /// is to ignore it wherever it lands.
    fn process_pdu(
        &mut self,
        action: ironrdp_pdu::Action,
        payload: &[u8],
    ) -> Result<Vec<ActiveStageOutput>> {
        match self.active_stage.process(&mut self.image, action, payload) {
            Ok(outputs) => Ok(outputs),
            Err(e) if format!("{e:?}").contains("non existing DVC channel") => {
                debug!(error = ?e, "Skipping data PDU for an unhandled DVC channel");
                Ok(Vec::new())
            }
            Err(e) => Err(anyhow::anyhow!("process PDU: {e:?}")),
        }
    }

    async fn dispatch_outputs(&mut self, outputs: Vec<ActiveStageOutput>) -> Result<()> {
        for out in outputs {
            match out {
                ActiveStageOutput::ResponseFrame(frame) => {
                    self.metrics.record_bytes_sent(frame.len() as u64);
                    self.writer
                        .write_all(&frame)
                        .await
                        .map_err(|e| anyhow::anyhow!("write response: {e}"))?;
                }
                ActiveStageOutput::GraphicsUpdate(_region) => {
                    self.metrics.record_frame();
                    debug!(frame = self.metrics.graphics_updates, "Graphics update");
                }
                ActiveStageOutput::Terminate(reason) => {
                    info!(?reason, "Server terminated session");
                    return Ok(());
                }
                ActiveStageOutput::DeactivateAll(mut connection_activation) => {
                    // MS-RDPBCGR 1.3.1.3: Deactivation-Reactivation Sequence
                    // Server sends Deactivate All (e.g. after resize), then
                    // re-runs the activation exchange to establish new params.
                    debug!("Server sent Deactivate All, running reactivation sequence");
                    let mut buf = WriteBuf::new();
                    loop {
                        let written = single_sequence_step_read(
                            &mut self.reader,
                            &mut *connection_activation,
                            &mut buf,
                        )
                        .await
                        .map_err(|e| anyhow::anyhow!("reactivation sequence: {e}"))?;

                        if written.size().is_some() {
                            self.writer
                                .write_all(buf.filled())
                                .await
                                .map_err(|e| anyhow::anyhow!("reactivation write: {e}"))?;
                        }

                        if let ConnectionActivationState::Finalized {
                            io_channel_id,
                            user_channel_id,
                            desktop_size,
                            enable_server_pointer,
                            pointer_software_rendering,
                            share_id,
                        } = connection_activation.connection_activation_state()
                        {
                            info!(
                                width = desktop_size.width,
                                height = desktop_size.height,
                                "Reactivation complete, new desktop size"
                            );
                            self.image = DecodedImage::new(
                                PixelFormat::RgbA32,
                                desktop_size.width,
                                desktop_size.height,
                            );
                            // Reset EGFX framebuffer to new size so captures
                            // don't use stale dimensions after resize
                            {
                                let mut fb =
                                    self.egfx_framebuffer.lock().expect("egfx framebuffer lock");
                                *fb = crate::gfx::Framebuffer::new(
                                    desktop_size.width,
                                    desktop_size.height,
                                );
                                self.egfx_has_content.store(false, Ordering::Relaxed);
                            }
                            self.active_stage.set_fastpath_processor(
                                fast_path::ProcessorBuilder {
                                    io_channel_id,
                                    user_channel_id,
                                    enable_server_pointer,
                                    pointer_software_rendering,
                                    share_id,
                                    bulk_decompressor: None,
                                }
                                .build(),
                            );
                            self.active_stage
                                .set_enable_server_pointer(enable_server_pointer);
                            break;
                        }
                    }
                }
                _ => {
                    trace!("Unhandled output variant");
                }
            }
        }

        // Handle any pending clipboard requests (set by backend callbacks)
        self.check_format_list_request().await?;
        self.check_clipboard_data_request().await?;
        self.check_file_list_data_request().await?;
        self.check_file_contents_request().await?;

        Ok(())
    }

    /// Save the current framebuffer as a PNG screenshot.
    pub(crate) fn save_screenshot(&self, path: &Path) -> Result<()> {
        let img = self.current_frame();

        img.save(path)
            .with_context(|| format!("save screenshot to {}", path.display()))?;

        info!(path = %path.display(), "Screenshot saved");
        Ok(())
    }

    /// Write the current framebuffer as PNG to a writer (e.g. stdout).
    pub(crate) fn write_screenshot_to<W: std::io::Write>(&self, writer: &mut W) -> Result<()> {
        let img = self.current_frame();
        let (width, height) = img.dimensions();

        let encoder = image::codecs::png::PngEncoder::new(writer);
        image::ImageEncoder::write_image(
            encoder,
            img.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .context("encode PNG")?;

        Ok(())
    }

    /// Extract a sub-region from the framebuffer as an RGBA image.
    pub(crate) fn capture_region(&self, x: u16, y: u16, w: u16, h: u16) -> image::RgbaImage {
        let frame = self.current_frame();
        let full_width = frame.width() as usize;
        let data = frame.as_raw();
        let stride = full_width * 4;

        let mut region = image::RgbaImage::new(u32::from(w), u32::from(h));
        for row in 0..usize::from(h) {
            let src_y = usize::from(y) + row;
            let src_offset = src_y * stride + usize::from(x) * 4;
            let row_len = usize::from(w) * 4;
            let src_slice = &data[src_offset..src_offset + row_len];
            for col in 0..usize::from(w) {
                let px_offset = col * 4;
                region.put_pixel(
                    col as u32,
                    row as u32,
                    image::Rgba([
                        src_slice[px_offset],
                        src_slice[px_offset + 1],
                        src_slice[px_offset + 2],
                        src_slice[px_offset + 3],
                    ]),
                );
            }
        }

        region
    }

    pub(crate) fn image_dimensions(&self) -> (u16, u16) {
        (self.image.width(), self.image.height())
    }

    /// Whether EGFX capabilities were negotiated (channel is active).
    pub(crate) fn egfx_active(&self) -> bool {
        self.egfx_negotiated.load(Ordering::Relaxed)
    }

    /// The server-confirmed EGFX capability tier, once negotiated (e.g. "V8.1 (AVC420)").
    pub(crate) fn egfx_caps(&self) -> Option<String> {
        self.egfx_caps
            .lock()
            .expect("EGFX caps mutex poisoned")
            .clone()
    }

    /// Capabilities observed while connecting (security, channels, codecs, color depth).
    pub(crate) fn observed_capabilities(&self) -> &crate::connection::ObservedCapabilities {
        &self.observed
    }

    pub(crate) fn performance_report(&self) -> PerformanceReport {
        let mut report = self.metrics.report();
        // Include EGFX decoded frames in the total (metrics only tracks GDI bitmap updates)
        report.total_frames += self.egfx_frame_count.load(Ordering::Relaxed);
        report
    }

    pub(crate) fn frame_count(&self) -> u64 {
        // Combine bitmap (GDI) updates and EGFX decoded frames
        self.metrics.graphics_updates + self.egfx_frame_count.load(Ordering::Relaxed)
    }

    /// Get the full framebuffer as an owned `RgbaImage`.
    ///
    /// Prefers the EGFX composited framebuffer when EGFX content has been
    /// rendered; falls back to the slow-path framebuffer otherwise.
    /// True once any screen content has been received. EGFX content can arrive
    /// via `SolidFill` / `RemoteFX` / surface copies that populate the framebuffer
    /// without incrementing the frame counter, so this checks the content flag.
    pub(crate) fn has_content(&self) -> bool {
        self.egfx_has_content.load(Ordering::Relaxed) || self.metrics.graphics_updates > 0
    }

    pub(crate) fn current_frame(&self) -> image::RgbaImage {
        if self.egfx_has_content.load(Ordering::Relaxed) {
            let fb = self.egfx_framebuffer.lock().expect("egfx framebuffer lock");
            image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(
                u32::from(fb.width()),
                u32::from(fb.height()),
                fb.data().to_vec(),
            )
            .expect("EGFX framebuffer dimensions must be consistent")
        } else {
            let width = u32::from(self.image.width());
            let height = u32::from(self.image.height());
            let data = self.image.data();
            image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(width, height, data.to_vec())
                .expect("framebuffer dimensions must be consistent")
        }
    }

    /// Duration since the last graphics frame was received, or None if no frames yet.
    pub(crate) fn time_since_last_frame(&self) -> Option<Duration> {
        self.metrics.frame_times.last().map(Instant::elapsed)
    }

    /// Wait until no new frames arrive for `stillness` duration.
    /// Returns true if the screen settled, false on timeout.
    pub(crate) async fn wait_still(
        &mut self,
        stillness: Duration,
        timeout: Duration,
    ) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        let mut last_frame_at = Instant::now();

        loop {
            let since_last = last_frame_at.elapsed();
            if since_last >= stillness {
                return Ok(true);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }

            let wait_for = stillness.saturating_sub(since_last).min(remaining);

            tokio::select! {
                frame = self.reader.read_pdu() => {
                    let result = match frame {
                        Ok((action, payload)) => {
                            self.metrics.record_bytes_received(payload.len() as u64);
                            self.process_pdu(action, &payload)
                        }
                        Err(e) => {
                            debug!("Peer disconnected during wait_still: {e}");
                            self.peer_disconnected = true;
                            return Ok(false);
                        }
                    };
                    let outputs = result?;
                    // Track whether this PDU carried a graphics update
                    let had_graphics = outputs.iter().any(|o| matches!(o, ActiveStageOutput::GraphicsUpdate(_)));
                    self.dispatch_outputs(outputs).await?;
                    if had_graphics {
                        last_frame_at = Instant::now();
                    }
                }
                () = tokio::time::sleep(wait_for) => {
                    // No frame arrived during the wait: check if stillness is satisfied
                }
            }
        }
    }

    /// Wait until a new graphics frame arrives.
    /// Returns true if a change was detected, false on timeout.
    pub(crate) async fn wait_change(&mut self, timeout: Duration) -> Result<bool> {
        self.wait_for_frame(timeout).await
    }

    pub(crate) fn peer_disconnected(&self) -> bool {
        self.peer_disconnected
    }

    pub(crate) fn bytes_received(&self) -> u64 {
        self.metrics.bytes_received
    }

    pub(crate) fn bytes_sent(&self) -> u64 {
        self.metrics.bytes_sent
    }

    pub(crate) fn session_uptime(&self) -> Duration {
        self.metrics.session_start.elapsed()
    }

    /// Mark the session as disconnected without actually closing the transport.
    /// Useful in the REPL to allow reconnecting later.
    pub(crate) fn force_disconnect(&mut self) {
        self.peer_disconnected = true;
    }

    // --- Display resize ---

    /// Send a `DisplayControl` `MonitorLayout` PDU to resize the desktop.
    pub(crate) async fn send_resize(&mut self, width: u32, height: u32) -> Result<bool> {
        let frame = match self.active_stage.encode_resize(width, height, None, None) {
            Some(Ok(frame)) => frame,
            Some(Err(e)) => bail!("encode resize: {e}"),
            None => {
                warn!("DisplayControl DVC not ready, resize skipped");
                return Ok(false);
            }
        };

        self.metrics.record_bytes_sent(frame.len() as u64);
        self.writer
            .write_all(&frame)
            .await
            .map_err(|e| anyhow::anyhow!("write resize PDU: {e}"))?;

        info!(width, height, "Resize PDU sent");
        Ok(true)
    }

    // --- Input injection ---

    pub(crate) async fn send_input_events(&mut self, events: &[FastPathInputEvent]) -> Result<()> {
        let outputs = self
            .active_stage
            .process_fastpath_input(&mut self.image, events)
            .map_err(|e| anyhow::anyhow!("process input: {e}"))?;
        self.dispatch_outputs(outputs).await
    }

    /// Send a single key press+release by scancode.
    pub(crate) async fn send_key(&mut self, scancode: u16) -> Result<()> {
        let events = self.input_injector.type_scancode(scancode);
        self.send_input_events(&events).await
    }

    /// Parse a key spec (e.g. "ctrl-c", "enter") and send the combo.
    pub(crate) async fn send_key_combo(&mut self, spec: &str) -> Result<()> {
        let action = input::parse_key_spec(spec)?;
        let events = self.input_injector.combo_events(&action);
        self.send_input_events(&events).await
    }

    /// Press a key combo, hold for specified duration, then release.
    pub(crate) async fn send_key_combo_held(&mut self, spec: &str, hold_ms: u64) -> Result<()> {
        let action = input::parse_key_spec(spec)?;
        let down_events = self.input_injector.combo_down(&action);
        self.send_input_events(&down_events).await?;
        self.run_for(Duration::from_millis(hold_ms)).await?;
        let up_events = self.input_injector.combo_up(&action);
        self.send_input_events(&up_events).await
    }

    /// Press a key without releasing it.
    pub(crate) async fn send_key_down(&mut self, spec: &str) -> Result<()> {
        let action = input::parse_key_spec(spec)?;
        if !action.modifiers.is_empty() {
            bail!("keydown does not accept modifiers, use a plain key name");
        }
        let events = self.input_injector.key_down(action.key);
        self.send_input_events(&events).await
    }

    /// Release a previously pressed key.
    pub(crate) async fn send_key_up(&mut self, spec: &str) -> Result<()> {
        let action = input::parse_key_spec(spec)?;
        if !action.modifiers.is_empty() {
            bail!("keyup does not accept modifiers, use a plain key name");
        }
        let events = self.input_injector.key_up(action.key);
        self.send_input_events(&events).await
    }

    /// Type ASCII text as scancode sequences, with a short delay between characters.
    pub(crate) async fn send_text(&mut self, text: &str) -> Result<()> {
        info!(len = text.len(), "Typing text");
        let batches = self.input_injector.type_text(text);
        for batch in batches {
            self.send_input_events(&batch).await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.process_pending().await?;
        }
        Ok(())
    }

    /// Type text using unicode keystroke events, with a short delay between characters.
    pub(crate) async fn send_unicode_text(&mut self, text: &str) -> Result<()> {
        info!(len = text.len(), "Typing unicode text");
        let batches = self.input_injector.type_unicode(text);
        for batch in batches {
            self.send_input_events(&batch).await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.process_pending().await?;
        }
        Ok(())
    }

    /// Type text without logging the content (for passwords).
    pub(crate) async fn send_password(&mut self, password: &str) -> Result<()> {
        debug!(len = password.len(), "Typing password (content redacted)");
        let batches = self.input_injector.type_text(password);
        for batch in batches {
            self.send_input_events(&batch).await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
            self.process_pending().await?;
        }
        Ok(())
    }

    /// Move mouse to absolute position.
    pub(crate) async fn mouse_move(&mut self, x: u16, y: u16) -> Result<()> {
        let events = self.input_injector.mouse_move(x, y);
        self.send_input_events(&events).await
    }

    /// Click a mouse button at the current position.
    pub(crate) async fn mouse_click(&mut self, button_name: &str) -> Result<()> {
        let button = input::parse_button(button_name)?;
        let events = self.input_injector.mouse_click(button);
        self.send_input_events(&events).await
    }

    /// Double-click a mouse button at the current position.
    pub(crate) async fn send_double_click(&mut self, button_name: &str) -> Result<()> {
        let button = input::parse_button(button_name)?;
        let events = self.input_injector.mouse_double_click(button);
        self.send_input_events(&events).await
    }

    /// Execute a mouse drag from one position to another.
    pub(crate) async fn send_drag(&mut self, from: (u16, u16), to: (u16, u16)) -> Result<()> {
        let steps = self
            .input_injector
            .mouse_drag(from, to, ironrdp_input::MouseButton::Left);
        for step in steps {
            self.send_input_events(&step).await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        Ok(())
    }

    /// Scroll the mouse wheel.
    pub(crate) async fn send_scroll(&mut self, up: bool, notches: u32) -> Result<()> {
        let batches = self.input_injector.scroll(up, notches);
        for batch in batches {
            self.send_input_events(&batch).await?;
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        Ok(())
    }

    // --- Clipboard ---

    /// Advertise clipboard content to the server.
    pub(crate) async fn set_clipboard(&mut self, text: &str) -> Result<()> {
        {
            let mut state = self.clipboard_state.lock().expect("clipboard lock");
            state.pending_send = Some(text.to_owned());
        }

        let formats = vec![ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)];

        let cliprdr = self
            .active_stage
            .get_svc_processor_mut::<CliprdrClient>()
            .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

        let svc_messages = cliprdr
            .initiate_copy(&formats)
            .map_err(|e| anyhow::anyhow!("initiate_copy: {e}"))?;

        let frame = self
            .active_stage
            .process_svc_processor_messages(svc_messages)
            .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

        self.writer
            .write_all(&frame)
            .await
            .map_err(|e| anyhow::anyhow!("write clipboard frame: {e}"))?;

        info!(
            text_len = text.len(),
            "Clipboard content advertised to server"
        );

        self.process_for(Duration::from_millis(500)).await?;
        self.check_format_list_request().await?;
        self.check_clipboard_data_request().await?;

        Ok(())
    }

    /// Request clipboard content from the server.
    pub(crate) async fn get_clipboard(&mut self, timeout: Duration) -> Result<Option<String>> {
        {
            let mut state = self.clipboard_state.lock().expect("clipboard lock");
            state.received_data = None;
        }

        // Process pending PDUs first — this allows the CLIPRDR handshake to
        // complete (MonitorReady → FormatList → FormatListResponse → Ready)
        // before we try initiate_paste. Without this, the channel may still
        // be in Initialization state and initiate_paste silently returns empty.
        self.process_for(Duration::from_millis(200)).await?;
        self.check_format_list_request().await?;
        // Give the server time to respond with FormatListResponse
        self.process_for(Duration::from_millis(300)).await?;

        let ready = self.clipboard_state.lock().expect("clipboard lock").ready;
        if !ready {
            debug!("CLIPRDR channel not ready after handshake attempt — paste may fail");
        }

        let cliprdr = self
            .active_stage
            .get_svc_processor_mut::<CliprdrClient>()
            .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

        let svc_messages = cliprdr
            .initiate_paste(ClipboardFormatId::CF_UNICODETEXT)
            .map_err(|e| anyhow::anyhow!("initiate_paste: {e}"))?;

        let frame = self
            .active_stage
            .process_svc_processor_messages(svc_messages)
            .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

        if frame.is_empty() {
            warn!("initiate_paste produced no PDUs — CLIPRDR channel likely not Ready");
            return Ok(None);
        }

        self.writer
            .write_all(&frame)
            .await
            .map_err(|e| anyhow::anyhow!("write clipboard frame: {e}"))?;

        info!("Clipboard paste requested from server");

        let deadline = Instant::now() + timeout;
        loop {
            {
                let state = self.clipboard_state.lock().expect("clipboard lock");
                if state.received_data.is_some() {
                    return Ok(state.received_data.clone());
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                debug!("Clipboard get timed out");
                return Ok(None);
            }

            tokio::select! {
                frame = self.reader.read_pdu() => {
                    let result = match frame {
                        Ok((action, payload)) => {
                            self.metrics.record_bytes_received(payload.len() as u64);
                            self.process_pdu(action, &payload)
                        }
                        Err(e) => {
                            self.peer_disconnected = true;
                            bail!("peer disconnected during clipboard get: {e}");
                        }
                    };
                    self.dispatch_outputs(result?).await?;
                    self.check_format_list_request().await?;
                    self.check_clipboard_data_request().await?;
                }
                () = tokio::time::sleep(remaining) => {
                    return Ok(None);
                }
            }
        }
    }

    /// Process any pending incoming PDUs without blocking.
    pub(crate) async fn process_pending(&mut self) -> Result<()> {
        self.process_for(Duration::from_millis(10)).await
    }

    /// Process incoming PDUs for a given duration.
    async fn process_for(&mut self, duration: Duration) -> Result<()> {
        let deadline = Instant::now() + duration;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }

            tokio::select! {
                frame = self.reader.read_pdu() => {
                    let result = match frame {
                        Ok((action, payload)) => {
                            self.metrics.record_bytes_received(payload.len() as u64);
                            self.process_pdu(action, &payload)
                        }
                        Err(e) => {
                            debug!("Peer disconnected: {e}");
                            self.peer_disconnected = true;
                            break;
                        }
                    };
                    self.dispatch_outputs(result?).await?;
                }
                () = tokio::time::sleep(remaining) => {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Respond to the CLIPRDR init handshake by sending our format list.
    ///
    /// The server sends `MonitorReady`, `IronRDP` calls `on_request_format_list` on the
    /// backend, and we must reply with `initiate_copy` (even with an empty list) to
    /// complete the handshake. Without this, the CLIPRDR channel stays in
    /// `Initialization` state and all paste operations are rejected.
    async fn check_format_list_request(&mut self) -> Result<()> {
        let (requested, pending_text) = {
            let mut state = self.clipboard_state.lock().expect("clipboard lock");
            if !state.format_list_requested {
                return Ok(());
            }
            state.format_list_requested = false;
            (true, state.pending_send.is_some())
        };

        if !requested {
            return Ok(());
        }

        let cliprdr = self
            .active_stage
            .get_svc_processor_mut::<CliprdrClient>()
            .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

        // If we have text queued to send, advertise CF_UNICODETEXT.
        // Otherwise send an empty list — the handshake still completes.
        let formats: Vec<ClipboardFormat> = if pending_text {
            vec![ClipboardFormat {
                id: ClipboardFormatId::CF_UNICODETEXT,
                name: None,
            }]
        } else {
            vec![]
        };

        let svc_messages = cliprdr
            .initiate_copy(&formats)
            .map_err(|e| anyhow::anyhow!("initiate_copy (format list handshake): {e}"))?;

        let frame = self
            .active_stage
            .process_svc_processor_messages(svc_messages)
            .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

        self.writer
            .write_all(&frame)
            .await
            .map_err(|e| anyhow::anyhow!("write format list response: {e}"))?;

        info!(
            "CLIPRDR handshake: sent format list ({} formats) — channel should transition to Ready",
            formats.len()
        );

        Ok(())
    }

    /// If the server has requested our clipboard data, respond with the pending text.
    async fn check_clipboard_data_request(&mut self) -> Result<()> {
        let pending_text = {
            let mut state = self.clipboard_state.lock().expect("clipboard lock");
            if !state.data_requested {
                return Ok(());
            }
            state.data_requested = false;
            state.pending_send.take()
        };

        if let Some(text) = pending_text {
            let response = FormatDataResponse::new_unicode_string(&text);

            let cliprdr = self
                .active_stage
                .get_svc_processor_mut::<CliprdrClient>()
                .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

            let svc_messages = cliprdr
                .submit_format_data(response)
                .map_err(|e| anyhow::anyhow!("submit_format_data: {e}"))?;

            let frame = self
                .active_stage
                .process_svc_processor_messages(svc_messages)
                .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

            self.writer
                .write_all(&frame)
                .await
                .map_err(|e| anyhow::anyhow!("write clipboard response: {e}"))?;

            info!("Clipboard data sent to server");
        } else {
            let response = FormatDataResponse::new_error();
            let cliprdr = self
                .active_stage
                .get_svc_processor_mut::<CliprdrClient>()
                .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

            let svc_messages = cliprdr
                .submit_format_data(response.into_owned())
                .map_err(|e| anyhow::anyhow!("submit_format_data error: {e}"))?;

            let frame = self
                .active_stage
                .process_svc_processor_messages(svc_messages)
                .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

            self.writer
                .write_all(&frame)
                .await
                .map_err(|e| anyhow::anyhow!("write clipboard error response: {e}"))?;
        }

        Ok(())
    }

    // --- Audio ---

    /// Start recording audio from the RDPSND channel.
    pub(crate) fn audio_start_recording(&self) {
        let mut state = self.audio_state.lock().expect("audio lock");
        state.recording = true;
        state.pcm_buffer.clear();
    }

    /// Stop recording and return the captured PCM data and format.
    pub(crate) fn audio_stop_recording(&self) -> (Vec<u8>, Option<crate::audio::CaptureFormat>) {
        let mut state = self.audio_state.lock().expect("audio lock");
        state.recording = false;
        let data = std::mem::take(&mut state.pcm_buffer);
        let format = state.capture_format.clone();
        (data, format)
    }

    /// Check if audio data has been received (non-silence detection).
    pub(crate) fn audio_rms(&self) -> f64 {
        let state = self.audio_state.lock().expect("audio lock");
        crate::audio::rms_amplitude(&state.pcm_buffer)
    }

    // --- File Transfer ---

    /// Stage a local file for clipboard transfer to the remote.
    ///
    /// Advertises `FileGroupDescriptorW` to the server. The actual data transfer
    /// happens lazily when the server requests it (e.g., after the user pastes
    /// with Ctrl+V). File contents requests are handled automatically in
    /// `dispatch_outputs`.
    pub(crate) async fn send_clipboard_file(&mut self, path: &Path) -> Result<()> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid file name: {}", path.display()))?
            .to_owned();

        let file_data = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("reading file '{}': {e}", path.display()))?;

        info!(
            file = %file_name,
            size = file_data.len(),
            "Staging file for clipboard transfer"
        );

        {
            let mut state = self.clipboard_state.lock().expect("clipboard lock");
            state.pending_file_send = Some(crate::clipboard::PendingFileSend {
                name: file_name.clone(),
                data: file_data,
            });
            state.file_list_data_requested = false;
            state.file_contents_request = None;
        }

        // Advertise the file list format to the server
        let formats = vec![
            ClipboardFormat::new(crate::clipboard::FILE_LIST_FORMAT_ID)
                .with_name(ClipboardFormatName::FILE_LIST),
        ];

        let cliprdr = self
            .active_stage
            .get_svc_processor_mut::<CliprdrClient>()
            .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

        let svc_messages = cliprdr
            .initiate_copy(&formats)
            .map_err(|e| anyhow::anyhow!("initiate_copy (file): {e}"))?;

        let frame = self
            .active_stage
            .process_svc_processor_messages(svc_messages)
            .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

        self.writer
            .write_all(&frame)
            .await
            .map_err(|e| anyhow::anyhow!("write clipboard frame: {e}"))?;

        // Brief processing time for the server to acknowledge
        self.process_for(Duration::from_millis(500)).await?;

        info!(
            file = %file_name,
            "File staged for clipboard transfer (paste with Ctrl+V to trigger)"
        );
        Ok(())
    }

    /// If the server has requested our file list descriptor, respond with it.
    /// Returns true if the response was sent.
    async fn check_file_list_data_request(&mut self) -> Result<bool> {
        let file_list = {
            let mut state = self.clipboard_state.lock().expect("clipboard lock");
            if !state.file_list_data_requested {
                return Ok(false);
            }
            state.file_list_data_requested = false;

            let pending = state
                .pending_file_send
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("file list requested but no file staged"))?;

            PackedFileList {
                files: vec![
                    FileDescriptor::new(pending.name.clone())
                        .with_attributes(ClipboardFileAttributes::NORMAL)
                        .with_file_size(pending.data.len() as u64),
                ],
            }
        };

        let response = FormatDataResponse::new_file_list(&file_list)
            .map_err(|e| anyhow::anyhow!("encoding file list: {e}"))?;

        let cliprdr = self
            .active_stage
            .get_svc_processor_mut::<CliprdrClient>()
            .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

        let svc_messages = cliprdr
            .submit_format_data(response.into_owned())
            .map_err(|e| anyhow::anyhow!("submit_format_data (file list): {e}"))?;

        let frame = self
            .active_stage
            .process_svc_processor_messages(svc_messages)
            .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

        self.writer
            .write_all(&frame)
            .await
            .map_err(|e| anyhow::anyhow!("write file list response: {e}"))?;

        info!("File list descriptor sent to server");
        Ok(true)
    }

    /// If the server has requested file contents (size or data), respond.
    /// Returns true if a response was sent.
    async fn check_file_contents_request(&mut self) -> Result<bool> {
        let (request, response) = {
            let mut state = self.clipboard_state.lock().expect("clipboard lock");
            let Some(request) = state.file_contents_request.take() else {
                return Ok(false);
            };

            let pending = state
                .pending_file_send
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("file contents requested but no file staged"))?;

            let response = if request.flags.contains(FileContentsFlags::SIZE) {
                FileContentsResponse::new_size_response(
                    request.stream_id,
                    pending.data.len() as u64,
                )
            } else if request.flags.contains(FileContentsFlags::RANGE) {
                let offset = request.position as usize;
                let len = (request.requested_size as usize).min(pending.data.len() - offset);
                let chunk = &pending.data[offset..offset + len];
                FileContentsResponse::new_data_response(request.stream_id, chunk.to_vec())
            } else {
                FileContentsResponse::new_error(request.stream_id)
            };

            (request, response)
        };

        let cliprdr = self
            .active_stage
            .get_svc_processor_mut::<CliprdrClient>()
            .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

        let svc_messages = cliprdr
            .submit_file_contents(response)
            .map_err(|e| anyhow::anyhow!("submit_file_contents: {e}"))?;

        let frame = self
            .active_stage
            .process_svc_processor_messages(svc_messages)
            .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

        self.writer
            .write_all(&frame)
            .await
            .map_err(|e| anyhow::anyhow!("write file contents response: {e}"))?;

        if request.flags.contains(FileContentsFlags::SIZE) {
            info!("File size sent to server");
        } else {
            info!(
                offset = request.position,
                requested = request.requested_size,
                "File data chunk sent to server"
            );
        }

        Ok(true)
    }

    /// Receive a file from the remote clipboard and save it locally.
    #[expect(clippy::too_many_lines)]
    pub(crate) async fn recv_clipboard_file(
        &mut self,
        save_path: &Path,
        timeout: Duration,
    ) -> Result<String> {
        // Check if the remote has offered a file list
        let file_list_format_id = {
            let state = self.clipboard_state.lock().expect("clipboard lock");
            state
                .remote_file_list_format_id
                .ok_or_else(|| anyhow::anyhow!("remote clipboard doesn't contain files"))?
        };

        // Request the file list from the server
        {
            let mut state = self.clipboard_state.lock().expect("clipboard lock");
            state.received_file_list = None;
        }

        let cliprdr = self
            .active_stage
            .get_svc_processor_mut::<CliprdrClient>()
            .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

        let svc_messages = cliprdr
            .initiate_paste(file_list_format_id)
            .map_err(|e| anyhow::anyhow!("initiate_paste (file list): {e}"))?;

        let frame = self
            .active_stage
            .process_svc_processor_messages(svc_messages)
            .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

        self.writer
            .write_all(&frame)
            .await
            .map_err(|e| anyhow::anyhow!("write clipboard frame: {e}"))?;

        // Wait for the file list
        let deadline = Instant::now() + timeout;
        let file_list = loop {
            {
                let state = self.clipboard_state.lock().expect("clipboard lock");
                if let Some(ref list) = state.received_file_list {
                    break list.clone();
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timeout waiting for file list from server");
            }

            tokio::select! {
                frame = self.reader.read_pdu() => {
                    let result = match frame {
                        Ok((action, payload)) => {
                            self.metrics.record_bytes_received(payload.len() as u64);
                            self.process_pdu(action, &payload)
                        }
                        Err(e) => {
                            self.peer_disconnected = true;
                            bail!("peer disconnected during file recv: {e}");
                        }
                    };
                    self.dispatch_outputs(result?).await?;
                }
                () = tokio::time::sleep(remaining.min(Duration::from_millis(100))) => {}
            }
        };

        if file_list.is_empty() {
            bail!("remote clipboard file list is empty");
        }

        info!(
            count = file_list.len(),
            first = %file_list[0].name,
            "Received file list from server"
        );

        // For now, receive only the first file
        let file_info = &file_list[0];

        // Request file size
        let file_size = self
            .request_file_contents_wait(0, FileContentsFlags::SIZE, 0, 8, timeout)
            .await?;

        let crate::clipboard::ReceivedFileContents::Size(size) = file_size else {
            bail!("unexpected file contents response (expected size)");
        };

        info!(file = %file_info.name, size, "File size received, requesting data");

        // Request file data in chunks (server may limit chunk size)
        let mut all_data = Vec::with_capacity(size as usize);
        let chunk_size: u32 = 64 * 1024; // 64 KiB chunks
        let mut offset: u64 = 0;

        while offset < size {
            let remaining_bytes = size - offset;
            let req_size = chunk_size.min(remaining_bytes as u32);

            let chunk = self
                .request_file_contents_wait(0, FileContentsFlags::RANGE, offset, req_size, timeout)
                .await?;

            match chunk {
                crate::clipboard::ReceivedFileContents::Data(data) => {
                    offset += data.len() as u64;
                    all_data.extend_from_slice(&data);
                }
                crate::clipboard::ReceivedFileContents::Error => {
                    bail!("server returned error for file data at offset {offset}");
                }
                crate::clipboard::ReceivedFileContents::Size(_) => {
                    bail!("unexpected file contents response (expected data, got size)");
                }
            }
        }

        // Determine save path: if it's a directory, append the remote filename
        let final_path = if save_path.is_dir() {
            save_path.join(&file_info.name)
        } else {
            save_path.to_owned()
        };

        std::fs::write(&final_path, &all_data)
            .map_err(|e| anyhow::anyhow!("writing file '{}': {e}", final_path.display()))?;

        info!(
            path = %final_path.display(),
            size = all_data.len(),
            "File saved"
        );

        Ok(final_path.display().to_string())
    }

    /// Send a `FileContentsRequest` and wait for the response.
    async fn request_file_contents_wait(
        &mut self,
        index: u32,
        flags: FileContentsFlags,
        position: u64,
        requested_size: u32,
        timeout: Duration,
    ) -> Result<crate::clipboard::ReceivedFileContents> {
        {
            let mut state = self.clipboard_state.lock().expect("clipboard lock");
            state.received_file_contents = None;
        }

        let stream_id = (index << 16)
            | if flags.contains(FileContentsFlags::SIZE) {
                1
            } else {
                2
            };

        let request = FileContentsRequest {
            stream_id,
            index: index.cast_signed(),
            flags,
            position,
            requested_size,
            data_id: None,
        };

        let cliprdr = self
            .active_stage
            .get_svc_processor_mut::<CliprdrClient>()
            .ok_or_else(|| anyhow::anyhow!("CLIPRDR channel not available"))?;

        let svc_messages = cliprdr
            .request_file_contents(request)
            .map_err(|e| anyhow::anyhow!("request_file_contents: {e}"))?;

        let frame = self
            .active_stage
            .process_svc_processor_messages(svc_messages)
            .map_err(|e| anyhow::anyhow!("encode clipboard SVC: {e}"))?;

        self.writer
            .write_all(&frame)
            .await
            .map_err(|e| anyhow::anyhow!("write file contents request: {e}"))?;

        // Wait for response
        let deadline = Instant::now() + timeout;
        loop {
            {
                let mut state = self.clipboard_state.lock().expect("clipboard lock");
                if let Some(contents) = state.received_file_contents.take() {
                    return Ok(contents);
                }
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timeout waiting for file contents response");
            }

            tokio::select! {
                frame = self.reader.read_pdu() => {
                    let result = match frame {
                        Ok((action, payload)) => {
                            self.metrics.record_bytes_received(payload.len() as u64);
                            self.process_pdu(action, &payload)
                        }
                        Err(e) => {
                            self.peer_disconnected = true;
                            bail!("peer disconnected during file contents request: {e}");
                        }
                    };
                    self.dispatch_outputs(result?).await?;
                }
                () = tokio::time::sleep(remaining.min(Duration::from_millis(100))) => {}
            }
        }
    }

    /// Attempt graceful shutdown.
    pub(crate) async fn shutdown(mut self) -> Result<()> {
        match self.active_stage.graceful_shutdown() {
            Ok(outputs) => {
                for out in outputs {
                    if let ActiveStageOutput::ResponseFrame(frame) = out {
                        let _ = self.writer.write_all(&frame).await;
                    }
                }
            }
            Err(e) => {
                debug!("Graceful shutdown failed (expected if server already closed): {e}");
            }
        }
        Ok(())
    }
}
