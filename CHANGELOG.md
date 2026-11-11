# Changelog

All notable changes to lamco-rdp-tools are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project follows
[Semantic Versioning](https://semver.org/). Versions tag the toolkit as a whole;
both binaries (`rdpsee`, `rdpdo`) ship together.

## [Unreleased]

## [1.3.0] - 2026-11-07

### Added

- `RDPDO_EGFX_CAPS=v8` makes `rdpdo` advertise EGFX 8.0 alone instead of 8.1 and
  8.0. An 8.0 client has neither H.264 nor RemoteFX Progressive, so a server has
  to send it raw pixels; this is how to test that path, since `xfreerdp3` always
  advertises 8.1 and later.

### Fixed

- `rdpdo` paints RemoteFX Progressive. A server with no H.264 encoder sends an
  EGFX 8.1 client Progressive, which `ironrdp-egfx` 0.3 could not decode, so
  the screen stayed blank. The client decodes it since 0.4, along with
  RemoteFX, ClearCodec and Planar, and hands the tiles over as RGBA bitmap
  updates.
- Progressive no longer leaves stale 64-pixel columns on screen against
  Windows 11. When the server moved a surface to a new codec context, the
  decoder started a fresh tile grid and lost the tiles already painted;
  `ironrdp-egfx` 0.5 keeps one grid per surface (IronRDP #2117).
- Tiles a server paints from the EGFX bitmap cache reach the screen.
  `ironrdp-egfx` applies `SurfaceToCache`, `CacheToSurface` and
  `EvictCacheEntry` to its own compositor only, and `rdpdo` keeps its own
  surfaces, so those tiles stayed black. Windows Server 2025 paints most
  repeated tiles, plain white window areas among them, this way.
- Every EGFX bitmap update now reaches the framebuffer whole. Its rectangle is
  exclusive and was composited as an inclusive one, which dropped the update's
  bottom row and wrote one stray pixel past its right edge. Under Progressive
  that left the bottom row of every 64-pixel tile unpainted.
- `rdpdo capture` again refuses, as 1.1.2 promised, to save a capture before
  anything was decoded, and `rcapture` and `baseline update` now refuse too.
  The guard and the initial-frame wait counted EGFX frames, and a frame counts
  even when nothing in it was painted (an H.264 frame with no decoder, or an
  update dropped as undecodable), so a black framebuffer could be written. Both
  now wait for decoded content, as `rdpsee shot` already did.
- Decoded content now means content that reached the screen. An update to a
  surface the server never mapped to the output, or one with no pixel data,
  no longer marks a blank framebuffer as painted, and a Reset Graphics clears
  the mark along with the framebuffer. A `SolidFill` now counts, so a screen
  painted only by fills is captured instead of refused.
- A `SurfaceToSurface` whose source rectangle runs past its surface no longer
  copies pixels from the following row; the part outside is left transparent
  black. Both this and a bitmap update shorter than its rectangle are now
  logged at debug level with their sizes instead of passing silently.
- Dependency updates clear the published advisories against the lockfile:
  `rustls` 0.23.46, `rustls-webpki` 0.103.16, `aws-lc-sys` 0.47, `tokio` 1.54.1,
  `h2` 0.4.20, `libz-sys` 1.1.31, `crossbeam-epoch` 0.9.21, `crypto-bigint`
  0.7.10, `ctutils` 0.4.7, `cryptoki` 0.12.1, `chacha20` 0.10.2 and `imageproc`
  0.25.1. RUSTSEC-2023-0071 (`rsa`, through `ironrdp-connector`'s `sspi` and
  `picky`) has no fixed release and remains.

### Changed

- `rdpdo` and `rdpsee` advertise the graphics pipeline in the Client Core
  Data (`RNS_UD_CS_SUPPORT_DYNVC_GFX_PROTOCOL`), which MS-RDPEGFX 1.5.1 asks
  of an EGFX client. gnome-remote-desktop closed the connection without it
  and now opens EGFX. Windows Server 2025 sent bitmap updates at 16-bit
  colour without it and now opens EGFX 8.1 at 32-bit. lamco-rdp-server
  ignores the flag.
- IronRDP crates move to the 2026-10-27 release wave (connector 0.12, session
  0.13, tokio 0.12, tls 0.3, pdu 0.11, core 0.3.1, cliprdr 0.9, dvc 0.10,
  egfx 0.5, graphics 0.11, rdpsnd 0.11, input 0.9, displaycontrol 0.10,
  svc 0.10), with `x509-cert` 0.3.
- **Breaking for source builds:** The minimum supported Rust version rises
  from 1.89 to 1.94, which the IronRDP crates now require. Release binaries
  are unaffected.
- H.264 (AVC420) is offered to the server only when the OpenH264 library
  loads. Without it, `rdpdo` used to end up offering EGFX 8.0 alone; it now
  offers 8.1 without H.264, so a server can still choose RemoteFX Progressive
  over raw pixels.
- AVIF encoding is no longer built in. `imageproc`'s defaults turned on every
  `image` format, and AVIF encoding brought in `rav1e`, `ravif` and the
  NCSA-licensed `libfuzzer-sys`. Every other format is still built, and
  `image` never read AVIF without its `avif-native` feature, so every image
  that loaded before still loads.
- RemoteFX is decoded by `ironrdp-egfx` too. rdpdo's own decoder, which painted
  the framebuffer but not the surface a later `SurfaceToSurface` copies from,
  is gone.
- `rdpdo audio-capture` takes the PCM format from the negotiated format list,
  which `ironrdp-rdpsnd` now passes with each wave, instead of guessing it
  from the observed data rate.
- The EGFX frame count (`perf`, `watch`, `wait-change`) counts `EndFrame`s. It
  counted bitmap updates, which `ironrdp-egfx` delivers per Progressive tile
  and per AVC420 region rectangle.

## [1.2.0] - 2026-09-29

### Added

- `rdpdo monitor set` now accepts a `WIDTHxHEIGHT[+WIDTHxHEIGHT...]` layout
  instead of a single primary monitor: each additional entry is tiled
  immediately to the right of the previous one (index 0 stays primary at
  (0,0)), sent as a proper N-entry `DisplayControl` `MonitorLayout` PDU.
  `monitor list` reports the layout actually requested this session
  (left/top/width/height per monitor) instead of a hardcoded single-monitor
  stub. Built to validate the server's new multi-monitor EGFX surfaces
  (`lamco-rdp-server` `c4b73fdc6`/`44143b649`); `capture <path> LEFT,TOP,W,H`
  is the existing mechanism for grabbing one monitor's region.

### Fixed

- `rdpdo` decodes AVC420 as full-range BT.709, as MS-RDPEGFX 3.3.8.3.1
  requires. Both decoder tiers used openh264's `write_rgba8`, which assumes
  limited-range BT.601, so captures of H.264 sessions came back with green
  about 16 low against the server's own screen and skewed `assert-pixel`,
  `find-color` and `checksum` baselines. The frame now converts through
  `yuv420_to_rgba` with the real plane strides (upstream fix: IronRDP #1923,
  not yet in a published `ironrdp-egfx`). Measured against a QMP screendump of
  the same frame: mean RGB (0.6, 66.2, 209.8) vs (1.0, 66.2, 210.1).
- `rdpdo audio-capture` no longer fails with "no wave packets arrived". The
  format lookup read a table that was never filled, and the wave format index
  cannot be resolved from outside `ironrdp-rdpsnd`, so the captured PCM format
  is now identified from the observed data rate. The WAV is written with the
  correct 16-bit / stereo / 44100 Hz header.
- `rdpdo exec` keeps answering the RDP server while the remote command runs.
  A server that fetches clipboard data only when an app pastes asks for it
  in the middle of `exec ... wl-paste`; rdpdo used to stop reading the
  connection during `exec`, so the paste and the `exec` waited on each other.
- `rdpdo set-clipboard` text is served for every paste until the server
  announces a copy of its own. Previously only the first request got the
  text and later pastes of the same copy got an error response.
- `rdpdo capture <path> <region>` no longer panics with an out-of-bounds
  slice when the desktop has resized between computing the region and
  capturing it (a real race, not just a multi-monitor edge case — any
  mid-session resize could trigger it). It now returns a clear error instead.

### Changed

- IronRDP crates move to the current release wave together (connector 0.10,
  session 0.11, pdu 0.9, cliprdr 0.7, dvc 0.8, egfx 0.3, graphics 0.9,
  rdpsnd 0.9, input 0.7, displaycontrol 0.8, svc 0.8, tls 0.2.2). Session
  reactivation now follows the `ironrdp-session` 0.11 ownership model. No
  command-line change.

## [1.1.2] - 2026-08-31

### Added

- `rdpsee cert` and `rdpsee id` now report the negotiated TLS protocol version
  and cipher suite (e.g. `TLSv1_3` / `TLS13_AES_256_GCM_SHA384`), sourced from
  `ironrdp_tls::negotiated()` (Devolutions/IronRDP PR #1384, shipped in
  `ironrdp-tls` 0.2.2). `None`/omitted when the active TLS backend can't
  report it. Informational only on `id`: not folded into the fingerprint
  hash, so existing fingerprints are unaffected by this release.

### Fixed

- `rdpdo capture` no longer accepts a blank first frame: it waits longer for
  the first real frame and refuses (rather than silently saving) a capture
  that never received one.
- `rdpdo capture` now warns when a capture is a single flat colour, since
  that usually means the screen hadn't finished rendering.

### Changed

- Cleared five clippy pedantic findings the current toolchain (1.98) added
  (`unneeded_wildcard_pattern`, `chunks_exact_to_as_chunks`) with no
  behavior change.

## [1.1.1] - 2026-06-26

### Fixed

- The session no longer aborts when a server opens a dynamic virtual channel we
  decline (for example xrdp's `ECHO` and `FreeRDP::Advanced::Input`) and then
  sends data on it. That PDU is skipped instead of ending the session, which
  previously surfaced as `access to non existing DVC channel` and failed a
  capture whenever the stray PDU arrived before the screen settled.
- OpenH264 discovery now follows the operating system library search path, so an
  `openh264.dll` installed in a system directory (System32 or on `PATH`) is found
  without copying it next to the executable.
- The library-load warning distinguishes a missing library from one that was
  found but failed to load.
- The initial-frame wait now counts EGFX decoded frames, not only legacy bitmap
  updates. EGFX frames arrive on the graphics pipeline and increment a separate
  counter, so against an AVC420 server the wait previously always timed out and
  logged a spurious "No initial frame received".

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
