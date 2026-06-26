//! Built-in command documentation for rdpdo.
//!
//! Every chained command is documented here with syntax, arguments,
//! defaults, examples, and notes. This module drives `rdpdo help`,
//! `rdpdo help <command>`, and the `--help` after-help summary.

use std::fmt::Write;

// ── Data model ──────────────────────────────────────────────────────

pub(crate) struct CommandDoc {
    /// Primary command name (e.g. "expect").
    pub name: &'static str,
    /// Alternative names the parser also accepts (e.g. "screenshot" for "capture").
    pub aliases: &'static [&'static str],
    /// Category for grouping in the full listing.
    pub category: Category,
    /// One-line summary shown in the compact table.
    pub summary: &'static str,
    /// Full syntax line (e.g. "expect <reference> [tolerance] [timeout] [--needles <dir>] [--tag <name>]").
    pub syntax: &'static str,
    /// Positional and optional argument descriptions.
    pub args: &'static [(&'static str, &'static str)],
    /// Practical examples, one per entry.
    pub examples: &'static [&'static str],
    /// Whether the command requires an RDP connection.
    pub needs_connection: bool,
    /// Additional notes (protocol details, tips, caveats).
    pub notes: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Category {
    Input,
    Capture,
    Matching,
    Stability,
    Clipboard,
    Display,
    Provisioning,
    Calibration,
    Audio,
    Pixel,
    Scripting,
    Info,
    Baseline,
    /// rdpsee observation/report category (unused by rdpdo).
    Report,
}

impl Category {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Capture => "Capture",
            Self::Matching => "Visual Matching",
            Self::Stability => "Screen Stability",
            Self::Clipboard => "Clipboard",
            Self::Display => "Display Control",
            Self::Provisioning => "Provisioning",
            Self::Calibration => "Calibration",
            Self::Audio => "Audio",
            Self::Pixel => "Pixel Analysis",
            Self::Scripting => "Scripting & Recording",
            Self::Info => "Info & Diagnostics",
            Self::Baseline => "Baseline Management",
            Self::Report => "Report",
        }
    }
}

/// Display order for categories.
const CATEGORY_ORDER: &[Category] = &[
    Category::Input,
    Category::Capture,
    Category::Matching,
    Category::Stability,
    Category::Clipboard,
    Category::Display,
    Category::Audio,
    Category::Pixel,
    Category::Provisioning,
    Category::Calibration,
    Category::Scripting,
    Category::Baseline,
    Category::Info,
];

// ── Command registry ────────────────────────────────────────────────

pub(crate) fn all_commands() -> &'static [CommandDoc] {
    ALL_COMMANDS
}

static ALL_COMMANDS: &[CommandDoc] = &[
    // ── Input ───────────────────────────────────────────────────────
    CommandDoc {
        name: "type",
        aliases: &[],
        category: Category::Input,
        summary: "Type ASCII text via US-QWERTY scancode sequences",
        syntax: "type <text>",
        args: &[(
            "<text>",
            "Text to type. Shift is applied automatically for uppercase and symbols.",
        )],
        examples: &[
            "rdpdo -s host type \"Hello, World!\"",
            "rdpdo -s host type \"user@example.com\" key tab type \"p4ssw0rd\" key enter",
        ],
        needs_connection: true,
        notes: "Sends scancode press/release pairs with 30ms inter-character delay. \
                For non-ASCII text or unknown keyboard layouts, use `utype` instead.",
    },
    CommandDoc {
        name: "utype",
        aliases: &[],
        category: Category::Input,
        summary: "Type text using Unicode keyboard events (any language)",
        syntax: "utype <text>",
        args: &[(
            "<text>",
            "Unicode text to type. Bypasses keyboard layout via TS_UNICODE_KEYBOARD_EVENT.",
        )],
        examples: &[
            "rdpdo -s host utype \"Привет мир\"",
            "rdpdo -s host utype \"日本語テスト\"",
        ],
        needs_connection: true,
        notes: "Sends Unicode codepoints directly, bypassing scancodes. Works with any \
                character on any remote keyboard layout. Slower than `type` for pure ASCII.",
    },
    CommandDoc {
        name: "key",
        aliases: &[],
        category: Category::Input,
        summary: "Send a key or key combination (press + release)",
        syntax: "key <spec> [--hold-ms <ms>]",
        args: &[
            (
                "<spec>",
                "Key name, hex scancode (0x1C), or combo (ctrl-alt-delete).",
            ),
            (
                "--hold-ms <ms>",
                "Hold the key for this many milliseconds before releasing.",
            ),
        ],
        examples: &[
            "rdpdo -s host key enter",
            "rdpdo -s host key ctrl-c",
            "rdpdo -s host key ctrl-alt-delete",
            "rdpdo -s host key f5 --hold-ms 500",
            "rdpdo -s host key 0x1C",
        ],
        needs_connection: true,
        notes: "Key names: enter, tab, escape, backspace, delete, insert, home, end, \
                pageup, pagedown, up, down, left, right, f1-f12, space, \
                ctrl/lctrl/rctrl, alt/lalt/ralt, shift/lshift/rshift, \
                super/lsuper/rsuper, capslock, numlock, scrolllock, printscreen, \
                pause, menu. Combos use dash separators: ctrl-shift-escape.",
    },
    CommandDoc {
        name: "keydown",
        aliases: &[],
        category: Category::Input,
        summary: "Press and hold a key (until keyup)",
        syntax: "keydown <key>",
        args: &[("<key>", "Key name or scancode to press and hold.")],
        examples: &[
            "rdpdo -s host keydown shift click 100,200 click 100,300 keyup shift",
            "rdpdo -s host keydown a pause 2 keyup a",
        ],
        needs_connection: true,
        notes: "The key stays pressed until an explicit `keyup` for the same key. \
                Useful for shift-click multi-select or holding keys for timed durations.",
    },
    CommandDoc {
        name: "keyup",
        aliases: &[],
        category: Category::Input,
        summary: "Release a previously held key",
        syntax: "keyup <key>",
        args: &[("<key>", "Key name or scancode to release.")],
        examples: &["rdpdo -s host keydown ctrl click 100,200 keyup ctrl"],
        needs_connection: true,
        notes: "Must match a prior `keydown`. Releasing a key that wasn't held is harmless.",
    },
    CommandDoc {
        name: "move",
        aliases: &[],
        category: Category::Input,
        summary: "Move mouse cursor to a position (no click)",
        syntax: "move <position>",
        args: &[(
            "<position>",
            "Target: pixels (500,300), percentage (50%,50%), or named (center).",
        )],
        examples: &[
            "rdpdo -s host move 500,300",
            "rdpdo -s host move 50%,50%",
            "rdpdo -s host move center",
        ],
        needs_connection: true,
        notes: "Named positions: center, top-left, top-right, bottom-left, bottom-right, \
                top-center, bottom-center, left-center, right-center. Named positions \
                use a 5% inset to avoid screen edges and taskbars.",
    },
    CommandDoc {
        name: "click",
        aliases: &[],
        category: Category::Input,
        summary: "Click at a position (move + press + release)",
        syntax: "click <position> [button]",
        args: &[
            (
                "<position>",
                "Target: pixels, percentage, or named position.",
            ),
            ("[button]", "left (default), right, or middle."),
        ],
        examples: &[
            "rdpdo -s host click 500,300",
            "rdpdo -s host click 500,300 right",
            "rdpdo -s host click center",
            "rdpdo -s host click 95%,5% middle",
        ],
        needs_connection: true,
        notes: "The mouse moves to the position, then presses and releases the button. \
                If a calibration profile is loaded (--calibration), coordinates are \
                corrected before sending.",
    },
    CommandDoc {
        name: "doubleclick",
        aliases: &[],
        category: Category::Input,
        summary: "Double-click at a position",
        syntax: "doubleclick <position>",
        args: &[(
            "<position>",
            "Target: pixels, percentage, or named position.",
        )],
        examples: &[
            "rdpdo -s host doubleclick 500,300",
            "rdpdo -s host doubleclick center",
        ],
        needs_connection: true,
        notes: "Sends two left clicks with a 50ms interval.",
    },
    CommandDoc {
        name: "drag",
        aliases: &[],
        category: Category::Input,
        summary: "Drag from one position to another",
        syntax: "drag <from> <to>",
        args: &[
            (
                "<from>",
                "Starting position (pixels, percentage, or named).",
            ),
            ("<to>", "Ending position (pixels, percentage, or named)."),
        ],
        examples: &[
            "rdpdo -s host drag 100,100 500,300",
            "rdpdo -s host drag 10%,10% 90%,90%",
        ],
        needs_connection: true,
        notes: "Press at <from>, move in steps to <to>, release. Inter-step delays \
                ensure the server registers the drag path.",
    },
    CommandDoc {
        name: "scroll",
        aliases: &[],
        category: Category::Input,
        summary: "Scroll up or down by N notches",
        syntax: "scroll <direction> <notches>",
        args: &[
            ("<direction>", "up or down."),
            ("<notches>", "Number of scroll notches (positive integer)."),
        ],
        examples: &[
            "rdpdo -s host scroll up 3",
            "rdpdo -s host scroll down 5",
            "rdpdo -s host move 500,300 scroll down 3",
        ],
        needs_connection: true,
        notes: "Each notch sends a wheel event with 120 rotation units (standard Windows \
                scroll delta). Position the cursor first with `move` if needed.",
    },
    CommandDoc {
        name: "type-password",
        aliases: &[],
        category: Category::Input,
        summary: "Type a password with content redacted from logs",
        syntax: "type-password <source>",
        args: &[(
            "<source>",
            "env:VAR_NAME (from environment), file:/path (from file), or literal text.",
        )],
        examples: &[
            "rdpdo -s host type-password env:RDP_PASSWORD key enter",
            "rdpdo -s host type-password file:/run/secrets/pass key enter",
            "rdpdo -s host type-password \"s3cret\" key enter",
        ],
        needs_connection: true,
        notes: "Sends scancodes identically to `type`, but all verbose/debug output \
                replaces the password with ***. In CI, prefer env: to avoid passwords \
                in command lines visible in process listings.",
    },
    // ── Capture ─────────────────────────────────────────────────────
    CommandDoc {
        name: "capture",
        aliases: &["screenshot"],
        category: Category::Capture,
        summary: "Save a screenshot to file (PNG format)",
        syntax: "capture <path> [region]",
        args: &[
            ("<path>", "Output file path, or \"-\" for stdout."),
            (
                "[region]",
                "Optional region: x,y,WxH in pixels or percentages (e.g. 100,200,400x300).",
            ),
        ],
        examples: &[
            "rdpdo -s host capture /tmp/desktop.png",
            "rdpdo -s host capture - | display",
            "rdpdo -s host capture /tmp/region.png 100,200,400x300",
        ],
        needs_connection: true,
        notes: "Always outputs PNG regardless of file extension. Use \"-\" to pipe to \
                other tools (ImageMagick, diff utilities, etc.).",
    },
    CommandDoc {
        name: "rcapture",
        aliases: &[],
        category: Category::Capture,
        summary: "Capture a specific screen region to file",
        syntax: "rcapture <region> <path>",
        args: &[
            ("<region>", "Region spec: x,y,WxH in pixels or percentages."),
            ("<path>", "Output file path, or \"-\" for stdout."),
        ],
        examples: &[
            "rdpdo -s host rcapture 0,0,400x300 /tmp/corner.png",
            "rdpdo -s host rcapture 25%,25%,50%x50% /tmp/center.png",
        ],
        needs_connection: true,
        notes: "Same as `capture` with region, but region comes first. Convenience for \
                scripts where the region is the primary parameter.",
    },
    CommandDoc {
        name: "timelapse",
        aliases: &[],
        category: Category::Capture,
        summary: "Periodic screenshot capture with termination conditions",
        syntax: "timelapse <path-template> --interval <dur> (--count <n> | --duration <dur> | --until <ref>)",
        args: &[
            (
                "<path-template>",
                "Output path with {n} for sequence number (e.g. /tmp/frame-{n}.png).",
            ),
            (
                "--interval <dur>",
                "Time between captures: 500ms, 1s, 2.5s (default: 1s).",
            ),
            ("--count <n>", "Maximum number of frames to capture."),
            ("--duration <dur>", "Maximum total duration: 30s, 2m."),
            (
                "--until <ref>",
                "Stop when screen matches this reference image.",
            ),
            (
                "--tolerance <0-1>",
                "Match tolerance for --until (default: 0.95).",
            ),
        ],
        examples: &[
            "rdpdo -s host timelapse /tmp/frame-{n}.png --interval 1s --count 10",
            "rdpdo -s host timelapse /tmp/boot-{n}.png --interval 500ms --duration 30s",
            "rdpdo -s host timelapse /tmp/progress-{n}.png --interval 2s --until /tmp/login-ready.png",
        ],
        needs_connection: true,
        notes: "{n} is replaced with a zero-padded sequence number (001, 002, ...). \
                Without {n}, frames overwrite each other (useful for \"latest screenshot\" \
                monitoring). Requires at least one of --count, --duration, or --until.",
    },
    // ── Visual Matching ─────────────────────────────────────────────
    CommandDoc {
        name: "expect",
        aliases: &[],
        category: Category::Matching,
        summary: "Wait until full screen matches a reference image",
        syntax: "expect <reference> [tolerance] [timeout] [--needles <dir>] [--tag <name>]",
        args: &[
            ("<reference>", "Path to reference PNG image."),
            (
                "[tolerance]",
                "Minimum match score: 0.0-1.0 or 1-100 (default: 0.95).",
            ),
            ("[timeout]", "Seconds to wait before failing (default: 30)."),
            (
                "--needles <dir>",
                "Search a directory of needle images (PNG+JSON pairs).",
            ),
            (
                "--tag <name>",
                "Only try needles with this tag (requires --needles).",
            ),
        ],
        examples: &[
            "rdpdo -s host expect /tmp/login-screen.png",
            "rdpdo -s host expect /tmp/desktop.png 0.90 60",
            "rdpdo -s host expect --needles ./needles/ --tag login 30",
        ],
        needs_connection: true,
        notes: "Uses Pearson correlation on full-screen grayscale comparison. On timeout, \
                saves the best-matching frame to /tmp/rdpdo-fail-NNN.png and reports \
                the best score achieved. Exit code 1 on timeout.",
    },
    CommandDoc {
        name: "waitfor",
        aliases: &[],
        category: Category::Matching,
        summary: "Search for a template image anywhere on screen",
        syntax: "waitfor <template> [tolerance] [timeout] [--needles <dir>] [--tag <name>]",
        args: &[
            ("<template>", "Path to template PNG (smaller than screen)."),
            ("[tolerance]", "Minimum match score (default: 0.95)."),
            ("[timeout]", "Seconds to wait (default: 30)."),
            ("--needles <dir>", "Search a needle directory instead."),
            ("--tag <name>", "Filter needles by tag."),
        ],
        examples: &[
            "rdpdo -s host waitfor /tmp/ok-button.png",
            "rdpdo -s host waitfor /tmp/dialog.png 0.90 20",
        ],
        needs_connection: true,
        notes: "Sliding-window NCC template search. Reports match location (x, y) on \
                success. Saves failure diagnostic on timeout.",
    },
    CommandDoc {
        name: "expectclick",
        aliases: &[],
        category: Category::Matching,
        summary: "Find a template on screen, then click its center",
        syntax: "expectclick <template> [tolerance] [timeout] [--needles <dir>] [--tag <name>]",
        args: &[
            ("<template>", "Path to template PNG to find and click."),
            ("[tolerance]", "Minimum match score (default: 0.95)."),
            ("[timeout]", "Seconds to wait (default: 30)."),
            ("--needles <dir>", "Search a needle directory."),
            ("--tag <name>", "Filter needles by tag."),
        ],
        examples: &[
            "rdpdo -s host expectclick /tmp/ok-button.png",
            "rdpdo -s host expectclick /tmp/submit.png 0.90 10",
        ],
        needs_connection: true,
        notes: "Combines `waitfor` + `click` at the matched location. For needle sets, \
                clicks the needle's click_point (from JSON) or the screen center as fallback.",
    },
    CommandDoc {
        name: "rexpect",
        aliases: &[],
        category: Category::Matching,
        summary: "Wait for a screen region to match a reference image",
        syntax: "rexpect <region> <reference> [tolerance] [timeout]",
        args: &[
            ("<region>", "Region spec: x,y,WxH in pixels or percentages."),
            (
                "<reference>",
                "Path to reference PNG (same dimensions as region).",
            ),
            ("[tolerance]", "Minimum match score (default: 0.95)."),
            ("[timeout]", "Seconds to wait (default: 30)."),
        ],
        examples: &[
            "rdpdo -s host rexpect 100,200,400x300 /tmp/region-ref.png",
            "rdpdo -s host rexpect 0,0,50%x100% /tmp/left-half.png 0.90 10",
        ],
        needs_connection: true,
        notes: "Crops the screen to the specified region before comparing. Useful for \
                matching specific UI elements without full-screen reference images.",
    },
    CommandDoc {
        name: "repeat-key",
        aliases: &[],
        category: Category::Matching,
        summary: "Press a key repeatedly until a template matches",
        syntax: "repeat-key <key> <template> [tolerance] [timeout] [interval_ms] [max_presses] [--needles <dir>] [--tag <name>]",
        args: &[
            ("<key>", "Key to press each iteration (e.g. tab, down)."),
            ("<template>", "Reference image to match against."),
            ("[tolerance]", "Minimum match score (default: 0.95)."),
            ("[timeout]", "Total seconds before giving up (default: 30)."),
            (
                "[interval_ms]",
                "Milliseconds between key presses (default: 300).",
            ),
            (
                "[max_presses]",
                "Maximum number of key presses before giving up.",
            ),
            ("--needles <dir>", "Search a needle directory."),
            ("--tag <name>", "Filter needles by tag."),
        ],
        examples: &[
            "rdpdo -s host repeat-key tab /tmp/share-button.png 0.95 10 300",
            "rdpdo -s host repeat-key down /tmp/install-option.png 0.90 20 500 15",
        ],
        needs_connection: true,
        notes: "Inspired by openQA send_key_until_needlematch. Each iteration: send key, \
                wait interval, capture frame, check match. Exits on match (code 0), \
                timeout (code 1), or max iterations (code 1).",
    },
    CommandDoc {
        name: "diff",
        aliases: &[],
        category: Category::Matching,
        summary: "Compare two images offline (no connection needed)",
        syntax: "diff <image-a> <image-b> [threshold] [--output <path>] [--mode <mode>]",
        args: &[
            ("<image-a>", "First image path."),
            ("<image-b>", "Second image path."),
            (
                "[threshold]",
                "If set, exit code 0 when score >= threshold, 1 otherwise.",
            ),
            ("--output <path>", "Save a visual diff image."),
            (
                "--mode <mode>",
                "Diff visualization: highlight (default), side-by-side, heatmap.",
            ),
        ],
        examples: &[
            "rdpdo diff /tmp/before.png /tmp/after.png",
            "rdpdo diff /tmp/a.png /tmp/b.png 0.95",
            "rdpdo diff /tmp/a.png /tmp/b.png --output /tmp/diff.png --mode heatmap",
        ],
        needs_connection: false,
        notes: "Reports correlation score and pixel difference count to stderr. With \
                --threshold, also sets exit code for CI use. Diff modes: highlight \
                (red overlay on changed pixels), side-by-side, heatmap (intensity-mapped).",
    },
    // ── Screen Stability ────────────────────────────────────────────
    CommandDoc {
        name: "wait-still",
        aliases: &[],
        category: Category::Stability,
        summary: "Wait until screen stops updating",
        syntax: "wait-still [stillness_ms] [timeout_secs]",
        args: &[
            (
                "[stillness_ms]",
                "Required quiet period in ms (default: 500).",
            ),
            (
                "[timeout_secs]",
                "Give up after this many seconds (default: 10).",
            ),
        ],
        examples: &[
            "rdpdo -s host wait-still",
            "rdpdo -s host wait-still 1000 20",
            "rdpdo -s host click center wait-still 500 5 capture /tmp/settled.png",
        ],
        needs_connection: true,
        notes: "Monitors EGFX frame arrivals. \"Still\" means no new frame for the \
                stillness period. Critical before visual matching when progressive \
                codecs (RFX) are active, to avoid matching intermediate frames.",
    },
    CommandDoc {
        name: "wait-change",
        aliases: &[],
        category: Category::Stability,
        summary: "Wait until screen content changes",
        syntax: "wait-change [timeout_secs]",
        args: &[(
            "[timeout_secs]",
            "Give up after this many seconds (default: 30).",
        )],
        examples: &[
            "rdpdo -s host click center wait-change 5",
            "rdpdo -s host key enter wait-change 10",
        ],
        needs_connection: true,
        notes: "Waits for any new graphics frame. Useful to verify an action caused \
                a visible response before continuing.",
    },
    // ── Clipboard ───────────────────────────────────────────────────
    CommandDoc {
        name: "set-clipboard",
        aliases: &[],
        category: Category::Clipboard,
        summary: "Set text on the remote clipboard",
        syntax: "set-clipboard <text>",
        args: &[(
            "<text>",
            "Text to place on the remote clipboard (CF_UNICODETEXT).",
        )],
        examples: &[
            "rdpdo -s host set-clipboard \"Hello from rdpdo\"",
            "rdpdo -s host set-clipboard \"test\" get-clipboard",
        ],
        needs_connection: true,
        notes: "Advertises CF_UNICODETEXT format to the server, then responds with \
                UTF-16LE encoded text when the server requests clipboard data.",
    },
    CommandDoc {
        name: "get-clipboard",
        aliases: &[],
        category: Category::Clipboard,
        summary: "Read text from the remote clipboard",
        syntax: "get-clipboard",
        args: &[],
        examples: &[
            "rdpdo -s host get-clipboard",
            "rdpdo -s host set-clipboard \"test\" get-clipboard",
        ],
        needs_connection: true,
        notes: "Requests CF_UNICODETEXT from the server, waits up to 5 seconds. \
                Prints clipboard text to stdout. Prints warning to stderr if no data.",
    },
    CommandDoc {
        name: "clipboard-send-file",
        aliases: &["send-file"],
        category: Category::Clipboard,
        summary: "Send a local file to the remote clipboard",
        syntax: "clipboard-send-file <path>",
        args: &[("<path>", "Path to the local file to send.")],
        examples: &[
            "rdpdo -s host clipboard-send-file /tmp/config.txt",
            "rdpdo -s host send-file ./script.sh",
        ],
        needs_connection: true,
        notes: "Uses the CLIPRDR file list format (MS-RDPECLIP). The remote desktop \
                receives a file paste operation. No SSH or drive redirection needed.",
    },
    CommandDoc {
        name: "clipboard-recv-file",
        aliases: &["recv-file"],
        category: Category::Clipboard,
        summary: "Receive a file from the remote clipboard",
        syntax: "clipboard-recv-file <path>",
        args: &[("<path>", "Local path to save the received file.")],
        examples: &[
            "rdpdo -s host clipboard-recv-file /tmp/received.txt",
            "rdpdo -s host recv-file ./output.log",
        ],
        needs_connection: true,
        notes: "Waits up to 30 seconds for the server to provide file data via CLIPRDR. \
                Prints the saved file path to stdout.",
    },
    // ── Display Control ─────────────────────────────────────────────
    CommandDoc {
        name: "resize",
        aliases: &[],
        category: Category::Display,
        summary: "Resize the remote desktop",
        syntax: "resize <WIDTHxHEIGHT>",
        args: &[(
            "<WIDTHxHEIGHT>",
            "New dimensions, e.g. 1920x1080, 2560x1440.",
        )],
        examples: &[
            "rdpdo -s host resize 2560x1440",
            "rdpdo -s host resize 1280x720 pause 2 capture /tmp/resized.png",
        ],
        needs_connection: true,
        notes: "Sends a DisplayControl MonitorLayout PDU. Waits 2 seconds for the \
                server to process the resize. Not all servers support dynamic resize.",
    },
    CommandDoc {
        name: "monitor",
        aliases: &[],
        category: Category::Display,
        summary: "Multi-monitor control (list, set)",
        syntax: "monitor <action> [args]",
        args: &[
            ("list", "Print current monitor layout as JSON."),
            (
                "set <WIDTHxHEIGHT>",
                "Set a single primary monitor with given dimensions.",
            ),
        ],
        examples: &[
            "rdpdo -s host monitor list",
            "rdpdo -s host monitor set 1920x1080",
        ],
        needs_connection: true,
        notes: "Uses RDP DisplayControl (MS-RDPEDISP) for dynamic layout changes.",
    },
    // ── Audio ───────────────────────────────────────────────────────
    CommandDoc {
        name: "audio-capture",
        aliases: &[],
        category: Category::Audio,
        summary: "Record audio from the RDP session to a WAV file",
        syntax: "audio-capture <output> [duration_secs]",
        args: &[
            ("<output>", "Path for the output WAV file."),
            (
                "[duration_secs]",
                "Recording duration in seconds (default: 5).",
            ),
        ],
        examples: &[
            "rdpdo -s host audio-capture /tmp/clip.wav",
            "rdpdo -s host audio-capture /tmp/audio.wav 10",
        ],
        needs_connection: true,
        notes: "Captures PCM audio from the RDPSND channel. Requires the server to \
                have audio playback enabled and the RDPSND channel negotiated. Output \
                is a standard WAV file.",
    },
    CommandDoc {
        name: "audio-assert-playing",
        aliases: &[],
        category: Category::Audio,
        summary: "Assert that audio is currently playing (non-silence)",
        syntax: "audio-assert-playing [timeout_secs]",
        args: &[(
            "[timeout_secs]",
            "Seconds to listen before declaring silence (default: 5).",
        )],
        examples: &[
            "rdpdo -s host audio-assert-playing",
            "rdpdo -s host audio-assert-playing 10",
        ],
        needs_connection: true,
        notes: "Polls 250ms audio windows, checking RMS levels against a silence \
                threshold. Exit code 0 if audio detected, 1 if silence persists.",
    },
    CommandDoc {
        name: "audio-verify",
        aliases: &[],
        category: Category::Audio,
        summary: "Compare two WAV files offline (Pearson correlation)",
        syntax: "audio-verify <captured> <reference> [tolerance]",
        args: &[
            ("<captured>", "Path to captured WAV file."),
            ("<reference>", "Path to reference WAV file."),
            ("[tolerance]", "Minimum correlation score (default: 0.85)."),
        ],
        examples: &[
            "rdpdo audio-verify /tmp/captured.wav /tmp/reference.wav",
            "rdpdo audio-verify /tmp/a.wav /tmp/b.wav 0.90",
        ],
        needs_connection: false,
        notes: "Offline command (no RDP connection needed). Reads WAV files, skips \
                headers, and computes Pearson correlation on raw PCM samples. Exit \
                code 0 if correlation >= tolerance.",
    },
    // ── Pixel Analysis ──────────────────────────────────────────────
    CommandDoc {
        name: "pixel",
        aliases: &[],
        category: Category::Pixel,
        summary: "Read the color of a pixel at a position",
        syntax: "pixel <position>",
        args: &[("<position>", "Pixel coordinates: x,y (e.g. 500,300).")],
        examples: &["rdpdo -s host pixel 500,300", "rdpdo -s host pixel 0,0"],
        needs_connection: true,
        notes: "Prints R,G,B,A values to stdout (e.g. \"255,128,0,255\"). Reads from \
                the current EGFX framebuffer.",
    },
    CommandDoc {
        name: "checksum",
        aliases: &[],
        category: Category::Pixel,
        summary: "CRC32 checksum of a screen region's pixel data",
        syntax: "checksum <region>",
        args: &[("<region>", "Region spec: x,y,WxH.")],
        examples: &[
            "rdpdo -s host checksum 0,0,1920x1080",
            "rdpdo -s host checksum 100,200,400x300",
        ],
        needs_connection: true,
        notes: "Fast change detection. Much cheaper than NCC image comparison for \
                \"did anything change?\" checks. Prints hex CRC32 to stdout.",
    },
    CommandDoc {
        name: "wait-checksum-change",
        aliases: &[],
        category: Category::Pixel,
        summary: "Wait until a region's checksum changes",
        syntax: "wait-checksum-change <region> [timeout_secs]",
        args: &[
            ("<region>", "Region spec: x,y,WxH."),
            ("[timeout_secs]", "Seconds to wait (default: 30)."),
        ],
        examples: &[
            "rdpdo -s host wait-checksum-change 100,200,400x300",
            "rdpdo -s host wait-checksum-change 0,0,1920x1080 10",
        ],
        needs_connection: true,
        notes: "Records the current checksum, then polls until it differs. Lightweight \
                change gate before expensive visual matching.",
    },
    CommandDoc {
        name: "assert-checksum",
        aliases: &[],
        category: Category::Pixel,
        summary: "Assert a region matches a known checksum",
        syntax: "assert-checksum <region> <expected>",
        args: &[
            ("<region>", "Region spec: x,y,WxH."),
            ("<expected>", "Expected hex CRC32 value."),
        ],
        examples: &["rdpdo -s host assert-checksum 0,0,1920x1080 a3f2b1c4"],
        needs_connection: true,
        notes: "Exit code 0 if checksum matches, 1 otherwise. Useful for verifying \
                pixel-exact state in CI pipelines.",
    },
    CommandDoc {
        name: "find-color",
        aliases: &[],
        category: Category::Pixel,
        summary: "Find clusters of a color on screen",
        syntax: "find-color <color> [region] [tolerance] [min_area]",
        args: &[
            ("<color>", "Hex color to find: #RRGGBB or RRGGBB."),
            ("[region]", "Optional region to search within: x,y,WxH."),
            (
                "[tolerance]",
                "Color distance tolerance per channel (default: 30).",
            ),
            ("[min_area]", "Minimum cluster size in pixels (default: 3)."),
        ],
        examples: &[
            "rdpdo -s host find-color \"#00ff41\"",
            "rdpdo -s host find-color ff0000 0,0,1920x1080 20 5",
        ],
        needs_connection: true,
        notes: "Finds connected-component clusters of pixels matching the target \
                color within tolerance. Reports cluster centers and sizes as JSON.",
    },
    // ── Provisioning ────────────────────────────────────────────────
    CommandDoc {
        name: "accept-portal",
        aliases: &[],
        category: Category::Provisioning,
        summary: "Accept a compositor's screen-sharing permission dialog",
        syntax: "accept-portal <compositor> [--profile <path>] [--verify]",
        args: &[
            (
                "<compositor>",
                "Compositor name: gnome49, gnome46, kde, sway, hyprland, niri, cosmic.",
            ),
            (
                "--profile <path>",
                "Path to a custom .rdpdo-script profile.",
            ),
            (
                "--verify",
                "Capture screenshot after sequence and verify acceptance.",
            ),
        ],
        examples: &[
            "rdpdo -s host accept-portal gnome49",
            "rdpdo -s host accept-portal kde --verify",
            "rdpdo -s host accept-portal --profile ./custom-portal.rdpdo-script",
        ],
        needs_connection: true,
        notes: "Uses built-in key sequences for each compositor's portal dialog. \
                Profiles are .rdpdo-script files; user profiles in \
                ~/.config/rdpdo/profiles/accept-portal/ override built-in defaults.",
    },
    CommandDoc {
        name: "unlock",
        aliases: &[],
        category: Category::Provisioning,
        summary: "Unlock a locked screen",
        syntax: "unlock <compositor> <password> [--profile <path>] [--verify]",
        args: &[
            ("<compositor>", "Compositor name: gnome, kde, sway, etc."),
            ("<password>", "Screen lock password."),
            ("--profile <path>", "Path to a custom unlock profile."),
            ("--verify", "Verify the screen unlocked after the sequence."),
        ],
        examples: &[
            "rdpdo -s host unlock gnome MyPassword",
            "rdpdo -s host unlock kde MyPassword --verify",
        ],
        needs_connection: true,
        notes: "Pattern: Esc to wake, type password, Enter. Compositor-specific \
                timing varies. Password is NOT redacted from args (use type-password \
                for sensitive contexts).",
    },
    CommandDoc {
        name: "login",
        aliases: &[],
        category: Category::Provisioning,
        summary: "Type username and password into a login screen",
        syntax: "login <username> <password> [--profile <name|path>] [--verify]",
        args: &[
            ("<username>", "Username to type."),
            ("<password>", "Password to type."),
            (
                "--profile <name|path>",
                "Profile name (default, domain, windows) or custom script path.",
            ),
            ("--verify", "Verify login success after the sequence."),
        ],
        examples: &[
            "rdpdo -s host login admin password123",
            "rdpdo -s host login greg MyPass --profile domain",
            "rdpdo -s host login admin pass --profile windows --verify",
        ],
        needs_connection: true,
        notes: "Built-in profiles: 'default' (user Tab pass Enter), 'domain' (domain \
                Tab user Tab pass Enter), 'windows' (Ctrl+Alt+Del first). Custom \
                profiles use {username} and {password} placeholders.",
    },
    CommandDoc {
        name: "boot-sequence",
        aliases: &[],
        category: Category::Provisioning,
        summary: "Run a provisioning script with extended timeouts",
        syntax: "boot-sequence <script>",
        args: &[("<script>", "Path to a .rdpdo-script file.")],
        examples: &["rdpdo -s host boot-sequence ./install.rdpdo-script"],
        needs_connection: true,
        notes: "Functionally identical to `run` but signals \"this is an OS install \
                or first-boot automation\" with longer default timeouts (5 min for expect).",
    },
    // ── Calibration ─────────────────────────────────────────────────
    CommandDoc {
        name: "calibrate",
        aliases: &[],
        category: Category::Calibration,
        summary: "Run click calibration grid to measure coordinate offset",
        syntax: "calibrate [--grid <NxN>] [--output <path>] [--deploy <method>] [--quick]",
        args: &[
            ("--grid <NxN>", "Grid size, e.g. 4x4 (default: 4x4)."),
            ("--output <path>", "Save calibration profile JSON."),
            (
                "--deploy <method>",
                "Page deployment: clipboard (default), ssh, manual.",
            ),
            (
                "--quick",
                "3-point quick calibration (center, top-left, bottom-right).",
            ),
        ],
        examples: &[
            "rdpdo -s host calibrate",
            "rdpdo -s host calibrate --grid 6x6 --output /tmp/cal.json",
            "rdpdo -s host calibrate --quick",
        ],
        needs_connection: true,
        notes: "Deploys an HTML calibration page, clicks grid points, measures where \
                clicks land vs where they were intended, computes correction offsets. \
                Results saved to ~/.config/rdpdo/calibration/ by default.",
    },
    // ── Scripting & Recording ───────────────────────────────────────
    CommandDoc {
        name: "run",
        aliases: &[],
        category: Category::Scripting,
        summary: "Execute commands from a .rdpdo-script file",
        syntax: "run <script>",
        args: &[(
            "<script>",
            "Path to a .rdpdo-script file (one command per line).",
        )],
        examples: &[
            "rdpdo -s host run ./setup.rdpdo-script",
            "rdpdo -s host --record /tmp/timed.rdpdo run ./install.rdpdo-script",
        ],
        needs_connection: true,
        notes: "Script format: one command per line, # comments, blank lines ignored, \
                double-quoted strings preserved. Scripts can nest (run another script).",
    },
    CommandDoc {
        name: "play",
        aliases: &[],
        category: Category::Scripting,
        summary: "Replay a timed .rdpdo recording",
        syntax: "play <recording> [speed]",
        args: &[
            (
                "<recording>",
                "Path to a .rdpdo recording file (JSON Lines with timestamps).",
            ),
            (
                "[speed]",
                "Playback speed multiplier (default: 1.0). 2.0 = double speed.",
            ),
        ],
        examples: &[
            "rdpdo -s host play /tmp/session.rdpdo",
            "rdpdo -s host play /tmp/session.rdpdo 2.0",
            "rdpdo -s host play /tmp/session.rdpdo 0.5",
        ],
        needs_connection: true,
        notes: "Replays commands with original timing (adjusted by speed multiplier). \
                Recording format: JSON Lines with 't' field (ms from session start).",
    },
    CommandDoc {
        name: "convert",
        aliases: &[],
        category: Category::Scripting,
        summary: "Convert between recording and script formats",
        syntax: "convert <input> <output>",
        args: &[
            ("<input>", "Input file (.rdpdo recording or .rdpdo-script)."),
            (
                "<output>",
                "Output file. Direction inferred from extensions.",
            ),
        ],
        examples: &[
            "rdpdo convert /tmp/session.rdpdo /tmp/session.rdpdo-script",
            "rdpdo convert /tmp/session.rdpdo-script /tmp/session.rdpdo",
        ],
        needs_connection: false,
        notes: "Offline command. Recording to script strips timing. Script to recording \
                adds 1-second intervals between commands.",
    },
    // ── Baseline Management ─────────────────────────────────────────
    CommandDoc {
        name: "baseline",
        aliases: &[],
        category: Category::Baseline,
        summary: "Save, list, or check reference screenshots",
        syntax: "baseline <action> [args] [--dir <path>]",
        args: &[
            ("update <name>", "Save current screen as a named baseline."),
            ("save <name>", "Alias for update."),
            ("list", "List saved baselines. Alias: ls."),
            (
                "check <name> [tolerance]",
                "Compare current screen against a baseline (default: 0.95).",
            ),
            ("--dir <path>", "Override baseline storage directory."),
        ],
        examples: &[
            "rdpdo -s host baseline update desktop-clean",
            "rdpdo -s host baseline list",
            "rdpdo -s host baseline check desktop-clean 0.90",
            "rdpdo -s host baseline check desktop-clean --dir ./baselines/",
        ],
        needs_connection: true,
        notes: "Baselines are stored in ~/.config/rdpdo/baselines/ by default. \
                Use --dir to override (useful for per-project baseline sets).",
    },
    // ── Info & Diagnostics ──────────────────────────────────────────
    CommandDoc {
        name: "info",
        aliases: &[],
        category: Category::Info,
        summary: "Print connection info as JSON",
        syntax: "info",
        args: &[],
        examples: &["rdpdo -s host info"],
        needs_connection: true,
        notes: "Reports the negotiated capabilities as JSON: selected security protocol, \
                desktop size, color depth, graphics mode and the server-confirmed EGFX tier, \
                advertised bitmap codecs, bulk compression, and the static channels actually \
                joined.",
    },
    CommandDoc {
        name: "perf",
        aliases: &[],
        category: Category::Info,
        summary: "Print performance metrics as JSON",
        syntax: "perf",
        args: &[],
        examples: &["rdpdo -s host perf", "rdpdo -s host pause 5 perf"],
        needs_connection: true,
        notes: "Reports FPS, frame count, time to first frame, bandwidth (kbps), \
                frame interval percentiles (P50/P99), and byte counters.",
    },
    CommandDoc {
        name: "pause",
        aliases: &["sleep", "wait"],
        category: Category::Info,
        summary: "Pause for N seconds (processes server PDUs during wait)",
        syntax: "pause <seconds>",
        args: &[(
            "<seconds>",
            "Duration in seconds (supports fractions: 0.5, 2.5).",
        )],
        examples: &[
            "rdpdo -s host type \"hello\" pause 2 capture /tmp/out.png",
            "rdpdo -s host key enter pause 0.5 key tab",
        ],
        needs_connection: true,
        notes: "Unlike a raw sleep, `pause` continues processing incoming RDP frames \
                during the wait. This keeps the session alive and prevents the server \
                from stalling on unacknowledged frames.",
    },
    CommandDoc {
        name: "help",
        aliases: &[],
        category: Category::Info,
        summary: "Show command help (this output)",
        syntax: "help [command]",
        args: &[("[command]", "Show detailed help for a specific command.")],
        examples: &["rdpdo help", "rdpdo help expect", "rdpdo help click"],
        needs_connection: false,
        notes: "Without an argument, lists all commands grouped by category. \
                With a command name, shows full syntax, arguments, examples, and notes.",
    },
    // ── Modifiers (Tier 2 gap audit) ──────────────────────────────
    CommandDoc {
        name: "retry",
        aliases: &[],
        category: Category::Scripting,
        summary: "Retry the next command up to N times on failure",
        syntax: "retry <count> <command...>",
        args: &[("<count>", "Maximum number of attempts (must be >= 1).")],
        examples: &[
            "rdpdo -s host retry 3 expect /tmp/desktop.png",
            "rdpdo -s host retry 5 waitfor /tmp/button.png 0.9 10",
            "rdpdo -s host soft retry 3 expect /tmp/optional.png",
        ],
        needs_connection: false,
        notes: "Applies to the next command in the chain. On failure, waits 500ms \
                between retries to let the screen settle. Combine with `soft` to \
                retry and then continue even if all attempts fail. The retry count \
                includes the first attempt: `retry 3` means up to 3 total tries.",
    },
    CommandDoc {
        name: "soft",
        aliases: &[],
        category: Category::Scripting,
        summary: "Make the next command non-fatal (log failure, continue chain)",
        syntax: "soft <command...>",
        args: &[],
        examples: &[
            "rdpdo -s host soft expect /tmp/optional-banner.png 0.9 5",
            "rdpdo -s host soft assert-pixel 100,50 #FF0000",
            "rdpdo -s host soft retry 3 waitfor /tmp/toast.png 0.9 10",
        ],
        needs_connection: false,
        notes: "On failure, prints 'SOFT FAIL: ...' to stderr and continues \
                the command chain. Soft failures are summarized at the end. \
                In JUnit output, soft failures appear as failed test cases. \
                Combine with `retry` for resilient automation scripts. \
                Inspired by Playwright's soft assertions and Robot Framework's \
                ContinueOnFailure mode.",
    },
    // ── New commands (Tier 1 gap audit) ─────────────────────────────
    CommandDoc {
        name: "mouse-hide",
        aliases: &[],
        category: Category::Input,
        summary: "Move cursor off-screen to prevent screenshot interference",
        syntax: "mouse-hide",
        args: &[],
        examples: &[
            "rdpdo -s host mouse-hide capture /tmp/clean.png",
            "rdpdo -s host mouse-hide expect /tmp/desktop.png",
        ],
        needs_connection: true,
        notes: "Moves the mouse to coordinates well beyond the desktop edge (65535,65535) \
                so it won't appear in screenshots. Essential for reliable visual matching \
                since cursor position causes false diffs. Equivalent to openQA's mouse_hide.",
    },
    CommandDoc {
        name: "wait-pixel",
        aliases: &[],
        category: Category::Pixel,
        summary: "Wait until pixel at position matches a color",
        syntax: "wait-pixel <pos> <color> [tolerance] [timeout]",
        args: &[
            ("<pos>", "Pixel position (x,y or named position)."),
            ("<color>", "Expected color as #RRGGBB or R,G,B."),
            ("[tolerance]", "Per-channel tolerance 0-255 (default: 30)."),
            ("[timeout]", "Timeout in seconds (default: 30)."),
        ],
        examples: &[
            "rdpdo -s host wait-pixel 100,50 #FF0000",
            "rdpdo -s host wait-pixel center #000000 10 60",
            "rdpdo -s host wait-pixel 0,0 255,255,255 30 15",
        ],
        needs_connection: true,
        notes: "Polls the pixel at the given position every 250ms until it matches the \
                expected color within tolerance. Lighter than full visual matching for \
                cases where a single pixel tells the story (loading indicators, status LEDs, \
                background color changes).",
    },
    CommandDoc {
        name: "assert-pixel",
        aliases: &[],
        category: Category::Pixel,
        summary: "Assert pixel at position matches expected color (fail if not)",
        syntax: "assert-pixel <pos> <color> [tolerance]",
        args: &[
            ("<pos>", "Pixel position (x,y or named position)."),
            ("<color>", "Expected color as #RRGGBB or R,G,B."),
            ("[tolerance]", "Per-channel tolerance 0-255 (default: 30)."),
        ],
        examples: &[
            "rdpdo -s host assert-pixel 960,540 #FFFFFF",
            "rdpdo -s host click 100,200 pause 1 assert-pixel 100,200 #00FF00",
        ],
        needs_connection: true,
        notes: "Reads the pixel once and fails immediately if it doesn't match. \
                Use `wait-pixel` if you need to wait for a transition.",
    },
    CommandDoc {
        name: "measure",
        aliases: &[],
        category: Category::Matching,
        summary: "Time how long until a visual match succeeds",
        syntax: "measure <template> [tolerance] [timeout] [--needles <dir>] [--tag <name>] [--label <name>]",
        args: &[
            ("<template>", "Reference image or needle to wait for."),
            ("[tolerance]", "Match threshold 0.0-1.0 (default: 0.95)."),
            ("[timeout]", "Maximum wait in seconds (default: 60)."),
            (
                "--label <name>",
                "Label for the measurement in JSON/JUnit output.",
            ),
            ("--needles <dir>", "Directory of needle JSON+PNG pairs."),
            ("--tag <name>", "Filter needles by tag."),
        ],
        examples: &[
            "rdpdo -s host type \"notepad\" key enter measure /tmp/notepad-ready.png --label app-launch",
            "rdpdo -s host --json measure /tmp/desktop.png 0.9 120 --label login-time",
        ],
        needs_connection: true,
        notes: "Like `waitfor` but reports elapsed time to stdout. With --json, output \
                includes {\"label\": \"...\", \"elapsed_ms\": N, \"matched\": true}. \
                Used for VDI performance testing: login time, app launch time, \
                screen transition timing. Fails if timeout expires without a match.",
    },
    // ── Session ─────────────────────────────────────────────────────
    CommandDoc {
        name: "session",
        aliases: &["repl", "interactive"],
        category: Category::Scripting,
        summary: "Enter interactive session mode (keeps connection alive)",
        syntax: "session",
        args: &[],
        examples: &[
            "rdpdo -s host session",
            "rdpdo -s host --no-auth click 400,400 session",
            "echo 'type hello\\nkey enter\\ncapture /tmp/out.png' | rdpdo -s host session",
        ],
        needs_connection: true,
        notes: "Reads commands from stdin one per line, same syntax as the command chain. \
                The RDP connection stays alive between commands, avoiding reconnection \
                overhead. Use 'quit' or 'exit' to disconnect. Shell commands can be \
                run with '!' prefix (e.g. '!ls /tmp'). Comments (#) and blank lines \
                are ignored. When stdin is piped, reads silently until EOF. \
                In interactive mode: readline editing, history (~/.config/rdpdo/history.txt), \
                Ctrl+C cancels current input. Session variables with 'set'/'get'/'vars'. \
                'disconnect' and 'reconnect' manage the connection without leaving the REPL.",
    },
    CommandDoc {
        name: "status",
        aliases: &[],
        category: Category::Info,
        summary: "Print compact session status (resolution, frames, FPS, bandwidth, uptime)",
        syntax: "status",
        args: &[],
        examples: &[
            "rdpdo -s host status",
            "rdpdo -s host session  # then type 'status' at the prompt",
        ],
        needs_connection: true,
        notes: "One-line summary of session health. With --json, outputs structured JSON \
                including server, resolution, graphics mode, frame count, avg FPS, \
                bytes received/sent, uptime, and disconnect state.",
    },
    CommandDoc {
        name: "exec",
        aliases: &[],
        category: Category::Scripting,
        summary: "Run a command on the remote host via SSH",
        syntax: "exec <user@host> <command>",
        args: &[
            ("<user@host>", "SSH destination (e.g. user@192.168.1.50)"),
            ("<command>", "Shell command to execute remotely"),
        ],
        examples: &[
            "rdpdo -s host exec user@host \"systemctl restart app\"",
            "rdpdo -s host exec root@host \"wl-paste\" capture /tmp/after.png",
        ],
        needs_connection: true,
        notes: "Runs ssh in BatchMode (no password prompts). Requires SSH key auth. \
                Fails the command chain if the remote command exits non-zero. \
                Useful for server-side verification between RDP GUI operations.",
    },
    CommandDoc {
        name: "watch",
        aliases: &[],
        category: Category::Info,
        summary: "Live frame rate monitoring for N seconds",
        syntax: "watch [duration]",
        args: &[("[duration]", "Monitoring duration in seconds (default: 10)")],
        examples: &[
            "rdpdo -s host watch",
            "rdpdo -s host watch 30",
            "rdpdo -s host --json watch 5",
        ],
        needs_connection: true,
        notes: "Prints per-second frame rate updates to stderr, then a summary. \
                With --json, outputs total frames, duration, and average FPS as JSON. \
                Useful for diagnosing rendering performance or codec issues.",
    },
];

// ── Output formatters ───────────────────────────────────────────────

/// Grouped, compact command table (category label + name/summary), shared by the
/// `--help` after-help summary of each binary. Caller supplies its own registry
/// and category order.
pub(crate) fn command_table(cmds: &[CommandDoc], category_order: &[Category]) -> String {
    let mut out = String::with_capacity(2048);
    for &cat in category_order {
        let in_cat: Vec<&CommandDoc> = cmds.iter().filter(|c| c.category == cat).collect();
        if in_cat.is_empty() {
            continue;
        }
        let _ = writeln!(out, "  {}", cat.label());
        for cmd in in_cat {
            let name_col = if cmd.aliases.is_empty() {
                cmd.name.to_owned()
            } else {
                format!("{} ({})", cmd.name, cmd.aliases.join(", "))
            };
            let _ = writeln!(out, "    {name_col:<34} {}", cmd.summary);
        }
        out.push('\n');
    }
    out
}

/// Grouped, detailed command listing (name, aliases, summary, syntax), shared by
/// the `<bin> help` full listing of each binary.
pub(crate) fn command_listing(cmds: &[CommandDoc], category_order: &[Category]) -> String {
    let mut out = String::with_capacity(4096);
    for &cat in category_order {
        let in_cat: Vec<&CommandDoc> = cmds.iter().filter(|c| c.category == cat).collect();
        if in_cat.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{}:", cat.label());
        for cmd in in_cat {
            let aliases = if cmd.aliases.is_empty() {
                String::new()
            } else {
                format!(" (aliases: {})", cmd.aliases.join(", "))
            };
            let conn = if cmd.needs_connection {
                ""
            } else {
                " [offline]"
            };
            let _ = writeln!(out, "  {}{aliases}{conn}", cmd.name);
            let _ = writeln!(out, "    {}", cmd.summary);
            let _ = writeln!(out, "    Syntax: {}", cmd.syntax);
            out.push('\n');
        }
    }
    out
}

/// Compact command summary for appending to `--help` output.
pub(crate) fn after_help_text() -> String {
    let mut out = String::with_capacity(4096);
    out.push_str("COMMANDS:\n");
    out.push_str("  Commands are chained on the command line after global flags.\n");
    out.push_str(
        "  Run `rdpdo help` for full documentation or `rdpdo help <cmd>` for details.\n\n",
    );

    out.push_str(&command_table(ALL_COMMANDS, CATEGORY_ORDER));

    out.push_str("COORDINATE FORMATS:\n");
    out.push_str("  Pixels:      500,300\n");
    out.push_str("  Percentage:  50%,50%\n");
    out.push_str("  Named:       center, top-left, top-right, bottom-left, bottom-right,\n");
    out.push_str("               top-center, bottom-center, left-center, right-center\n\n");

    out.push_str("TOLERANCE:\n");
    out.push_str("  Accepts 0.0-1.0 or 1-100 (divided by 100). Both 0.95 and 95 mean 95%.\n\n");

    out.push_str("OFFLINE COMMANDS:\n");
    out.push_str("  These commands work without -s/--server:\n");
    out.push_str("    diff, convert, audio-verify, help\n\n");

    out.push_str("EXAMPLES:\n");
    out.push_str("  rdpdo -s host type \"hello\" key enter pause 2 capture /tmp/out.png\n");
    out.push_str("  rdpdo -s host --no-auth expect /tmp/desktop.png 0.95 30\n");
    out.push_str("  rdpdo -s host expectclick /tmp/ok-button.png\n");
    out.push_str("  rdpdo -s host --record /tmp/session.rdpdo run ./setup.rdpdo-script\n");
    out.push_str("  rdpdo diff /tmp/before.png /tmp/after.png --output /tmp/diff.png\n");

    out
}

/// Full help listing (all commands grouped by category).
pub(crate) fn full_help() -> String {
    let mut out = String::with_capacity(8192);
    out.push_str("rdpdo - RDP automation tool\n\n");
    out.push_str("Commands are chained on the command line after global flags:\n");
    out.push_str("  rdpdo -s host:3389 type \"hello\" key enter pause 2 capture /tmp/out.png\n\n");
    out.push_str("Run `rdpdo --help` for global flags. Run `rdpdo help <cmd>` for details.\n");
    out.push_str("─────────────────────────────────────────────────────────────────────\n\n");

    out.push_str(&command_listing(ALL_COMMANDS, CATEGORY_ORDER));

    out.push_str("Coordinate formats: pixels (500,300), percentage (50%,50%), named (center)\n");
    out.push_str("Tolerance: 0.0-1.0 or 1-100 (95 and 0.95 are equivalent)\n");
    out.push_str("Offline commands (no -s needed): diff, convert, audio-verify, help\n");

    out
}

/// Detailed help for a single command.
pub(crate) fn command_help(bin: &str, cmds: &[CommandDoc], name: &str) -> Option<String> {
    let cmd = cmds
        .iter()
        .find(|c| c.name == name || c.aliases.contains(&name))?;

    let mut out = String::with_capacity(2048);
    let _ = writeln!(out, "{bin} {}", cmd.name);
    if !cmd.aliases.is_empty() {
        let _ = writeln!(out, "Aliases: {}", cmd.aliases.join(", "));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", cmd.summary);
    let _ = writeln!(out);
    let _ = writeln!(out, "SYNTAX:");
    let _ = writeln!(out, "  {}", cmd.syntax);
    let _ = writeln!(out);

    if !cmd.args.is_empty() {
        let _ = writeln!(out, "ARGUMENTS:");
        let max_name = cmd.args.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        for (arg_name, desc) in cmd.args {
            let _ = writeln!(out, "  {arg_name:<max_name$}  {desc}");
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "EXAMPLES:");
    for ex in cmd.examples {
        let _ = writeln!(out, "  {ex}");
    }
    let _ = writeln!(out);

    if !cmd.notes.is_empty() {
        let _ = writeln!(out, "NOTES:");
        // Word-wrap notes at ~76 columns with 2-space indent
        let mut line = String::from("  ");
        for word in cmd.notes.split_whitespace() {
            if line.len() + word.len() + 1 > 78 && line.len() > 2 {
                let _ = writeln!(out, "{line}");
                line = String::from("  ");
            }
            if line.len() > 2 {
                line.push(' ');
            }
            line.push_str(word);
        }
        if line.len() > 2 {
            let _ = writeln!(out, "{line}");
        }
        let _ = writeln!(out);
    }

    let conn_label = if cmd.needs_connection {
        "Yes (requires -s/--server)"
    } else {
        "No (offline command)"
    };
    let _ = writeln!(out, "CONNECTION: {conn_label}");

    Some(out)
}

/// List of all command names (for error messages and suggestions).
pub(crate) fn all_command_names(cmds: &[CommandDoc]) -> Vec<&'static str> {
    let mut names: Vec<&str> = cmds
        .iter()
        .flat_map(|c| std::iter::once(c.name).chain(c.aliases.iter().copied()))
        .collect();
    names.sort_unstable();
    names
}
