use std::{
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
    time::Instant,
};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// A single recorded event with a timestamp relative to session start.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct RecordedEvent {
    /// Milliseconds from session start.
    pub t: u64,
    /// Command name (type, key, click, etc.).
    pub cmd: String,
    /// Command arguments.
    pub args: Vec<String>,
    /// Optional command-specific options.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opts: Option<serde_json::Value>,
}

/// Records commands as JSON Lines with timestamps.
pub(crate) struct SessionRecorder {
    writer: BufWriter<File>,
    start: Instant,
}

impl SessionRecorder {
    pub(crate) fn new(path: &str, server: &str, width: u16, height: u16) -> Result<Self> {
        let file =
            File::create(path).map_err(|e| anyhow::anyhow!("creating recording '{path}': {e}"))?;
        let mut writer = BufWriter::new(file);

        // Header comments
        let now = chrono::Utc::now().to_rfc3339();
        writeln!(writer, "# Recorded by rdpdo {now}")?;
        writeln!(writer, "# Host: {server}, Desktop: {width}x{height}")?;

        Ok(Self {
            writer,
            start: Instant::now(),
        })
    }

    /// Record a command execution.
    pub(crate) fn record(&mut self, cmd: &str, args: &[&str]) -> Result<()> {
        let event = RecordedEvent {
            t: self.start.elapsed().as_millis() as u64,
            cmd: cmd.to_owned(),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            opts: None,
        };
        serde_json::to_writer(&mut self.writer, &event)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }

    /// Record a command with additional options.
    pub(crate) fn record_with_opts(
        &mut self,
        cmd: &str,
        args: &[&str],
        opts: serde_json::Value,
    ) -> Result<()> {
        let event = RecordedEvent {
            t: self.start.elapsed().as_millis() as u64,
            cmd: cmd.to_owned(),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            opts: Some(opts),
        };
        serde_json::to_writer(&mut self.writer, &event)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

/// A parsed recording ready for playback.
pub(crate) struct Recording {
    pub events: Vec<RecordedEvent>,
}

impl Recording {
    /// Parse a .rdpdo JSON Lines recording file.
    pub(crate) fn load(path: &str) -> Result<Self> {
        let file =
            File::open(path).map_err(|e| anyhow::anyhow!("opening recording '{path}': {e}"))?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for (line_num, line) in reader.lines().enumerate() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let event: RecordedEvent = serde_json::from_str(trimmed)
                .map_err(|e| anyhow::anyhow!("line {}: {e}", line_num + 1))?;
            events.push(event);
        }

        if events.is_empty() {
            bail!("recording '{path}' contains no events");
        }

        Ok(Self { events })
    }

    /// Convert recorded events into Command tokens for dispatch.
    /// Each event becomes a sequence of tokens like `["type", "hello"]`.
    pub(crate) fn to_command_tokens(&self) -> Vec<Vec<String>> {
        self.events
            .iter()
            .map(|ev| {
                let mut tokens = vec![ev.cmd.clone()];
                tokens.extend(ev.args.clone());
                tokens
            })
            .collect()
    }
}

/// Convert a recording file to a plain-text script (strip timing).
pub(crate) fn convert_to_script(recording_path: &str, script_path: &str) -> Result<()> {
    let recording = Recording::load(recording_path)?;
    let output_path = Path::new(script_path);
    let mut writer = BufWriter::new(File::create(output_path)?);

    writeln!(writer, "# Converted from {recording_path}")?;
    for event in &recording.events {
        let mut line = event.cmd.clone();
        for arg in &event.args {
            if arg.contains(' ') || arg.contains('"') {
                line.push_str(" \"");
                line.push_str(&arg.replace('"', "\\\""));
                line.push('"');
            } else {
                line.push(' ');
                line.push_str(arg);
            }
        }
        writeln!(writer, "{line}")?;
    }

    Ok(())
}
