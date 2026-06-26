use anyhow::{Context, Result};

use crate::cli::{self, Command};

/// Script variable context for substitution.
pub(crate) struct ScriptVars {
    pub server: String,
    pub width: u16,
    pub height: u16,
}

/// Parse a .rdpdo-script file into a flat sequence of commands.
///
/// Format:
/// - One command per line (same syntax as CLI positional args)
/// - Lines starting with `#` are comments
/// - Blank lines are ignored
/// - Double-quoted strings preserve internal whitespace
/// - Variables: `{env:VAR}`, `{width}`, `{height}`, `{server}`
pub(crate) fn parse_script(path: &str) -> Result<Vec<Command>> {
    parse_script_with_vars(path, None)
}

/// Parse script with variable substitution context.
pub(crate) fn parse_script_with_vars(
    path: &str,
    vars: Option<&ScriptVars>,
) -> Result<Vec<Command>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading script '{path}'"))?;

    let mut all_commands = Vec::new();

    for (line_num, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let expanded = if line.contains('{') {
            substitute_vars(line, vars)
        } else {
            line.to_owned()
        };

        let tokens = tokenize_line(&expanded);
        if tokens.is_empty() {
            continue;
        }

        let cmds = cli::parse_commands(&tokens)
            .with_context(|| format!("{path}:{}: {line}", line_num + 1))?;

        all_commands.extend(cmds);
    }

    Ok(all_commands)
}

/// Expand `{env:VAR}`, `{width}`, `{height}`, `{server}` in a line.
fn substitute_vars(line: &str, vars: Option<&ScriptVars>) -> String {
    let mut result = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '{' {
            // Collect until closing brace
            let mut var_name = String::new();
            let mut found_close = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    found_close = true;
                    break;
                }
                var_name.push(inner);
            }

            if !found_close {
                // Unterminated brace, emit literally
                result.push('{');
                result.push_str(&var_name);
                continue;
            }

            // Resolve the variable
            if let Some(env_var) = var_name.strip_prefix("env:") {
                if let Ok(val) = std::env::var(env_var) {
                    result.push_str(&val);
                } else {
                    result.push_str("{env:");
                    result.push_str(env_var);
                    result.push('}');
                }
            } else if let Some(v) = vars {
                match var_name.as_str() {
                    "width" => result.push_str(&v.width.to_string()),
                    "height" => result.push_str(&v.height.to_string()),
                    "server" => result.push_str(&v.server),
                    _ => {
                        result.push('{');
                        result.push_str(&var_name);
                        result.push('}');
                    }
                }
            } else {
                // No vars context, leave as-is
                result.push('{');
                result.push_str(&var_name);
                result.push('}');
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Split a line into tokens, respecting double-quoted strings.
/// `type "hello world"` becomes `["type", "hello world"]`.
pub(crate) fn tokenize_line(line: &str) -> Vec<String> {
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
