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
| RFX Progressive | Decoded |
| ClearCodec, Planar | Decoded |
| H.264 / AVC420 | Decoded via OpenH264 (loaded at runtime; not offered to the server if absent) |
| AVC444 | Not offered |

The tools offer the server only what they can decode. If a frame still can't be
decoded, it's skipped rather than rendered incorrectly. `rdpdo` refuses a
capture until something has actually been decoded, so it never saves the blank
initial framebuffer as if it were the screen. `rdpsee shot` warns, then saves
what it has, if nothing decodes within ten seconds.

## OpenH264 is optional

H.264/AVC420 decode uses the OpenH264 shared library, loaded at runtime via
`libloading`. It is **optional**: If the library is not present, the tools don't
offer H.264, and the server sends another codec instead (RemoteFX Progressive,
RemoteFX, planar or uncompressed). Install OpenH264 to let a server stream H.264.

The library file is `libopenh264.so` on Linux and `openh264.dll` on Windows. The
tools look for it next to the executable first, then in the system locations, and
finally wherever `OPENH264_LIBRARY_PATH` points. On Windows the simplest setup is
to drop `openh264.dll` next to `rdpdo.exe` and `rdpsee.exe`.

## Why `shot` waits

EGFX frames arrive over the dynamic channel and can take a few seconds on a busy
or slow server. `rdpsee shot` (and `rdpdo`'s capture flow) poll for real content
before saving, so they do not write a blank image while the first frame is still
in flight. This is also why `wait-still` before a `capture` produces a more
stable frame: it waits for the stream to settle.

## What this means for capture

- A server using EGFX with AVC420 captures cleanly when OpenH264 is available.
- A server using RemoteFX, RemoteFX Progressive, ClearCodec, planar or
  uncompressed bitmaps captures with no extra dependencies.
- AVC444 is never offered, so a server falls back to one of the above; `report`
  shows you what the server negotiated.

See [capture recon screenshots](../how-to/capture-screenshots.md) for the capture
commands and [the connection and security model](connection-and-security.md) for
where graphics sit in the connection sequence.
