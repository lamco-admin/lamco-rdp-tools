# Changelog

All notable changes to lamco-rdp-tools are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project follows
[Semantic Versioning](https://semver.org/). Versions tag the toolkit as a whole;
both binaries (`rdpsee`, `rdpdo`) ship together.

## [1.1.0] - 2026-06-26

Windows support.

### Added

- Self-contained Windows binaries: `rdpsee.exe` and `rdpdo.exe` for
  `x86_64-pc-windows-msvc`, statically linked (no Visual C++ redistributable
  required), on the pure-Rust rustls TLS stack.

### Changed

- The OpenH264 library is now discovered per platform (`libopenh264.so` on
  Linux, `openh264.dll` on Windows) and next to the executable, so a library
  placed beside the binary is found. Without it, other codecs still decode.
- The RDP client name now uses `COMPUTERNAME` on Windows.

## [1.0.0] - 2026-06-26

First public release.

### rdpsee (observe)

- Server inspection that never drives the session, across three tiers:
  - `scan` — connectionless pre-auth security probe for one or many targets
    (host, `host:port`, IPv4 CIDR), concurrent, with `--ci`/`--expect` gating.
  - `cert` — TLS certificate inspection (no authentication).
  - `id` — stable JA4-style server fingerprint plus certificate SHA-256.
  - `report` — negotiated capability report (security, desktop size, color
    depth, EGFX tier, advertised codecs, compression, joined channels).
  - `shot` — recon screenshot (login screen or post-login desktop).

### rdpdo (act)

- Headless RDP session automation over IronRDP: keyboard and mouse input
  (scancode and Unicode), screen capture (full, region, stdout, timelapse),
  visual matching (template, needle, region, measure, diff), screen-stability
  waits, pixel and color inspection, clipboard text and file transfer, audio
  capture and verification, display resize and multi-monitor control,
  provisioning (portal, login, unlock, boot sequence), click calibration,
  session record/replay, scripting, baselines, and `--json` / JUnit output.
- Graphics: EGFX with RemoteFX, and H.264/AVC420 decode via OpenH264 loaded at
  runtime (skipped when the library is absent).

### Project

- Dual-licensed MIT OR Apache-2.0.
- Dependencies pinned to published crates.io IronRDP releases (reproducible).
- Man pages for both binaries (`man rdpsee`, `man rdpdo`).
