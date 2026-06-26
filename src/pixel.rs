use anyhow::{Result, bail};
use image::RgbaImage;

/// A color threshold for pixel matching.
/// Each channel has a target value and a tolerance (max absolute difference).
pub(crate) struct ColorThreshold {
    pub r: (u8, u8), // (min, max)
    pub g: (u8, u8),
    pub b: (u8, u8),
}

impl ColorThreshold {
    /// Create from a hex color string and tolerance.
    /// e.g. "#00ff41" with tolerance 30 → R: 0±30, G: 255±30, B: 65±30
    pub(crate) fn from_hex(hex: &str, tolerance: u8) -> Option<Self> {
        let hex = hex.trim_start_matches('#');
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Self {
            r: (r.saturating_sub(tolerance), r.saturating_add(tolerance)),
            g: (g.saturating_sub(tolerance), g.saturating_add(tolerance)),
            b: (b.saturating_sub(tolerance), b.saturating_add(tolerance)),
        })
    }

    /// Green marker threshold for calibration: R < 80, G > 180, B < 120
    pub(crate) fn green_marker() -> Self {
        Self {
            r: (0, 80),
            g: (180, 255),
            b: (0, 120),
        }
    }

    fn matches(&self, r: u8, g: u8, b: u8) -> bool {
        r >= self.r.0
            && r <= self.r.1
            && g >= self.g.0
            && g <= self.g.1
            && b >= self.b.0
            && b <= self.b.1
    }
}

/// A detected color cluster with centroid and pixel count.
#[derive(Debug)]
pub(crate) struct ColorCluster {
    pub cx: f64,
    pub cy: f64,
    pub area: u32,
    pub min_x: u32,
    pub min_y: u32,
    pub max_x: u32,
    pub max_y: u32,
}

/// Find all pixels matching a color threshold in a region.
/// Returns a binary mask (true = match) and the matching pixel count.
pub(crate) fn build_color_mask(
    image: &RgbaImage,
    threshold: &ColorThreshold,
    region: Option<(u32, u32, u32, u32)>, // (x, y, w, h)
) -> (Vec<Vec<bool>>, u32) {
    let (img_w, img_h) = image.dimensions();
    let (rx, ry, rw, rh) = region.unwrap_or((0, 0, img_w, img_h));

    let mut mask = vec![vec![false; rw as usize]; rh as usize];
    let mut count = 0;

    for dy in 0..rh {
        for dx in 0..rw {
            let px = rx + dx;
            let py = ry + dy;
            if px >= img_w || py >= img_h {
                continue;
            }
            let pixel = image.get_pixel(px, py);
            if threshold.matches(pixel[0], pixel[1], pixel[2]) {
                mask[dy as usize][dx as usize] = true;
                count += 1;
            }
        }
    }

    (mask, count)
}

/// Connected-component labeling with 4-connectivity flood fill.
/// Returns clusters with area >= `min_area`, sorted by area descending.
pub(crate) fn find_clusters(
    mask: &[Vec<bool>],
    offset_x: u32,
    offset_y: u32,
    min_area: u32,
) -> Vec<ColorCluster> {
    let h = mask.len();
    if h == 0 {
        return Vec::new();
    }
    let w = mask[0].len();
    let mut visited = vec![vec![false; w]; h];
    let mut clusters = Vec::new();

    for start_y in 0..h {
        for start_x in 0..w {
            if !mask[start_y][start_x] || visited[start_y][start_x] {
                continue;
            }

            // Flood fill from this pixel
            let mut stack = vec![(start_x, start_y)];
            let mut sum_x: u64 = 0;
            let mut sum_y: u64 = 0;
            let mut area: u32 = 0;
            let mut min_x = start_x as u32;
            let mut min_y = start_y as u32;
            let mut max_x = start_x as u32;
            let mut max_y = start_y as u32;

            while let Some((cx, cy)) = stack.pop() {
                if visited[cy][cx] {
                    continue;
                }
                visited[cy][cx] = true;

                sum_x += cx as u64;
                sum_y += cy as u64;
                area += 1;
                min_x = min_x.min(cx as u32);
                min_y = min_y.min(cy as u32);
                max_x = max_x.max(cx as u32);
                max_y = max_y.max(cy as u32);

                // 4-connectivity neighbors
                if cx > 0 && mask[cy][cx - 1] && !visited[cy][cx - 1] {
                    stack.push((cx - 1, cy));
                }
                if cx + 1 < w && mask[cy][cx + 1] && !visited[cy][cx + 1] {
                    stack.push((cx + 1, cy));
                }
                if cy > 0 && mask[cy - 1][cx] && !visited[cy - 1][cx] {
                    stack.push((cx, cy - 1));
                }
                if cy + 1 < h && mask[cy + 1][cx] && !visited[cy + 1][cx] {
                    stack.push((cx, cy + 1));
                }
            }

            if area >= min_area {
                clusters.push(ColorCluster {
                    cx: sum_x as f64 / f64::from(area) + f64::from(offset_x),
                    cy: sum_y as f64 / f64::from(area) + f64::from(offset_y),
                    area,
                    min_x: min_x + offset_x,
                    min_y: min_y + offset_y,
                    max_x: max_x + offset_x,
                    max_y: max_y + offset_y,
                });
            }
        }
    }

    clusters.sort_by_key(|c| std::cmp::Reverse(c.area));
    clusters
}

/// Parse a color string: "#RRGGBB" or "R,G,B".
pub(crate) fn parse_color(s: &str) -> Result<[u8; 3]> {
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 {
            bail!("invalid hex color '{s}' (expected #RRGGBB)");
        }
        let r = u8::from_str_radix(&hex[0..2], 16)
            .map_err(|_| anyhow::anyhow!("invalid hex color '{s}'"))?;
        let g = u8::from_str_radix(&hex[2..4], 16)
            .map_err(|_| anyhow::anyhow!("invalid hex color '{s}'"))?;
        let b = u8::from_str_radix(&hex[4..6], 16)
            .map_err(|_| anyhow::anyhow!("invalid hex color '{s}'"))?;
        Ok([r, g, b])
    } else if s.contains(',') {
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() != 3 {
            bail!("invalid color '{s}' (expected R,G,B or #RRGGBB)");
        }
        let r: u8 = parts[0]
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid red value in '{s}'"))?;
        let g: u8 = parts[1]
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid green value in '{s}'"))?;
        let b: u8 = parts[2]
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid blue value in '{s}'"))?;
        Ok([r, g, b])
    } else {
        bail!("invalid color '{s}' (expected #RRGGBB or R,G,B)");
    }
}

/// Read the pixel at (x,y) from an image. Returns [R,G,B].
pub(crate) fn get_pixel(image: &RgbaImage, px: u32, py: u32) -> [u8; 3] {
    let (img_w, img_h) = image.dimensions();
    if px >= img_w || py >= img_h {
        return [0, 0, 0];
    }
    let pixel = image.get_pixel(px, py);
    [pixel[0], pixel[1], pixel[2]]
}

/// Check if two colors match within per-channel tolerance.
pub(crate) fn colors_match(actual: [u8; 3], expected: [u8; 3], tolerance: u8) -> bool {
    let tol = i16::from(tolerance);
    (i16::from(actual[0]) - i16::from(expected[0])).abs() <= tol
        && (i16::from(actual[1]) - i16::from(expected[1])).abs() <= tol
        && (i16::from(actual[2]) - i16::from(expected[2])).abs() <= tol
}

/// Find all clusters of a given color in an image region.
/// Combines mask building and connected-component labeling.
pub(crate) fn find_color(
    image: &RgbaImage,
    threshold: &ColorThreshold,
    region: Option<(u32, u32, u32, u32)>,
    min_area: u32,
) -> Vec<ColorCluster> {
    let (rx, ry, _, _) = region.unwrap_or((0, 0, 0, 0));
    let (mask, count) = build_color_mask(image, threshold, region);
    if count == 0 {
        return Vec::new();
    }
    find_clusters(&mask, rx, ry, min_area)
}
