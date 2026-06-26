use anyhow::{Result, bail};
use clap::Parser;

/// Visual diff output mode.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum DiffMode {
    /// Red overlay on changed pixels (default).
    #[default]
    Highlight,
    /// Before and after side by side.
    SideBySide,
    /// Intensity-mapped change visualization.
    Heatmap,
}

/// Multi-monitor action.
#[derive(Debug)]
pub(crate) enum MonitorAction {
    /// Print current monitor info as JSON.
    List,
    /// Set monitor layout: single primary with given dimensions.
    Set { width: u32, height: u32 },
}

/// RDP automation tool. Commands are chained on the command line:
///
///   rdpdo -s host type "hello" key enter pause 2 capture /tmp/out.png
#[derive(Parser, Debug)]
#[command(name = "rdpdo", version, after_help = crate::help::after_help_text())]
#[expect(clippy::struct_excessive_bools)]
pub(crate) struct Cli {
    /// Server address (host or host:port, default port 3389).
    /// Not required for offline commands like `diff`.
    #[arg(short = 's', long, default_value = "")]
    pub server: String,

    /// RDP username
    #[arg(short, long)]
    pub user: Option<String>,

    /// RDP password
    #[arg(short, long)]
    pub password: Option<String>,

    /// Disable NLA/CredSSP (use TLS only)
    #[arg(long)]
    pub no_nla: bool,

    /// No authentication (for servers with --no-auth)
    #[arg(long)]
    pub no_auth: bool,

    /// Overall timeout in seconds
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,

    /// Desktop width in pixels
    #[arg(long, default_value_t = 1920)]
    pub width: u16,

    /// Desktop height in pixels
    #[arg(long, default_value_t = 1080)]
    pub height: u16,

    /// Enable verbose tracing to stderr
    #[arg(long)]
    pub verbose: bool,

    /// Record all commands with timing to a .rdpdo file
    #[arg(long)]
    pub record: Option<String>,

    /// Apply calibration correction to click/move coordinates.
    /// Use a path to a calibration JSON file, or "auto" to search
    /// ~/.config/rdpdo/calibration/ by server+resolution.
    #[arg(long)]
    pub calibration: Option<String>,

    /// Emit machine-readable JSON output for matching, stability,
    /// pixel, and measurement commands.
    #[arg(long)]
    pub json: bool,

    /// Save a diagnostic screenshot on command failure.
    /// Provide a directory path where PNGs will be written.
    #[arg(long)]
    pub fail_capture: Option<String>,

    /// Write a `JUnit` XML test report after the command chain completes.
    #[arg(long)]
    pub junit: Option<String>,

    /// Remaining tokens are the command chain
    #[arg(trailing_var_arg = true)]
    pub commands: Vec<String>,
}

/// A single command in the chain.
#[derive(Debug)]
pub(crate) enum Command {
    /// Type ASCII text as scancode sequences.
    Type(String),
    /// Type text using unicode keyboard events.
    Utype(String),
    /// Send a key combo: "enter", "ctrl-c", "ctrl-alt-delete", "0x1C".
    Key { spec: String, hold_ms: Option<u64> },
    /// Press a key without releasing.
    Keydown(String),
    /// Release a previously pressed key.
    Keyup(String),
    /// Move mouse to position.
    Move(String),
    /// Click at position with optional button (default: left).
    Click(String, String),
    /// Double-click at position.
    Doubleclick(String),
    /// Drag from one position to another.
    Drag(String, String),
    /// Scroll direction (up/down) by N notches.
    Scroll(String, u32),
    /// Pause for N seconds (supports fractions).
    Pause(f64),
    /// Save a screenshot. Path "-" writes PNG to stdout. Optional region.
    Capture(String, Option<String>),
    /// Type a password without logging it. Source: `env:VAR`, `file:PATH`, or literal.
    TypePassword(String),
    /// Resize the desktop (`WIDTHxHEIGHT`).
    Resize(String),
    /// Set clipboard text on the server.
    SetClipboard(String),
    /// Read clipboard text from the server.
    GetClipboard,
    /// Wait until screen stops updating for the given stillness period.
    WaitStill {
        stillness_ms: u64,
        timeout_secs: u64,
    },
    /// Wait until screen changes (a new frame arrives).
    WaitChange { timeout_secs: u64 },
    /// Wait for full screen to match a reference image or needle set.
    Expect {
        reference: String,
        timeout_secs: u64,
        tolerance: f32,
        needles_dir: Option<String>,
        tag: Option<String>,
    },
    /// Search for a template image anywhere on screen and wait until found.
    Waitfor {
        template: String,
        timeout_secs: u64,
        tolerance: f32,
        needles_dir: Option<String>,
        tag: Option<String>,
    },
    /// Search for a template image, then click its center.
    Expectclick {
        template: String,
        timeout_secs: u64,
        tolerance: f32,
        needles_dir: Option<String>,
        tag: Option<String>,
    },
    /// Wait for a screen region to match a reference image.
    Rexpect {
        region: String,
        reference: String,
        timeout_secs: u64,
        tolerance: f32,
    },
    /// Press a key repeatedly until a template matches on screen.
    RepeatKey {
        key: String,
        template: String,
        timeout_secs: u64,
        tolerance: f32,
        interval_ms: u64,
        max_presses: Option<u32>,
        needles_dir: Option<String>,
        tag: Option<String>,
    },
    /// Print connection info as JSON.
    Info,
    /// Print performance metrics as JSON.
    Perf,
    /// Capture a specific region to a file.
    Rcapture { region: String, path: String },
    /// Execute commands from a .rdpdo-script file.
    Run(String),
    /// Replay a timed .rdpdo recording with optional speed multiplier.
    Play { path: String, speed: f64 },
    /// Convert between recording (.rdpdo) and script (.rdpdo-script) formats.
    Convert { input: String, output: String },
    /// Offline comparison of two images.
    Diff {
        image_a: String,
        image_b: String,
        threshold: Option<f32>,
        output: Option<String>,
        mode: DiffMode,
        /// Regions to exclude from comparison ("x,y,WxH").
        exclude: Vec<String>,
    },
    /// Accept a portal permission dialog for a compositor.
    AcceptPortal {
        compositor: String,
        profile: Option<String>,
        verify: bool,
    },
    /// Unlock a locked screen.
    Unlock {
        compositor: String,
        password: String,
        profile: Option<String>,
        verify: bool,
    },
    /// Log in with username and password.
    Login {
        username: String,
        password: String,
        profile_name: String,
        domain: Option<String>,
        custom_profile: Option<String>,
        verify: bool,
    },
    /// Run a provisioning script with longer default timeouts.
    BootSequence(String),
    /// Read pixel color at a position (returns R,G,B,A).
    Pixel(String),
    /// CRC32 checksum of a screen region's pixel data.
    Checksum(String),
    /// Wait until a region's checksum changes from its current value.
    WaitChecksumChange { region: String, timeout_secs: u64 },
    /// Assert a region matches a known checksum.
    AssertChecksum { region: String, expected: String },
    /// Run click calibration grid.
    Calibrate {
        grid: String,
        output: Option<String>,
        deploy: String,
    },
    /// Save current screen as a named baseline reference.
    BaselineUpdate { name: String, dir: Option<String> },
    /// List saved baselines.
    BaselineList { dir: Option<String> },
    /// Compare current screen against a saved baseline.
    BaselineCheck {
        name: String,
        tolerance: f32,
        dir: Option<String>,
    },
    /// Multi-monitor: list, add, remove, resize.
    Monitor(MonitorAction),
    /// Periodic screenshot capture.
    Timelapse {
        /// Output path template with {n} for sequence number.
        path_template: String,
        /// Interval between captures in milliseconds.
        interval_ms: u64,
        /// Maximum number of frames (0 = unlimited, use duration or until).
        count: u32,
        /// Maximum duration in seconds (0 = unlimited, use count).
        duration_secs: u64,
        /// Stop when screen matches this reference image.
        until: Option<String>,
        /// Match tolerance for --until (0.0-1.0).
        tolerance: f32,
    },
    /// Send a local file to the remote clipboard.
    ClipboardSendFile(String),
    /// Receive a file from the remote clipboard and save locally.
    ClipboardRecvFile(String),
    /// Capture audio from the RDPSND channel.
    AudioCapture { output: String, duration_secs: u64 },
    /// Assert that audio is currently playing (non-silence).
    AudioAssertPlaying { timeout_secs: u64 },
    /// Offline comparison of two WAV files.
    AudioVerify {
        captured: String,
        reference: String,
        tolerance: f32,
    },
    /// Find clusters of a color on screen.
    FindColor {
        color: String,
        region: Option<String>,
        tolerance: u8,
        min_area: u32,
    },
    /// Show help for all commands or a specific command.
    Help(Option<String>),
    /// Modifier: retry the next command up to N times on failure.
    Retry(u32),
    /// Modifier: make the next command non-fatal (log failure, continue chain).
    Soft,
    /// Move mouse cursor off-screen to avoid screenshot interference.
    MouseHide,
    /// Wait until pixel at position matches a color.
    WaitPixel {
        pos: String,
        color: String,
        tolerance: u8,
        timeout_secs: u64,
    },
    /// Assert pixel at position matches expected color.
    AssertPixel {
        pos: String,
        color: String,
        tolerance: u8,
    },
    /// Measure time until a visual match succeeds.
    Measure {
        template: String,
        timeout_secs: u64,
        tolerance: f32,
        label: Option<String>,
        needles_dir: Option<String>,
        tag: Option<String>,
    },
    /// Enter interactive session mode (REPL).
    Session,
    /// Print compact session status line.
    Status,
    /// Run a command on the remote host via SSH.
    Exec {
        destination: String,
        command: String,
    },
    /// Live frame rate monitoring for N seconds.
    Watch { duration_secs: u64 },
}

/// Parse the trailing command tokens into a sequence of Commands.
#[expect(clippy::too_many_lines)]
pub(crate) fn parse_commands(tokens: &[String]) -> Result<Vec<Command>> {
    let mut commands = Vec::new();
    let mut i = 0;

    while i < tokens.len() {
        let cmd = tokens[i].as_str();
        match cmd {
            "type" => {
                let text = require_arg(tokens, &mut i, "type")?;
                commands.push(Command::Type(text));
            }
            "utype" => {
                let text = require_arg(tokens, &mut i, "utype")?;
                commands.push(Command::Utype(text));
            }
            "key" => {
                let spec = require_arg(tokens, &mut i, "key")?;
                // Check for optional --hold-ms flag
                let hold_ms = if i + 1 < tokens.len() && tokens[i + 1] == "--hold-ms" {
                    i += 1;
                    if i + 1 < tokens.len() {
                        i += 1;
                        Some(
                            tokens[i]
                                .parse::<u64>()
                                .map_err(|e| anyhow::anyhow!("invalid --hold-ms value: {e}"))?,
                        )
                    } else {
                        bail!("--hold-ms requires a value in milliseconds");
                    }
                } else {
                    None
                };
                commands.push(Command::Key { spec, hold_ms });
            }
            "keydown" => {
                let spec = require_arg(tokens, &mut i, "keydown")?;
                commands.push(Command::Keydown(spec));
            }
            "keyup" => {
                let spec = require_arg(tokens, &mut i, "keyup")?;
                commands.push(Command::Keyup(spec));
            }
            "move" => {
                let pos = require_arg(tokens, &mut i, "move")?;
                commands.push(Command::Move(pos));
            }
            "click" => {
                let pos = require_arg(tokens, &mut i, "click")?;
                // Optional button name follows if it's one of left/right/middle
                let button = peek_button(tokens, &mut i);
                commands.push(Command::Click(pos, button));
            }
            "doubleclick" => {
                let pos = require_arg(tokens, &mut i, "doubleclick")?;
                commands.push(Command::Doubleclick(pos));
            }
            "drag" => {
                let from = require_arg(tokens, &mut i, "drag (from)")?;
                let to = require_arg(tokens, &mut i, "drag (to)")?;
                commands.push(Command::Drag(from, to));
            }
            "scroll" => {
                let direction = require_arg(tokens, &mut i, "scroll")?;
                let notches_str = require_arg(tokens, &mut i, "scroll (notches)")?;
                let notches: u32 = notches_str
                    .parse()
                    .map_err(|e| anyhow::anyhow!("invalid scroll notches '{notches_str}': {e}"))?;
                commands.push(Command::Scroll(direction, notches));
            }
            "pause" | "sleep" | "wait" => {
                let secs_str = require_arg(tokens, &mut i, "pause")?;
                let secs: f64 = secs_str
                    .parse()
                    .map_err(|e| anyhow::anyhow!("invalid pause duration '{secs_str}': {e}"))?;
                commands.push(Command::Pause(secs));
            }
            "capture" | "screenshot" => {
                let path = require_arg(tokens, &mut i, "capture")?;
                // Optional region follows if next token contains a comma
                let region = peek_region(tokens, &mut i);
                commands.push(Command::Capture(path, region));
            }
            "type-password" => {
                let source = require_arg(tokens, &mut i, "type-password")?;
                commands.push(Command::TypePassword(source));
            }
            "resize" => {
                let dims = require_arg(tokens, &mut i, "resize")?;
                commands.push(Command::Resize(dims));
            }
            "set-clipboard" => {
                let text = require_arg(tokens, &mut i, "set-clipboard")?;
                commands.push(Command::SetClipboard(text));
            }
            "get-clipboard" => {
                commands.push(Command::GetClipboard);
            }
            "audio-capture" => {
                let output = require_arg(tokens, &mut i, "audio-capture")?;
                let duration_secs = peek_number_u64(tokens, &mut i).unwrap_or(5);
                commands.push(Command::AudioCapture {
                    output,
                    duration_secs,
                });
            }
            "audio-assert-playing" => {
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(5);
                commands.push(Command::AudioAssertPlaying { timeout_secs });
            }
            "audio-verify" => {
                let captured = require_arg(tokens, &mut i, "audio-verify (captured)")?;
                let reference = require_arg(tokens, &mut i, "audio-verify (reference)")?;
                let tolerance = peek_number_f32(tokens, &mut i).unwrap_or(0.85);
                commands.push(Command::AudioVerify {
                    captured,
                    reference,
                    tolerance,
                });
            }
            "clipboard-send-file" | "send-file" => {
                let path = require_arg(tokens, &mut i, "clipboard-send-file")?;
                commands.push(Command::ClipboardSendFile(path));
            }
            "clipboard-recv-file" | "recv-file" => {
                let path = require_arg(tokens, &mut i, "clipboard-recv-file")?;
                commands.push(Command::ClipboardRecvFile(path));
            }
            "wait-still" => {
                let stillness_ms = peek_number_u64(tokens, &mut i).unwrap_or(500);
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(10);
                commands.push(Command::WaitStill {
                    stillness_ms,
                    timeout_secs,
                });
            }
            "wait-change" => {
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(30);
                commands.push(Command::WaitChange { timeout_secs });
            }
            "expect" => {
                let reference = require_arg(tokens, &mut i, "expect")?;
                let tolerance =
                    normalize_tolerance(peek_number_f32(tokens, &mut i).unwrap_or(0.95));
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(30);
                let (needles_dir, tag) = peek_needle_opts(tokens, &mut i);
                commands.push(Command::Expect {
                    reference,
                    timeout_secs,
                    tolerance,
                    needles_dir,
                    tag,
                });
            }
            "waitfor" => {
                let template = require_arg(tokens, &mut i, "waitfor")?;
                let tolerance =
                    normalize_tolerance(peek_number_f32(tokens, &mut i).unwrap_or(0.95));
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(30);
                let (needles_dir, tag) = peek_needle_opts(tokens, &mut i);
                commands.push(Command::Waitfor {
                    template,
                    timeout_secs,
                    tolerance,
                    needles_dir,
                    tag,
                });
            }
            "expectclick" => {
                let template = require_arg(tokens, &mut i, "expectclick")?;
                let tolerance =
                    normalize_tolerance(peek_number_f32(tokens, &mut i).unwrap_or(0.95));
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(30);
                let (needles_dir, tag) = peek_needle_opts(tokens, &mut i);
                commands.push(Command::Expectclick {
                    template,
                    timeout_secs,
                    tolerance,
                    needles_dir,
                    tag,
                });
            }
            "repeat-key" => {
                let key = require_arg(tokens, &mut i, "repeat-key (key)")?;
                let template = require_arg(tokens, &mut i, "repeat-key (template)")?;
                let tolerance =
                    normalize_tolerance(peek_number_f32(tokens, &mut i).unwrap_or(0.95));
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(30);
                let interval_ms = peek_number_u64(tokens, &mut i).unwrap_or(300);
                let max_presses = peek_number_u64(tokens, &mut i).map(|v| v as u32);
                let (needles_dir, tag) = peek_needle_opts(tokens, &mut i);
                commands.push(Command::RepeatKey {
                    key,
                    template,
                    timeout_secs,
                    tolerance,
                    interval_ms,
                    max_presses,
                    needles_dir,
                    tag,
                });
            }
            "rexpect" => {
                let region = require_arg(tokens, &mut i, "rexpect (region)")?;
                let reference = require_arg(tokens, &mut i, "rexpect (reference)")?;
                let tolerance =
                    normalize_tolerance(peek_number_f32(tokens, &mut i).unwrap_or(0.95));
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(30);
                commands.push(Command::Rexpect {
                    region,
                    reference,
                    timeout_secs,
                    tolerance,
                });
            }
            "info" => {
                commands.push(Command::Info);
            }
            "perf" => {
                commands.push(Command::Perf);
            }
            "rcapture" => {
                let region = require_arg(tokens, &mut i, "rcapture (region)")?;
                let path = require_arg(tokens, &mut i, "rcapture (path)")?;
                commands.push(Command::Rcapture { region, path });
            }
            "run" => {
                let script_path = require_arg(tokens, &mut i, "run")?;
                commands.push(Command::Run(script_path));
            }
            "play" => {
                let path = require_arg(tokens, &mut i, "play")?;
                let speed = peek_number_f32(tokens, &mut i).map_or(1.0, f64::from);
                commands.push(Command::Play { path, speed });
            }
            "convert" => {
                let input = require_arg(tokens, &mut i, "convert (input)")?;
                let output = require_arg(tokens, &mut i, "convert (output)")?;
                commands.push(Command::Convert { input, output });
            }
            "diff" => {
                let image_a = require_arg(tokens, &mut i, "diff (image A)")?;
                let image_b = require_arg(tokens, &mut i, "diff (image B)")?;
                let threshold = peek_number_f32(tokens, &mut i);
                let mut output = None;
                let mut mode = DiffMode::Highlight;
                let mut exclude = Vec::new();
                while i + 1 < tokens.len() {
                    let next = &tokens[i + 1];
                    if next == "--output" {
                        i += 1;
                        if i + 1 < tokens.len() {
                            i += 1;
                            output = Some(tokens[i].clone());
                        }
                    } else if next == "--mode" {
                        i += 1;
                        if i + 1 < tokens.len() {
                            i += 1;
                            mode = match tokens[i].as_str() {
                                "highlight" => DiffMode::Highlight,
                                "side-by-side" => DiffMode::SideBySide,
                                "heatmap" => DiffMode::Heatmap,
                                other => bail!(
                                    "unknown diff mode '{other}' (use highlight, side-by-side, or heatmap)"
                                ),
                            };
                        }
                    } else if next == "--exclude" {
                        i += 1;
                        if i + 1 < tokens.len() {
                            i += 1;
                            exclude.push(tokens[i].clone());
                        }
                    } else {
                        break;
                    }
                }
                commands.push(Command::Diff {
                    image_a,
                    image_b,
                    threshold,
                    output,
                    mode,
                    exclude,
                });
            }
            "accept-portal" => {
                let compositor = require_arg(tokens, &mut i, "accept-portal")?;
                let (profile, verify) = peek_provisioning_opts(tokens, &mut i);
                commands.push(Command::AcceptPortal {
                    compositor,
                    profile,
                    verify,
                });
            }
            "unlock" => {
                let compositor = require_arg(tokens, &mut i, "unlock")?;
                let password = require_arg(tokens, &mut i, "unlock (password)")?;
                let (profile, verify) = peek_provisioning_opts(tokens, &mut i);
                commands.push(Command::Unlock {
                    compositor,
                    password,
                    profile,
                    verify,
                });
            }
            "login" => {
                let username = require_arg(tokens, &mut i, "login (username)")?;
                let password = require_arg(tokens, &mut i, "login (password)")?;
                let (profile, verify) = peek_provisioning_opts(tokens, &mut i);
                // Profile name defaults to "default", domain extracted from --profile domain
                let profile_name = profile
                    .as_deref()
                    .and_then(|p| {
                        // If it doesn't look like a path, treat it as a profile name
                        if !p.contains('/') && !p.contains('.') {
                            Some(p.to_owned())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| "default".to_owned());
                let custom_profile = profile.as_deref().and_then(|p| {
                    if p.contains('/') || p.contains('.') {
                        Some(p.to_owned())
                    } else {
                        None
                    }
                });
                commands.push(Command::Login {
                    username,
                    password,
                    profile_name,
                    domain: None,
                    custom_profile,
                    verify,
                });
            }
            "boot-sequence" => {
                let script_path = require_arg(tokens, &mut i, "boot-sequence")?;
                commands.push(Command::BootSequence(script_path));
            }
            "pixel" => {
                let pos = require_arg(tokens, &mut i, "pixel")?;
                commands.push(Command::Pixel(pos));
            }
            "checksum" => {
                let region = require_arg(tokens, &mut i, "checksum")?;
                commands.push(Command::Checksum(region));
            }
            "wait-checksum-change" => {
                let region = require_arg(tokens, &mut i, "wait-checksum-change")?;
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(30);
                commands.push(Command::WaitChecksumChange {
                    region,
                    timeout_secs,
                });
            }
            "assert-checksum" => {
                let region = require_arg(tokens, &mut i, "assert-checksum (region)")?;
                let expected = require_arg(tokens, &mut i, "assert-checksum (checksum)")?;
                commands.push(Command::AssertChecksum { region, expected });
            }
            "calibrate" => {
                let mut grid = "4x4".to_owned();
                let mut output = None;
                let mut deploy = "clipboard".to_owned();
                // Consume optional flags
                while i + 1 < tokens.len() {
                    let next = &tokens[i + 1];
                    if next == "--grid" {
                        i += 1;
                        if i + 1 < tokens.len() {
                            i += 1;
                            grid.clone_from(&tokens[i]);
                        }
                    } else if next == "--output" {
                        i += 1;
                        if i + 1 < tokens.len() {
                            i += 1;
                            output = Some(tokens[i].clone());
                        }
                    } else if next == "--deploy" {
                        i += 1;
                        if i + 1 < tokens.len() {
                            i += 1;
                            deploy.clone_from(&tokens[i]);
                        }
                    } else if next == "--quick" {
                        i += 1;
                        "3x1".clone_into(&mut grid);
                    } else {
                        break;
                    }
                }
                commands.push(Command::Calibrate {
                    grid,
                    output,
                    deploy,
                });
            }
            "find-color" => {
                let color = require_arg(tokens, &mut i, "find-color")?;
                // Optional region, tolerance, min-area
                let region = peek_region(tokens, &mut i);
                let tolerance = peek_number_u64(tokens, &mut i).map_or(30, |v| v as u8);
                let min_area = peek_number_u64(tokens, &mut i).map_or(3, |v| v as u32);
                commands.push(Command::FindColor {
                    color,
                    region,
                    tolerance,
                    min_area,
                });
            }
            "baseline" => {
                let action = require_arg(tokens, &mut i, "baseline")?;
                match action.as_str() {
                    "update" | "save" => {
                        let name = require_arg(tokens, &mut i, "baseline update (name)")?;
                        let dir = peek_flag_value(tokens, &mut i, "--dir");
                        commands.push(Command::BaselineUpdate { name, dir });
                    }
                    "list" | "ls" => {
                        let dir = peek_flag_value(tokens, &mut i, "--dir");
                        commands.push(Command::BaselineList { dir });
                    }
                    "check" | "compare" => {
                        let name = require_arg(tokens, &mut i, "baseline check (name)")?;
                        let tolerance =
                            normalize_tolerance(peek_number_f32(tokens, &mut i).unwrap_or(0.95));
                        let dir = peek_flag_value(tokens, &mut i, "--dir");
                        commands.push(Command::BaselineCheck {
                            name,
                            tolerance,
                            dir,
                        });
                    }
                    other => bail!("unknown baseline action '{other}' (use: update, list, check)"),
                }
            }
            "monitor" => {
                let action = require_arg(tokens, &mut i, "monitor")?;
                match action.as_str() {
                    "list" | "ls" => {
                        commands.push(Command::Monitor(MonitorAction::List));
                    }
                    "set" => {
                        let dims = require_arg(tokens, &mut i, "monitor set (WIDTHxHEIGHT)")?;
                        let (w, h) = parse_resize(&dims)?;
                        commands.push(Command::Monitor(MonitorAction::Set {
                            width: w,
                            height: h,
                        }));
                    }
                    other => bail!("unknown monitor action '{other}' (use: list, set)"),
                }
            }
            "timelapse" => {
                let path_template = require_arg(tokens, &mut i, "timelapse")?;
                let mut interval_ms: u64 = 1000;
                let mut count: u32 = 0;
                let mut duration_secs: u64 = 0;
                let mut until: Option<String> = None;
                let mut tolerance: f32 = 0.95;

                // Parse optional flags
                while i + 1 < tokens.len() && tokens[i + 1].starts_with("--") {
                    i += 1;
                    match tokens[i].as_str() {
                        "--interval" => {
                            i += 1;
                            interval_ms = parse_duration_ms(&tokens[i])?;
                        }
                        "--count" => {
                            i += 1;
                            count = tokens[i]
                                .parse()
                                .map_err(|e| anyhow::anyhow!("invalid --count value: {e}"))?;
                        }
                        "--duration" => {
                            i += 1;
                            duration_secs = parse_duration_secs(&tokens[i])?;
                        }
                        "--until" => {
                            i += 1;
                            until = Some(tokens[i].clone());
                        }
                        "--tolerance" => {
                            i += 1;
                            tolerance =
                                normalize_tolerance(tokens[i].parse().map_err(|e| {
                                    anyhow::anyhow!("invalid --tolerance value: {e}")
                                })?);
                        }
                        other => bail!("unknown timelapse flag '{other}'"),
                    }
                }

                // Must have at least one termination condition
                if count == 0 && duration_secs == 0 && until.is_none() {
                    bail!("timelapse requires --count, --duration, or --until");
                }

                commands.push(Command::Timelapse {
                    path_template,
                    interval_ms,
                    count,
                    duration_secs,
                    until,
                    tolerance,
                });
            }
            "retry" => {
                let count_str = require_arg(tokens, &mut i, "retry")?;
                let count: u32 = count_str
                    .parse()
                    .map_err(|e| anyhow::anyhow!("invalid retry count '{count_str}': {e}"))?;
                if count == 0 {
                    bail!("retry count must be >= 1");
                }
                commands.push(Command::Retry(count));
            }
            "soft" => {
                commands.push(Command::Soft);
            }
            "help" => {
                let topic = if i + 1 < tokens.len() && !is_command_keyword(&tokens[i + 1]) {
                    i += 1;
                    Some(tokens[i].clone())
                } else if i + 1 < tokens.len() {
                    // Allow help for command keywords too: "help expect"
                    i += 1;
                    Some(tokens[i].clone())
                } else {
                    None
                };
                commands.push(Command::Help(topic));
            }
            "mouse-hide" => {
                commands.push(Command::MouseHide);
            }
            "wait-pixel" => {
                let pos = require_arg(tokens, &mut i, "wait-pixel (position)")?;
                let color = require_arg(tokens, &mut i, "wait-pixel (color)")?;
                let tolerance = peek_number_u64(tokens, &mut i).map_or(30, |v| v as u8);
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(30);
                commands.push(Command::WaitPixel {
                    pos,
                    color,
                    tolerance,
                    timeout_secs,
                });
            }
            "assert-pixel" => {
                let pos = require_arg(tokens, &mut i, "assert-pixel (position)")?;
                let color = require_arg(tokens, &mut i, "assert-pixel (color)")?;
                let tolerance = peek_number_u64(tokens, &mut i).map_or(30, |v| v as u8);
                commands.push(Command::AssertPixel {
                    pos,
                    color,
                    tolerance,
                });
            }
            "measure" => {
                let template = require_arg(tokens, &mut i, "measure")?;
                let tolerance =
                    normalize_tolerance(peek_number_f32(tokens, &mut i).unwrap_or(0.95));
                let timeout_secs = peek_number_u64(tokens, &mut i).unwrap_or(60);
                let (needles_dir, tag) = peek_needle_opts(tokens, &mut i);
                let label = peek_flag_value(tokens, &mut i, "--label");
                commands.push(Command::Measure {
                    template,
                    timeout_secs,
                    tolerance,
                    label,
                    needles_dir,
                    tag,
                });
            }
            "session" | "repl" | "interactive" => {
                commands.push(Command::Session);
            }
            "status" => {
                commands.push(Command::Status);
            }
            "exec" => {
                let destination = require_arg(tokens, &mut i, "exec (user@host)")?;
                let command = require_arg(tokens, &mut i, "exec (command)")?;
                commands.push(Command::Exec {
                    destination,
                    command,
                });
            }
            "watch" => {
                let duration_secs = peek_number_u64(tokens, &mut i).unwrap_or(10);
                commands.push(Command::Watch { duration_secs });
            }
            other => {
                bail!(
                    "unknown command '{other}' at position {i}. Run `rdpdo help` for a list of commands."
                );
            }
        }
        i += 1;
    }

    Ok(commands)
}

fn require_arg(tokens: &[String], i: &mut usize, cmd_name: &str) -> Result<String> {
    *i += 1;
    if *i >= tokens.len() {
        bail!("'{cmd_name}' requires an argument");
    }
    Ok(tokens[*i].clone())
}

/// If the next token is a known button name, consume it. Otherwise default to "left".
fn peek_button(tokens: &[String], i: &mut usize) -> String {
    if *i + 1 < tokens.len() {
        let next = tokens[*i + 1].as_str();
        if matches!(next, "left" | "right" | "middle") {
            *i += 1;
            return next.to_owned();
        }
    }
    "left".to_owned()
}

/// If the next token looks like a region spec (contains comma), consume it.
fn peek_region(tokens: &[String], i: &mut usize) -> Option<String> {
    if *i + 1 < tokens.len() {
        let next = &tokens[*i + 1];
        // A region spec has the form "x,y,WxH"; it won't be a command keyword
        if next.contains(',') && !is_command_keyword(next) {
            *i += 1;
            return Some(next.clone());
        }
    }
    None
}

fn is_command_keyword(s: &str) -> bool {
    matches!(
        s,
        "type"
            | "utype"
            | "key"
            | "keydown"
            | "keyup"
            | "move"
            | "click"
            | "doubleclick"
            | "drag"
            | "scroll"
            | "pause"
            | "sleep"
            | "wait"
            | "capture"
            | "screenshot"
            | "type-password"
            | "resize"
            | "set-clipboard"
            | "get-clipboard"
            | "clipboard-send-file"
            | "send-file"
            | "clipboard-recv-file"
            | "recv-file"
            | "audio-capture"
            | "audio-assert-playing"
            | "audio-verify"
            | "wait-still"
            | "wait-change"
            | "expect"
            | "waitfor"
            | "expectclick"
            | "repeat-key"
            | "rexpect"
            | "info"
            | "perf"
            | "rcapture"
            | "run"
            | "play"
            | "convert"
            | "diff"
            | "accept-portal"
            | "unlock"
            | "login"
            | "boot-sequence"
            | "pixel"
            | "checksum"
            | "wait-checksum-change"
            | "assert-checksum"
            | "calibrate"
            | "find-color"
            | "timelapse"
            | "baseline"
            | "monitor"
            | "help"
            | "retry"
            | "soft"
            | "mouse-hide"
            | "wait-pixel"
            | "assert-pixel"
            | "measure"
            | "session"
            | "repl"
            | "interactive"
            | "status"
            | "exec"
            | "watch"
    )
}

/// Consume the next token as u64 if it parses as a number and isn't a command keyword.
fn peek_number_u64(tokens: &[String], i: &mut usize) -> Option<u64> {
    if *i + 1 < tokens.len() {
        let next = &tokens[*i + 1];
        if !is_command_keyword(next)
            && let Ok(v) = next.parse::<u64>()
        {
            *i += 1;
            return Some(v);
        }
    }
    None
}

/// Consume the next token as f32 if it parses as a number and isn't a command keyword.
fn peek_number_f32(tokens: &[String], i: &mut usize) -> Option<f32> {
    if *i + 1 < tokens.len() {
        let next = &tokens[*i + 1];
        if !is_command_keyword(next)
            && let Ok(v) = next.parse::<f32>()
        {
            *i += 1;
            return Some(v);
        }
    }
    None
}

/// Consume a specific `--flag value` pair if present.
fn peek_flag_value(tokens: &[String], i: &mut usize, flag: &str) -> Option<String> {
    if *i + 2 < tokens.len() && tokens[*i + 1] == flag {
        *i += 2;
        Some(tokens[*i].clone())
    } else {
        None
    }
}

/// Consume optional `--needles <dir>` and `--tag <name>` flags from tokens.
fn peek_needle_opts(tokens: &[String], i: &mut usize) -> (Option<String>, Option<String>) {
    let mut needles_dir = None;
    let mut tag = None;

    while *i + 1 < tokens.len() {
        let next = &tokens[*i + 1];
        if next == "--needles" {
            *i += 1;
            if *i + 1 < tokens.len() {
                *i += 1;
                needles_dir = Some(tokens[*i].clone());
            }
        } else if next == "--tag" {
            *i += 1;
            if *i + 1 < tokens.len() {
                *i += 1;
                tag = Some(tokens[*i].clone());
            }
        } else {
            break;
        }
    }

    (needles_dir, tag)
}

/// Consume optional `--profile <path>` and `--verify` flags from tokens.
fn peek_provisioning_opts(tokens: &[String], i: &mut usize) -> (Option<String>, bool) {
    let mut profile = None;
    let mut verify = false;

    // Scan ahead for --profile and --verify without consuming command keywords
    while *i + 1 < tokens.len() {
        let next = &tokens[*i + 1];
        if next == "--profile" {
            *i += 1;
            if *i + 1 < tokens.len() {
                *i += 1;
                profile = Some(tokens[*i].clone());
            }
        } else if next == "--verify" {
            *i += 1;
            verify = true;
        } else {
            break;
        }
    }

    (profile, verify)
}

/// Accept tolerance as either 0.0-1.0 or 1-100 (divided by 100).
fn normalize_tolerance(t: f32) -> f32 {
    if t > 1.0 { t / 100.0 } else { t }
}

/// Resolve a password source: `env:VAR_NAME`, `file:/path`, or literal text.
pub(crate) fn resolve_password(source: &str) -> Result<String> {
    if let Some(var_name) = source.strip_prefix("env:") {
        std::env::var(var_name)
            .map_err(|_| anyhow::anyhow!("environment variable '{var_name}' not set"))
    } else if let Some(path) = source.strip_prefix("file:") {
        std::fs::read_to_string(path)
            .map(|s| s.trim_end().to_owned())
            .map_err(|e| anyhow::anyhow!("reading password file '{path}': {e}"))
    } else {
        Ok(source.to_owned())
    }
}

/// Parse a duration string like "500ms", "1s", "2.5s", "30" (seconds) into milliseconds.
fn parse_duration_ms(s: &str) -> Result<u64> {
    if let Some(ms_str) = s.strip_suffix("ms") {
        return ms_str
            .parse::<u64>()
            .map_err(|e| anyhow::anyhow!("invalid duration '{s}': {e}"));
    }
    let secs_str = s.strip_suffix('s').unwrap_or(s);
    let secs: f64 = secs_str
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid duration '{s}': {e}"))?;
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok((secs * 1000.0) as u64)
}

/// Parse a duration string like "30s", "60", "2.5s" into whole seconds.
fn parse_duration_secs(s: &str) -> Result<u64> {
    let secs_str = s.strip_suffix('s').unwrap_or(s);
    let secs: f64 = secs_str
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid duration '{s}': {e}"))?;
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Ok(secs.ceil() as u64)
}

/// Parse a resize spec like "1280x720" into (width, height).
pub(crate) fn parse_resize(spec: &str) -> Result<(u32, u32)> {
    let (w_str, h_str) = spec
        .split_once('x')
        .or_else(|| spec.split_once('X'))
        .ok_or_else(|| anyhow::anyhow!("resize must be WIDTHxHEIGHT (e.g. 1280x720)"))?;

    let w: u32 = w_str
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid width: {e}"))?;
    let h: u32 = h_str
        .trim()
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid height: {e}"))?;

    Ok((w, h))
}
