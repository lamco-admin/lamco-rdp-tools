//! rdpdo application: the authenticated, session-driving RDP automation tool.
//!
//! Owns the command chain (`cli::Command`), dispatch, and the rdpdo help
//! registry. Connection, session, capture, matching, and the other building
//! blocks live in sibling modules shared with `rdpsee`.

use std::{
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use clap::Parser;
use tracing::info;

use crate::{
    audio, calibrate, capture, cli,
    cli::{Cli, Command},
    connection, coords, help, matching, needle, pixel, provision, recorder, report, script,
    session::HeadlessSession,
};

/// Global JSON output mode flag, set once at startup from --json.
static JSON_MODE: AtomicBool = AtomicBool::new(false);

fn cli_json_mode() -> bool {
    JSON_MODE.load(Ordering::Relaxed)
}

/// Global fail-capture directory, set once at startup from --fail-capture.
static FAIL_CAPTURE_DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub async fn run_cli() {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    // Set global flags from CLI
    JSON_MODE.store(cli.json, Ordering::Relaxed);
    if let Some(ref dir) = cli.fail_capture {
        let _ = FAIL_CAPTURE_DIR.set(dir.clone());
    }

    let timeout = Duration::from_secs(cli.timeout);
    let result = tokio::time::timeout(timeout, run(&cli)).await;

    match result {
        Ok(Ok(())) => std::process::exit(0),
        Ok(Err(e)) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("error: timeout after {}s", cli.timeout);
            std::process::exit(1);
        }
    }
}

/// Counter for failure diagnostic screenshots.
static FAIL_COUNTER: AtomicU32 = AtomicU32::new(0);

fn next_fail_id() -> u32 {
    FAIL_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[expect(clippy::too_many_lines)]
async fn run(cli: &Cli) -> Result<()> {
    let commands = cli::parse_commands(&cli.commands)?;

    // Handle offline-only commands (diff, convert, help) without connecting
    if commands.iter().all(|c| {
        matches!(
            c,
            Command::Diff { .. }
                | Command::Convert { .. }
                | Command::AudioVerify { .. }
                | Command::Help(_)
        )
    }) {
        for cmd in &commands {
            dispatch_offline(cmd)?;
        }
        return Ok(());
    }

    // Connect — server is required for non-offline commands
    if cli.server.is_empty() {
        bail!("--server is required for commands that need an RDP connection");
    }
    let dest: connection::Destination = cli.server.parse()?;
    let connector_config = connection::build_connector_config(
        cli.user.as_deref(),
        cli.password.as_deref(),
        cli.no_auth,
        cli.no_nla,
        cli.width,
        cli.height,
    );

    info!(destination = %dest.addr_string(), "Connecting");

    let connect_result = connection::connect_headless(&dest, connector_config).await?;
    let mut session = HeadlessSession::from_connect_result(connect_result);
    session.set_server_addr(&cli.server);

    // Load calibration profile if requested
    if let Some(cal_path) = &cli.calibration {
        let profile = calibrate::load_profile(cal_path, &cli.server, cli.width, cli.height)?;
        info!(
            offset_x = format!("{:.1}", profile.correction.offset_x),
            offset_y = format!("{:.1}", profile.correction.offset_y),
            "Calibration loaded"
        );
        session.set_calibration_offset(profile.correction.offset_x, profile.correction.offset_y);
    }

    // Wait for initial frame before executing commands
    let got_frame = session.wait_for_frame(Duration::from_secs(2)).await?;
    if !got_frame {
        info!("No initial frame received within 2s, proceeding anyway");
    }

    // Set up recorder if --record was specified
    let mut maybe_recorder = cli
        .record
        .as_ref()
        .map(|path| recorder::SessionRecorder::new(path, &cli.server, cli.width, cli.height))
        .transpose()?;

    // JUnit report accumulator (if --junit specified)
    let mut junit = cli
        .junit
        .as_ref()
        .map(|_| report::JunitReport::new(&cli.server));

    // Dispatch command chain with retry/soft modifier support
    let mut chain_error = None;
    let mut soft_failures: Vec<String> = Vec::new();
    let mut pending_retry: u32 = 0;
    let mut pending_soft = false;

    for (cmd_idx, cmd) in commands.iter().enumerate() {
        if session.peer_disconnected() {
            eprintln!("warning: server disconnected, skipping remaining commands");
            break;
        }

        // Handle modifier and special commands
        match cmd {
            Command::Retry(n) => {
                pending_retry = *n;
                continue;
            }
            Command::Soft => {
                pending_soft = true;
                continue;
            }
            Command::Session => {
                pending_retry = 0;
                pending_soft = false;
                if let Err(e) = run_session_repl(&mut session, &dest, cli).await {
                    chain_error = Some(e);
                    break;
                }
                continue;
            }
            _ => {}
        }

        // Consume modifiers for this command
        let retry_count = std::mem::take(&mut pending_retry);
        let is_soft = std::mem::take(&mut pending_soft);

        if let Some(ref mut rec) = maybe_recorder {
            record_command(rec, cmd)?;
        }

        let cmd_start = Instant::now();
        let cmd_name = format_command_name(cmd, cmd_idx);

        // Execute with retry logic
        let mut last_err = None;
        let attempts = retry_count.max(1);
        for attempt in 1..=attempts {
            match dispatch_command(&mut session, cmd).await {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(e) => {
                    if attempt < attempts {
                        eprintln!("retry {attempt}/{attempts}: {e:#}");
                        // Brief pause between retries to let the screen settle
                        session.run_for(Duration::from_millis(500)).await?;
                    }
                    last_err = Some(e);
                }
            }
        }

        if let Some(e) = last_err {
            if let Some(dir) = FAIL_CAPTURE_DIR.get() {
                let fail_id = next_fail_id();
                let path = format!("{dir}/fail-{fail_id:03}.png");
                if let Err(save_err) = capture::save_capture(&session, &path, None) {
                    eprintln!("warning: failed to save failure screenshot: {save_err}");
                } else {
                    eprintln!("failure screenshot: {path}");
                }
            }

            if is_soft {
                let msg = format!("{cmd_name}: {e:#}");
                eprintln!("SOFT FAIL: {msg}");
                soft_failures.push(msg.clone());
                if let Some(ref mut j) = junit {
                    j.add(&cmd_name, cmd_start.elapsed(), Some(format!("{e:#}")));
                }
                // Continue the chain despite the failure
            } else {
                if let Some(ref mut j) = junit {
                    j.add(&cmd_name, cmd_start.elapsed(), Some(format!("{e:#}")));
                }
                chain_error = Some(e);
                break;
            }
        } else if let Some(ref mut j) = junit {
            j.add(&cmd_name, cmd_start.elapsed(), None);
        }
    }

    // Report soft failures at the end
    if !soft_failures.is_empty() {
        eprintln!("\n{} soft failure(s):", soft_failures.len());
        for f in &soft_failures {
            eprintln!("  - {f}");
        }
    }

    // Write JUnit XML if requested
    if let (Some(j), Some(path)) = (&junit, &cli.junit) {
        let xml = j.to_xml();
        std::fs::write(path, &xml)
            .map_err(|e| anyhow::anyhow!("writing JUnit report to '{path}': {e}"))?;
        eprintln!("junit report: {path}");
    }

    // Graceful disconnect
    let _ = session.shutdown().await;

    if let Some(e) = chain_error {
        return Err(e);
    }

    Ok(())
}

/// Short name for a command in the test report.
fn format_command_name(cmd: &Command, idx: usize) -> String {
    let base = match cmd {
        Command::Type(t) => format!("type {}", truncate(t, 30)),
        Command::Utype(t) => format!("utype {}", truncate(t, 30)),
        Command::Key { spec, .. } => format!("key {spec}"),
        Command::Keydown(k) => format!("keydown {k}"),
        Command::Keyup(k) => format!("keyup {k}"),
        Command::Move(p) => format!("move {p}"),
        Command::Click(p, b) => format!("click {p} {b}"),
        Command::Doubleclick(p) => format!("doubleclick {p}"),
        Command::Drag(f, t) => format!("drag {f} {t}"),
        Command::Scroll(d, n) => format!("scroll {d} {n}"),
        Command::Pause(s) => format!("pause {s}"),
        Command::Capture(p, _) => format!("capture {p}"),
        Command::TypePassword(_) => "type-password ***".to_owned(),
        Command::Resize(d) => format!("resize {d}"),
        Command::SetClipboard(_) => "set-clipboard".to_owned(),
        Command::GetClipboard => "get-clipboard".to_owned(),
        Command::WaitStill { .. } => "wait-still".to_owned(),
        Command::WaitChange { .. } => "wait-change".to_owned(),
        Command::Expect { reference, .. } => format!("expect {}", truncate(reference, 30)),
        Command::Waitfor { template, .. } => format!("waitfor {}", truncate(template, 30)),
        Command::Expectclick { template, .. } => format!("expectclick {}", truncate(template, 30)),
        Command::RepeatKey { key, .. } => format!("repeat-key {key}"),
        Command::Rexpect { region, .. } => format!("rexpect {region}"),
        Command::Info => "info".to_owned(),
        Command::Perf => "perf".to_owned(),
        Command::Rcapture { region, .. } => format!("rcapture {region}"),
        Command::Run(s) => format!("run {s}"),
        Command::Play { path, .. } => format!("play {path}"),
        Command::Convert { .. } => "convert".to_owned(),
        Command::Diff { .. } => "diff".to_owned(),
        Command::AcceptPortal { compositor, .. } => format!("accept-portal {compositor}"),
        Command::Unlock { compositor, .. } => format!("unlock {compositor}"),
        Command::Login { username, .. } => format!("login {username}"),
        Command::BootSequence(s) => format!("boot-sequence {s}"),
        Command::Pixel(p) => format!("pixel {p}"),
        Command::Checksum(r) => format!("checksum {r}"),
        Command::WaitChecksumChange { region, .. } => format!("wait-checksum-change {region}"),
        Command::AssertChecksum { region, expected } => {
            format!("assert-checksum {region} {expected}")
        }
        Command::ClipboardSendFile(p) => format!("clipboard-send-file {p}"),
        Command::ClipboardRecvFile(p) => format!("clipboard-recv-file {p}"),
        Command::AudioCapture { .. } => "audio-capture".to_owned(),
        Command::AudioAssertPlaying { .. } => "audio-assert-playing".to_owned(),
        Command::AudioVerify { .. } => "audio-verify".to_owned(),
        Command::FindColor { color, .. } => format!("find-color {color}"),
        Command::BaselineUpdate { name, .. } => format!("baseline update {name}"),
        Command::BaselineList { .. } => "baseline list".to_owned(),
        Command::BaselineCheck { name, .. } => format!("baseline check {name}"),
        Command::Monitor(_) => "monitor".to_owned(),
        Command::Calibrate { .. } => "calibrate".to_owned(),
        Command::Timelapse { .. } => "timelapse".to_owned(),
        Command::Help(_) => "help".to_owned(),
        Command::Retry(n) => format!("retry {n}"),
        Command::Soft => "soft".to_owned(),
        Command::MouseHide => "mouse-hide".to_owned(),
        Command::WaitPixel { pos, color, .. } => format!("wait-pixel {pos} {color}"),
        Command::AssertPixel { pos, color, .. } => format!("assert-pixel {pos} {color}"),
        Command::Measure { template, .. } => format!("measure {}", truncate(template, 30)),
        Command::Session => "session".to_owned(),
        Command::Status => "status".to_owned(),
        Command::Exec { destination, .. } => format!("exec {destination}"),
        Command::Watch { duration_secs } => format!("watch {duration_secs}s"),
    };
    format!("[{idx}] {base}")
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

fn dispatch_command<'a>(
    session: &'a mut HeadlessSession,
    cmd: &'a Command,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + 'a>> {
    Box::pin(dispatch_command_inner(session, cmd))
}

#[expect(clippy::too_many_lines)]
async fn dispatch_command_inner(session: &mut HeadlessSession, cmd: &Command) -> Result<()> {
    match cmd {
        Command::Type(text) => {
            session.send_text(text).await?;
            session.process_pending().await?;
        }
        Command::Utype(text) => {
            session.send_unicode_text(text).await?;
            session.process_pending().await?;
        }
        Command::Key { spec, hold_ms } => {
            if let Some(ms) = hold_ms {
                session.send_key_combo_held(spec, *ms).await?;
            } else {
                session.send_key_combo(spec).await?;
            }
            session.process_pending().await?;
        }
        Command::Keydown(spec) => {
            session.send_key_down(spec).await?;
        }
        Command::Keyup(spec) => {
            session.send_key_up(spec).await?;
        }
        Command::Move(pos) => {
            let (width, height) = session.image_dimensions();
            let (x, y) = coords::resolve_position(pos, width, height)?;
            let (x, y) = session.calibrate_position(x, y);
            session.mouse_move(x, y).await?;
        }
        Command::Click(pos, button) => {
            let (width, height) = session.image_dimensions();
            let (x, y) = coords::resolve_position(pos, width, height)?;
            let (x, y) = session.calibrate_position(x, y);
            session.mouse_move(x, y).await?;
            session.mouse_click(button).await?;
            session.process_pending().await?;
        }
        Command::Doubleclick(pos) => {
            let (width, height) = session.image_dimensions();
            let (x, y) = coords::resolve_position(pos, width, height)?;
            let (x, y) = session.calibrate_position(x, y);
            session.mouse_move(x, y).await?;
            session.send_double_click("left").await?;
            session.process_pending().await?;
        }
        Command::Drag(from, to) => {
            let (width, height) = session.image_dimensions();
            let (fx, fy) = coords::resolve_position(from, width, height)?;
            let (tx, ty) = coords::resolve_position(to, width, height)?;
            let from_pos = session.calibrate_position(fx, fy);
            let to_pos = session.calibrate_position(tx, ty);
            session.send_drag(from_pos, to_pos).await?;
            session.process_pending().await?;
        }
        Command::Scroll(direction, notches) => {
            let up = match direction.as_str() {
                "up" => true,
                "down" => false,
                other => anyhow::bail!("scroll direction must be 'up' or 'down', got '{other}'"),
            };
            session.send_scroll(up, *notches).await?;
            session.process_pending().await?;
        }
        Command::Pause(secs) => {
            info!(secs, "Pausing");
            let millis = (*secs * 1000.0) as u64;
            // Process incoming PDUs during the pause rather than just sleeping,
            // so the session stays responsive to server traffic.
            session.run_for(Duration::from_millis(millis)).await?;
        }
        Command::Capture(path, region) => {
            capture::save_capture(session, path, region.as_deref())?;
        }
        Command::TypePassword(source) => {
            let password = cli::resolve_password(source)?;
            session.send_password(&password).await?;
            session.process_pending().await?;
        }
        Command::Resize(dims) => {
            let (w, h) = cli::parse_resize(dims)?;
            let sent = session.send_resize(w, h).await?;
            if sent {
                // Wait for the server to send DeactivateAll and complete reactivation
                session.run_for(Duration::from_secs(2)).await?;
                let (new_w, new_h) = session.image_dimensions();
                info!(
                    requested_w = w,
                    requested_h = h,
                    actual_w = new_w,
                    actual_h = new_h,
                    "Resize complete"
                );
                println!("resize: {new_w}x{new_h} (requested {w}x{h})");
            } else {
                eprintln!("warning: DisplayControl DVC not ready, resize skipped");
            }
        }
        Command::SetClipboard(text) => {
            session.set_clipboard(text).await?;
        }
        Command::GetClipboard => {
            let text = session.get_clipboard(Duration::from_secs(5)).await?;
            match text {
                Some(content) => println!("{content}"),
                None => eprintln!("warning: no clipboard data received"),
            }
        }
        Command::ClipboardSendFile(path) => {
            let file_path = std::path::Path::new(path);
            if !file_path.exists() {
                bail!("file not found: {path}");
            }
            session.send_clipboard_file(file_path).await?;
        }
        Command::ClipboardRecvFile(path) => {
            let save_path = std::path::Path::new(path);
            let saved = session
                .recv_clipboard_file(save_path, Duration::from_secs(30))
                .await?;
            println!("{saved}");
        }
        Command::WaitStill {
            stillness_ms,
            timeout_secs,
        } => {
            info!(stillness_ms, timeout_secs, "Waiting for screen to settle");
            let start = Instant::now();
            let settled = session
                .wait_still(
                    Duration::from_millis(*stillness_ms),
                    Duration::from_secs(*timeout_secs),
                )
                .await?;
            if cli_json_mode() {
                println!(
                    "{{\"settled\":{settled},\"elapsed_ms\":{}}}",
                    start.elapsed().as_millis()
                );
            } else if settled {
                info!("Screen settled");
            } else {
                eprintln!("warning: screen did not settle within {timeout_secs}s");
            }
        }
        Command::WaitChange { timeout_secs } => {
            info!(timeout_secs, "Waiting for screen change");
            let start = Instant::now();
            let changed = session
                .wait_change(Duration::from_secs(*timeout_secs))
                .await?;
            if cli_json_mode() {
                println!(
                    "{{\"changed\":{changed},\"elapsed_ms\":{}}}",
                    start.elapsed().as_millis()
                );
            } else if changed {
                info!("Screen changed");
            } else {
                eprintln!("warning: no screen change within {timeout_secs}s");
            }
        }
        Command::Expect {
            reference,
            timeout_secs,
            tolerance,
            needles_dir,
            tag,
        } => {
            // Load needle set if --needles was provided
            let needles = if let Some(dir) = needles_dir {
                Some(needle::load_needle_dir(
                    std::path::Path::new(dir),
                    tag.as_deref(),
                )?)
            } else {
                None
            };

            let ref_image = if needles.is_none() {
                Some(
                    image::open(reference)
                        .map_err(|e| anyhow::anyhow!("loading reference '{reference}': {e}"))?
                        .to_rgba8(),
                )
            } else {
                None
            };

            let deadline = Instant::now() + Duration::from_secs(*timeout_secs);
            let mut best_score = 0.0_f32;
            let mut frame_num = 0_u32;

            info!(reference, tolerance, "Waiting for screen to match");

            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let screen = session.current_frame();
                    let fail_path = format!("/tmp/rdpdo-fail-{:03}.png", next_fail_id());
                    let _ = screen.save(&fail_path);
                    eprintln!(
                        "FAIL: best match {best_score:.4} after {frame_num} frames, saved {fail_path}"
                    );
                    bail!("expect timeout: best score {best_score:.4} < {tolerance}");
                }

                frame_num += 1;
                let screen = session.current_frame();

                // Try needle set first, then fall back to single reference
                let score = if let Some(ref ndls) = needles {
                    if let Some(result) = needle::match_needle_set(&screen, ndls) {
                        info!(
                            needle = result.needle_name,
                            score = result.score,
                            "Needle matched"
                        );
                        break;
                    }
                    0.0
                } else {
                    matching::compare_full(&screen, ref_image.as_ref().expect("ref_image set"))
                };

                if score > best_score {
                    best_score = score;
                }
                if score >= *tolerance {
                    if cli_json_mode() {
                        println!(
                            "{{\"matched\":true,\"score\":{score:.4},\"frames\":{frame_num}}}"
                        );
                    }
                    info!(score, frame_num, "expect matched");
                    break;
                }

                // Process PDUs (including EGFX) for 500ms before comparing again
                session.run_for(Duration::from_millis(500)).await?;
            }
        }
        Command::Waitfor {
            template,
            timeout_secs,
            tolerance,
            needles_dir,
            tag,
        } => {
            let needles = if let Some(dir) = needles_dir {
                Some(needle::load_needle_dir(
                    std::path::Path::new(dir),
                    tag.as_deref(),
                )?)
            } else {
                None
            };
            let tmpl_image = if needles.is_none() {
                Some(
                    image::open(template)
                        .map_err(|e| anyhow::anyhow!("loading template '{template}': {e}"))?
                        .to_rgba8(),
                )
            } else {
                None
            };
            let deadline = Instant::now() + Duration::from_secs(*timeout_secs);
            let mut best_score = 0.0_f32;
            let mut frame_num = 0_u32;

            info!(template, tolerance, "Searching for template on screen");

            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let screen = session.current_frame();
                    let fail_path = format!("/tmp/rdpdo-fail-{:03}.png", next_fail_id());
                    let _ = screen.save(&fail_path);
                    eprintln!(
                        "FAIL: best match {best_score:.4} after {frame_num} frames, saved {fail_path}"
                    );
                    bail!("waitfor timeout: best score {best_score:.4} < {tolerance}");
                }

                session.run_for(Duration::from_millis(500)).await?;
                frame_num += 1;
                let screen = session.current_frame();

                // Needle set mode: try all needles (area-based matching)
                if let Some(ref ndls) = needles {
                    if let Some(result) = needle::match_needle_set(&screen, ndls) {
                        info!(
                            needle = result.needle_name,
                            score = result.score,
                            "Needle matched"
                        );
                        break;
                    }
                    continue;
                }

                // Single template mode: sliding window search
                let result =
                    matching::find_template(&screen, tmpl_image.as_ref().expect("tmpl_image set"));
                if result.score > best_score {
                    best_score = result.score;
                }
                if result.score >= *tolerance {
                    if cli_json_mode() {
                        println!(
                            "{{\"matched\":true,\"score\":{:.4},\"x\":{},\"y\":{},\"frames\":{}}}",
                            result.score, result.location.0, result.location.1, frame_num
                        );
                    }
                    info!(
                        score = result.score,
                        x = result.location.0,
                        y = result.location.1,
                        "waitfor matched"
                    );
                    break;
                }
            }
        }
        Command::Expectclick {
            template,
            timeout_secs,
            tolerance,
            needles_dir,
            tag,
        } => {
            let needles = if let Some(dir) = needles_dir {
                Some(needle::load_needle_dir(
                    std::path::Path::new(dir),
                    tag.as_deref(),
                )?)
            } else {
                None
            };
            let tmpl_image = if needles.is_none() {
                Some(
                    image::open(template)
                        .map_err(|e| anyhow::anyhow!("loading template '{template}': {e}"))?
                        .to_rgba8(),
                )
            } else {
                None
            };
            let deadline = Instant::now() + Duration::from_secs(*timeout_secs);
            let mut best_score = 0.0_f32;
            let mut frame_num = 0_u32;

            info!(template, tolerance, "Searching for template to click");

            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let screen = session.current_frame();
                    let fail_path = format!("/tmp/rdpdo-fail-{:03}.png", next_fail_id());
                    let _ = screen.save(&fail_path);
                    eprintln!(
                        "FAIL: best match {best_score:.4} after {frame_num} frames, saved {fail_path}"
                    );
                    bail!("expectclick timeout: best score {best_score:.4} < {tolerance}");
                }

                session.run_for(Duration::from_millis(500)).await?;
                frame_num += 1;
                let screen = session.current_frame();

                // Needle set mode
                if let Some(ref ndls) = needles {
                    if let Some(result) = needle::match_needle_set(&screen, ndls) {
                        // Use needle's click_point or center of first match area
                        let (cx, cy) = result.click_point.unwrap_or_else(|| {
                            // Default: center of screen (not great but safe fallback)
                            let (w, h) = screen.dimensions();
                            (w / 2, h / 2)
                        });
                        info!(
                            needle = result.needle_name,
                            score = result.score,
                            x = cx,
                            y = cy,
                            "Needle matched, clicking"
                        );
                        session.mouse_move(cx as u16, cy as u16).await?;
                        session.mouse_click("left").await?;
                        session.process_pending().await?;
                        break;
                    }
                    continue;
                }

                // Single template mode
                let result =
                    matching::find_template(&screen, tmpl_image.as_ref().expect("tmpl_image set"));
                if result.score > best_score {
                    best_score = result.score;
                }
                if result.score >= *tolerance {
                    let cx = (result.location.0 + result.template_size.0 / 2) as u16;
                    let cy = (result.location.1 + result.template_size.1 / 2) as u16;
                    info!(
                        score = result.score,
                        x = cx,
                        y = cy,
                        "expectclick matched, clicking"
                    );
                    session.mouse_move(cx, cy).await?;
                    session.mouse_click("left").await?;
                    session.process_pending().await?;
                    break;
                }
            }
        }
        Command::RepeatKey {
            key,
            template,
            timeout_secs,
            tolerance,
            interval_ms,
            max_presses,
            needles_dir,
            tag,
        } => {
            let needles = if let Some(dir) = needles_dir {
                Some(needle::load_needle_dir(
                    std::path::Path::new(dir),
                    tag.as_deref(),
                )?)
            } else {
                None
            };
            let tmpl_image = if needles.is_none() {
                Some(
                    image::open(template)
                        .map_err(|e| anyhow::anyhow!("loading template '{template}': {e}"))?
                        .to_rgba8(),
                )
            } else {
                None
            };
            let deadline = Instant::now() + Duration::from_secs(*timeout_secs);
            let mut best_score = 0.0_f32;
            let mut press_count = 0_u32;

            info!(key, template, tolerance, "Repeat-key until match");

            loop {
                if deadline.saturating_duration_since(Instant::now()).is_zero() {
                    let screen = session.current_frame();
                    let fail_path = format!("/tmp/rdpdo-fail-{:03}.png", next_fail_id());
                    let _ = screen.save(&fail_path);
                    eprintln!(
                        "FAIL: best match {best_score:.4} after {press_count} presses, saved {fail_path}"
                    );
                    bail!("repeat-key timeout: best score {best_score:.4} < {tolerance}");
                }

                if let Some(max) = max_presses
                    && press_count >= *max
                {
                    let screen = session.current_frame();
                    let fail_path = format!("/tmp/rdpdo-fail-{:03}.png", next_fail_id());
                    let _ = screen.save(&fail_path);
                    eprintln!(
                        "FAIL: best match {best_score:.4} after {press_count}/{max} presses, saved {fail_path}"
                    );
                    bail!("repeat-key max presses: best score {best_score:.4} < {tolerance}");
                }

                session.send_key_combo(key).await?;
                press_count += 1;
                session.run_for(Duration::from_millis(*interval_ms)).await?;

                let screen = session.current_frame();

                // Needle set mode
                if let Some(ref ndls) = needles {
                    if let Some(result) = needle::match_needle_set(&screen, ndls) {
                        info!(
                            needle = result.needle_name,
                            score = result.score,
                            press_count,
                            "repeat-key needle matched"
                        );
                        break;
                    }
                    continue;
                }

                // Single template mode
                let result =
                    matching::find_template(&screen, tmpl_image.as_ref().expect("tmpl_image set"));
                if result.score > best_score {
                    best_score = result.score;
                }
                if result.score >= *tolerance {
                    info!(
                        score = result.score,
                        press_count,
                        x = result.location.0,
                        y = result.location.1,
                        "repeat-key matched"
                    );
                    break;
                }
            }
        }
        Command::Rexpect {
            region,
            reference,
            timeout_secs,
            tolerance,
        } => {
            let ref_image = image::open(reference)
                .map_err(|e| anyhow::anyhow!("loading reference '{reference}': {e}"))?
                .to_rgba8();
            let deadline = Instant::now() + Duration::from_secs(*timeout_secs);
            let mut best_score = 0.0_f32;
            let mut frame_num = 0_u32;

            info!(reference, region, tolerance, "Waiting for region to match");

            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    let screen = session.current_frame();
                    let fail_path = format!("/tmp/rdpdo-fail-{:03}.png", next_fail_id());
                    let _ = screen.save(&fail_path);
                    eprintln!(
                        "FAIL: best match {best_score:.4} after {frame_num} frames, saved {fail_path}"
                    );
                    bail!("rexpect timeout: best score {best_score:.4} < {tolerance}");
                }

                let (width, height) = session.image_dimensions();
                let (rx, ry, rw, rh) = coords::resolve_region(region, width, height)?;

                frame_num += 1;
                let screen = session.current_frame();
                let score = matching::compare_region(
                    &screen,
                    (u32::from(rx), u32::from(ry), u32::from(rw), u32::from(rh)),
                    &ref_image,
                );
                if score > best_score {
                    best_score = score;
                }
                if score >= *tolerance {
                    info!(score, frame_num, "rexpect matched");
                    break;
                }

                session.run_for(Duration::from_millis(500)).await?;
            }
        }
        Command::Info => {
            let (width, height) = session.image_dimensions();
            let info = report::ConnectionReport::observe(
                session.observed_capabilities(),
                session.egfx_caps(),
                session.egfx_active(),
                width,
                height,
            );
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        Command::Perf => {
            let perf = session.performance_report();
            println!("{}", serde_json::to_string_pretty(&perf)?);
        }
        Command::Rcapture { region, path } => {
            capture::save_capture(session, path, Some(region.as_str()))?;
        }
        Command::AcceptPortal {
            compositor,
            profile,
            verify,
        } => {
            info!(compositor, "Accepting portal dialog");
            let profile_cmds = provision::accept_portal_commands(compositor, profile.as_deref())?;
            for pcmd in &profile_cmds {
                dispatch_command(session, pcmd).await?;
            }
            if *verify {
                info!("Verification requested but needle system not yet implemented");
            }
        }
        Command::Unlock {
            compositor,
            password,
            profile,
            verify,
        } => {
            info!(compositor, "Unlocking screen");
            let resolved_password = cli::resolve_password(password)?;
            let profile_cmds =
                provision::unlock_commands(compositor, &resolved_password, profile.as_deref())?;
            for pcmd in &profile_cmds {
                dispatch_command(session, pcmd).await?;
            }
            if *verify {
                info!("Verification requested but needle system not yet implemented");
            }
        }
        Command::Login {
            username,
            password,
            profile_name,
            domain,
            custom_profile,
            verify,
        } => {
            info!(username, profile_name, "Logging in");
            let resolved_password = cli::resolve_password(password)?;
            let profile_cmds = provision::login_commands(
                profile_name,
                username,
                &resolved_password,
                domain.as_deref(),
                custom_profile.as_deref(),
            )?;
            for pcmd in &profile_cmds {
                dispatch_command(session, pcmd).await?;
            }
            if *verify {
                info!("Verification requested but needle system not yet implemented");
            }
        }
        Command::Pixel(pos) => {
            let (width, height) = session.image_dimensions();
            let (x, y) = coords::resolve_position(pos, width, height)?;
            let frame = session.current_frame();
            let pixel = frame.get_pixel(u32::from(x), u32::from(y));
            if cli_json_mode() {
                println!(
                    "{{\"x\":{x},\"y\":{y},\"r\":{},\"g\":{},\"b\":{},\"a\":{}}}",
                    pixel[0], pixel[1], pixel[2], pixel[3]
                );
            } else {
                println!("{},{},{},{}", pixel[0], pixel[1], pixel[2], pixel[3]);
            }
        }
        Command::Checksum(region_str) => {
            let (width, height) = session.image_dimensions();
            let (rx, ry, rw, rh) = coords::resolve_region(region_str, width, height)?;
            let frame = session.current_frame();
            let crc = region_crc32(&frame, rx, ry, rw, rh);
            if cli_json_mode() {
                println!("{{\"checksum\":\"{crc:08x}\",\"region\":\"{region_str}\"}}");
            } else {
                println!("{crc:08x}");
            }
        }
        Command::WaitChecksumChange {
            region,
            timeout_secs,
        } => {
            let (width, height) = session.image_dimensions();
            let (rx, ry, rw, rh) = coords::resolve_region(region, width, height)?;
            let initial_crc = region_crc32(&session.current_frame(), rx, ry, rw, rh);
            info!(
                region,
                initial_crc = format!("{initial_crc:08x}"),
                "Waiting for checksum change"
            );

            let deadline = Instant::now() + Duration::from_secs(*timeout_secs);
            loop {
                if deadline.saturating_duration_since(Instant::now()).is_zero() {
                    bail!("wait-checksum-change timeout: region unchanged after {timeout_secs}s");
                }
                session.run_for(Duration::from_millis(250)).await?;
                let current_crc = region_crc32(&session.current_frame(), rx, ry, rw, rh);
                if current_crc != initial_crc {
                    info!(new_crc = format!("{current_crc:08x}"), "Checksum changed");
                    break;
                }
            }
        }
        Command::AssertChecksum { region, expected } => {
            let (width, height) = session.image_dimensions();
            let (rx, ry, rw, rh) = coords::resolve_region(region, width, height)?;
            let frame = session.current_frame();
            let actual = region_crc32(&frame, rx, ry, rw, rh);
            let actual_hex = format!("{actual:08x}");
            if actual_hex != *expected {
                bail!("assert-checksum failed: expected {expected}, got {actual_hex}");
            }
            info!(checksum = actual_hex, "Checksum matches");
        }
        Command::FindColor {
            color,
            region,
            tolerance,
            min_area,
        } => {
            let threshold = pixel::ColorThreshold::from_hex(color, *tolerance)
                .ok_or_else(|| anyhow::anyhow!("invalid hex color '{color}' (expected #RRGGBB)"))?;
            let rgn = if let Some(r) = region {
                let (width, height) = session.image_dimensions();
                let (rx, ry, rw, rh) = coords::resolve_region(r, width, height)?;
                Some((u32::from(rx), u32::from(ry), u32::from(rw), u32::from(rh)))
            } else {
                None
            };
            let frame = session.current_frame();
            let clusters = pixel::find_color(&frame, &threshold, rgn, *min_area);
            if cli_json_mode() {
                let entries: Vec<String> = clusters
                    .iter()
                    .map(|c| {
                        format!(
                            "{{\"cx\":{:.0},\"cy\":{:.0},\"area\":{},\"bounds\":[{},{},{},{}]}}",
                            c.cx, c.cy, c.area, c.min_x, c.min_y, c.max_x, c.max_y
                        )
                    })
                    .collect();
                println!("[{}]", entries.join(","));
            } else if clusters.is_empty() {
                eprintln!("no matches found");
            } else {
                for cluster in &clusters {
                    println!(
                        "{:.0},{:.0} area={} bounds={},{},{},{}",
                        cluster.cx,
                        cluster.cy,
                        cluster.area,
                        cluster.min_x,
                        cluster.min_y,
                        cluster.max_x,
                        cluster.max_y
                    );
                }
            }
        }
        Command::Calibrate {
            grid,
            output,
            deploy,
        } => {
            let (cols, rows) = calibrate::parse_grid(grid)?;

            // Deploy calibration page if requested
            match deploy.as_str() {
                "clipboard" => {
                    info!("Deploying calibration page via clipboard");
                    calibrate::deploy_via_clipboard(session).await?;
                }
                "none" => {
                    info!("Skipping calibration page deployment (--deploy none)");
                }
                other => bail!("unknown deploy method '{other}' (use 'clipboard' or 'none')"),
            }

            // Run the calibration grid
            let points = calibrate::run_calibration_grid(session, cols, rows).await?;
            let (width, height) = session.image_dimensions();

            // Build and save profile
            let profile = calibrate::build_profile(
                session.server_address(),
                width,
                height,
                cols,
                rows,
                points,
            );

            info!(
                offset_x = format!("{:.1}", profile.correction.offset_x),
                offset_y = format!("{:.1}", profile.correction.offset_y),
                max_error = format!("{:.1}", profile.correction.max_error),
                avg_error = format!("{:.1}", profile.correction.avg_error),
                "Calibration complete"
            );

            let saved_path = calibrate::save_profile(&profile, output.as_deref())?;
            println!("{saved_path}");
        }
        Command::BootSequence(script_path) => {
            info!(script_path, "Executing boot sequence");
            let (width, height) = session.image_dimensions();
            let vars = script::ScriptVars {
                server: session.server_address().to_owned(),
                width,
                height,
            };
            let script_commands = script::parse_script_with_vars(script_path, Some(&vars))?;
            for script_cmd in &script_commands {
                if session.peer_disconnected() {
                    eprintln!("warning: server disconnected during boot sequence, aborting");
                    break;
                }
                dispatch_command(session, script_cmd).await?;
            }
        }
        Command::Run(script_path) => {
            info!(script_path, "Executing script");
            let (width, height) = session.image_dimensions();
            let vars = script::ScriptVars {
                server: session.server_address().to_owned(),
                width,
                height,
            };
            let script_commands = script::parse_script_with_vars(script_path, Some(&vars))?;
            for script_cmd in &script_commands {
                if session.peer_disconnected() {
                    eprintln!("warning: server disconnected during script, aborting");
                    break;
                }
                dispatch_command(session, script_cmd).await?;
            }
        }
        Command::Play { path, speed } => {
            info!(path, speed, "Replaying recording");
            let recording = recorder::Recording::load(path)?;
            let mut last_t = 0_u64;
            let token_sets = recording.to_command_tokens();
            let events = &recording.events;

            for (idx, tokens) in token_sets.iter().enumerate() {
                if session.peer_disconnected() {
                    eprintln!("warning: server disconnected during playback, aborting");
                    break;
                }

                // Respect inter-event timing
                let event_t = events[idx].t;
                if event_t > last_t {
                    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let delay_ms = ((event_t - last_t) as f64 / speed) as u64;
                    if delay_ms > 0 {
                        session.run_for(Duration::from_millis(delay_ms)).await?;
                    }
                }
                last_t = event_t;

                let parsed = cli::parse_commands(tokens)?;
                for play_cmd in &parsed {
                    dispatch_command(session, play_cmd).await?;
                }
            }
        }
        Command::BaselineUpdate { name, dir } => {
            let baselines_dir = baseline_dir(dir.as_deref());
            std::fs::create_dir_all(&baselines_dir)
                .map_err(|e| anyhow::anyhow!("create baseline dir: {e}"))?;

            let path = baselines_dir.join(format!("{name}.png"));
            session.save_screenshot(&path)?;
            info!(name, path = %path.display(), "Baseline saved");
            println!("{}", path.display());
        }
        Command::BaselineList { dir } => {
            let baselines_dir = baseline_dir(dir.as_deref());
            if baselines_dir.is_dir() {
                let mut entries: Vec<_> = std::fs::read_dir(&baselines_dir)?
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|ext| ext == "png"))
                    .collect();
                entries.sort();
                if entries.is_empty() {
                    println!("No baselines in {}", baselines_dir.display());
                } else {
                    for entry in &entries {
                        let stem = entry.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                        let meta = std::fs::metadata(entry).ok();
                        let size = meta.as_ref().map_or(0, std::fs::Metadata::len);
                        println!("{stem}  ({size} bytes)");
                    }
                }
            } else {
                println!("No baselines directory: {}", baselines_dir.display());
            }
        }
        Command::BaselineCheck {
            name,
            tolerance,
            dir,
        } => {
            let baselines_dir = baseline_dir(dir.as_deref());
            let path = baselines_dir.join(format!("{name}.png"));
            if !path.exists() {
                bail!("baseline '{name}' not found at {}", path.display());
            }

            let reference = image::open(&path)
                .map_err(|e| anyhow::anyhow!("loading baseline '{name}': {e}"))?
                .to_rgba8();
            let screen = session.current_frame();
            let score = matching::compare_full(&screen, &reference);

            if score >= *tolerance {
                println!("PASS: {score:.4} >= {tolerance} (baseline: {name})");
            } else {
                let diff_img = matching::diff_images(&screen, &reference);
                let diff_path = format!("/tmp/rdpdo-baseline-diff-{name}.png");
                let _ = diff_img.save(&diff_path);
                eprintln!(
                    "FAIL: {score:.4} < {tolerance} (baseline: {name}), diff saved to {diff_path}"
                );
                bail!("baseline check failed: {score:.4} < {tolerance} for '{name}'");
            }
        }
        Command::Monitor(action) => match action {
            cli::MonitorAction::List => {
                let (width, height) = session.image_dimensions();
                let json = serde_json::json!({
                    "monitors": [{
                        "id": 1,
                        "primary": true,
                        "width": width,
                        "height": height,
                    }]
                });
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            cli::MonitorAction::Set { width, height } => {
                info!(width, height, "Setting monitor layout");
                let sent = session.send_resize(*width, *height).await?;
                if sent {
                    session.run_for(Duration::from_secs(2)).await?;
                    let (new_w, new_h) = session.image_dimensions();
                    info!(new_w, new_h, "Monitor resized");
                } else {
                    eprintln!("warning: DisplayControl not available, resize skipped");
                }
            }
        },
        Command::Timelapse {
            path_template,
            interval_ms,
            count,
            duration_secs,
            until,
            tolerance,
        } => {
            let until_image = if let Some(path) = &until {
                Some(
                    image::open(path)
                        .map_err(|e| anyhow::anyhow!("loading --until reference '{path}': {e}"))?
                        .to_rgba8(),
                )
            } else {
                None
            };

            let deadline = if *duration_secs > 0 {
                Some(Instant::now() + Duration::from_secs(*duration_secs))
            } else {
                None
            };

            let mut frame_num: u32 = 0;
            info!(
                interval_ms,
                count,
                duration_secs,
                until = until.as_deref().unwrap_or("none"),
                "Starting timelapse"
            );

            loop {
                // Check count limit
                if *count > 0 && frame_num >= *count {
                    info!(frame_num, "Timelapse complete (count reached)");
                    break;
                }

                // Check duration limit
                if let Some(dl) = deadline
                    && Instant::now() >= dl
                {
                    info!(frame_num, "Timelapse complete (duration reached)");
                    break;
                }

                // Capture the frame
                let screen = session.current_frame();
                let out_path = if path_template.contains("{n}") {
                    path_template.replace("{n}", &format!("{frame_num:03}"))
                } else {
                    path_template.clone()
                };
                screen
                    .save(&out_path)
                    .map_err(|e| anyhow::anyhow!("saving timelapse frame '{out_path}': {e}"))?;

                frame_num += 1;

                // Check --until match
                if let Some(ref ref_img) = until_image {
                    let score = matching::compare_full(&screen, ref_img);
                    if score >= *tolerance {
                        info!(
                            score,
                            frame_num,
                            path = out_path,
                            "Timelapse --until matched"
                        );
                        break;
                    }
                }

                // Wait for the interval, processing PDUs during the wait
                session.run_for(Duration::from_millis(*interval_ms)).await?;
            }
        }
        Command::AudioCapture {
            output,
            duration_secs,
        } => {
            session.audio_start_recording();
            info!(duration_secs, "Recording audio");
            session.run_for(Duration::from_secs(*duration_secs)).await?;
            let (pcm, format) = session.audio_stop_recording();
            let Some(format) = format else {
                bail!(
                    "No audio data received from server \
                     (RDPSND channel negotiated but no wave packets arrived)"
                );
            };
            let path = std::path::Path::new(output);
            audio::write_wav(path, &format, &pcm)?;
            let rms = audio::rms_amplitude(&pcm);
            info!(
                output,
                samples = pcm.len() / 2,
                rms = format!("{rms:.4}"),
                "Audio captured"
            );
            println!("audio_file: {output}");
            println!("rms: {rms:.6}");
            println!("samples: {}", pcm.len() / 2);
        }
        Command::AudioAssertPlaying { timeout_secs } => {
            session.audio_start_recording();
            let deadline = Instant::now() + Duration::from_secs(*timeout_secs);
            let mut detected = false;
            while Instant::now() < deadline {
                session.run_for(Duration::from_millis(250)).await?;
                let rms = session.audio_rms();
                if rms > 0.01 {
                    info!(rms = format!("{rms:.4}"), "Audio detected");
                    detected = true;
                    break;
                }
            }
            // Stop recording and discard captured data
            session.audio_stop_recording();
            if detected {
                println!("audio_playing: true");
            } else {
                bail!("No audio detected within {timeout_secs}s");
            }
        }
        Command::MouseHide => {
            // Move far beyond desktop bounds so cursor vanishes
            session.mouse_move(65535, 65535).await?;
            session.process_pending().await?;
            info!("Cursor hidden");
        }
        Command::WaitPixel {
            pos,
            color,
            tolerance,
            timeout_secs,
        } => {
            let (width, height) = session.image_dimensions();
            let (x, y) = coords::resolve_position(pos, width, height)?;
            let expected = pixel::parse_color(color)?;
            let deadline = Instant::now() + Duration::from_secs(*timeout_secs);

            loop {
                let frame = session.current_frame();
                let actual = pixel::get_pixel(&frame, u32::from(x), u32::from(y));
                if pixel::colors_match(actual, expected, *tolerance) {
                    println!(
                        "matched at {x},{y}: #{:02X}{:02X}{:02X}",
                        actual[0], actual[1], actual[2]
                    );
                    break;
                }
                if Instant::now() >= deadline {
                    bail!(
                        "wait-pixel timeout: pixel at {x},{y} is #{:02X}{:02X}{:02X}, expected #{:02X}{:02X}{:02X} (tolerance {tolerance})",
                        actual[0],
                        actual[1],
                        actual[2],
                        expected[0],
                        expected[1],
                        expected[2]
                    );
                }
                session.run_for(Duration::from_millis(250)).await?;
            }
        }
        Command::AssertPixel {
            pos,
            color,
            tolerance,
        } => {
            let (width, height) = session.image_dimensions();
            let (x, y) = coords::resolve_position(pos, width, height)?;
            let expected = pixel::parse_color(color)?;
            let frame = session.current_frame();
            let actual = pixel::get_pixel(&frame, u32::from(x), u32::from(y));

            if pixel::colors_match(actual, expected, *tolerance) {
                println!(
                    "PASS: pixel {x},{y} = #{:02X}{:02X}{:02X}",
                    actual[0], actual[1], actual[2]
                );
            } else {
                bail!(
                    "assert-pixel failed: pixel at {x},{y} is #{:02X}{:02X}{:02X}, expected #{:02X}{:02X}{:02X} (tolerance {tolerance})",
                    actual[0],
                    actual[1],
                    actual[2],
                    expected[0],
                    expected[1],
                    expected[2]
                );
            }
        }
        Command::Measure {
            template,
            timeout_secs,
            tolerance,
            label,
            needles_dir,
            tag,
        } => {
            let needles = if let Some(dir) = needles_dir {
                Some(needle::load_needle_dir(
                    std::path::Path::new(dir),
                    tag.as_deref(),
                )?)
            } else {
                None
            };

            let tmpl_image = if needles.is_none() {
                Some(
                    image::open(template)
                        .map_err(|e| anyhow::anyhow!("loading template '{template}': {e}"))?
                        .to_rgba8(),
                )
            } else {
                None
            };

            let start = Instant::now();
            let deadline = start + Duration::from_secs(*timeout_secs);

            let found = loop {
                session.run_for(Duration::from_millis(250)).await?;
                let frame = session.current_frame();

                // Needle set mode
                if let Some(ref ns) = needles {
                    if let Some(result) = needle::match_needle_set(&frame, ns) {
                        let elapsed = start.elapsed();
                        let label_str = label.as_deref().unwrap_or(&result.needle_name);
                        if cli_json_mode() {
                            println!(
                                "{{\"label\":\"{label_str}\",\"elapsed_ms\":{},\"matched\":true,\"score\":{:.4}}}",
                                elapsed.as_millis(),
                                result.score
                            );
                        } else {
                            println!(
                                "measure: {label_str} matched in {}ms (score {:.4})",
                                elapsed.as_millis(),
                                result.score
                            );
                        }
                        break true;
                    }
                } else {
                    // Single template mode
                    let result = matching::find_template(
                        &frame,
                        tmpl_image.as_ref().expect("tmpl_image set"),
                    );
                    if result.score >= *tolerance {
                        let elapsed = start.elapsed();
                        let label_str = label.as_deref().unwrap_or(template);
                        if cli_json_mode() {
                            println!(
                                "{{\"label\":\"{label_str}\",\"elapsed_ms\":{},\"matched\":true,\"score\":{:.4}}}",
                                elapsed.as_millis(),
                                result.score
                            );
                        } else {
                            println!(
                                "measure: {label_str} matched in {}ms (score {:.4})",
                                elapsed.as_millis(),
                                result.score
                            );
                        }
                        break true;
                    }
                }

                if Instant::now() >= deadline {
                    break false;
                }
            };

            if !found {
                let elapsed = start.elapsed();
                let label_str = label.as_deref().unwrap_or("measure");
                if cli_json_mode() {
                    println!(
                        "{{\"label\":\"{label_str}\",\"elapsed_ms\":{},\"matched\":false}}",
                        elapsed.as_millis()
                    );
                }
                bail!("measure timeout: no match within {timeout_secs}s");
            }
        }
        // Modifiers and session are consumed by the dispatch loop, never reach here
        Command::Retry(_) | Command::Soft | Command::Session => {}
        Command::Status => {
            let (w, h) = session.image_dimensions();
            let gfx = if session.egfx_active() {
                "EGFX"
            } else {
                "bitmap"
            };
            let frames = session.frame_count();
            let report = session.performance_report();
            let uptime = session.session_uptime();
            let fps_str = report
                .avg_fps
                .map_or_else(|| "-".to_owned(), |f| format!("{f:.1}"));
            let rx = session.bytes_received();
            let tx = session.bytes_sent();
            let up_secs = uptime.as_secs();
            let up_min = up_secs / 60;
            let up_sec = up_secs % 60;
            if cli_json_mode() {
                let json = serde_json::json!({
                    "server": session.server_address(),
                    "resolution": format!("{w}x{h}"),
                    "graphics": gfx,
                    "frames": frames,
                    "avg_fps": report.avg_fps,
                    "bytes_rx": rx,
                    "bytes_tx": tx,
                    "uptime_secs": up_secs,
                    "disconnected": session.peer_disconnected(),
                });
                println!("{json}");
            } else {
                eprintln!(
                    "{w}x{h} {gfx} | {frames} frames {fps_str} fps | rx {rx} tx {tx} | up {up_min}m{up_sec:02}s"
                );
            }
        }
        Command::Exec {
            destination,
            command,
        } => {
            let status = tokio::process::Command::new("ssh")
                .arg("-o")
                .arg("BatchMode=yes")
                .arg("-o")
                .arg("StrictHostKeyChecking=accept-new")
                .arg(destination)
                .arg(command)
                .status()
                .await
                .map_err(|e| anyhow::anyhow!("ssh exec: {e}"))?;
            if !status.success() {
                bail!("exec: ssh exited with code {}", status.code().unwrap_or(-1));
            }
        }
        Command::Watch { duration_secs } => {
            let duration = Duration::from_secs(*duration_secs);
            let watch_start = Instant::now();
            let deadline = watch_start + duration;
            let start_frames = session.frame_count();
            let mut interval_start = Instant::now();
            let mut interval_frames = start_frames;

            eprintln!("Monitoring frame rate for {duration_secs}s...");

            while Instant::now() < deadline {
                session.run_for(Duration::from_secs(1)).await?;
                if session.peer_disconnected() {
                    eprintln!("Server disconnected during watch.");
                    break;
                }
                let now_frames = session.frame_count();
                let elapsed = interval_start.elapsed().as_secs_f64();
                if elapsed > 0.0 {
                    let fps = (now_frames - interval_frames) as f64 / elapsed;
                    let total_secs = watch_start.elapsed().as_secs();
                    eprintln!("  [{total_secs:>3}s] {fps:.1} fps ({now_frames} total frames)");
                }
                interval_start = Instant::now();
                interval_frames = now_frames;
            }

            let total_frames = session.frame_count() - start_frames;
            let total_elapsed = watch_start.elapsed().as_secs_f64();
            let avg_fps = if total_elapsed > 0.0 {
                total_frames as f64 / total_elapsed
            } else {
                0.0
            };

            if cli_json_mode() {
                let json = serde_json::json!({
                    "duration_secs": duration_secs,
                    "total_frames": total_frames,
                    "avg_fps": avg_fps,
                });
                println!("{json}");
            } else {
                eprintln!(
                    "Watch complete: {total_frames} frames in {duration_secs}s ({avg_fps:.1} avg fps)"
                );
            }
        }
        Command::Help(_)
        | Command::Convert { .. }
        | Command::Diff { .. }
        | Command::AudioVerify { .. } => {
            dispatch_offline(cmd)?;
        }
    }

    Ok(())
}

#[expect(clippy::too_many_lines)]
fn dispatch_offline(cmd: &Command) -> Result<()> {
    match cmd {
        Command::Convert { input, output } => {
            recorder::convert_to_script(input, output)?;
            info!("Converted {input} -> {output}");
        }
        Command::Diff {
            image_a,
            image_b,
            threshold,
            output,
            mode,
            exclude,
        } => {
            let mut a = image::open(image_a)
                .map_err(|e| anyhow::anyhow!("loading '{image_a}': {e}"))?
                .to_rgba8();
            let mut b = image::open(image_b)
                .map_err(|e| anyhow::anyhow!("loading '{image_b}': {e}"))?
                .to_rgba8();

            // Mask excluded regions with identical pixels so they don't affect comparison
            for region_spec in exclude {
                let (aw, ah) = a.dimensions();
                if let Ok((rx, ry, rw, rh)) =
                    coords::resolve_region(region_spec, aw as u16, ah as u16)
                {
                    for py in u32::from(ry)..u32::from(ry) + u32::from(rh) {
                        for px in u32::from(rx)..u32::from(rx) + u32::from(rw) {
                            if px < aw && py < ah {
                                let black = image::Rgba([0, 0, 0, 255]);
                                a.put_pixel(px, py, black);
                                if px < b.width() && py < b.height() {
                                    b.put_pixel(px, py, black);
                                }
                            }
                        }
                    }
                }
            }

            let score = matching::compare_full(&a, &b);
            let different = matching::count_different_pixels(&a, &b, 8);
            let (aw, ah) = a.dimensions();
            let (bw, bh) = b.dimensions();

            let total_pixels = u64::from(aw.min(bw)) * u64::from(ah.min(bh));
            let pct_different = if total_pixels > 0 {
                (different as f64 / total_pixels as f64) * 100.0
            } else {
                0.0
            };

            println!("correlation: {score:.6}");
            println!("different_pixels: {different}/{total_pixels} ({pct_different:.2}%)");
            println!("dimensions: {aw}x{ah} vs {bw}x{bh}");

            // Generate visual diff output if --output specified
            if let Some(out_path) = output {
                let diff_img = match mode {
                    cli::DiffMode::Highlight => matching::diff_images(&a, &b),
                    cli::DiffMode::SideBySide => matching::side_by_side(&a, &b),
                    cli::DiffMode::Heatmap => matching::heatmap_diff(&a, &b),
                };
                diff_img
                    .save(out_path)
                    .map_err(|e| anyhow::anyhow!("saving diff image '{out_path}': {e}"))?;
                println!("diff image: {out_path}");
            }

            if let Some(t) = threshold {
                let norm_t = if *t > 1.0 { *t / 100.0 } else { *t };
                if score < norm_t {
                    if output.is_none() {
                        let diff_img = matching::diff_images(&a, &b);
                        let diff_path = format!("/tmp/rdpdo-diff-{:03}.png", next_fail_id());
                        let _ = diff_img.save(&diff_path);
                        eprintln!(
                            "FAIL: correlation {score:.4} < threshold {norm_t}, diff saved to {diff_path}"
                        );
                    } else {
                        eprintln!("FAIL: correlation {score:.4} < threshold {norm_t}");
                    }
                    bail!("diff below threshold: {score:.4} < {norm_t}");
                }
                println!("PASS: {score:.4} >= {t}");
            }
        }
        Command::AudioVerify {
            captured,
            reference,
            tolerance,
        } => {
            let cap_pcm = std::fs::read(captured)
                .map_err(|e| anyhow::anyhow!("reading '{captured}': {e}"))?;
            let ref_pcm = std::fs::read(reference)
                .map_err(|e| anyhow::anyhow!("reading '{reference}': {e}"))?;

            // Skip WAV headers (44 bytes) if present
            let cap_data = if cap_pcm.starts_with(b"RIFF") && cap_pcm.len() > 44 {
                &cap_pcm[44..]
            } else {
                &cap_pcm
            };
            let ref_data = if ref_pcm.starts_with(b"RIFF") && ref_pcm.len() > 44 {
                &ref_pcm[44..]
            } else {
                &ref_pcm
            };

            let score = audio::compare_audio(cap_data, ref_data);
            let cap_rms = audio::rms_amplitude(cap_data);
            let ref_rms = audio::rms_amplitude(ref_data);

            println!("correlation: {score:.6}");
            println!("captured_rms: {cap_rms:.6}");
            println!("reference_rms: {ref_rms:.6}");

            let norm_t = if *tolerance > 1.0 {
                *tolerance / 100.0
            } else {
                *tolerance
            };
            if score < f64::from(norm_t) {
                bail!("audio mismatch: correlation {score:.4} < tolerance {norm_t}");
            }
            println!("PASS: {score:.4} >= {norm_t}");
        }
        Command::Help(topic) => match topic {
            Some(cmd_name) => {
                if let Some(text) = help::command_help("rdpdo", help::all_commands(), cmd_name) {
                    print!("{text}");
                } else {
                    let all_names = help::all_command_names(help::all_commands());
                    eprintln!("unknown command '{cmd_name}'");
                    eprintln!("available commands: {}", all_names.join(", "));
                    bail!("no help for '{cmd_name}'");
                }
            }
            None => {
                print!("{}", help::full_help());
            }
        },
        _ => bail!("dispatch_offline called with non-offline command"),
    }
    Ok(())
}

/// Log a command to the session recorder.
#[expect(clippy::too_many_lines)]
fn record_command(rec: &mut recorder::SessionRecorder, cmd: &Command) -> Result<()> {
    match cmd {
        Command::Type(text) => rec.record("type", &[text]),
        Command::Utype(text) => rec.record("utype", &[text]),
        Command::Key { spec, hold_ms } => {
            if let Some(ms) = hold_ms {
                rec.record("key", &[spec, "--hold-ms", &ms.to_string()])
            } else {
                rec.record("key", &[spec])
            }
        }
        Command::Keydown(spec) => rec.record("keydown", &[spec]),
        Command::Keyup(spec) => rec.record("keyup", &[spec]),
        Command::Move(pos) => rec.record("move", &[pos]),
        Command::Click(pos, button) => rec.record("click", &[pos, button]),
        Command::Doubleclick(pos) => rec.record("doubleclick", &[pos]),
        Command::Drag(from, to) => rec.record("drag", &[from, to]),
        Command::Scroll(dir, notches) => rec.record("scroll", &[dir, &notches.to_string()]),
        Command::Pause(secs) => rec.record("pause", &[&secs.to_string()]),
        Command::Capture(path, region) => {
            if let Some(r) = region {
                rec.record("capture", &[path, r])
            } else {
                rec.record("capture", &[path.as_str()])
            }
        }
        // Redact password from recordings
        Command::TypePassword(_) => rec.record("type-password", &["***"]),
        Command::Resize(dims) => rec.record("resize", &[dims]),
        Command::SetClipboard(text) => rec.record("set-clipboard", &[text]),
        Command::GetClipboard => rec.record("get-clipboard", &[]),
        Command::WaitStill {
            stillness_ms,
            timeout_secs,
        } => rec.record(
            "wait-still",
            &[&stillness_ms.to_string(), &timeout_secs.to_string()],
        ),
        Command::WaitChange { timeout_secs } => {
            rec.record("wait-change", &[&timeout_secs.to_string()])
        }
        Command::Expect {
            reference,
            timeout_secs,
            tolerance,
            ..
        } => rec.record(
            "expect",
            &[reference, &tolerance.to_string(), &timeout_secs.to_string()],
        ),
        Command::Waitfor {
            template,
            timeout_secs,
            tolerance,
            ..
        } => rec.record(
            "waitfor",
            &[template, &tolerance.to_string(), &timeout_secs.to_string()],
        ),
        Command::Expectclick {
            template,
            timeout_secs,
            tolerance,
            ..
        } => rec.record(
            "expectclick",
            &[template, &tolerance.to_string(), &timeout_secs.to_string()],
        ),
        Command::RepeatKey {
            key,
            template,
            timeout_secs,
            tolerance,
            interval_ms,
            max_presses,
            ..
        } => {
            let mut args = vec![key.as_str(), template.as_str()];
            let tol_s = tolerance.to_string();
            let timeout_s = timeout_secs.to_string();
            let interval_s = interval_ms.to_string();
            args.extend_from_slice(&[&tol_s, &timeout_s, &interval_s]);
            if let Some(max) = max_presses {
                let max_s = max.to_string();
                args.push(&max_s);
                rec.record("repeat-key", &args)
            } else {
                rec.record("repeat-key", &args)
            }
        }
        Command::Rexpect {
            region,
            reference,
            timeout_secs,
            tolerance,
        } => rec.record(
            "rexpect",
            &[
                region,
                reference,
                &tolerance.to_string(),
                &timeout_secs.to_string(),
            ],
        ),
        Command::Info => rec.record("info", &[]),
        Command::Perf => rec.record("perf", &[]),
        Command::Rcapture { region, path } => rec.record("rcapture", &[region, path]),
        Command::Run(script) => rec.record("run", &[script.as_str()]),
        Command::AcceptPortal { compositor, .. } => rec.record("accept-portal", &[compositor]),
        Command::Unlock {
            compositor,
            password: _,
            ..
        } => rec.record("unlock", &[compositor, "***"]),
        Command::Login {
            username,
            password: _,
            ..
        } => rec.record("login", &[username, "***"]),
        Command::BootSequence(script) => rec.record("boot-sequence", &[script.as_str()]),
        Command::Pixel(pos) => rec.record("pixel", &[pos]),
        Command::Checksum(region) => rec.record("checksum", &[region]),
        Command::WaitChecksumChange {
            region,
            timeout_secs,
        } => rec.record("wait-checksum-change", &[region, &timeout_secs.to_string()]),
        Command::AssertChecksum { region, expected } => {
            rec.record("assert-checksum", &[region, expected])
        }
        Command::ClipboardSendFile(path) => rec.record("clipboard-send-file", &[path]),
        Command::ClipboardRecvFile(path) => rec.record("clipboard-recv-file", &[path]),
        Command::AudioCapture {
            output,
            duration_secs,
        } => rec.record("audio-capture", &[output, &duration_secs.to_string()]),
        Command::AudioAssertPlaying { timeout_secs } => {
            rec.record("audio-assert-playing", &[&timeout_secs.to_string()])
        }
        Command::AudioVerify {
            captured,
            reference,
            tolerance,
        } => rec.record(
            "audio-verify",
            &[captured, reference, &tolerance.to_string()],
        ),
        Command::FindColor { color, .. } => rec.record("find-color", &[color]),
        Command::BaselineUpdate { name, .. } => rec.record("baseline", &["update", name]),
        Command::BaselineList { .. } => rec.record("baseline", &["list"]),
        Command::BaselineCheck { name, .. } => rec.record("baseline", &["check", name]),
        Command::Monitor(cli::MonitorAction::List) => rec.record("monitor", &["list"]),
        Command::Monitor(cli::MonitorAction::Set { width, height }) => {
            let dims = format!("{width}x{height}");
            rec.record("monitor", &["set", &dims])
        }
        Command::Calibrate { grid, .. } => rec.record("calibrate", &[grid]),
        Command::Timelapse {
            path_template,
            interval_ms,
            count,
            ..
        } => rec.record(
            "timelapse",
            &[path_template, &interval_ms.to_string(), &count.to_string()],
        ),
        Command::MouseHide => rec.record("mouse-hide", &[]),
        Command::WaitPixel {
            pos,
            color,
            tolerance,
            timeout_secs,
        } => rec.record(
            "wait-pixel",
            &[
                pos,
                color,
                &tolerance.to_string(),
                &timeout_secs.to_string(),
            ],
        ),
        Command::AssertPixel {
            pos,
            color,
            tolerance,
        } => rec.record("assert-pixel", &[pos, color, &tolerance.to_string()]),
        Command::Measure {
            template,
            timeout_secs,
            tolerance,
            ..
        } => rec.record(
            "measure",
            &[template, &tolerance.to_string(), &timeout_secs.to_string()],
        ),
        Command::Retry(n) => rec.record("retry", &[&n.to_string()]),
        Command::Soft => rec.record("soft", &[]),
        // play/convert/diff/help/session are meta-commands, don't record them inside recordings
        Command::Play { .. }
        | Command::Convert { .. }
        | Command::Diff { .. }
        | Command::Help(_)
        | Command::Session => Ok(()),
        Command::Status => rec.record("status", &[]),
        Command::Exec {
            destination,
            command,
        } => rec.record("exec", &[destination, command]),
        Command::Watch { duration_secs } => rec.record("watch", &[&duration_secs.to_string()]),
    }
}

/// Default baselines directory: ~/.config/rdpdo/baselines/ or custom path.
fn baseline_dir(custom: Option<&str>) -> std::path::PathBuf {
    if let Some(dir) = custom {
        std::path::PathBuf::from(dir)
    } else {
        dirs::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("rdpdo")
            .join("baselines")
    }
}

/// CRC32 of the raw pixel data in a screen region.
fn region_crc32(frame: &image::RgbaImage, x: u16, y: u16, w: u16, h: u16) -> u32 {
    let mut hasher = crc32_hasher::Crc32Hasher::new();
    for py in u32::from(y)..u32::from(y) + u32::from(h) {
        for px in u32::from(x)..u32::from(x) + u32::from(w) {
            let pixel = frame.get_pixel(px, py);
            hasher.update(&pixel.0);
        }
    }
    hasher.finalize()
}

/// Minimal CRC32 implementation (IEEE polynomial, no external dependency).
mod crc32_hasher {
    pub(crate) struct Crc32Hasher {
        crc: u32,
    }

    impl Crc32Hasher {
        pub(crate) fn new() -> Self {
            Self { crc: 0xFFFF_FFFF }
        }

        pub(crate) fn update(&mut self, data: &[u8]) {
            for &byte in data {
                let index = (self.crc ^ u32::from(byte)) & 0xFF;
                self.crc = CRC32_TABLE[index as usize] ^ (self.crc >> 8);
            }
        }

        pub(crate) fn finalize(self) -> u32 {
            self.crc ^ 0xFFFF_FFFF
        }
    }

    // Pre-computed CRC32 table (IEEE polynomial 0xEDB88320)
    const CRC32_TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut crc = i as u32;
            let mut j = 0;
            while j < 8 {
                if crc & 1 != 0 {
                    crc = 0xEDB8_8320 ^ (crc >> 1);
                } else {
                    crc >>= 1;
                }
                j += 1;
            }
            table[i] = crc;
            i += 1;
        }
        table
    };
}

/// Messages from the readline thread to the async REPL loop.
enum ReplLine {
    Input(String),
    CtrlC,
    Eof,
}

/// Interactive REPL: read commands from stdin, dispatch against a live session.
///
/// When stdin is a terminal, uses rustyline for readline editing and history.
/// Ctrl+C cancels the current line (during input) or is ignored (during commands).
/// When piped, reads silently until EOF — suitable for `echo "type hello" | rdpdo -s host session`.
///
/// REPL-only commands: `set KEY VALUE`, `get KEY`, `vars`, `disconnect`, `reconnect`.
async fn run_session_repl(
    session: &mut HeadlessSession,
    dest: &connection::Destination,
    cli: &Cli,
) -> Result<()> {
    use std::{collections::HashMap, io::IsTerminal};

    let interactive = std::io::stdin().is_terminal();
    let mut variables: HashMap<String, String> = HashMap::new();

    // Seed built-in variables
    variables.insert("server".to_owned(), dest.addr_string());
    let (w, h) = session.image_dimensions();
    variables.insert("width".to_owned(), w.to_string());
    variables.insert("height".to_owned(), h.to_string());

    if interactive {
        eprintln!("rdpdo session mode. Type 'help' for commands, 'quit' to exit.");
        eprintln!("  Ctrl+C cancels input. 'set'/'get'/'vars' for variables.");
        eprintln!("  'disconnect'/'reconnect' manage the connection.");
    }

    if interactive {
        run_repl_interactive(session, dest, cli, &mut variables).await
    } else {
        run_repl_piped(session, &mut variables).await
    }
}

/// Piped (non-interactive) REPL: read from stdin line by line.
async fn run_repl_piped(
    session: &mut HeadlessSession,
    variables: &mut std::collections::HashMap<String, String>,
) -> Result<()> {
    let mut line_task = tokio::task::spawn_blocking(|| {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        let mut line = String::new();
        match stdin.lock().read_line(&mut line) {
            Ok(0) => Ok(None),
            Ok(_) => Ok(Some(line)),
            Err(e) => Err(anyhow::anyhow!("stdin: {e}")),
        }
    });

    loop {
        if session.peer_disconnected() {
            eprintln!("Server disconnected.");
            break;
        }

        let line = loop {
            tokio::select! {
                biased;
                result = &mut line_task => {
                    break result.map_err(|e| anyhow::anyhow!("stdin task: {e}"))??;
                }
                _ = session.run_for(Duration::from_millis(200)) => {
                    if session.peer_disconnected() {
                        return Ok(());
                    }
                }
            }
        };

        let Some(raw) = line else {
            break;
        };

        let trimmed = raw.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            repl_dispatch_line(session, trimmed, variables).await;
        }

        // Spawn next read
        line_task = tokio::task::spawn_blocking(|| {
            use std::io::BufRead;
            let stdin = std::io::stdin();
            let mut line = String::new();
            match stdin.lock().read_line(&mut line) {
                Ok(0) => Ok(None),
                Ok(_) => Ok(Some(line)),
                Err(e) => Err(anyhow::anyhow!("stdin: {e}")),
            }
        });
    }

    Ok(())
}

/// Interactive REPL using rustyline in a dedicated thread.
#[expect(clippy::too_many_lines)]
async fn run_repl_interactive(
    session: &mut HeadlessSession,
    dest: &connection::Destination,
    cli: &Cli,
    variables: &mut std::collections::HashMap<String, String>,
) -> Result<()> {
    use std::sync::{Arc, Mutex};

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ReplLine>();

    // Shared prompt string updated on reconnect
    let prompt = Arc::new(Mutex::new(format!("rdpdo[{}]> ", session.server_address())));
    let prompt_clone = Arc::clone(&prompt);

    // History file
    let history_path = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rdpdo")
        .join("history.txt");

    // Spawn readline thread
    std::thread::spawn(move || {
        use rustyline::{Config, DefaultEditor, error::ReadlineError};

        let config = Config::builder()
            .max_history_size(1000)
            .expect("valid history size")
            .auto_add_history(true)
            .build();

        let mut rl = DefaultEditor::with_config(config).expect("rustyline init");

        // Load history (ignore errors, file may not exist)
        if let Some(parent) = history_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = rl.load_history(&history_path);

        loop {
            let current_prompt = prompt_clone.lock().expect("prompt lock").clone();
            match rl.readline(&current_prompt) {
                Ok(line) => {
                    if tx.send(ReplLine::Input(line)).is_err() {
                        break;
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    // Ctrl+C: cancel current line
                    if tx.send(ReplLine::CtrlC).is_err() {
                        break;
                    }
                }
                Err(ReadlineError::Eof | _) => {
                    let _ = tx.send(ReplLine::Eof);
                    break;
                }
            }
        }

        // Save history on exit
        let _ = rl.save_history(&history_path);
    });

    // Main async loop: receive lines from readline thread, process RDP traffic
    loop {
        let line = loop {
            tokio::select! {
                biased;
                msg = rx.recv() => {
                    break msg;
                }
                _ = session.run_for(Duration::from_millis(200)) => {
                    if session.peer_disconnected() {
                        eprintln!("\nServer disconnected. Use 'reconnect' or 'quit'.");
                    }
                }
            }
        };

        let Some(repl_line) = line else {
            // Channel closed (readline thread exited)
            break;
        };

        match repl_line {
            ReplLine::CtrlC => {
                // Rustyline already printed ^C and a new prompt
            }
            ReplLine::Eof => {
                eprintln!();
                break;
            }
            ReplLine::Input(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }

                match trimmed {
                    "quit" | "exit" => break,
                    "disconnect" => {
                        if session.peer_disconnected() {
                            eprintln!("Already disconnected.");
                        } else {
                            session.force_disconnect();
                            eprintln!("Disconnected. Use 'reconnect' to re-establish.");
                        }
                        continue;
                    }
                    "reconnect" => {
                        eprintln!("Reconnecting to {}...", dest.addr_string());
                        let connector_config = connection::build_connector_config(
                            cli.user.as_deref(),
                            cli.password.as_deref(),
                            cli.no_auth,
                            cli.no_nla,
                            cli.width,
                            cli.height,
                        );
                        match connection::connect_headless(dest, connector_config).await {
                            Ok(result) => {
                                let old = std::mem::replace(
                                    session,
                                    HeadlessSession::from_connect_result(result),
                                );
                                session.set_server_addr(&cli.server);

                                // Update prompt
                                if let Ok(mut p) = prompt.lock() {
                                    *p = format!("rdpdo[{}]> ", session.server_address());
                                }

                                // Update built-in variables
                                let (w, h) = session.image_dimensions();
                                variables.insert("width".to_owned(), w.to_string());
                                variables.insert("height".to_owned(), h.to_string());

                                // Shut down old session in the background
                                let _ = old.shutdown().await;

                                // Wait for initial frame
                                let _ = session.wait_for_frame(Duration::from_secs(2)).await;
                                eprintln!("Reconnected.");
                            }
                            Err(e) => {
                                eprintln!("reconnect failed: {e:#}");
                            }
                        }
                        continue;
                    }
                    "vars" => {
                        if variables.is_empty() {
                            eprintln!("No variables set.");
                        } else {
                            let mut keys: Vec<&String> = variables.keys().collect();
                            keys.sort();
                            for k in keys {
                                eprintln!("  {k} = {}", variables[k]);
                            }
                        }
                        continue;
                    }
                    _ => {}
                }

                // Handle set/get as REPL-only commands
                if let Some(rest) = trimmed.strip_prefix("set ") {
                    let parts: Vec<&str> = rest.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        variables.insert(parts[0].to_owned(), parts[1].to_owned());
                        eprintln!("  {0} = {1}", parts[0], parts[1]);
                    } else {
                        eprintln!("usage: set KEY VALUE");
                    }
                    continue;
                }
                if let Some(key) = trimmed.strip_prefix("get ") {
                    let key = key.trim();
                    match variables.get(key) {
                        Some(val) => println!("{val}"),
                        None => eprintln!("variable '{key}' not set"),
                    }
                    continue;
                }

                // Shell escape
                if let Some(shell_cmd) = trimmed.strip_prefix('!') {
                    let status = tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(shell_cmd)
                        .status()
                        .await;
                    match status {
                        Ok(s) if !s.success() => {
                            eprintln!("shell: exit {}", s.code().unwrap_or(-1));
                        }
                        Err(e) => eprintln!("shell: {e}"),
                        _ => {}
                    }
                    continue;
                }

                // Substitute variables in the line before parsing
                let expanded = substitute_repl_vars(trimmed, variables);
                repl_dispatch_line(session, &expanded, variables).await;
            }
        }
    }

    Ok(())
}

/// Substitute `$VAR` and `${VAR}` references in a REPL input line.
fn substitute_repl_vars(
    line: &str,
    variables: &std::collections::HashMap<String, String>,
) -> String {
    let mut result = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            if chars.peek() == Some(&'{') {
                chars.next(); // consume '{'
                let mut var_name = String::new();
                for inner in chars.by_ref() {
                    if inner == '}' {
                        break;
                    }
                    var_name.push(inner);
                }
                if let Some(val) = variables.get(&var_name) {
                    result.push_str(val);
                } else {
                    result.push_str("${");
                    result.push_str(&var_name);
                    result.push('}');
                }
            } else {
                // Bare $VAR: collect alphanumeric + underscore
                let mut var_name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        var_name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if var_name.is_empty() {
                    result.push('$');
                } else if let Some(val) = variables.get(&var_name) {
                    result.push_str(val);
                } else {
                    result.push('$');
                    result.push_str(&var_name);
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Dispatch a single REPL input line (shared by interactive and piped modes).
async fn repl_dispatch_line(
    session: &mut HeadlessSession,
    line: &str,
    _variables: &mut std::collections::HashMap<String, String>,
) {
    let tokens = script::tokenize_line(line);
    if tokens.is_empty() {
        return;
    }

    match cli::parse_commands(&tokens) {
        Ok(commands) => {
            for cmd in &commands {
                if matches!(cmd, Command::Session) {
                    eprintln!("Already in session mode.");
                    continue;
                }
                match dispatch_command(session, cmd).await {
                    Ok(()) => {}
                    Err(e) => {
                        eprintln!("error: {e:#}");
                        if let Some(dir) = FAIL_CAPTURE_DIR.get() {
                            let fail_id = next_fail_id();
                            let path = format!("{dir}/fail-{fail_id:03}.png");
                            if let Err(save_err) = capture::save_capture(session, &path, None) {
                                eprintln!("warning: failed to save failure screenshot: {save_err}");
                            } else {
                                eprintln!("failure screenshot: {path}");
                            }
                        }
                        break;
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("parse error: {e:#}");
        }
    }
}

fn init_tracing(verbose: bool) {
    use tracing_subscriber::EnvFilter;

    let filter = if verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
