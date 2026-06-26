use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{pixel, session::HeadlessSession};

const CALIBRATION_HTML: &str = include_str!("../profiles/calibration.html");

/// Calibration profile saved to disk.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CalibrationProfile {
    pub version: u32,
    pub server: String,
    pub timestamp: String,
    pub session: SessionInfo,
    pub grid: GridSpec,
    pub points: Vec<CalibrationPoint>,
    pub correction: CorrectionResult,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct SessionInfo {
    pub rdp_width: u16,
    pub rdp_height: u16,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct GridSpec {
    pub cols: u32,
    pub rows: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CalibrationPoint {
    pub target_x: u16,
    pub target_y: u16,
    pub actual_x: f64,
    pub actual_y: f64,
    pub error_x: f64,
    pub error_y: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CorrectionResult {
    pub offset_x: f64,
    pub offset_y: f64,
    pub max_error: f64,
    pub avg_error: f64,
    pub point_count: u32,
}

/// Parse a grid spec like "4x4" into (cols, rows).
pub(crate) fn parse_grid(spec: &str) -> Result<(u32, u32)> {
    let (c, r) = spec
        .split_once('x')
        .or_else(|| spec.split_once('X'))
        .ok_or_else(|| anyhow::anyhow!("grid must be COLSxROWS (e.g. 4x4)"))?;
    let cols: u32 = c
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid grid cols: {e}"))?;
    let rows: u32 = r
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid grid rows: {e}"))?;
    if cols == 0 || rows == 0 {
        bail!("grid dimensions must be > 0");
    }
    Ok((cols, rows))
}

/// Deploy calibration page via clipboard method.
///
/// Sets the HTML content to clipboard, then types a shell command to
/// save it to a file and open in a browser fullscreen.
pub(crate) async fn deploy_via_clipboard(session: &mut HeadlessSession) -> Result<()> {
    // Set clipboard to calibration HTML
    session.set_clipboard(CALIBRATION_HTML).await?;
    session.run_for(Duration::from_millis(500)).await?;

    // Type command to save clipboard to file and open in browser
    // Works with wl-paste (Wayland) or xclip (X11)
    let cmd = "bash -c 'wl-paste > /tmp/rdpdo-cal.html 2>/dev/null || \
               xclip -selection clipboard -o > /tmp/rdpdo-cal.html; \
               xdg-open /tmp/rdpdo-cal.html' &";

    // Open a terminal first (most desktops have a terminal shortcut)
    // We'll try the generic approach: open a run dialog or terminal
    info!("Deploying calibration page via clipboard");

    // Use keyboard shortcut to open terminal (compositor-dependent)
    // For now, assume a terminal is already open or use a generic approach
    session.send_text(cmd).await?;
    session.send_key_combo("enter").await?;

    // Wait for browser to open and page to render
    info!("Waiting for calibration page to load...");
    session.run_for(Duration::from_secs(3)).await?;

    // Press F11 for fullscreen
    session.send_key_combo("f11").await?;
    session.run_for(Duration::from_secs(1)).await?;

    Ok(())
}

/// Run the calibration grid: click each point, detect green markers.
pub(crate) async fn run_calibration_grid(
    session: &mut HeadlessSession,
    cols: u32,
    rows: u32,
) -> Result<Vec<CalibrationPoint>> {
    let (width, height) = session.image_dimensions();
    let mut points = Vec::new();
    let green = pixel::ColorThreshold::green_marker();

    // Take baseline screenshot (before any clicks)
    let baseline = session.current_frame();
    let (baseline_mask, _) = pixel::build_color_mask(&baseline, &green, None);
    let baseline_clusters = pixel::find_clusters(&baseline_mask, 0, 0, 3);

    for r in 0..rows {
        for c in 0..cols {
            // Target is center of each grid cell
            let target_x = (u32::from(width) * (2 * c + 1) / (2 * cols)) as u16;
            let target_y = (u32::from(height) * (2 * r + 1) / (2 * rows)) as u16;

            info!(target_x, target_y, col = c, row = r, "Clicking grid point");

            // Click at target
            session.mouse_move(target_x, target_y).await?;
            session.mouse_click("left").await?;

            // Wait for page to render the green marker
            session.run_for(Duration::from_millis(300)).await?;

            // Screenshot and detect new green markers
            let after = session.current_frame();
            let clusters = pixel::find_color(&after, &green, None, 3);

            // Find the new cluster (not in baseline)
            let new_marker = find_new_cluster(&clusters, &baseline_clusters, 10.0);

            let (actual_x, actual_y) = if let Some(cluster) = new_marker {
                (cluster.cx, cluster.cy)
            } else {
                info!("No new marker detected for point ({target_x},{target_y})");
                (f64::from(target_x), f64::from(target_y))
            };

            let error_x = actual_x - f64::from(target_x);
            let error_y = actual_y - f64::from(target_y);

            info!(
                actual_x = format!("{actual_x:.1}"),
                actual_y = format!("{actual_y:.1}"),
                error_x = format!("{error_x:.1}"),
                error_y = format!("{error_y:.1}"),
                "Marker detected"
            );

            points.push(CalibrationPoint {
                target_x,
                target_y,
                actual_x,
                actual_y,
                error_x,
                error_y,
            });
        }
    }

    Ok(points)
}

/// Find a cluster in `current` that wasn't present in `baseline`.
fn find_new_cluster<'a>(
    current: &'a [pixel::ColorCluster],
    baseline: &[pixel::ColorCluster],
    merge_distance: f64,
) -> Option<&'a pixel::ColorCluster> {
    'outer: for cluster in current {
        for base in baseline {
            let dx = cluster.cx - base.cx;
            let dy = cluster.cy - base.cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist < merge_distance {
                continue 'outer;
            }
        }
        return Some(cluster);
    }
    None
}

/// Compute uniform offset correction from calibration points.
pub(crate) fn compute_correction(points: &[CalibrationPoint]) -> CorrectionResult {
    if points.is_empty() {
        return CorrectionResult {
            offset_x: 0.0,
            offset_y: 0.0,
            max_error: 0.0,
            avg_error: 0.0,
            point_count: 0,
        };
    }

    let n = points.len() as f64;
    let offset_x: f64 = points.iter().map(|p| p.error_x).sum::<f64>() / n;
    let offset_y: f64 = points.iter().map(|p| p.error_y).sum::<f64>() / n;

    let errors: Vec<f64> = points
        .iter()
        .map(|p| {
            let dx = p.error_x - offset_x;
            let dy = p.error_y - offset_y;
            (dx * dx + dy * dy).sqrt()
        })
        .collect();

    let max_error = errors.iter().copied().fold(0.0_f64, f64::max);
    let avg_error = errors.iter().sum::<f64>() / n;

    CorrectionResult {
        offset_x,
        offset_y,
        max_error,
        avg_error,
        point_count: points.len() as u32,
    }
}

/// Build a full calibration profile.
pub(crate) fn build_profile(
    server: &str,
    width: u16,
    height: u16,
    cols: u32,
    rows: u32,
    points: Vec<CalibrationPoint>,
) -> CalibrationProfile {
    let correction = compute_correction(&points);
    CalibrationProfile {
        version: 1,
        server: server.to_owned(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        session: SessionInfo {
            rdp_width: width,
            rdp_height: height,
        },
        grid: GridSpec { cols, rows },
        points,
        correction,
    }
}

/// Save calibration profile to the auto-discovery location or a custom path.
pub(crate) fn save_profile(profile: &CalibrationProfile, output: Option<&str>) -> Result<String> {
    let path = if let Some(p) = output {
        p.to_owned()
    } else {
        // Auto-discovery: ~/.config/rdpdo/calibration/<server>-<WxH>.json
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine config directory"))?;
        let cal_dir = config_dir.join("rdpdo").join("calibration");
        std::fs::create_dir_all(&cal_dir)?;
        let server_safe = profile.server.replace(':', "-").replace('/', "_");
        cal_dir
            .join(format!(
                "{}-{}x{}.json",
                server_safe, profile.session.rdp_width, profile.session.rdp_height
            ))
            .to_string_lossy()
            .into_owned()
    };

    let json = serde_json::to_string_pretty(profile)?;
    std::fs::write(&path, &json)?;
    Ok(path)
}

/// Load a calibration profile from a path or auto-discover.
pub(crate) fn load_profile(
    path_or_auto: &str,
    server: &str,
    width: u16,
    height: u16,
) -> Result<CalibrationProfile> {
    let path = if path_or_auto == "auto" {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine config directory"))?;
        let server_safe = server.replace(':', "-").replace('/', "_");
        let cal_path = config_dir
            .join("rdpdo")
            .join("calibration")
            .join(format!("{server_safe}-{width}x{height}.json"));
        if !cal_path.exists() {
            bail!(
                "no calibration profile found for {server} at {width}x{height} (looked at {})",
                cal_path.display()
            );
        }
        cal_path.to_string_lossy().into_owned()
    } else {
        path_or_auto.to_owned()
    };

    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("reading calibration profile '{path}': {e}"))?;
    let profile: CalibrationProfile = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("parsing calibration profile '{path}': {e}"))?;

    // Resolution mismatch warning
    if profile.session.rdp_width != width || profile.session.rdp_height != height {
        eprintln!(
            "WARNING: Calibration profile recorded at {}x{} but current session is {width}x{height}",
            profile.session.rdp_width, profile.session.rdp_height
        );
        eprintln!("         Re-run 'rdpdo calibrate' for accurate correction at this resolution");
    }

    Ok(profile)
}

/// Apply calibration correction to coordinates.
pub(crate) fn apply_correction(x: u16, y: u16, profile: &CalibrationProfile) -> (u16, u16) {
    let corrected_x = f64::from(x) - profile.correction.offset_x;
    let corrected_y = f64::from(y) - profile.correction.offset_y;
    // Clamp to valid range
    let cx = corrected_x.round().max(0.0) as u16;
    let cy = corrected_y.round().max(0.0) as u16;
    (cx, cy)
}
