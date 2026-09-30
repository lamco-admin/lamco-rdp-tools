use std::{io::Write, path::Path};

use anyhow::{Context, Result};

use crate::{coords, session::HeadlessSession};

/// Save a screenshot, optionally cropped to a region.
///
/// - If `path` is `"-"`, writes PNG to stdout.
/// - If `region` is provided, extracts a sub-image using the `"x,y,WxH"` format.
/// - Otherwise saves the full framebuffer.
pub(crate) fn save_capture(
    session: &HeadlessSession,
    path: &str,
    region: Option<&str>,
) -> Result<()> {
    let (width, height) = session.image_dimensions();

    if let Some(region_spec) = region {
        let (rx, ry, rw, rh) = coords::resolve_region(region_spec, width, height)?;
        let sub_image = session.capture_region(rx, ry, rw, rh)?;

        if path == "-" {
            let mut stdout = std::io::stdout().lock();
            let encoder = image::codecs::png::PngEncoder::new(&mut stdout);
            image::ImageEncoder::write_image(
                encoder,
                sub_image.as_raw(),
                u32::from(rw),
                u32::from(rh),
                image::ExtendedColorType::Rgba8,
            )
            .context("encode PNG to stdout")?;
            stdout.flush().context("flush stdout")?;
        } else {
            sub_image
                .save(path)
                .with_context(|| format!("save region capture to {path}"))?;
        }
    } else if path == "-" {
        let mut stdout = std::io::stdout().lock();
        session.write_screenshot_to(&mut stdout)?;
        stdout.flush().context("flush stdout")?;
    } else {
        session.save_screenshot(Path::new(path))?;
        warn_if_uniform(session, path);
    }

    Ok(())
}

/// Warn when a saved capture is one flat colour.
///
/// Frames can arrive and still carry nothing: a compositor whose screen has
/// blanked sends real, well-formed, entirely black frames. That passes a
/// frame-count check and produces a file a test will happily compare against,
/// so say so on stderr. It is a warning rather than an error because a
/// legitimately uniform screen exists.
fn warn_if_uniform(session: &HeadlessSession, path: &str) {
    let frame = session.current_frame();
    let mut pixels = frame.pixels();
    let Some(first) = pixels.next() else {
        return;
    };
    if pixels.all(|p| p == first) {
        let [r, g, b, _] = first.0;
        tracing::warn!(
            "capture {path} is a single colour (#{r:02x}{g:02x}{b:02x}) across {}x{}: the remote screen may be blanked",
            frame.width(),
            frame.height()
        );
    }
}
