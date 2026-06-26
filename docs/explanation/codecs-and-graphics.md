# Codecs and graphics

RDP servers can send the screen in several encodings. What `rdpdo` and
`rdpsee shot` can capture, and what `rdpsee report` tells you about a server's
graphics, both depend on which codecs are involved. This page explains the
graphics path.

## How the screen arrives

Modern RDP carries graphics two ways:

- **The main bitmap channel** — the classic path, which can carry uncompressed
  bitmaps and codecs such as RemoteFX and NSCodec, negotiated in the server's
  capability sets.
- **EGFX (the graphics pipeline)** — a dynamic virtual channel over `drdynvc`
  that carries modern codecs, including H.264/AVC. When EGFX is active, most of
  the real graphics flow through it.

`rdpsee report` reports both: `egfx_active` and the confirmed EGFX tier
(`egfx_caps`, for example `V8.1 (AVC420)`) describe the pipeline, while `codecs`
lists the main-channel bitmap codecs the server advertised, and `color_depth` the
negotiated bits per pixel.

## What the tools decode

The capture path decodes:

| Codec | Status |
|---|---|
| Uncompressed | Decoded |
| RemoteFX (RFX) | Decoded |
| H.264 / AVC420 | Decoded via OpenH264 (loaded at runtime; skipped if absent) |
| ClearCodec | Pending upstream support |
| RFX Progressive | Not yet |
| AVC444 | Not yet |

When a frame uses a codec the tools do not decode, that frame is skipped rather
than rendered incorrectly — so a capture reflects what could be decoded, and the
tool tells you when nothing rendered.

## OpenH264 is optional

H.264/AVC420 decode uses the OpenH264 shared library, loaded at runtime via
`libloading`. It is **optional**: if the library is not present, H.264 frames are
skipped and the other codecs still decode. Install OpenH264 if you need to capture
servers that stream H.264.

## Why `shot` waits

EGFX frames arrive over the dynamic channel and can take a few seconds on a busy
or slow server. `rdpsee shot` (and `rdpdo`'s capture flow) poll for real content
before saving, so they do not write a blank image while the first frame is still
in flight. This is also why `wait-still` before a `capture` produces a more
stable frame: it waits for the stream to settle.

## What this means for capture

- A server using EGFX with AVC420 captures cleanly when OpenH264 is available.
- A server using only RemoteFX or uncompressed bitmaps captures with no extra
  dependencies.
- A server using a not-yet-supported codec (ClearCodec, AVC444, progressive RFX)
  may produce partial or empty captures; `report` will show you what the server
  negotiated so you know what to expect.

See [capture recon screenshots](../how-to/capture-screenshots.md) for the capture
commands and [the connection and security model](connection-and-security.md) for
where graphics sit in the connection sequence.
