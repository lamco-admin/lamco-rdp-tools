use anyhow::{Result, bail};

/// Resolve a position specification to pixel coordinates.
///
/// Formats:
/// - Pixels: `"500,300"` -> `(500, 300)`
/// - Percentage: `"50%,50%"` -> resolved against desktop dimensions
/// - Named: `"center"`, `"top-left"`, etc. -> resolved with 5% inset margin
pub(crate) fn resolve_position(spec: &str, width: u16, height: u16) -> Result<(u16, u16)> {
    // Named positions (5% inset from edges)
    let margin_x = (u32::from(width) * 5 / 100) as u16;
    let margin_y = (u32::from(height) * 5 / 100) as u16;

    match spec {
        "center" => Ok((width / 2, height / 2)),
        "top-left" => Ok((margin_x, margin_y)),
        "top-right" => Ok((width - margin_x, margin_y)),
        "bottom-left" => Ok((margin_x, height - margin_y)),
        "bottom-right" => Ok((width - margin_x, height - margin_y)),
        "top-center" => Ok((width / 2, margin_y)),
        "bottom-center" => Ok((width / 2, height - margin_y)),
        "left-center" => Ok((margin_x, height / 2)),
        "right-center" => Ok((width - margin_x, height / 2)),
        _ => parse_numeric_position(spec, width, height),
    }
}

fn parse_numeric_position(spec: &str, width: u16, height: u16) -> Result<(u16, u16)> {
    let (x_str, y_str) = spec
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!(
            "position must be X,Y or X%,Y% or a named position (center, top-left, etc.): got '{spec}'"
        ))?;

    let x = parse_coordinate(x_str.trim(), width)?;
    let y = parse_coordinate(y_str.trim(), height)?;

    Ok((x, y))
}

fn parse_coordinate(s: &str, dimension: u16) -> Result<u16> {
    if let Some(pct_str) = s.strip_suffix('%') {
        let pct: f64 = pct_str
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid percentage '{s}': {e}"))?;
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok((f64::from(dimension) * pct / 100.0) as u16)
    } else {
        s.parse::<u16>()
            .map_err(|e| anyhow::anyhow!("invalid coordinate '{s}': {e}"))
    }
}

/// Parse a region specification: `"x,y,WxH"`.
///
/// Returns `(x, y, width, height)`.
pub(crate) fn resolve_region(
    spec: &str,
    desktop_width: u16,
    desktop_height: u16,
) -> Result<(u16, u16, u16, u16)> {
    // Format: "x,y,WxH" or "x,y,W,H"
    let parts: Vec<&str> = spec.splitn(3, ',').collect();
    if parts.len() < 3 {
        bail!("region must be 'x,y,WxH' (e.g. '100,200,640x480'): got '{spec}'");
    }

    let x = parse_coordinate(parts[0].trim(), desktop_width)?;
    let y = parse_coordinate(parts[1].trim(), desktop_height)?;

    let (w, h) = if let Some((w_str, h_str)) = parts[2].split_once('x') {
        (
            parse_coordinate(w_str.trim(), desktop_width)?,
            parse_coordinate(h_str.trim(), desktop_height)?,
        )
    } else if let Some((w_str, h_str)) = parts[2].split_once(',') {
        (
            parse_coordinate(w_str.trim(), desktop_width)?,
            parse_coordinate(h_str.trim(), desktop_height)?,
        )
    } else {
        bail!("region size must be 'WxH' or 'W,H': got '{}'", parts[2]);
    };

    Ok((x, y, w, h))
}
