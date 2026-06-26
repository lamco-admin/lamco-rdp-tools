use std::collections::HashMap;

use anyhow::{Result, bail};
use ironrdp_input::{Database, MouseButton, MousePosition, Operation, Scancode, WheelRotations};
use ironrdp_pdu::input::fast_path::FastPathInputEvent;
use smallvec::SmallVec;

/// Wraps `ironrdp_input::Database` to generate `FastPathInputEvent` sequences
/// for keyboard and mouse injection.
pub(crate) struct InputInjector {
    db: Database,
}

/// Result of parsing a key spec like "ctrl-c" or "enter".
pub(crate) struct KeyAction {
    pub modifiers: Vec<Scancode>,
    pub key: Scancode,
}

impl InputInjector {
    pub(crate) fn new() -> Self {
        Self {
            db: Database::new(),
        }
    }

    /// Press and release a single scancode, returning the resulting events.
    pub(crate) fn type_scancode(&mut self, scancode: u16) -> SmallVec<[FastPathInputEvent; 4]> {
        let sc = Scancode::from_u16(scancode);
        let mut events = SmallVec::new();
        events.extend(self.db.apply([Operation::KeyPressed(sc)]));
        events.extend(self.db.apply([Operation::KeyReleased(sc)]));
        events
    }

    /// Map ASCII text to scancode press/release sequences.
    /// Returns one batch of events per character.
    pub(crate) fn type_text(&mut self, text: &str) -> Vec<SmallVec<[FastPathInputEvent; 4]>> {
        text.chars()
            .filter_map(ascii_to_scancode)
            .map(|(scancode, needs_shift)| {
                let mut events = SmallVec::new();
                if needs_shift {
                    events.extend(self.db.apply([Operation::KeyPressed(LSHIFT)]));
                }
                events.extend(self.db.apply([Operation::KeyPressed(scancode)]));
                events.extend(self.db.apply([Operation::KeyReleased(scancode)]));
                if needs_shift {
                    events.extend(self.db.apply([Operation::KeyReleased(LSHIFT)]));
                }
                events
            })
            .collect()
    }

    /// Type unicode text using UnicodeKeyPressed/Released operations.
    /// Each character becomes a press+release pair; surrogate pairs are handled
    /// by ironrdp-input automatically.
    pub(crate) fn type_unicode(&mut self, text: &str) -> Vec<SmallVec<[FastPathInputEvent; 2]>> {
        text.chars()
            .map(|ch| {
                let mut events = SmallVec::new();
                events.extend(self.db.apply([Operation::UnicodeKeyPressed(ch)]));
                events.extend(self.db.apply([Operation::UnicodeKeyReleased(ch)]));
                events
            })
            .collect()
    }

    /// Press modifiers in order, press+release key, release modifiers in reverse.
    pub(crate) fn combo_events(&mut self, action: &KeyAction) -> SmallVec<[FastPathInputEvent; 8]> {
        let mut events = SmallVec::new();

        // Press modifiers
        for &modifier in &action.modifiers {
            events.extend(self.db.apply([Operation::KeyPressed(modifier)]));
        }

        // Press and release the key
        events.extend(self.db.apply([Operation::KeyPressed(action.key)]));
        events.extend(self.db.apply([Operation::KeyReleased(action.key)]));

        // Release modifiers in reverse order
        for &modifier in action.modifiers.iter().rev() {
            events.extend(self.db.apply([Operation::KeyReleased(modifier)]));
        }

        events
    }

    /// Press a key without releasing it.
    pub(crate) fn key_down(&mut self, scancode: Scancode) -> SmallVec<[FastPathInputEvent; 2]> {
        self.db.apply([Operation::KeyPressed(scancode)])
    }

    /// Release a previously pressed key.
    pub(crate) fn key_up(&mut self, scancode: Scancode) -> SmallVec<[FastPathInputEvent; 2]> {
        self.db.apply([Operation::KeyReleased(scancode)])
    }

    /// Press modifiers + key (no release). For hold-ms support.
    pub(crate) fn combo_down(&mut self, action: &KeyAction) -> SmallVec<[FastPathInputEvent; 8]> {
        let mut events = SmallVec::new();
        for &modifier in &action.modifiers {
            events.extend(self.db.apply([Operation::KeyPressed(modifier)]));
        }
        events.extend(self.db.apply([Operation::KeyPressed(action.key)]));
        events
    }

    /// Release key + modifiers (reverse order). For hold-ms support.
    pub(crate) fn combo_up(&mut self, action: &KeyAction) -> SmallVec<[FastPathInputEvent; 8]> {
        let mut events = SmallVec::new();
        events.extend(self.db.apply([Operation::KeyReleased(action.key)]));
        for &modifier in action.modifiers.iter().rev() {
            events.extend(self.db.apply([Operation::KeyReleased(modifier)]));
        }
        events
    }

    /// Move mouse to absolute position.
    pub(crate) fn mouse_move(&mut self, x: u16, y: u16) -> SmallVec<[FastPathInputEvent; 2]> {
        self.db
            .apply([Operation::MouseMove(MousePosition { x, y })])
    }

    /// Click a mouse button (press + release).
    pub(crate) fn mouse_click(&mut self, button: MouseButton) -> SmallVec<[FastPathInputEvent; 4]> {
        let mut events = SmallVec::new();
        events.extend(self.db.apply([Operation::MouseButtonPressed(button)]));
        events.extend(self.db.apply([Operation::MouseButtonReleased(button)]));
        events
    }

    /// Double-click: two press+release cycles.
    pub(crate) fn mouse_double_click(
        &mut self,
        button: MouseButton,
    ) -> SmallVec<[FastPathInputEvent; 8]> {
        let mut events = SmallVec::new();
        events.extend(self.db.apply([Operation::MouseButtonPressed(button)]));
        events.extend(self.db.apply([Operation::MouseButtonReleased(button)]));
        events.extend(self.db.apply([Operation::MouseButtonPressed(button)]));
        events.extend(self.db.apply([Operation::MouseButtonReleased(button)]));
        events
    }

    /// Generate a mouse drag sequence: move to start, press, move to end, release.
    pub(crate) fn mouse_drag(
        &mut self,
        from: (u16, u16),
        to: (u16, u16),
        button: MouseButton,
    ) -> Vec<SmallVec<[FastPathInputEvent; 2]>> {
        // Each step is a separate batch so callers can insert delays between them
        let move_to_start = self.db.apply([Operation::MouseMove(MousePosition {
            x: from.0,
            y: from.1,
        })]);
        let press = self.db.apply([Operation::MouseButtonPressed(button)]);
        let move_to_end = self
            .db
            .apply([Operation::MouseMove(MousePosition { x: to.0, y: to.1 })]);
        let release = self.db.apply([Operation::MouseButtonReleased(button)]);
        vec![move_to_start, press, move_to_end, release]
    }

    /// Generate wheel scroll events.
    /// `up` = true scrolls up (positive), false scrolls down (negative).
    /// Each notch is a separate event for better compatibility.
    pub(crate) fn scroll(
        &mut self,
        up: bool,
        notches: u32,
    ) -> Vec<SmallVec<[FastPathInputEvent; 2]>> {
        let rotation_units: i16 = if up { 120 } else { -120 };
        (0..notches)
            .map(|_| {
                self.db.apply([Operation::WheelRotations(WheelRotations {
                    is_vertical: true,
                    rotation_units,
                })])
            })
            .collect()
    }
}

// Left Shift scancode
const LSHIFT: Scancode = Scancode::from_u16(0x2A);

/// Parse a key spec like "ctrl-c", "ctrl-alt-delete", "enter", or "0x1C".
/// Returns the modifiers and final key as scancodes.
pub(crate) fn parse_key_spec(spec: &str) -> Result<KeyAction> {
    let table = key_name_table();

    // Split on '-' but handle hex codes like "0x1C" which contain no modifiers
    if spec.starts_with("0x") || spec.starts_with("0X") {
        let code = parse_hex_scancode(spec)?;
        return Ok(KeyAction {
            modifiers: Vec::new(),
            key: Scancode::from_u16(code),
        });
    }

    let parts: Vec<&str> = spec.split('-').collect();

    if parts.is_empty() {
        bail!("empty key spec");
    }

    // Last part is the key, everything before is a modifier
    let key_name = parts[parts.len() - 1].to_lowercase();
    let modifier_names = &parts[..parts.len() - 1];

    let key = if key_name.len() == 1 {
        // Single character: look up its scancode via ascii mapping
        let ch = key_name.chars().next().unwrap();
        match ascii_to_scancode(ch) {
            Some((sc, _)) => sc,
            None => bail!("unknown key character: '{ch}'"),
        }
    } else {
        match table.get(key_name.as_str()) {
            Some(&sc) => Scancode::from_u16(sc),
            None => bail!("unknown key name: '{key_name}'"),
        }
    };

    let mut modifiers = Vec::new();
    for name in modifier_names {
        let lower = name.to_lowercase();
        match table.get(lower.as_str()) {
            Some(&sc) => modifiers.push(Scancode::from_u16(sc)),
            None => bail!("unknown modifier: '{name}'"),
        }
    }

    Ok(KeyAction { modifiers, key })
}

fn parse_hex_scancode(s: &str) -> Result<u16> {
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    u16::from_str_radix(s, 16).map_err(|e| anyhow::anyhow!("invalid scancode hex '{s}': {e}"))
}

/// Parse a mouse button name to a `MouseButton`.
pub(crate) fn parse_button(name: &str) -> Result<MouseButton> {
    match name {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        other => bail!("unknown mouse button: '{other}' (expected left/right/middle)"),
    }
}

/// Build the key name -> scancode lookup table.
fn key_name_table() -> HashMap<&'static str, u16> {
    let mut m = HashMap::new();

    // Common keys
    m.insert("enter", 0x1C);
    m.insert("return", 0x1C);
    m.insert("tab", 0x0F);
    m.insert("escape", 0x01);
    m.insert("esc", 0x01);
    m.insert("backspace", 0x0E);
    m.insert("space", 0x39);

    // Navigation
    m.insert("insert", 0xE052);
    m.insert("delete", 0xE053);
    m.insert("home", 0xE047);
    m.insert("end", 0xE04F);
    m.insert("pageup", 0xE049);
    m.insert("pagedown", 0xE051);

    // Arrow keys
    m.insert("up", 0xE048);
    m.insert("down", 0xE050);
    m.insert("left", 0xE04B);
    m.insert("right", 0xE04D);

    // Function keys
    m.insert("f1", 0x3B);
    m.insert("f2", 0x3C);
    m.insert("f3", 0x3D);
    m.insert("f4", 0x3E);
    m.insert("f5", 0x3F);
    m.insert("f6", 0x40);
    m.insert("f7", 0x41);
    m.insert("f8", 0x42);
    m.insert("f9", 0x43);
    m.insert("f10", 0x44);
    m.insert("f11", 0x57);
    m.insert("f12", 0x58);

    // Modifiers
    m.insert("ctrl", 0x1D);
    m.insert("lctrl", 0x1D);
    m.insert("rctrl", 0xE01D);
    m.insert("alt", 0x38);
    m.insert("lalt", 0x38);
    m.insert("ralt", 0xE038);
    m.insert("shift", 0x2A);
    m.insert("lshift", 0x2A);
    m.insert("rshift", 0x36);
    m.insert("super", 0xE05B);
    m.insert("lsuper", 0xE05B);
    m.insert("win", 0xE05B);
    m.insert("rsuper", 0xE05C);

    // Lock keys
    m.insert("capslock", 0x3A);
    m.insert("numlock", 0x45);
    m.insert("scrolllock", 0x46);

    // Special
    m.insert("printscreen", 0xE037);
    m.insert("pause", 0xE11D);
    m.insert("menu", 0xE05D);

    m
}

/// Map an ASCII character to (`Scancode`, `needs_shift`).
/// Covers a-z, 0-9, space, enter, and common punctuation on US QWERTY layout.
fn ascii_to_scancode(ch: char) -> Option<(Scancode, bool)> {
    let (code, shift) = match ch {
        // Letters (scancodes match US QWERTY)
        'a' | 'A' => (0x1E, ch.is_ascii_uppercase()),
        'b' | 'B' => (0x30, ch.is_ascii_uppercase()),
        'c' | 'C' => (0x2E, ch.is_ascii_uppercase()),
        'd' | 'D' => (0x20, ch.is_ascii_uppercase()),
        'e' | 'E' => (0x12, ch.is_ascii_uppercase()),
        'f' | 'F' => (0x21, ch.is_ascii_uppercase()),
        'g' | 'G' => (0x22, ch.is_ascii_uppercase()),
        'h' | 'H' => (0x23, ch.is_ascii_uppercase()),
        'i' | 'I' => (0x17, ch.is_ascii_uppercase()),
        'j' | 'J' => (0x24, ch.is_ascii_uppercase()),
        'k' | 'K' => (0x25, ch.is_ascii_uppercase()),
        'l' | 'L' => (0x26, ch.is_ascii_uppercase()),
        'm' | 'M' => (0x32, ch.is_ascii_uppercase()),
        'n' | 'N' => (0x31, ch.is_ascii_uppercase()),
        'o' | 'O' => (0x18, ch.is_ascii_uppercase()),
        'p' | 'P' => (0x19, ch.is_ascii_uppercase()),
        'q' | 'Q' => (0x10, ch.is_ascii_uppercase()),
        'r' | 'R' => (0x13, ch.is_ascii_uppercase()),
        's' | 'S' => (0x1F, ch.is_ascii_uppercase()),
        't' | 'T' => (0x14, ch.is_ascii_uppercase()),
        'u' | 'U' => (0x16, ch.is_ascii_uppercase()),
        'v' | 'V' => (0x2F, ch.is_ascii_uppercase()),
        'w' | 'W' => (0x11, ch.is_ascii_uppercase()),
        'x' | 'X' => (0x2D, ch.is_ascii_uppercase()),
        'y' | 'Y' => (0x15, ch.is_ascii_uppercase()),
        'z' | 'Z' => (0x2C, ch.is_ascii_uppercase()),

        // Number row
        '1' => (0x02, false),
        '2' => (0x03, false),
        '3' => (0x04, false),
        '4' => (0x05, false),
        '5' => (0x06, false),
        '6' => (0x07, false),
        '7' => (0x08, false),
        '8' => (0x09, false),
        '9' => (0x0A, false),
        '0' => (0x0B, false),

        // Shifted number row
        '!' => (0x02, true),
        '@' => (0x03, true),
        '#' => (0x04, true),
        '$' => (0x05, true),
        '%' => (0x06, true),
        '^' => (0x07, true),
        '&' => (0x08, true),
        '*' => (0x09, true),
        '(' => (0x0A, true),
        ')' => (0x0B, true),

        // Common keys
        ' ' => (0x39, false),
        '\n' => (0x1C, false),
        '\t' => (0x0F, false),

        // Punctuation
        '-' => (0x0C, false),
        '_' => (0x0C, true),
        '=' => (0x0D, false),
        '+' => (0x0D, true),
        '[' => (0x1A, false),
        '{' => (0x1A, true),
        ']' => (0x1B, false),
        '}' => (0x1B, true),
        '\\' => (0x2B, false),
        '|' => (0x2B, true),
        ';' => (0x27, false),
        ':' => (0x27, true),
        '\'' => (0x28, false),
        '"' => (0x28, true),
        ',' => (0x33, false),
        '<' => (0x33, true),
        '.' => (0x34, false),
        '>' => (0x34, true),
        '/' => (0x35, false),
        '?' => (0x35, true),
        '`' => (0x29, false),
        '~' => (0x29, true),

        _ => return None,
    };

    Some((Scancode::from_u16(code), shift))
}
