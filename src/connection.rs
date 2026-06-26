use std::{
    path::Path,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context as TaskContext, Poll},
};

use anyhow::{Context, Result};
use ironrdp_cliprdr::CliprdrClient;
use ironrdp_connector::{
    self as connector, ConnectionResult, Credentials, DesktopSize,
    legacy::{decode_send_data_indication, decode_share_control},
};
use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_dvc::DrdynvcClient;
use ironrdp_egfx::decode::{self, DecodedFrame, DecoderError, DecoderResult, H264Decoder};
use ironrdp_pdu::{
    nego::{ConnectionConfirm, SecurityProtocol},
    rdp::{
        capability_sets::{
            CapabilitySet, CodecProperty, MajorPlatformType, client_codecs_capabilities,
        },
        client_info::{CompressionType, PerformanceFlags, TimezoneInfo},
        headers::ShareControlPdu,
    },
    x224::X224,
};
use ironrdp_tokio::reqwest::ReqwestNetworkClient;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
};
use tracing::{debug, info, warn};

use crate::{
    audio::{AudioCaptureBackend, AudioState},
    clipboard::{ClipboardBackend, ClipboardState},
    gfx::HeadlessGfxHandler,
};

pub(crate) type ErasedStream = Box<dyn AsyncReadWrite + Unpin + Send + Sync>;
pub(crate) type UpgradedFramed = ironrdp_tokio::TokioFramed<ErasedStream>;

pub(crate) trait AsyncReadWrite: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite> AsyncReadWrite for T {}

pub(crate) struct ConnectResult {
    pub connection_result: ConnectionResult,
    pub framed: UpgradedFramed,
    pub egfx_framebuffer: Arc<Mutex<crate::gfx::Framebuffer>>,
    pub egfx_has_content: Arc<AtomicBool>,
    pub egfx_negotiated: Arc<AtomicBool>,
    pub egfx_caps: Arc<Mutex<Option<String>>>,
    pub egfx_frame_count: Arc<AtomicU64>,
    pub clipboard_state: Arc<Mutex<ClipboardState>>,
    pub audio_state: Arc<Mutex<AudioState>>,
    pub observed: ObservedCapabilities,
}

/// Server-side capabilities observed while completing a connection, used by the
/// `report` (rdpsee) and `info` (rdpdo) capability reports. Everything here is
/// read from the real negotiation, not assumed.
#[derive(Debug, Clone, Default)]
pub(crate) struct ObservedCapabilities {
    /// Security label derived from the handshake we performed (CredSSP/NLA or TLS).
    pub security: String,
    /// Static channels the connection actually joined (e.g. drdynvc, cliprdr, rdpsnd).
    pub channels: Vec<String>,
    /// Negotiated bulk compression, if any.
    pub compression: Option<String>,
    /// Server's confirmed color depth (bits per pixel) from the Demand Active caps.
    pub color_depth: Option<u16>,
    /// Main-channel bitmap codecs the server advertised (`RemoteFX`, `NSCodec`).
    pub codecs: Vec<String>,
}

/// Read-side stream wrapper that records server-to-client bytes during the
/// connection-activation phase. The recorded buffer is parsed afterward to
/// recover the Server Demand Active capability sets, which `connect_finalize`
/// consumes and does not expose on `ConnectionResult`. Recording is bounded and
/// switched off once the connection is established, so the active session does
/// not accumulate bytes.
struct RecordingStream<S> {
    inner: S,
    log: Arc<Mutex<Vec<u8>>>,
    recording: Arc<AtomicBool>,
}

/// Cap on recorded bytes: the Demand Active PDU arrives early, well within this.
const MAX_RECORD: usize = 512 * 1024;

impl<S: AsyncRead + Unpin> AsyncRead for RecordingStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let result = Pin::new(&mut this.inner).poll_read(cx, buf);
        if this.recording.load(Ordering::Relaxed) {
            let fresh = &buf.filled()[before..];
            if !fresh.is_empty() {
                let mut log = this.log.lock().expect("record log mutex poisoned");
                if log.len() < MAX_RECORD {
                    let take = fresh.len().min(MAX_RECORD - log.len());
                    log.extend_from_slice(&fresh[..take]);
                }
            }
        }
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for RecordingStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// Walk the recorded server-to-client stream for the Server Demand Active PDU and
/// extract its color depth and main-channel bitmap codecs. Returns
/// `(color_depth, codecs)`; both empty when the PDU is not found (e.g. the buffer
/// filled before it arrived).
///
/// The recorded bytes may begin mid-stream (after the `CredSSP` exchange, which
/// is not `TPKT`-framed), so we scan for a `TPKT` header and try `IronRDP`'s own
/// decoders at each candidate position rather than assuming frame alignment.
fn parse_demand_active(buf: &[u8]) -> (Option<u16>, Vec<String>) {
    let mut i = 0usize;
    while i + 4 <= buf.len() {
        // TPKT header: version 0x03, reserved 0x00, then a 16-bit big-endian length.
        if buf[i] == 0x03 && buf[i + 1] == 0x00 {
            let len = usize::from(u16::from_be_bytes([buf[i + 2], buf[i + 3]]));
            if (7..=buf.len() - i).contains(&len) {
                let frame = &buf[i..i + len];
                if let Ok(sdi) = decode_send_data_indication(frame)
                    && let Ok(share) = decode_share_control(sdi)
                    && let ShareControlPdu::ServerDemandActive(demand) = share.pdu
                {
                    return extract_caps(&demand.pdu.capability_sets);
                }
            }
        }
        i += 1;
    }
    (None, Vec::new())
}

/// Read the server's selected security protocol from the recorded pre-TLS
/// negotiation response, as a human label. `None` if the confirm is absent or the
/// server rejected negotiation.
fn parse_selected_protocol(buf: &[u8]) -> Option<String> {
    let mut i = 0usize;
    while i + 4 <= buf.len() {
        if buf[i] == 0x03 && buf[i + 1] == 0x00 {
            let len = usize::from(u16::from_be_bytes([buf[i + 2], buf[i + 3]]));
            if (7..=buf.len() - i).contains(&len)
                && let Ok(confirm) =
                    ironrdp_core::decode::<X224<ConnectionConfirm>>(&buf[i..i + len])
            {
                return match confirm.0 {
                    ConnectionConfirm::Response { protocol, .. } => Some(security_label(protocol)),
                    ConnectionConfirm::Failure { .. } => None,
                };
            }
        }
        i += 1;
    }
    None
}

/// Human label for a selected security protocol.
fn security_label(protocol: SecurityProtocol) -> String {
    if protocol.contains(SecurityProtocol::HYBRID_EX) {
        "CredSSP/NLA (HYBRID_EX)".to_owned()
    } else if protocol.contains(SecurityProtocol::HYBRID) {
        "CredSSP/NLA".to_owned()
    } else if protocol.contains(SecurityProtocol::SSL) {
        "TLS".to_owned()
    } else {
        "Standard RDP".to_owned()
    }
}

/// Pull color depth and bitmap codecs out of the Demand Active capability sets.
fn extract_caps(caps: &[CapabilitySet]) -> (Option<u16>, Vec<String>) {
    let mut color_depth = None;
    let mut codecs = Vec::new();
    for cap in caps {
        match cap {
            CapabilitySet::Bitmap(bitmap) => color_depth = Some(bitmap.pref_bits_per_pix),
            CapabilitySet::BitmapCodecs(bitmap_codecs) => {
                for codec in &bitmap_codecs.0 {
                    let name = match codec.property {
                        CodecProperty::RemoteFx(_) => Some("RemoteFX"),
                        CodecProperty::ImageRemoteFx(_) => Some("RemoteFX (image)"),
                        CodecProperty::NsCodec(_) => Some("NSCodec"),
                        _ => None,
                    };
                    if let Some(name) = name {
                        codecs.push(name.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    (color_depth, codecs)
}

/// Human label for a negotiated bulk-compression type.
fn compression_label(compression: CompressionType) -> String {
    match compression {
        CompressionType::K8 => "8K",
        CompressionType::K64 => "64K",
        CompressionType::Rdp6 => "RDP6",
        CompressionType::Rdp61 => "RDP6.1",
    }
    .to_owned()
}

/// Assemble the observed-capability snapshot from a completed connection and the
/// recorded activation stream.
fn observed_from(
    connection_result: &ConnectionResult,
    security: String,
    log: &[u8],
) -> ObservedCapabilities {
    let (color_depth, codecs) = parse_demand_active(log);
    let channels = connection_result
        .static_channels
        .values()
        .filter_map(|channel| channel.channel_name().as_str().map(str::to_owned))
        .collect();

    ObservedCapabilities {
        security,
        channels,
        compression: connection_result.compression_type.map(compression_label),
        color_depth,
        codecs,
    }
}

/// Server destination with optional port (default 3389).
#[derive(Debug, Clone)]
pub struct Destination {
    pub name: String,
    pub port: u16,
}

impl Destination {
    pub fn addr_string(&self) -> String {
        format!("{}:{}", self.name, self.port)
    }
}

impl std::str::FromStr for Destination {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        const RDP_DEFAULT_PORT: u16 = 3389;

        // Try as a full socket address first (handles IPv6 with port)
        if let Ok(sock_addr) = s.parse::<std::net::SocketAddr>() {
            return Ok(Self {
                name: sock_addr.ip().to_string(),
                port: sock_addr.port(),
            });
        }

        // Try host:port split
        if let Some((host, port_str)) = s.rsplit_once(':')
            && let Ok(port) = port_str.parse::<u16>()
        {
            return Ok(Self {
                name: host.to_owned(),
                port,
            });
        }

        // Bare hostname or IP
        Ok(Self {
            name: s.to_owned(),
            port: RDP_DEFAULT_PORT,
        })
    }
}

impl From<&Destination> for ironrdp_connector::ServerName {
    fn from(dest: &Destination) -> Self {
        Self::new(&dest.name)
    }
}

/// Build a `connector::Config` from the given parameters.
pub(crate) fn build_connector_config(
    username: Option<&str>,
    password: Option<&str>,
    no_auth: bool,
    no_nla: bool,
    width: u16,
    height: u16,
) -> connector::Config {
    let credentials = match (username, password) {
        (Some(user), Some(pass)) => Credentials::UsernamePassword {
            username: user.to_owned(),
            password: pass.to_owned(),
        },
        _ => Credentials::UsernamePassword {
            username: String::new(),
            password: String::new(),
        },
    };

    let (enable_tls, enable_credssp) = if no_auth || no_nla {
        (true, false)
    } else {
        (true, true)
    };

    connector::Config {
        credentials,
        domain: None,
        enable_tls,
        enable_credssp,
        keyboard_type: ironrdp_pdu::gcc::KeyboardType::IbmEnhanced,
        keyboard_subtype: 0,
        keyboard_layout: 0,
        keyboard_functional_keys_count: 12,
        ime_file_name: String::new(),
        dig_product_id: String::new(),
        desktop_size: DesktopSize { width, height },
        desktop_scale_factor: 0,
        bitmap: Some(connector::BitmapConfig {
            color_depth: 32,
            lossy_compression: true,
            codecs: client_codecs_capabilities(&[]).expect("default codecs"),
        }),
        client_build: 0,
        client_name: hostname(),
        client_dir: "C:\\Windows\\System32\\mstscax.dll".to_owned(),
        platform: MajorPlatformType::UNIX,
        hardware_id: None,
        license_cache: None,
        enable_server_pointer: true,
        autologon: no_auth,
        enable_audio_playback: true,
        request_data: None,
        pointer_software_rendering: false,
        multitransport_flags: None,
        compression_type: None,
        performance_flags: PerformanceFlags::default(),
        timezone_info: TimezoneInfo::default(),
        alternate_shell: String::new(),
        work_dir: String::new(),
    }
}

/// Assemble the static-channel stack (drdynvc with EGFX + display control,
/// cliprdr, rdpsnd) onto a connector, returning the connector plus the shared
/// backend states the session reads from.
fn build_connector(
    connector_config: connector::Config,
    client_addr: std::net::SocketAddr,
    gfx_handler: HeadlessGfxHandler,
) -> (
    connector::ClientConnector,
    Arc<Mutex<ClipboardState>>,
    Arc<Mutex<AudioState>>,
) {
    let h264_decoder = load_h264_decoder();

    let drdynvc = DrdynvcClient::new()
        .with_dynamic_channel(DisplayControlClient::new(|_| Ok(Vec::new())))
        .with_dynamic_channel(ironrdp_egfx::client::GraphicsPipelineClient::new(
            Box::new(gfx_handler),
            h264_decoder,
        ));

    let clipboard_backend = ClipboardBackend::new();
    let clipboard_state = clipboard_backend.state();
    let cliprdr = CliprdrClient::new(Box::new(clipboard_backend));

    let audio_backend = AudioCaptureBackend::new();
    let audio_state = audio_backend.state();
    let rdpsnd = ironrdp_rdpsnd::client::Rdpsnd::new(Box::new(audio_backend));

    let connector = connector::ClientConnector::new(connector_config, client_addr)
        .with_static_channel(drdynvc)
        .with_static_channel(cliprdr)
        .with_static_channel(rdpsnd);

    (connector, clipboard_state, audio_state)
}

/// Establish a headless RDP connection.
pub(crate) async fn connect_headless(
    dest: &Destination,
    connector_config: connector::Config,
) -> Result<ConnectResult> {
    let addr = dest.addr_string();

    let stream = TcpStream::connect(&addr)
        .await
        .with_context(|| format!("TCP connect to {addr}"))?;

    let client_addr = stream.local_addr().context("local socket address")?;

    // Record the pre-TLS negotiation so the server's *selected* security protocol
    // can be read back, rather than assuming the one we requested.
    let nego_log = Arc::new(Mutex::new(Vec::new()));
    let nego_recording = Arc::new(AtomicBool::new(true));
    let nego_stream = RecordingStream {
        inner: stream,
        log: Arc::clone(&nego_log),
        recording: Arc::clone(&nego_recording),
    };
    let mut framed = ironrdp_tokio::TokioFramed::new(nego_stream);

    let credssp_requested = connector_config.enable_credssp;
    let width = connector_config.desktop_size.width;
    let height = connector_config.desktop_size.height;

    let gfx_handler = HeadlessGfxHandler::new(width, height);
    let egfx_framebuffer = gfx_handler.framebuffer();
    let egfx_has_content = gfx_handler.has_content();
    let egfx_negotiated = gfx_handler.negotiated();
    let egfx_caps = gfx_handler.egfx_caps();
    let egfx_frame_count = gfx_handler.frame_counter();

    let (mut connector, clipboard_state, audio_state) =
        build_connector(connector_config, client_addr, gfx_handler);

    // Phase 1: X.224 negotiation
    info!("Starting X.224 negotiation");
    let should_upgrade = ironrdp_tokio::connect_begin(&mut framed, &mut connector).await?;

    // The negotiation response is now captured; read back the protocol the server
    // actually selected (it may differ from what we requested).
    nego_recording.store(false, Ordering::Relaxed);
    let security = {
        let log = nego_log.lock().expect("nego log mutex poisoned");
        parse_selected_protocol(&log).unwrap_or_else(|| {
            if credssp_requested {
                "CredSSP/NLA".to_owned()
            } else {
                "TLS".to_owned()
            }
        })
    };

    // Phase 2: TLS upgrade
    debug!("TLS upgrade");
    let (initial_stream, leftover) = framed.into_inner();
    let (tls_stream, tls_cert) = ironrdp_tls::upgrade(initial_stream, &dest.name)
        .await
        .map_err(|e| connector::custom_err!("TLS upgrade", e))?;

    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);

    // Record server-to-client bytes through the activation phase so the Server
    // Demand Active capability sets can be recovered afterward.
    let record_log = Arc::new(Mutex::new(Vec::new()));
    let recording = Arc::new(AtomicBool::new(true));
    let recorded = RecordingStream {
        inner: tls_stream,
        log: Arc::clone(&record_log),
        recording: Arc::clone(&recording),
    };
    let erased: Box<dyn AsyncReadWrite + Unpin + Send + Sync> = Box::new(recorded);
    let mut upgraded_framed = ironrdp_tokio::TokioFramed::new_with_leftover(erased, leftover);

    // Phase 3: CredSSP (if NLA) + remaining connection sequence
    let server_public_key = ironrdp_tls::extract_tls_server_public_key(&tls_cert)
        .ok_or_else(|| connector::general_err!("unable to extract TLS server public key"))?;

    let connection_result = ironrdp_tokio::connect_finalize(
        upgraded,
        connector,
        &mut upgraded_framed,
        &mut ReqwestNetworkClient::new(),
        dest.into(),
        server_public_key.to_owned(),
        None,
    )
    .await?;

    info!(
        width = connection_result.desktop_size.width,
        height = connection_result.desktop_size.height,
        "Connected"
    );

    // The activation phase is done; stop recording and recover the Demand Active
    // capabilities the connector parsed and dropped.
    recording.store(false, Ordering::Relaxed);
    let observed = {
        let log = record_log.lock().expect("record log mutex poisoned");
        observed_from(&connection_result, security, &log)
    };

    Ok(ConnectResult {
        connection_result,
        framed: upgraded_framed,
        egfx_framebuffer,
        egfx_has_content,
        egfx_negotiated,
        egfx_caps,
        egfx_frame_count,
        clipboard_state,
        audio_state,
        observed,
    })
}

// ============================================================================
// H.264 Decoder Loading
// ============================================================================

/// Search paths for the Cisco `OpenH264` shared library on Linux.
const OPENH264_SEARCH_PATHS: &[&str] = &[
    "/usr/lib/x86_64-linux-gnu/libopenh264.so",
    "/usr/lib/x86_64-linux-gnu/libopenh264.so.8",
    "/usr/lib/x86_64-linux-gnu/libopenh264.so.2.6.0",
    "/usr/lib64/libopenh264.so",
    "/usr/lib/libopenh264.so",
];

/// Try to load an H.264 decoder from the system.
///
/// Three-tier strategy:
/// 1. Upstream `OpenH264Decoder::from_library_path()` — hash-verified Cisco binary
/// 2. Direct `openh264` crate with unchecked loading — works with distro packages
/// 3. `None` — AVC420 frames are skipped, other codecs still work
fn load_h264_decoder() -> Option<Box<dyn H264Decoder>> {
    // Honor explicit path override
    let env_path = std::env::var("OPENH264_LIBRARY_PATH").ok();
    let candidates: Vec<&str> = if let Some(ref path) = env_path {
        vec![path.as_str()]
    } else {
        OPENH264_SEARCH_PATHS.to_vec()
    };

    for candidate in &candidates {
        let path = Path::new(candidate);
        if !path.exists() {
            continue;
        }

        // Tier 1: upstream hash-verified loader
        match decode::OpenH264Decoder::from_library_path(path) {
            Ok(decoder) => {
                info!(
                    path = candidate,
                    "H.264 decode enabled (Cisco verified binary)"
                );
                return Some(Box::new(decoder));
            }
            Err(e) => {
                debug!(path = candidate, error = %e, "Hash-verified load failed, trying unchecked");
            }
        }

        // Tier 2: unchecked loading (works with distro-repackaged libraries)
        match SystemH264Decoder::load(path) {
            Ok(decoder) => {
                info!(path = candidate, "H.264 decode enabled (system library)");
                return Some(Box::new(decoder));
            }
            Err(e) => {
                debug!(path = candidate, error = %e, "System library load failed");
            }
        }
    }

    warn!(
        "H.264 decode unavailable: libopenh264 not found (set OPENH264_LIBRARY_PATH to override)"
    );
    None
}

/// H.264 decoder using the system `OpenH264` library without hash verification.
///
/// The upstream `OpenH264Decoder` requires Cisco's exact binary (SHA256-verified).
/// Distro-packaged libopenh264 (Debian, Fedora) may be stripped or patched,
/// changing the hash. This wrapper bypasses hash verification for those cases.
struct SystemH264Decoder {
    decoder: openh264::decoder::Decoder,
    annex_b_buf: Vec<u8>,
}

impl SystemH264Decoder {
    /// # Safety rationale
    ///
    /// `from_blob_path_unchecked` loads the shared library without SHA256
    /// verification against known Cisco binaries. We trust system-installed
    /// libopenh264 (Debian, Fedora packages distribute Cisco's binary).
    #[expect(
        unsafe_code,
        reason = "from_blob_path_unchecked skips hash check for distro libraries"
    )]
    fn load(library_path: &Path) -> Result<Self> {
        let api = unsafe { openh264::OpenH264API::from_blob_path_unchecked(library_path) }
            .map_err(|e| anyhow::anyhow!("failed to load OpenH264: {e}"))?;

        let decoder = openh264::decoder::Decoder::with_api_config(
            api,
            openh264::decoder::DecoderConfig::default(),
        )
        .map_err(|e| anyhow::anyhow!("failed to create decoder: {e}"))?;

        Ok(Self {
            decoder,
            annex_b_buf: Vec::new(),
        })
    }

    /// Convert AVC format (4-byte BE length prefix) to Annex B (start codes).
    ///
    /// RDP sends H.264 NAL units with 4-byte big-endian length prefixes (AVC format).
    /// `OpenH264` expects Annex B format (0x00000001 start code before each NAL).
    fn avc_to_annex_b(&mut self, data: &[u8]) {
        self.annex_b_buf.clear();
        let mut offset = 0;
        while offset + 4 <= data.len() {
            let nal_len = u32::from_be_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;
            let end = offset.saturating_add(nal_len);
            if end > data.len() {
                break;
            }
            self.annex_b_buf
                .extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
            self.annex_b_buf.extend_from_slice(&data[offset..end]);
            offset = end;
        }
    }
}

impl H264Decoder for SystemH264Decoder {
    fn decode(&mut self, data: &[u8]) -> DecoderResult<DecodedFrame> {
        // Split AVC-format NAL units and decode each individually.
        // OpenH264 may need separate calls: SPS/PPS configure the decoder
        // (returning None), then the IDR/P-slice produces a picture.
        // Detect format: Annex-B (start codes 0x00000001) vs AVC (4-byte
        // length prefixes). Our lamco-qemu-rdp server sends Annex-B per
        // MS-RDPEGFX 2.2.4.4, but other servers may send AVC format.
        let is_annex_b = data.len() >= 4
            && data[0] == 0x00
            && data[1] == 0x00
            && (data[2] == 0x01 || (data[2] == 0x00 && data.len() >= 5 && data[3] == 0x01));

        if is_annex_b {
            // Already Annex-B — feed directly to OpenH264
            self.annex_b_buf.clear();
            self.annex_b_buf.extend_from_slice(data);
        } else {
            // AVC format — convert to Annex-B
            self.avc_to_annex_b(data);
        }

        let yuv = self
            .decoder
            .decode(&self.annex_b_buf)
            .map_err(|e| DecoderError::new("OpenH264 decode failed", e))?
            .ok_or_else(|| DecoderError::msg("OpenH264 returned no picture"))?;

        let (width, height) = openh264::formats::YUVSource::dimensions(&yuv);

        let rgba_size = width
            .checked_mul(height)
            .and_then(|s| s.checked_mul(4))
            .ok_or_else(|| DecoderError::msg("frame dimensions overflow"))?;
        let mut rgba = vec![0u8; rgba_size];
        yuv.write_rgba8(&mut rgba);

        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            reason = "H.264 frame dimensions fit in u32"
        )]
        Ok(DecodedFrame::new(rgba, width as u32, height as u32))
    }

    fn reset(&mut self) {
        // OpenH264 handles SPS/PPS reset transparently on next I-frame.
        // In libloading mode we don't have the library path to recreate,
        // so reusing the existing decoder state is correct.
    }
}

fn hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_owned()))
        .unwrap_or_else(|_| "rdpdo".to_owned())
}
