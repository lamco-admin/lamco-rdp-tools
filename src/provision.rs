use std::collections::HashMap;

use anyhow::Result;

use crate::cli::{self, Command};

// Built-in accept-portal profiles
const PORTAL_GNOME49: &str = include_str!("../profiles/accept-portal/gnome49.rdpdo-script");
const PORTAL_GNOME46: &str = include_str!("../profiles/accept-portal/gnome46.rdpdo-script");
const PORTAL_KDE: &str = include_str!("../profiles/accept-portal/kde.rdpdo-script");
const PORTAL_SWAY: &str = include_str!("../profiles/accept-portal/sway.rdpdo-script");
const PORTAL_HYPRLAND: &str = include_str!("../profiles/accept-portal/hyprland.rdpdo-script");
const PORTAL_NIRI: &str = include_str!("../profiles/accept-portal/niri.rdpdo-script");
const PORTAL_COSMIC: &str = include_str!("../profiles/accept-portal/cosmic.rdpdo-script");

// Built-in unlock profiles
const UNLOCK_DEFAULT: &str = include_str!("../profiles/unlock/default.rdpdo-script");
const UNLOCK_KDE: &str = include_str!("../profiles/unlock/kde.rdpdo-script");

// Built-in login profiles
const LOGIN_DEFAULT: &str = include_str!("../profiles/login/default.rdpdo-script");
const LOGIN_DOMAIN: &str = include_str!("../profiles/login/domain.rdpdo-script");
const LOGIN_WINDOWS: &str = include_str!("../profiles/login/windows.rdpdo-script");

/// Resolve a profile script: check user overrides first, then built-in defaults.
///
/// User profiles: `~/.config/rdpdo/profiles/{category}/{name}.rdpdo-script`
/// Built-in profiles: embedded at compile time from `profiles/` directory.
fn resolve_profile(category: &str, name: &str, custom_path: Option<&str>) -> Result<String> {
    // Custom path takes priority over everything
    if let Some(path) = custom_path {
        return std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading profile '{path}': {e}"));
    }

    // Check user override directory
    if let Some(mut config_dir) = dirs::config_dir() {
        config_dir.push("rdpdo");
        config_dir.push("profiles");
        config_dir.push(category);
        config_dir.push(format!("{name}.rdpdo-script"));
        if config_dir.exists() {
            return std::fs::read_to_string(&config_dir).map_err(|e| {
                anyhow::anyhow!("reading user profile '{}': {e}", config_dir.display())
            });
        }
    }

    // Fall back to built-in profiles
    let builtin = match (category, name) {
        ("accept-portal", "gnome49") => Some(PORTAL_GNOME49),
        ("accept-portal", "gnome46" | "gnome") => Some(PORTAL_GNOME46),
        ("accept-portal", "kde" | "kwin" | "kwin_wayland") => Some(PORTAL_KDE),
        ("accept-portal", "sway") => Some(PORTAL_SWAY),
        ("accept-portal", "hyprland") => Some(PORTAL_HYPRLAND),
        ("accept-portal", "niri") => Some(PORTAL_NIRI),
        ("accept-portal", "cosmic" | "cosmic-comp") => Some(PORTAL_COSMIC),
        ("unlock", "kde" | "kwin" | "kwin_wayland") => Some(UNLOCK_KDE),
        ("unlock", _) => Some(UNLOCK_DEFAULT),
        ("login", "domain") => Some(LOGIN_DOMAIN),
        ("login", "windows") => Some(LOGIN_WINDOWS),
        ("login", "default" | _) if category == "login" => Some(LOGIN_DEFAULT),
        _ => None,
    };

    builtin
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("no built-in profile for {category}/{name}"))
}

/// Replace `{placeholder}` tokens in a script with their values.
fn substitute_placeholders(script: &str, vars: &HashMap<&str, &str>) -> String {
    let mut result = script.to_owned();
    for (key, value) in vars {
        let placeholder = format!("{{{key}}}");
        result = result.replace(&placeholder, value);
    }
    result
}

/// Parse a profile script (with placeholders already substituted) into commands.
fn parse_profile_script(content: &str) -> Result<Vec<Command>> {
    let mut all_commands = Vec::new();

    for (line_num, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let tokens = tokenize_line(line);
        if tokens.is_empty() {
            continue;
        }

        let cmds = cli::parse_commands(&tokens)
            .map_err(|e| anyhow::anyhow!("profile line {}: {e}", line_num + 1))?;

        all_commands.extend(cmds);
    }

    Ok(all_commands)
}

/// Split a line into tokens, respecting double-quoted strings.
fn tokenize_line(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => {
                current.push(ch);
            }
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Resolve and parse an accept-portal profile into executable commands.
pub(crate) fn accept_portal_commands(
    compositor: &str,
    custom_profile: Option<&str>,
) -> Result<Vec<Command>> {
    let script = resolve_profile("accept-portal", compositor, custom_profile)?;
    // No placeholders needed for accept-portal
    parse_profile_script(&script)
}

/// Resolve and parse an unlock profile into executable commands.
pub(crate) fn unlock_commands(
    compositor: &str,
    password: &str,
    custom_profile: Option<&str>,
) -> Result<Vec<Command>> {
    let script = resolve_profile("unlock", compositor, custom_profile)?;
    let mut vars = HashMap::new();
    vars.insert("password", password);
    let substituted = substitute_placeholders(&script, &vars);
    parse_profile_script(&substituted)
}

/// Resolve and parse a login profile into executable commands.
pub(crate) fn login_commands(
    profile_name: &str,
    username: &str,
    password: &str,
    domain: Option<&str>,
    custom_profile: Option<&str>,
) -> Result<Vec<Command>> {
    let script = resolve_profile("login", profile_name, custom_profile)?;
    let mut vars = HashMap::new();
    vars.insert("username", username);
    vars.insert("password", password);
    if let Some(d) = domain {
        vars.insert("domain", d);
    }
    let substituted = substitute_placeholders(&script, &vars);
    parse_profile_script(&substituted)
}

/// List available built-in profiles for a category.
pub(crate) fn list_builtin_profiles(category: &str) -> Vec<&'static str> {
    match category {
        "accept-portal" => vec![
            "gnome49", "gnome46", "kde", "sway", "hyprland", "niri", "cosmic",
        ],
        "unlock" => vec!["default", "kde"],
        "login" => vec!["default", "domain", "windows"],
        _ => vec![],
    }
}
