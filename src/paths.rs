//! Per-user configuration directory for rdpdo data (profiles, calibration,
//! baselines, history).

use std::path::PathBuf;

/// Base configuration directory: `~/.config` on macOS, so it matches the
/// documented Linux paths. Elsewhere, `dirs::config_dir()` (`~/.config` on
/// Linux, `%APPDATA%` on Windows).
#[cfg(target_os = "macos")]
pub(crate) fn config_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".config"))
}

/// Base configuration directory from `dirs::config_dir()`.
#[cfg(not(target_os = "macos"))]
pub(crate) fn config_dir() -> Option<PathBuf> {
    dirs::config_dir()
}
