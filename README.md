# lamco-rdp-tools

A toolkit of small, focused RDP utilities built on the
[IronRDP](https://github.com/Devolutions/IronRDP) protocol stack. It ships two
standalone binaries that are built, documented, and operated the same way:

- **`rdpsee`** *(observe)* inspects a server and reports on it, and never drives
  it: a connectionless pre-auth security scan (multi-target and CIDR), TLS
  certificate inspection, a JA4-style server fingerprint, a negotiated
  capability report, and a recon screenshot.
- **`rdpdo`** *(act)* drives an authenticated session: keyboard and mouse input,
  screen capture, visual matching, clipboard and file transfer, audio capture,
  display control, provisioning, calibration, session record/replay, and
  scripting. It is the headless RDP automation CLI that RDP never had, in the
  spirit of `vncdotool` but much expanded.

Neither tool needs a guest-side agent: both drive a standard RDP server over the
wire. **Website:** <https://lamco.ai/products/lamco-rdp-tools/>

## Tools

| Binary | Role | One line |
|---|---|---|
| `rdpsee` | observe | Inspect a server: scan, certificate, fingerprint, report, screenshot |
| `rdpdo` | act | Drive a session: input, capture, match, script, record |

Each is a single standalone binary. Both share one IronRDP connection core and
one help and flag style, accept the same connection flags (`-s/--server`,
`-u/--user`, `-p/--password`, `--no-nla`, `--no-auth`, `--timeout`,
`--width`/`--height`, `--verbose`), and honor `--json` on every reporting
command. Run `<tool> --help` for global flags and `<tool> help <command>` for
any command's full detail.

### rdpsee (observe)

Inspect an RDP server without driving it. **Model:** one action per invocation,
`rdpsee <command> [target]`, across three observation tiers.

| Command | Tier | What it does |
|---|---|---|
| `scan` | connectionless | Pre-auth security posture for one or many targets (host, `host:port`, IPv4 CIDR), probed concurrently. Reports the selected security protocol, NLA requirement, and pre-auth capability flags. `--ci`/`--expect` gate in CI. |
| `cert` | TLS, no auth | Server certificate: subject, issuer, validity, algorithms, SANs, SHA-256 |
| `id` | TLS, no auth | Stable JA4-style server fingerprint plus the certificate SHA-256 |
| `report` | session | Negotiated capabilities: security, desktop size, color depth, EGFX tier, advertised codecs, compression, joined channels |
| `shot` | session | Recon screenshot (PNG); login screen without credentials, desktop with them |

```bash
rdpsee -s host scan                       # pre-auth posture
rdpsee scan 10.0.0.0/24 --ci --expect tls,nla
rdpsee -s host cert                       # TLS certificate
rdpsee -s host id                         # server fingerprint
rdpsee -s host report                     # negotiated capabilities
rdpsee -s host -u user -p pass shot d.png # post-login screenshot
```

### rdpdo (act)

Drive an authenticated RDP session. **Model:** a chain of commands in one
invocation, run in order against one persistent connection,
`rdpdo -s host <cmd> <cmd> ...`.

| Category | Commands |
|---|---|
| Input | type, utype, key, click, doubleclick, drag, scroll, type-password, mouse-hide |
| Capture | capture (full/region/stdout), rcapture, timelapse |
| Visual matching | expect, waitfor, expectclick, rexpect, repeat-key, measure, diff |
| Stability | wait-still, wait-change |
| Clipboard | set/get-clipboard, clipboard-send-file, clipboard-recv-file |
| Display | resize, monitor |
| Audio | audio-capture, audio-assert-playing, audio-verify |
| Pixel | pixel, assert-pixel, wait-pixel, checksum, find-color |
| Provisioning | accept-portal, unlock, login, boot-sequence |
| Calibration | calibrate (click-grid profile generation) |
| Scripting | run, play, convert, retry, soft |
| Baseline | baseline update/list/check |
| Info | info, perf, status, watch, help |

```bash
rdpdo -s host type "hello" key enter pause 2 capture /tmp/out.png
rdpdo -s host -u user -p pass login user pass expect /tmp/desktop.png
rdpdo -s host --junit results.xml expect /tmp/app.png 0.95 30
rdpdo -s host --record /tmp/session.rdpdo run ./setup.rdpdo-script
```

It speaks EGFX (RemoteFX, and H.264/AVC420 decode when OpenH264 is present),
writes `--json` output for matching, pixel, and report commands, and can emit a
JUnit XML report for CI.

## Quick start

```bash
# Inspect a server (no login)
rdpsee -s 192.168.1.10 scan
rdpsee -s 192.168.1.10 report

# Drive a session
rdpdo -s 192.168.1.10 -u user -p pass type "hello" key enter capture /tmp/out.png
```

## Installation

Download a signed release binary from the
[releases page](https://github.com/lamco-admin/lamco-rdp-tools/releases), or
build from source:

```bash
git clone https://github.com/lamco-admin/lamco-rdp-tools.git
cd lamco-rdp-tools
cargo build --release
# binaries: target/release/rdpsee and target/release/rdpdo
```

## Requirements

- Rust 1.89+ and edition 2024 (to build from source)
- A reachable RDP target (the tools are clients; they do not host a server)
- **Optional:** OpenH264 shared library for H.264/AVC420 decode in `rdpsee shot`
  and `rdpdo` capture. It is loaded at runtime via `libloading`; without it,
  H.264 frames are skipped and other codecs (uncompressed, RemoteFX) still
  decode.

## Documentation

- **Website:** <https://lamco.ai/products/lamco-rdp-tools/>
- **Guides and reference:** [`docs/`](docs/README.md) — a getting-started
  tutorial, task-oriented how-to guides, use cases, concept explanations, and
  JSON/exit-code reference
- **Releases:** <https://github.com/lamco-admin/lamco-rdp-tools/releases> (signed binaries)
- **Repository:** <https://github.com/lamco-admin/lamco-rdp-tools>
- Man pages: `man rdpsee`, `man rdpdo`

## Related

- [lamco-rdp-server](https://lamco.ai/products/lamco-rdp-server/) — the Lamco RDP
  server. `rdpsee` and `rdpdo` are used to inspect and test it, but both work
  against any standards-compliant RDP server.

## About

lamco-rdp-tools is developed by **Lamco Development LLC**
(<https://lamco.ai/about/>) and is licensed under **MIT OR Apache-2.0**.
