//! Connectionless RDP security/capability probe.
//!
//! Drives only the X.224 security negotiation (`RDP_NEG_REQ` / `RDP_NEG_RSP`) — no
//! authentication, no MCS, no session. This reveals the server's security
//! posture and a few capability flags entirely pre-auth, which is rdpsee's core
//! differentiator over tools that must log in first.
//!
//! We talk the nego layer directly via `ironrdp-pdu` (the same `encode_vec` /
//! `decode::<X224<..>>` path the connector uses) rather than driving the full
//! `ClientConnector`, whose connection state is internal and would carry us all
//! the way to `CredSSP`.

use std::time::Duration;

use anyhow::{Result, bail};
use ironrdp_core::{decode, encode_vec};
use ironrdp_pdu::{
    nego::{ConnectionConfirm, ConnectionRequest, FailureCode, RequestFlags, SecurityProtocol},
    x224::X224,
};
use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use crate::connection::Destination;

/// Maximum size we will read for a negotiation response (the `RDP_NEG_RSP` is tiny;
/// this bounds a hostile or non-RDP peer).
const MAX_TPKT_LEN: usize = 4096;

/// Result of a connectionless security-negotiation probe.
#[derive(Debug, Serialize)]
pub(crate) struct ProbeReport {
    pub server: String,
    pub reachable: bool,
    /// Selected security protocol, e.g. `HYBRID_EX` or `STANDARD_RDP_SECURITY`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
    /// True when the server selected `CredSSP` (`HYBRID` / `HYBRID_EX`), i.e. NLA.
    pub nla_required: bool,
    /// True when the response advertises the EGFX dynamic-channel graphics pipeline.
    pub egfx_capable: bool,
    pub restricted_admin: bool,
    pub redirected_auth: bool,
    pub extended_client_data: bool,
    /// Set when the server rejected negotiation, e.g. `HYBRID_REQUIRED_BY_SERVER`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// Free-form note (unreachable reason, or "not an RDP server").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl ProbeReport {
    fn blank(server: String) -> Self {
        Self {
            server,
            reachable: false,
            security: None,
            nla_required: false,
            egfx_capable: false,
            restricted_admin: false,
            redirected_auth: false,
            extended_client_data: false,
            failure: None,
            note: None,
        }
    }

    pub(crate) fn print_human(&self) {
        println!("server:           {}", self.server);
        if !self.reachable {
            println!(
                "reachable:        no ({})",
                self.note.as_deref().unwrap_or("unknown")
            );
            return;
        }
        println!("reachable:        yes");

        if let Some(security) = &self.security {
            println!("security:         {security}");
            println!(
                "nla (CredSSP):    {}",
                if self.nla_required {
                    "required"
                } else {
                    "not required"
                }
            );
            println!("egfx capable:     {}", yes_no(self.egfx_capable));
            println!("restricted admin: {}", yes_no(self.restricted_admin));
            println!("redirected auth:  {}", yes_no(self.redirected_auth));
            println!("ext client data:  {}", yes_no(self.extended_client_data));
        }

        if let Some(failure) = &self.failure {
            println!("negotiation:      rejected ({failure})");
            if self.nla_required {
                println!("nla (CredSSP):    required");
            }
        }

        if self.security.is_none()
            && self.failure.is_none()
            && let Some(note) = &self.note
        {
            println!("note:             {note}");
        }
    }

    pub(crate) fn print_table_header() {
        println!(
            "{:<22} {:<6} {:<22} {:<4} {:<4}",
            "TARGET", "REACH", "SECURITY", "NLA", "EGFX"
        );
    }

    pub(crate) fn print_row(&self) {
        let reach = if self.reachable { "yes" } else { "no" };
        let security = self
            .security
            .clone()
            .or_else(|| self.failure.as_ref().map(|f| format!("rejected:{f}")))
            .unwrap_or_else(|| "-".to_owned());
        let nla = if !self.reachable {
            "-"
        } else if self.nla_required {
            "yes"
        } else {
            "no"
        };
        let egfx = if self.reachable {
            yes_no(self.egfx_capable)
        } else {
            "-"
        };
        println!(
            "{:<22} {reach:<6} {security:<22} {nla:<4} {egfx}",
            self.server
        );
    }

    /// True when this report satisfies every requested capability.
    pub(crate) fn meets(&self, expected: &Expectations) -> bool {
        if expected.reachable && !self.reachable {
            return false;
        }
        if expected.nla && !self.nla_required {
            return false;
        }
        if expected.egfx && !self.egfx_capable {
            return false;
        }
        if expected.tls {
            let enhanced = self
                .security
                .as_deref()
                .is_some_and(|s| s != "STANDARD_RDP_SECURITY");
            if !enhanced {
                return false;
            }
        }
        true
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Probe a server's security posture without authenticating.
///
/// Returns a report rather than erroring for the common "server not listening"
/// and "not an RDP server" cases, so a future multi-target sweep can collect
/// them as ordinary rows.
pub(crate) async fn probe(dest: &Destination, timeout: Duration) -> Result<ProbeReport> {
    let addr = dest.addr_string();

    let stream = match tokio::time::timeout(timeout, TcpStream::connect(&addr)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(e)) => {
            let mut report = ProbeReport::blank(addr);
            report.note = Some(e.to_string());
            return Ok(report);
        }
        Err(_) => {
            let mut report = ProbeReport::blank(addr);
            report.note = Some("connect timeout".to_owned());
            return Ok(report);
        }
    };

    negotiate(stream, addr, timeout).await
}

async fn negotiate(mut stream: TcpStream, addr: String, timeout: Duration) -> Result<ProbeReport> {
    // Offer the three mainstream enhanced-security protocols and let the server
    // pick its preferred one. RDSTLS (RD Gateway) and RDSAAD (Azure AD) are niche
    // and omitted for now.
    let requested = SecurityProtocol::SSL | SecurityProtocol::HYBRID | SecurityProtocol::HYBRID_EX;

    let mut report = ProbeReport::blank(addr);
    report.reachable = true;

    let confirm = match negotiate_protocol(&mut stream, requested, timeout).await {
        Ok(confirm) => confirm,
        Err(e) => {
            report.note = Some(format!("not an RDP server? {e}"));
            return Ok(report);
        }
    };

    match confirm {
        ConnectionConfirm::Response { flags, protocol } => {
            use ironrdp_pdu::nego::ResponseFlags;
            report.security = Some(protocol.to_string());
            report.nla_required = protocol.contains(SecurityProtocol::HYBRID)
                || protocol.contains(SecurityProtocol::HYBRID_EX);
            report.egfx_capable = flags.contains(ResponseFlags::DYNVC_GFX_PROTOCOL_SUPPORTED);
            report.restricted_admin =
                flags.contains(ResponseFlags::RESTRICTED_ADMIN_MODE_SUPPORTED);
            report.redirected_auth =
                flags.contains(ResponseFlags::REDIRECTED_AUTHENTICATION_MODE_SUPPORTED);
            report.extended_client_data =
                flags.contains(ResponseFlags::EXTENDED_CLIENT_DATA_SUPPORTED);
        }
        ConnectionConfirm::Failure { code } => {
            report.failure = Some(failure_name(code));
            // A HYBRID_REQUIRED rejection still tells us NLA is mandatory.
            report.nla_required = u32::from(code) == 5;
        }
    }

    Ok(report)
}

/// Send an X.224 Connection Request offering `requested` protocols and read the
/// server's Connection Confirm. Shared by the scan probe and cert inspection.
pub(crate) async fn negotiate_protocol(
    stream: &mut TcpStream,
    requested: SecurityProtocol,
    timeout: Duration,
) -> Result<ConnectionConfirm> {
    let request = ConnectionRequest {
        nego_data: None,
        flags: RequestFlags::empty(),
        protocol: requested,
    };
    let wire =
        encode_vec(&X224(request)).map_err(|e| anyhow::anyhow!("encode nego request: {e}"))?;
    stream.write_all(&wire).await?;
    stream.flush().await?;
    read_confirm(stream, timeout).await
}

/// Read one TPKT-framed X.224 PDU and decode it as a Connection Confirm.
async fn read_confirm(stream: &mut TcpStream, timeout: Duration) -> Result<ConnectionConfirm> {
    let mut header = [0u8; 4];
    tokio::time::timeout(timeout, stream.read_exact(&mut header))
        .await
        .map_err(|_| anyhow::anyhow!("timeout reading negotiation response"))??;

    if header[0] != 0x03 {
        bail!("unexpected TPKT version {:#04x}", header[0]);
    }

    let total = usize::from(u16::from_be_bytes([header[2], header[3]]));
    if !(4..=MAX_TPKT_LEN).contains(&total) {
        bail!("invalid TPKT length {total}");
    }

    let mut buf = vec![0u8; total];
    buf[..4].copy_from_slice(&header);
    stream.read_exact(&mut buf[4..]).await?;

    let confirm = decode::<X224<ConnectionConfirm>>(&buf)
        .map_err(|e| anyhow::anyhow!("decode negotiation response: {e}"))?;
    Ok(confirm.0)
}

/// MS-RDPBCGR negotiation failure codes.
fn failure_name(code: FailureCode) -> String {
    match u32::from(code) {
        1 => "SSL_REQUIRED_BY_SERVER".to_owned(),
        2 => "SSL_NOT_ALLOWED_BY_SERVER".to_owned(),
        3 => "SSL_CERT_NOT_ON_SERVER".to_owned(),
        4 => "INCONSISTENT_FLAGS".to_owned(),
        5 => "HYBRID_REQUIRED_BY_SERVER".to_owned(),
        6 => "SSL_WITH_USER_AUTH_REQUIRED_BY_SERVER".to_owned(),
        other => format!("UNKNOWN({other})"),
    }
}

/// Capability requirements for `--ci` gating.
#[derive(Debug, Default)]
pub(crate) struct Expectations {
    reachable: bool,
    tls: bool,
    nla: bool,
    egfx: bool,
}

impl Expectations {
    /// Parse a comma-separated capability list (e.g. "nla,egfx").
    pub(crate) fn parse(spec: &str) -> Result<Self> {
        let mut expected = Self::default();
        for token in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match token {
                "reachable" => expected.reachable = true,
                "tls" => expected.tls = true,
                "nla" => expected.nla = true,
                "egfx" => expected.egfx = true,
                other => {
                    bail!("unknown --expect capability '{other}' (use: reachable, tls, nla, egfx)")
                }
            }
        }
        Ok(expected)
    }
}

/// Expand target specs (hostname, host:port, or IPv4 `a.b.c.d/n`) into destinations.
pub(crate) fn expand_targets(specs: &[String]) -> Result<Vec<Destination>> {
    const MAX_TARGETS: usize = 65536;

    let mut out = Vec::new();
    for spec in specs {
        match expand_cidr_v4(spec) {
            Some(cidr) => out.extend(cidr?),
            None => out.push(spec.parse::<Destination>()?),
        }
        if out.len() > MAX_TARGETS {
            bail!("too many targets (> {MAX_TARGETS}); narrow the range");
        }
    }
    Ok(out)
}

/// Expand an IPv4 `a.b.c.d/n` range. Returns `None` when `spec` is not IPv4 CIDR.
fn expand_cidr_v4(spec: &str) -> Option<Result<Vec<Destination>>> {
    let (ip_str, prefix_str) = spec.split_once('/')?;
    let ip: std::net::Ipv4Addr = ip_str.parse().ok()?;
    let prefix: u32 = prefix_str.parse().ok()?;
    Some(expand_cidr_v4_range(ip, prefix))
}

fn expand_cidr_v4_range(ip: std::net::Ipv4Addr, prefix: u32) -> Result<Vec<Destination>> {
    const RDP_DEFAULT_PORT: u16 = 3389;

    if prefix > 32 {
        bail!("invalid CIDR prefix /{prefix}");
    }
    if prefix < 16 {
        bail!("CIDR /{prefix} too large (max 65536 hosts; use /16 or smaller)");
    }

    let host_bits = 32 - prefix;
    let base = u32::from(ip) & (u32::MAX << host_bits);
    let count = 1u32 << host_bits;

    let mut out = Vec::with_capacity(count as usize);
    for offset in 0..count {
        out.push(Destination {
            name: std::net::Ipv4Addr::from(base + offset).to_string(),
            port: RDP_DEFAULT_PORT,
        });
    }
    Ok(out)
}

/// Probe many targets concurrently (bounded), preserving input order.
pub(crate) async fn scan_many(targets: Vec<Destination>, timeout: Duration) -> Vec<ProbeReport> {
    const MAX_CONCURRENT: usize = 256;

    let mut reports = Vec::with_capacity(targets.len());
    for chunk in targets.chunks(MAX_CONCURRENT) {
        let mut handles = Vec::with_capacity(chunk.len());
        for dest in chunk {
            let dest = dest.clone();
            handles.push(tokio::spawn(async move { probe(&dest, timeout).await }));
        }
        for handle in handles {
            if let Ok(Ok(report)) = handle.await {
                reports.push(report);
            }
        }
    }
    reports
}
