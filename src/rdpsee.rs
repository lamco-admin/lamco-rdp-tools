//! rdpsee application: the "observe" companion to rdpdo.
//!
//! rdpsee inspects an RDP server and reports; it never drives the session. It
//! shares rdpdo's building blocks (connection, session, report) and the same
//! help framework (`crate::help`), so the two tools present an identical help
//! and flag style.
//!
//! The capabilities span three observation tiers: connectionless (`scan`, a
//! pre-auth X.224 security probe over one or many targets), TLS-handshake with
//! no authentication (`cert` certificate inspection, `id` server fingerprint),
//! and a completed session (`report` capability report, `shot` recon
//! screenshot). Only `report` and `shot` may authenticate, and only to read.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use clap::Parser;
use tracing::info;

use crate::{
    capture, cert, connection, fingerprint, help,
    help::{Category, CommandDoc},
    probe, report,
    session::HeadlessSession,
};

/// Inspect and report on an RDP server (the observe companion to rdpdo).
///
///   rdpsee -s host report
#[derive(Parser, Debug)]
#[command(name = "rdpsee", version, after_help = crate::rdpsee::after_help())]
struct Cli {
    /// Server address (host or host:port, default port 3389).
    /// Not required for the offline `help` command.
    #[arg(short = 's', long, default_value = "")]
    server: String,

    /// RDP username (optional; rdpsee observes pre-auth where the server allows it).
    #[arg(short, long)]
    user: Option<String>,

    /// RDP password.
    #[arg(short, long)]
    password: Option<String>,

    /// Disable NLA/CredSSP (use TLS only).
    #[arg(long)]
    no_nla: bool,

    /// No authentication (for servers that allow anonymous connections).
    #[arg(long)]
    no_auth: bool,

    /// Overall timeout in seconds.
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Desktop width requested during capability negotiation.
    #[arg(long, default_value_t = 1920)]
    width: u16,

    /// Desktop height requested during capability negotiation.
    #[arg(long, default_value_t = 1080)]
    height: u16,

    /// Machine-readable JSON output.
    #[arg(long)]
    json: bool,

    /// Verbose tracing to stderr.
    #[arg(long)]
    verbose: bool,

    /// Exit non-zero unless every scanned target meets --expect (CI gating).
    #[arg(long)]
    ci: bool,

    /// Required capabilities for --ci, comma-separated: reachable,tls,nla,egfx.
    #[arg(long)]
    expect: Option<String>,

    /// Action: `scan`, `cert`, `id`, `shot`, `report` (default), or
    /// `help [command]`, followed by any targets. Global flags may appear anywhere.
    commands: Vec<String>,
}

/// Display order for rdpsee's command categories.
const CATEGORY_ORDER: &[Category] = &[Category::Report, Category::Info];

/// rdpsee's command registry, rendered through the shared `help` formatters.
static COMMANDS: &[CommandDoc] = &[
    CommandDoc {
        name: "scan",
        aliases: &[],
        category: Category::Report,
        summary: "Probe one or many servers' security posture pre-auth (no login)",
        syntax: "scan [target...] [--ci] [--expect <caps>]",
        args: &[
            (
                "[target...]",
                "Hosts, host:port, or IPv4 CIDR (e.g. 10.0.0.0/24). Falls back to \
                 -s/--server. CIDR is /16 to /32, capped at 65536 hosts.",
            ),
            (
                "--ci",
                "Exit non-zero unless every target meets --expect (CI gating).",
            ),
            (
                "--expect <caps>",
                "Comma-separated capabilities required by --ci: reachable, tls, nla, egfx.",
            ),
        ],
        examples: &[
            "rdpsee -s host scan",
            "rdpsee scan 10.0.0.0/24",
            "rdpsee scan host1 host2:3390 --json",
            "rdpsee scan 10.0.0.0/24 --ci --expect tls,nla,egfx",
        ],
        needs_connection: true,
        notes: "Drives only the X.224 security negotiation (RDP_NEG_REQ / RDP_NEG_RSP): no \
                TLS handshake, no MCS, no authentication. This pre-auth probe is rdpsee's \
                core differentiator over tools that must log in first. It reports \
                reachability, the selected security protocol (STANDARD_RDP_SECURITY, \
                SSL/TLS, HYBRID, or HYBRID_EX), whether NLA/CredSSP is required, and the \
                pre-auth capability flags: EGFX graphics pipeline, restricted-admin mode, \
                redirected authentication, and extended client data. A rejected negotiation \
                is reported with its failure code (e.g. SSL_REQUIRED_BY_SERVER, \
                HYBRID_REQUIRED_BY_SERVER), which still reveals what the server requires. \
                Targets accept hostnames, host:port, and IPv4 CIDR ranges, probed \
                concurrently (up to 256 at once) in input order. A single target prints a \
                detail block; multiple targets print a compact sweep table; --json always \
                emits the full array. With --ci, exits non-zero unless every target meets \
                --expect.",
    },
    CommandDoc {
        name: "cert",
        aliases: &[],
        category: Category::Report,
        summary: "Inspect a server's TLS certificate (no login required)",
        syntax: "cert",
        args: &[],
        examples: &["rdpsee -s host cert", "rdpsee -s host --json cert"],
        needs_connection: true,
        notes: "Negotiates an enhanced-security protocol (SSL/HYBRID/HYBRID_EX), completes \
                the TLS handshake, and reports the server certificate without authenticating \
                (no CredSSP, no session). Reported fields: subject, issuer, self-signed \
                status, validity window (not-before / not-after), serial number, signature \
                and public-key algorithms (common OIDs mapped to names such as SHA256-RSA or \
                ECDSA, otherwise the dotted OID), subject alternative names, and the SHA-256 \
                fingerprint of the certificate DER. Fails if the server offers only standard \
                RDP security, since there is no TLS to inspect. The negotiated TLS version \
                and cipher suite are not yet shown: the published ironrdp-tls API does not \
                surface them, pending Devolutions/IronRDP PR #1384.",
    },
    CommandDoc {
        name: "id",
        aliases: &[],
        category: Category::Report,
        summary: "Fingerprint a server (security + capability + certificate profile)",
        syntax: "id",
        args: &[],
        examples: &["rdpsee -s host id", "rdpsee -s host --json id"],
        needs_connection: true,
        notes: "Negotiates TLS (no authentication) and derives a stable JA4-style \
                fingerprint of the server's configuration, distinct from its exact-instance \
                identity (the certificate SHA-256, also reported). Two servers with the same \
                fingerprint share the same RDP security and capability posture; identical \
                certificate SHA-256 means the same instance. \
                Fingerprint format: rdp_<sec><caps>_<signing><key>/<hash>. \
                <sec> security tier: q=HYBRID_EX, h=HYBRID, s=SSL/TLS, r=standard-RDP. \
                <caps> capability flags, any of: g=EGFX, a=restricted-admin, \
                d=redirected-auth, e=extended-client-data (- when none). \
                <signing>: S=self-signed, C=CA-signed. \
                <key> server key type: r=RSA, e=ECDSA, d=Ed25519, x=other. \
                <hash>: 16 hex chars, a truncated SHA-256 over the raw protocol bits and \
                certificate algorithm OIDs (independent of name formatting). Also reports \
                the selected security protocol, NLA-required, EGFX-capable, public-key and \
                signature algorithms, and self-signed status.",
    },
    CommandDoc {
        name: "shot",
        aliases: &[],
        category: Category::Report,
        summary: "Capture a screenshot of the server's current screen (PNG)",
        syntax: "shot [path]",
        args: &[("[path]", "Output PNG path (default: screenshot.png).")],
        examples: &[
            "rdpsee -s host shot",
            "rdpsee -s host shot login.png",
            "rdpsee -s host -u user -p pass shot desktop.png",
        ],
        needs_connection: true,
        notes: "Completes the RDP connection (unlike scan/cert/id it needs decoded frames), \
                waits for the screen to render, then saves a PNG. It polls for real content \
                for up to 10 seconds in 250ms steps, counting EGFX frames that arrive over \
                the dynamic graphics channel as well as bitmap updates, then lets the screen \
                settle for one more second before capturing. With no credentials it captures \
                the login screen where the server allows it (pair with --no-nla / --no-auth); \
                pass -u/-p to capture the post-login desktop. Reuses rdpdo's capture path.",
    },
    CommandDoc {
        name: "report",
        aliases: &[],
        category: Category::Report,
        summary: "Connect and report the server's negotiated capabilities",
        syntax: "report",
        args: &[],
        examples: &[
            "rdpsee -s host report",
            "rdpsee -s host -u user -p pass report",
            "rdpsee -s host --json report",
        ],
        needs_connection: true,
        notes: "Completes a connection (authenticating only if -u/-p are supplied) and reports \
                the negotiated capabilities: the selected security protocol, desktop size, \
                color depth, graphics mode (EGFX vs bitmap) with the server-confirmed EGFX \
                tier, the bitmap codecs the server advertised (RemoteFX, NSCodec), bulk \
                compression, and the static channels actually joined. With --json, emits the \
                structured ConnectionReport. This is the default action when no verb is given. \
                For pre-auth security posture across one or many targets without logging in, \
                use `scan`.",
    },
    CommandDoc {
        name: "help",
        aliases: &[],
        category: Category::Info,
        summary: "Show command help (this output)",
        syntax: "help [command]",
        args: &[("[command]", "Show detailed help for a specific command.")],
        examples: &["rdpsee help", "rdpsee help scan", "rdpsee help id"],
        needs_connection: false,
        notes: "Without an argument, lists all commands grouped by category. With a command \
                name, shows full syntax, arguments, examples, and notes. Works offline (no \
                -s/--server needed).",
    },
];

/// Compact command summary appended to `rdpsee --help`.
fn after_help() -> String {
    let mut out = String::with_capacity(2048);
    out.push_str("COMMANDS:\n");
    out.push_str("  rdpsee inspects an RDP server and reports; it does not drive the session.\n");
    out.push_str(
        "  Run `rdpsee help` for full documentation or `rdpsee help <cmd>` for details.\n\n",
    );

    out.push_str(&help::command_table(COMMANDS, CATEGORY_ORDER));

    out.push_str("OBSERVATION TIERS:\n");
    out.push_str("  Connectionless (no auth):   scan\n");
    out.push_str("  TLS handshake (no auth):    cert, id\n");
    out.push_str("  Completed session:          report, shot  (auth only with -u/-p)\n\n");

    out.push_str("TARGET FORMATS (scan takes many; other verbs take one via -s):\n");
    out.push_str("  Host:        host   or   host:3389\n");
    out.push_str("  IPv4 CIDR:   10.0.0.0/24   (scan only, /16 to /32)\n\n");

    out.push_str("EXIT CODES:\n");
    out.push_str("  0  success (with scan --ci, every target met --expect)\n");
    out.push_str("  1  failure, timeout, or an unmet --expect\n\n");

    out.push_str("OFFLINE COMMANDS:\n");
    out.push_str("  These commands work without -s/--server:\n");
    out.push_str("    help\n\n");

    out.push_str("EXAMPLES:\n");
    out.push_str("  rdpsee -s host scan\n");
    out.push_str("  rdpsee scan 10.0.0.0/24 --ci --expect tls,nla\n");
    out.push_str("  rdpsee -s host cert\n");
    out.push_str("  rdpsee -s host id\n");
    out.push_str("  rdpsee -s host -u user -p pass shot desktop.png\n");
    out.push_str("  rdpsee -s host --json report\n");

    out
}

/// Full help listing for `rdpsee help`.
fn full_help() -> String {
    let mut out = String::with_capacity(2048);
    out.push_str("rdpsee - RDP server inspection tool (the observe companion to rdpdo)\n\n");
    out.push_str("Usage: rdpsee -s host[:port] [scan | cert | id | shot | report]\n");
    out.push_str("Run `rdpsee --help` for global flags. Run `rdpsee help <cmd>` for details.\n");
    out.push_str("─────────────────────────────────────────────────────────────────────\n\n");

    out.push_str(&help::command_listing(COMMANDS, CATEGORY_ORDER));

    out.push_str(
        "Observation tiers: scan (connectionless) | cert, id (TLS, no auth) | \
         report, shot (session)\n",
    );
    out.push_str("Targets: host, host:port, or IPv4 CIDR (scan only, /16 to /32)\n");
    out.push_str(
        "Exit codes: 0 success (scan --ci: all targets met --expect), 1 failure/timeout\n",
    );
    out.push_str("rdpsee observes only (capability/security/identity); it never sends input.\n");
    out.push_str("Offline commands (no -s needed): help\n");

    out
}

pub async fn run_cli() {
    let cli = Cli::parse();

    init_tracing(cli.verbose);

    let timeout = Duration::from_secs(cli.timeout);
    let result = tokio::time::timeout(timeout, run(&cli)).await;

    match result {
        Ok(Ok(())) => std::process::exit(0),
        Ok(Err(e)) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("error: timeout after {}s", cli.timeout);
            std::process::exit(1);
        }
    }
}

async fn run(cli: &Cli) -> Result<()> {
    let action = cli.commands.first().map_or("report", String::as_str);
    match action {
        "help" => {
            print_help(cli.commands.get(1).map(String::as_str))?;
            Ok(())
        }
        "scan" => scan_targets(cli).await,
        "cert" => cert_server(cli).await,
        "id" => id_server(cli).await,
        "shot" => shot_server(cli).await,
        "report" => report_server(cli).await,
        other => bail!("unknown command '{other}'. Run `rdpsee help` for a list of commands."),
    }
}

fn print_help(topic: Option<&str>) -> Result<()> {
    match topic {
        Some(name) => {
            if let Some(text) = help::command_help("rdpsee", COMMANDS, name) {
                print!("{text}");
            } else {
                eprintln!("unknown command '{name}'");
                eprintln!(
                    "available commands: {}",
                    help::all_command_names(COMMANDS).join(", ")
                );
                bail!("no help for '{name}'");
            }
        }
        None => print!("{}", full_help()),
    }
    Ok(())
}

async fn scan_targets(cli: &Cli) -> Result<()> {
    // Targets come from positional args after `scan`, plus -s/--server if set.
    let mut specs: Vec<String> = cli.commands.iter().skip(1).cloned().collect();
    if !cli.server.is_empty() {
        specs.push(cli.server.clone());
    }
    if specs.is_empty() {
        bail!("rdpsee scan needs a target: `rdpsee scan <host|cidr>...` or `-s host`");
    }

    let targets = probe::expand_targets(&specs)?;
    let timeout = Duration::from_secs(cli.timeout);

    let reports = if targets.len() == 1 {
        vec![probe::probe(&targets[0], timeout).await?]
    } else {
        probe::scan_many(targets, timeout).await
    };

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else if let [single] = reports.as_slice() {
        single.print_human();
    } else {
        probe::ProbeReport::print_table_header();
        for report in &reports {
            report.print_row();
        }
    }

    if cli.ci {
        let expected = probe::Expectations::parse(cli.expect.as_deref().unwrap_or(""))?;
        let failed: Vec<&str> = reports
            .iter()
            .filter(|report| !report.meets(&expected))
            .map(|report| report.server.as_str())
            .collect();
        if !failed.is_empty() {
            bail!(
                "ci: {}/{} target(s) did not meet --expect ({})",
                failed.len(),
                reports.len(),
                failed.join(", ")
            );
        }
    }

    Ok(())
}

async fn cert_server(cli: &Cli) -> Result<()> {
    if cli.server.is_empty() {
        bail!("rdpsee cert requires -s/--server");
    }
    let dest: connection::Destination = cli.server.parse()?;
    let info = cert::fetch_cert(&dest, Duration::from_secs(cli.timeout)).await?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        info.print_human();
    }
    Ok(())
}

async fn id_server(cli: &Cli) -> Result<()> {
    if cli.server.is_empty() {
        bail!("rdpsee id requires -s/--server");
    }
    let dest: connection::Destination = cli.server.parse()?;
    let info = fingerprint::fingerprint(&dest, Duration::from_secs(cli.timeout)).await?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&info)?);
    } else {
        info.print_human();
    }
    Ok(())
}

async fn shot_server(cli: &Cli) -> Result<()> {
    if cli.server.is_empty() {
        bail!("rdpsee shot requires -s/--server");
    }
    let path = cli
        .commands
        .get(1)
        .cloned()
        .unwrap_or_else(|| "screenshot.png".to_owned());

    let dest: connection::Destination = cli.server.parse()?;
    let connector_config = connection::build_connector_config(
        cli.user.as_deref(),
        cli.password.as_deref(),
        cli.no_auth,
        cli.no_nla,
        cli.width,
        cli.height,
    );

    info!(destination = %dest.addr_string(), "Capturing");

    let connect_result = connection::connect_headless(&dest, connector_config).await?;
    let mut session = HeadlessSession::from_connect_result(connect_result);
    session.set_server_addr(&cli.server);

    // shot needs decoded frames, so complete the connection and let the screen
    // render before capturing.
    // EGFX frames arrive via the DVC graphics pipeline, which `wait_for_frame`
    // does not count; poll the combined frame counter so this works for both
    // bitmap and EGFX servers. EGFX can take several seconds for the first frame.
    let deadline = Instant::now() + Duration::from_secs(10);
    while !session.has_content() && Instant::now() < deadline {
        session.run_for(Duration::from_millis(250)).await?;
    }
    if !session.has_content() {
        eprintln!("warning: no frame received within 10s; capturing whatever is present");
    }
    session.run_for(Duration::from_secs(1)).await?;

    capture::save_capture(&session, &path, None)?;
    let (width, height) = session.image_dimensions();
    let _ = session.shutdown().await;

    if cli.json {
        let report = serde_json::json!({
            "server": cli.server,
            "path": path,
            "width": width,
            "height": height,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("saved {path} ({width}x{height})");
    }

    Ok(())
}

async fn report_server(cli: &Cli) -> Result<()> {
    if cli.server.is_empty() {
        bail!("rdpsee report requires -s/--server");
    }

    let dest: connection::Destination = cli.server.parse()?;
    let connector_config = connection::build_connector_config(
        cli.user.as_deref(),
        cli.password.as_deref(),
        cli.no_auth,
        cli.no_nla,
        cli.width,
        cli.height,
    );

    info!(destination = %dest.addr_string(), "Inspecting");

    let connect_result = connection::connect_headless(&dest, connector_config).await?;
    let mut session = HeadlessSession::from_connect_result(connect_result);
    session.set_server_addr(&cli.server);
    let _ = session.wait_for_frame(Duration::from_secs(2)).await?;

    let (width, height) = session.image_dimensions();
    let report = report::ConnectionReport::observe(
        session.observed_capabilities(),
        session.egfx_caps(),
        session.egfx_active(),
        width,
        height,
    );

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let graphics = if report.egfx_active { "EGFX" } else { "bitmap" };
        println!("{:<12}{}", "server:", cli.server);
        println!("{:<12}{}", "security:", report.security_protocol);
        println!("{:<12}{width}x{height}", "desktop:");
        if let Some(depth) = report.color_depth {
            println!("{:<12}{depth}-bit", "color:");
        }
        println!("{:<12}{graphics}", "graphics:");
        if let Some(caps) = &report.egfx_caps {
            println!("{:<12}{caps}", "egfx:");
        }
        if !report.codecs.is_empty() {
            println!("{:<12}{}", "codecs:", report.codecs.join(", "));
        }
        if let Some(compression) = &report.compression {
            println!("{:<12}{compression}", "compression:");
        }
        println!("{:<12}{}", "channels:", report.static_channels.join(", "));
    }

    let _ = session.shutdown().await;
    Ok(())
}

fn init_tracing(verbose: bool) {
    use tracing_subscriber::EnvFilter;

    let filter = if verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
