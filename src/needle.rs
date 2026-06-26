use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use image::RgbaImage;
use serde::Deserialize;

/// A loaded needle: PNG image + JSON metadata.
pub(crate) struct Needle {
    pub name: String,
    pub image: RgbaImage,
    pub areas: Vec<NeedleArea>,
    pub tags: Vec<String>,
}

/// A match/exclude area within a needle.
pub(crate) struct NeedleArea {
    pub area_type: AreaType,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub tolerance: f32,
    /// Click point relative to this area (for expectclick).
    pub click_point: Option<(u32, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AreaType {
    Match,
    Exclude,
}

// --- JSON deserialization (dual-format: openQA + native) ---

#[derive(Deserialize)]
struct RawNeedle {
    areas: Vec<RawArea>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct RawArea {
    #[serde(rename = "type")]
    area_type: String,

    // Native format fields
    x: Option<u32>,
    y: Option<u32>,
    w: Option<u32>,
    h: Option<u32>,
    tolerance: Option<f32>,

    // openQA format fields
    xpos: Option<u32>,
    ypos: Option<u32>,
    width: Option<u32>,
    height: Option<u32>,
    #[serde(rename = "match")]
    match_score: Option<u32>,

    click_point: Option<RawClickPoint>,
}

#[derive(Deserialize)]
struct RawClickPoint {
    // Native
    x: Option<u32>,
    y: Option<u32>,
    // openQA
    xpos: Option<u32>,
    ypos: Option<u32>,
}

/// Load a needle from a JSON path. The PNG is expected alongside it with same stem.
pub(crate) fn load_needle(json_path: &Path) -> Result<Needle> {
    let name = json_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_owned();

    let png_path = json_path.with_extension("png");
    if !png_path.exists() {
        bail!(
            "needle PNG not found: {} (expected alongside {})",
            png_path.display(),
            json_path.display()
        );
    }

    let json_content = std::fs::read_to_string(json_path)
        .map_err(|e| anyhow::anyhow!("reading needle JSON '{}': {e}", json_path.display()))?;
    let raw: RawNeedle = serde_json::from_str(&json_content)
        .map_err(|e| anyhow::anyhow!("parsing needle JSON '{}': {e}", json_path.display()))?;

    let image = image::open(&png_path)
        .map_err(|e| anyhow::anyhow!("loading needle PNG '{}': {e}", png_path.display()))?
        .to_rgba8();

    // Detect format: if any area has xpos, it's openQA format
    let is_openqa = raw.areas.iter().any(|a| a.xpos.is_some());

    let areas = raw
        .areas
        .into_iter()
        .map(|a| parse_area(a, is_openqa))
        .collect::<Result<Vec<_>>>()?;

    Ok(Needle {
        name,
        image,
        areas,
        tags: raw.tags,
    })
}

fn parse_area(raw: RawArea, is_openqa: bool) -> Result<NeedleArea> {
    let area_type = match raw.area_type.as_str() {
        "match" => AreaType::Match,
        "exclude" => AreaType::Exclude,
        other => bail!("unknown needle area type: '{other}'"),
    };

    let (x, y, w, h, tolerance) = if is_openqa {
        let x = raw.xpos.unwrap_or(0);
        let y = raw.ypos.unwrap_or(0);
        let w = raw.width.unwrap_or(0);
        let h = raw.height.unwrap_or(0);
        // openQA match score is 0-100, convert to 0.0-1.0
        let tol = raw.match_score.map_or(0.95, |s| f32::from(s as u8) / 100.0);
        (x, y, w, h, tol)
    } else {
        let x = raw.x.unwrap_or(0);
        let y = raw.y.unwrap_or(0);
        let w = raw.w.unwrap_or(0);
        let h = raw.h.unwrap_or(0);
        let tol = raw.tolerance.unwrap_or(0.95);
        (x, y, w, h, tol)
    };

    let click_point = raw.click_point.map(|cp| {
        if is_openqa {
            (cp.xpos.unwrap_or(0), cp.ypos.unwrap_or(0))
        } else {
            (cp.x.unwrap_or(0), cp.y.unwrap_or(0))
        }
    });

    Ok(NeedleArea {
        area_type,
        x,
        y,
        w,
        h,
        tolerance,
        click_point,
    })
}

/// Load all needles from a directory, optionally filtering by tag.
pub(crate) fn load_needle_dir(dir: &Path, tag: Option<&str>) -> Result<Vec<Needle>> {
    if !dir.is_dir() {
        bail!("needle directory does not exist: {}", dir.display());
    }

    let mut json_files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    json_files.sort();

    let mut needles = Vec::new();
    for json_path in &json_files {
        let needle = load_needle(json_path)?;
        if let Some(tag_filter) = tag
            && !needle.tags.iter().any(|t| t == tag_filter)
        {
            continue;
        }
        needles.push(needle);
    }

    Ok(needles)
}

/// Match a needle against a screen image.
/// Returns the overall score (minimum across all match areas) and an optional click point.
pub(crate) fn match_needle(screen: &RgbaImage, needle: &Needle) -> NeedleMatchResult {
    let mut min_score: f32 = 1.0;
    let mut click_point: Option<(u32, u32)> = None;
    let mut all_passed = true;

    for area in &needle.areas {
        if area.area_type == AreaType::Exclude {
            continue;
        }

        // Crop the area from both needle and screen
        let (nw, nh) = needle.image.dimensions();
        let (sw, sh) = screen.dimensions();

        // Bounds check
        if area.x + area.w > nw || area.y + area.h > nh {
            all_passed = false;
            min_score = 0.0;
            continue;
        }
        if area.x + area.w > sw || area.y + area.h > sh {
            all_passed = false;
            min_score = 0.0;
            continue;
        }

        let needle_crop =
            image::imageops::crop_imm(&needle.image, area.x, area.y, area.w, area.h).to_image();

        let screen_crop =
            image::imageops::crop_imm(screen, area.x, area.y, area.w, area.h).to_image();

        let score = crate::matching::compare_full(&screen_crop, &needle_crop);
        if score < min_score {
            min_score = score;
        }
        if score < area.tolerance {
            all_passed = false;
        }

        // Use the first match area's click_point
        if click_point.is_none()
            && let Some((cx, cy)) = area.click_point
        {
            click_point = Some((area.x + cx, area.y + cy));
        }
    }

    NeedleMatchResult {
        matched: all_passed,
        score: min_score,
        needle_name: needle.name.clone(),
        click_point,
    }
}

pub(crate) struct NeedleMatchResult {
    pub matched: bool,
    pub score: f32,
    pub needle_name: String,
    pub click_point: Option<(u32, u32)>,
}

/// Try all needles against a screen, return first match.
pub(crate) fn match_needle_set(
    screen: &RgbaImage,
    needles: &[Needle],
) -> Option<NeedleMatchResult> {
    for needle in needles {
        let result = match_needle(screen, needle);
        if result.matched {
            return Some(result);
        }
    }
    None
}
