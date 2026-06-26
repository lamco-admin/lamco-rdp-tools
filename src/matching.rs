use image::RgbaImage;
use imageproc::template_matching::{self, MatchTemplateMethod};

/// Result of a template search within a screen image.
pub(crate) struct MatchResult {
    /// NCC score between 0.0 (no match) and 1.0 (perfect match).
    pub score: f32,
    /// (x, y) pixel coordinate of the best match's top-left corner.
    pub location: (u32, u32),
    /// (width, height) of the template that was searched for.
    pub template_size: (u32, u32),
}

/// Compare two same-sized images using Pearson correlation on their grayscale values.
/// Returns a score from -1.0 to 1.0 (1.0 = identical).
pub(crate) fn compare_full(screen: &RgbaImage, reference: &RgbaImage) -> f32 {
    let a = image::DynamicImage::ImageRgba8(screen.clone()).into_luma8();
    let b = image::DynamicImage::ImageRgba8(reference.clone()).into_luma8();

    if a.dimensions() != b.dimensions() {
        return 0.0;
    }

    pearson_correlation(a.as_raw(), b.as_raw())
}

/// Search for a template anywhere in the screen using normalized cross-correlation.
/// Template must be strictly smaller than the screen in both dimensions.
pub(crate) fn find_template(screen: &RgbaImage, template: &RgbaImage) -> MatchResult {
    let screen_gray = image::DynamicImage::ImageRgba8(screen.clone()).into_luma8();
    let tmpl_gray = image::DynamicImage::ImageRgba8(template.clone()).into_luma8();

    let (sw, sh) = screen_gray.dimensions();
    let (tw, th) = tmpl_gray.dimensions();

    // imageproc requires template strictly smaller
    if tw >= sw || th >= sh {
        let score = compare_full(screen, template);
        return MatchResult {
            score,
            location: (0, 0),
            template_size: (tw, th),
        };
    }

    let result = template_matching::match_template(
        &screen_gray,
        &tmpl_gray,
        MatchTemplateMethod::CrossCorrelationNormalized,
    );

    let extremes = template_matching::find_extremes(&result);

    MatchResult {
        score: extremes.max_value,
        location: (extremes.max_value_location.0, extremes.max_value_location.1),
        template_size: (tw, th),
    }
}

/// Compare a rectangular region of the screen against a reference image.
pub(crate) fn compare_region(
    screen: &RgbaImage,
    region: (u32, u32, u32, u32),
    reference: &RgbaImage,
) -> f32 {
    let (x, y, w, h) = region;
    let sub = image::imageops::crop_imm(screen, x, y, w, h).to_image();
    compare_full(&sub, reference)
}

/// Generate a visual diff image: red overlay on pixels that differ beyond a threshold.
pub(crate) fn diff_images(a: &RgbaImage, b: &RgbaImage) -> RgbaImage {
    let (w, h) = a.dimensions();
    let (bw, bh) = b.dimensions();
    let out_w = w.min(bw);
    let out_h = h.min(bh);

    let mut diff = RgbaImage::new(out_w, out_h);

    for y in 0..out_h {
        for x in 0..out_w {
            let pa = a.get_pixel(x, y);
            let pb = b.get_pixel(x, y);

            let dr = pa[0].abs_diff(pb[0]);
            let dg = pa[1].abs_diff(pb[1]);
            let db = pa[2].abs_diff(pb[2]);

            // If any channel differs by more than a small amount, mark red
            if dr > 8 || dg > 8 || db > 8 {
                diff.put_pixel(x, y, image::Rgba([255, 0, 0, 200]));
            } else {
                // Dimmed original pixel to make red overlay stand out
                diff.put_pixel(x, y, image::Rgba([pa[0] / 2, pa[1] / 2, pa[2] / 2, 255]));
            }
        }
    }

    diff
}

/// Pearson correlation coefficient between two equal-length byte slices.
/// Returns 0.0 for empty or constant inputs.
fn pearson_correlation(a: &[u8], b: &[u8]) -> f32 {
    let n = a.len();
    if n == 0 {
        return 0.0;
    }

    // Accumulate sums for Pearson coefficient: r = (N*Σxy - Σx*Σy) / sqrt((N*Σx²-Σx²)(N*Σy²-Σy²))
    let mut total_left: f64 = 0.0;
    let mut total_right: f64 = 0.0;
    let mut cross_product: f64 = 0.0;
    let mut sq_left: f64 = 0.0;
    let mut sq_right: f64 = 0.0;

    for i in 0..n {
        let va = f64::from(a[i]);
        let vb = f64::from(b[i]);
        total_left += va;
        total_right += vb;
        cross_product += va * vb;
        sq_left += va * va;
        sq_right += vb * vb;
    }

    #[expect(clippy::cast_precision_loss)]
    let nf = n as f64;
    let numerator = nf * cross_product - total_left * total_right;
    let denominator = ((nf * sq_left - total_left * total_left)
        * (nf * sq_right - total_right * total_right))
        .sqrt();

    if denominator < f64::EPSILON {
        return 0.0;
    }

    (numerator / denominator) as f32
}

/// Count pixels that differ between two images beyond the given per-channel threshold.
pub(crate) fn count_different_pixels(a: &RgbaImage, b: &RgbaImage, threshold: u8) -> u64 {
    let (w, h) = a.dimensions();
    let (bw, bh) = b.dimensions();
    let out_w = w.min(bw);
    let out_h = h.min(bh);
    let mut count: u64 = 0;

    for y in 0..out_h {
        for x in 0..out_w {
            let pa = a.get_pixel(x, y);
            let pb = b.get_pixel(x, y);
            if pa[0].abs_diff(pb[0]) > threshold
                || pa[1].abs_diff(pb[1]) > threshold
                || pa[2].abs_diff(pb[2]) > threshold
            {
                count += 1;
            }
        }
    }

    count
}

/// Place images A and B side by side with a 2px separator.
pub(crate) fn side_by_side(a: &RgbaImage, b: &RgbaImage) -> RgbaImage {
    let (aw, ah) = a.dimensions();
    let (bw, bh) = b.dimensions();
    let sep = 2;
    let out_w = aw + sep + bw;
    let out_h = ah.max(bh);
    let mut out = RgbaImage::from_pixel(out_w, out_h, image::Rgba([40, 40, 40, 255]));

    // Copy image A
    for y in 0..ah {
        for x in 0..aw {
            out.put_pixel(x, y, *a.get_pixel(x, y));
        }
    }
    // Copy image B
    for y in 0..bh {
        for x in 0..bw {
            out.put_pixel(aw + sep + x, y, *b.get_pixel(x, y));
        }
    }

    out
}

/// Heatmap diff: per-pixel change magnitude mapped to blue-red color gradient.
#[expect(clippy::many_single_char_names)]
pub(crate) fn heatmap_diff(a: &RgbaImage, b: &RgbaImage) -> RgbaImage {
    let (w, h) = a.dimensions();
    let (bw, bh) = b.dimensions();
    let out_w = w.min(bw);
    let out_h = h.min(bh);
    let mut out = RgbaImage::new(out_w, out_h);

    for y in 0..out_h {
        for x in 0..out_w {
            let pa = a.get_pixel(x, y);
            let pb = b.get_pixel(x, y);

            let dr = u16::from(pa[0].abs_diff(pb[0]));
            let dg = u16::from(pa[1].abs_diff(pb[1]));
            let db = u16::from(pa[2].abs_diff(pb[2]));

            // Max channel difference as intensity (0-255)
            let intensity = dr.max(dg).max(db).min(255) as u8;

            if intensity < 8 {
                // Below threshold: dimmed original
                out.put_pixel(x, y, image::Rgba([pa[0] / 3, pa[1] / 3, pa[2] / 3, 255]));
            } else {
                // Map intensity to blue(cold) -> yellow -> red(hot)
                let t = f32::from(intensity) / 255.0;
                let r = (t * 2.0).min(1.0);
                let g = if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 };
                let b = (1.0 - t * 2.0).max(0.0);
                #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let pixel =
                    image::Rgba([(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]);
                out.put_pixel(x, y, pixel);
            }
        }
    }

    out
}
