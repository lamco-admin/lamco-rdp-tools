use std::{fmt::Write, time::Duration};

use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct ConnectionReport {
    pub connected: bool,
    pub security_protocol: String,
    pub desktop_size: DesktopSizeReport,
    pub static_channels: Vec<String>,
    pub egfx_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_depth: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub egfx_caps: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub codecs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performance: Option<PerformanceReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<ClipboardReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resize: Option<ResizeReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ConnectionReport {
    /// Build a capability report from observed connection facts. Shared by
    /// rdpsee's `report` verb and rdpdo's `info` command so both surface the same
    /// real data. The session-activity fields (performance, clipboard, resize,
    /// screenshot) are left unset; this is the capability snapshot.
    pub(crate) fn observe(
        observed: &crate::connection::ObservedCapabilities,
        egfx_caps: Option<String>,
        egfx_active: bool,
        width: u16,
        height: u16,
    ) -> Self {
        Self {
            connected: true,
            security_protocol: observed.security.clone(),
            desktop_size: DesktopSizeReport { width, height },
            static_channels: observed.channels.clone(),
            egfx_active,
            color_depth: observed.color_depth,
            compression: observed.compression.clone(),
            egfx_caps,
            codecs: observed.codecs.clone(),
            performance: None,
            clipboard: None,
            resize: None,
            screenshot_path: None,
            error: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct ClipboardReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round_trip_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct DesktopSizeReport {
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Serialize)]
pub(crate) struct ResizeReport {
    pub requested_width: u32,
    pub requested_height: u32,
    pub sent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_resize_fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_resize_fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_resize_p50_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_resize_p50_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_resize_p99_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_resize_p99_ms: Option<f64>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PerformanceReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_first_frame_ms: Option<u64>,
    pub total_frames: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_fps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth_kbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_interval_p50_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_interval_p99_ms: Option<f64>,
    pub bytes_received: u64,
    pub bytes_sent: u64,
}

// ── JUnit XML report ────────────────────────────────────────────────

/// A single test case result from one command in the chain.
pub(crate) struct JunitTestCase {
    pub name: String,
    pub duration: Duration,
    pub failure: Option<String>,
}

/// Accumulates test cases and writes `JUnit` XML.
pub(crate) struct JunitReport {
    pub suite_name: String,
    pub cases: Vec<JunitTestCase>,
}

impl JunitReport {
    pub(crate) fn new(suite_name: &str) -> Self {
        Self {
            suite_name: suite_name.to_owned(),
            cases: Vec::new(),
        }
    }

    pub(crate) fn add(&mut self, name: &str, duration: Duration, failure: Option<String>) {
        self.cases.push(JunitTestCase {
            name: name.to_owned(),
            duration,
            failure,
        });
    }

    pub(crate) fn to_xml(&self) -> String {
        let total_time: f64 = self.cases.iter().map(|c| c.duration.as_secs_f64()).sum();
        let failures = self.cases.iter().filter(|c| c.failure.is_some()).count();

        let mut xml = String::with_capacity(4096);
        xml.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let _ = writeln!(
            xml,
            "<testsuites tests=\"{}\" failures=\"{failures}\" time=\"{total_time:.3}\">",
            self.cases.len()
        );
        let _ = writeln!(
            xml,
            "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{failures}\" time=\"{total_time:.3}\">",
            xml_escape(&self.suite_name),
            self.cases.len()
        );

        for case in &self.cases {
            let secs = case.duration.as_secs_f64();
            if let Some(ref msg) = case.failure {
                let _ = writeln!(
                    xml,
                    "    <testcase name=\"{}\" time=\"{secs:.3}\">",
                    xml_escape(&case.name)
                );
                let _ = writeln!(xml, "      <failure message=\"{}\"/>", xml_escape(msg));
                xml.push_str("    </testcase>\n");
            } else {
                let _ = writeln!(
                    xml,
                    "    <testcase name=\"{}\" time=\"{secs:.3}\"/>",
                    xml_escape(&case.name)
                );
            }
        }

        xml.push_str("  </testsuite>\n");
        xml.push_str("</testsuites>\n");
        xml
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
