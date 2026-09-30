use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
};

use ironrdp_core::impl_as_any;
use ironrdp_rdpsnd::{
    client::RdpsndClientHandler,
    pdu::{AudioFormat, AudioFormatFlags, PitchPdu, VolumePdu, WaveFormat},
};
use tracing::debug;

/// Captured audio state shared between the RDPSND backend and the session.
#[derive(Debug)]
pub(crate) struct AudioState {
    /// True once the RDPSND channel is negotiated and ready.
    pub ready: bool,
    /// Whether we're currently recording audio data.
    pub recording: bool,
    /// Accumulated PCM samples during recording.
    pub pcm_buffer: Vec<u8>,
    /// Format parameters for the captured audio, resolved from the observed
    /// data rate (see `wave()` -- there is no reliable way to resolve
    /// `wFormatNo` directly, see that method's doc comment).
    pub capture_format: Option<CaptureFormat>,
    /// `audio_timestamp` of the first Wave2 PDU seen, used as the baseline
    /// for the data-rate measurement that resolves `capture_format`.
    first_wave_audio_ts: Option<u32>,
    /// Bytes received since `first_wave_audio_ts`, for the same measurement.
    bytes_since_first_wave: usize,
    /// Set once the data-rate measurement has resolved `capture_format`, so
    /// `wave()` stops re-measuring (one confirmation is enough; the estimate
    /// only gets noisier by re-measuring from a moving baseline).
    format_resolved: bool,
}

/// WAV-compatible format metadata for captured audio.
#[derive(Debug, Clone)]
pub(crate) struct CaptureFormat {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub block_align: u16,
}

impl From<&AudioFormat> for CaptureFormat {
    fn from(fmt: &AudioFormat) -> Self {
        Self {
            channels: fmt.n_channels,
            sample_rate: fmt.n_samples_per_sec,
            bits_per_sample: fmt.bits_per_sample,
            block_align: fmt.n_block_align,
        }
    }
}

impl AudioState {
    pub(crate) fn new() -> Self {
        Self {
            ready: false,
            recording: false,
            pcm_buffer: Vec::new(),
            capture_format: None,
            first_wave_audio_ts: None,
            bytes_since_first_wave: 0,
            format_resolved: false,
        }
    }
}

/// Audio backend that captures PCM data from the RDPSND channel.
#[derive(Debug)]
pub(crate) struct AudioCaptureBackend {
    state: Arc<Mutex<AudioState>>,
}

impl_as_any!(AudioCaptureBackend);

impl AudioCaptureBackend {
    pub(crate) fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(AudioState::new())),
        }
    }

    pub(crate) fn state(&self) -> Arc<Mutex<AudioState>> {
        Arc::clone(&self.state)
    }
}

impl RdpsndClientHandler for AudioCaptureBackend {
    fn get_flags(&self) -> AudioFormatFlags {
        AudioFormatFlags::empty()
    }

    fn get_formats(&self) -> &[AudioFormat] {
        // We accept PCM in common configurations
        // The Rdpsnd processor intersects these with server formats
        &SUPPORTED_FORMATS
    }

    /// `format_no` is documented (MS-RDPEA 3.1.1.2, 2.2.2.3) as an index into
    /// the format list the *client* sent in the Client Audio Formats PDU --
    /// not the server's original offer. `ironrdp_rdpsnd::client::Rdpsnd`
    /// builds that client-sent list internally via `HashSet::intersection()`
    /// (unordered) and never exposes the resulting list or its order, so
    /// `format_no` cannot be resolved back to a concrete `AudioFormat` from
    /// out here; `Rdpsnd::get_format()` indexes the server's original offer
    /// instead, which is a different list and gives the wrong entry whenever
    /// more than one of our `SUPPORTED_FORMATS` overlaps the server's offer
    /// (the common case -- confirmed live: server's format 0 decodes as
    /// Opus/48kHz, but the negotiated wire format was PCM/44100Hz).
    ///
    /// Since PCM byte-rate (`n_avg_bytes_per_sec`) is unique across almost
    /// all of `SUPPORTED_FORMATS`, resolve the format instead by measuring
    /// the actual observed rate (bytes received per elapsed `audio_timestamp`
    /// millisecond) and matching it to the closest candidate. `format_no` is
    /// constant for the life of a session, so one measurement is enough.
    fn wave(&mut self, _format_no: usize, ts: u32, data: Cow<'_, [u8]>) {
        let mut state = self.state.lock().expect("audio lock");

        if !state.format_resolved {
            match state.first_wave_audio_ts {
                None => {
                    state.first_wave_audio_ts = Some(ts);
                    // Seed a provisional guess so the first wave's bytes are
                    // still captured under a reasonable label; corrected
                    // below once enough samples exist to measure a real rate.
                    state
                        .capture_format
                        .get_or_insert_with(|| (&SUPPORTED_FORMATS[0]).into());
                }
                Some(first_ts) => {
                    // A single ~20ms frame is too noisy to resolve to a
                    // specific candidate rate; wait for a wider window.
                    let elapsed_ms = ts.saturating_sub(first_ts);
                    if elapsed_ms >= 200 {
                        let observed_bytes_per_sec =
                            state.bytes_since_first_wave as f64 * 1000.0 / f64::from(elapsed_ms);
                        if let Some(best) = SUPPORTED_FORMATS.iter().min_by(|a, b| {
                            let da =
                                (f64::from(a.n_avg_bytes_per_sec) - observed_bytes_per_sec).abs();
                            let db =
                                (f64::from(b.n_avg_bytes_per_sec) - observed_bytes_per_sec).abs();
                            da.total_cmp(&db)
                        }) {
                            debug!(
                                observed_bytes_per_sec,
                                channels = best.n_channels,
                                sample_rate = best.n_samples_per_sec,
                                bits = best.bits_per_sample,
                                "Audio format resolved from observed data rate"
                            );
                            state.capture_format = Some(best.into());
                        }
                        state.format_resolved = true;
                    }
                }
            }
        }
        state.bytes_since_first_wave += data.len();

        if state.recording {
            state.pcm_buffer.extend_from_slice(&data);
        }
    }

    fn set_volume(&mut self, _volume: VolumePdu) {}

    fn set_pitch(&mut self, _pitch: PitchPdu) {}

    fn close(&mut self) {
        debug!("RDPSND channel closed");
    }
}

/// PCM formats we're willing to accept from the server.
/// We prefer 16-bit PCM at common sample rates.
static SUPPORTED_FORMATS: [AudioFormat; 6] = [
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: 2,
        n_samples_per_sec: 48000,
        n_avg_bytes_per_sec: 48000 * 2 * 2,
        n_block_align: 4,
        bits_per_sample: 16,
        data: None,
    },
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: 2,
        n_samples_per_sec: 44100,
        n_avg_bytes_per_sec: 44100 * 2 * 2,
        n_block_align: 4,
        bits_per_sample: 16,
        data: None,
    },
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: 1,
        n_samples_per_sec: 48000,
        n_avg_bytes_per_sec: 48000 * 2,
        n_block_align: 2,
        bits_per_sample: 16,
        data: None,
    },
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: 1,
        n_samples_per_sec: 44100,
        n_avg_bytes_per_sec: 44100 * 2,
        n_block_align: 2,
        bits_per_sample: 16,
        data: None,
    },
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: 2,
        n_samples_per_sec: 22050,
        n_avg_bytes_per_sec: 22050 * 2 * 2,
        n_block_align: 4,
        bits_per_sample: 16,
        data: None,
    },
    AudioFormat {
        format: WaveFormat::PCM,
        n_channels: 1,
        n_samples_per_sec: 22050,
        n_avg_bytes_per_sec: 22050 * 2,
        n_block_align: 2,
        bits_per_sample: 16,
        data: None,
    },
];

/// Write captured PCM data as a WAV file.
pub(crate) fn write_wav(
    path: &std::path::Path,
    format: &CaptureFormat,
    pcm_data: &[u8],
) -> anyhow::Result<()> {
    use std::io::Write;

    let data_len = pcm_data.len() as u32;
    let byte_rate =
        format.sample_rate * u32::from(format.channels) * u32::from(format.bits_per_sample / 8);

    let mut file = std::fs::File::create(path)?;

    // RIFF header
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_len).to_le_bytes())?;
    file.write_all(b"WAVE")?;

    // fmt chunk
    file.write_all(b"fmt ")?;
    file.write_all(&16_u32.to_le_bytes())?; // chunk size
    file.write_all(&1_u16.to_le_bytes())?; // PCM format
    file.write_all(&format.channels.to_le_bytes())?;
    file.write_all(&format.sample_rate.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&format.block_align.to_le_bytes())?;
    file.write_all(&format.bits_per_sample.to_le_bytes())?;

    // data chunk
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;
    file.write_all(pcm_data)?;

    Ok(())
}

/// Calculate RMS (root mean square) amplitude of 16-bit PCM audio.
/// Returns a value between 0.0 and 1.0.
pub(crate) fn rms_amplitude(pcm_data: &[u8]) -> f64 {
    if pcm_data.len() < 2 {
        return 0.0;
    }

    let sample_count = pcm_data.len() / 2;
    let mut sum_sq: f64 = 0.0;

    for chunk in pcm_data.as_chunks::<2>().0 {
        let sample = i16::from_le_bytes(*chunk);
        let normalized = f64::from(sample) / f64::from(i16::MAX);
        sum_sq += normalized * normalized;
    }

    (sum_sq / sample_count as f64).sqrt()
}

/// Compare two PCM audio buffers using cross-correlation.
/// Both must be 16-bit PCM. Returns a similarity score 0.0-1.0.
pub(crate) fn compare_audio(a: &[u8], b: &[u8]) -> f64 {
    let samples_a: Vec<f64> = a
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| f64::from(i16::from_le_bytes(*c)))
        .collect();
    let samples_b: Vec<f64> = b
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| f64::from(i16::from_le_bytes(*c)))
        .collect();

    if samples_a.is_empty() || samples_b.is_empty() {
        return 0.0;
    }

    // Pearson correlation on the shorter overlap
    let n = samples_a.len().min(samples_b.len());
    let a = &samples_a[..n];
    let b = &samples_b[..n];

    let mean_a: f64 = a.iter().sum::<f64>() / n as f64;
    let mean_b: f64 = b.iter().sum::<f64>() / n as f64;

    let mut cov = 0.0;
    let mut var_a = 0.0;
    let mut var_b = 0.0;

    for i in 0..n {
        let da = a[i] - mean_a;
        let db = b[i] - mean_b;
        cov += da * db;
        var_a += da * da;
        var_b += db * db;
    }

    let denom = (var_a * var_b).sqrt();
    if denom < f64::EPSILON {
        return 0.0;
    }

    (cov / denom).clamp(0.0, 1.0)
}
