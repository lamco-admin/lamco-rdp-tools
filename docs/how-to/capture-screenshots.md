# Capture recon screenshots

Save a PNG of what an RDP server is showing right now. `rdpsee shot` is the
quickest path: it completes a connection, waits for the screen to render, and
writes the image. For capture inside a larger automation chain, `rdpdo capture`
is the counterpart.

## Capture with rdpsee

Without credentials, `shot` captures the login screen where the server allows
anonymous connections:

```bash
rdpsee -s 192.168.1.10 shot login.png
```

With credentials, it captures the post-login desktop:

```bash
rdpsee -s 192.168.1.10 -u user -p pass shot desktop.png
```

If you omit the path, it writes `screenshot.png`. `shot` polls for real content
for up to ten seconds before saving, so it does not write a blank frame on a
slow server; if no frame arrives it tells you rather than saving nothing useful.

## Capture during automation with rdpdo

When you are already driving a session, use `rdpdo capture` as one command in the
chain:

```bash
rdpdo -s 192.168.1.10 -u user -p pass \
  wait-still capture /tmp/desktop.png
```

`wait-still` first waits for the screen to stop changing, so the frame is stable.
`capture` also accepts a region and can write to stdout:

```bash
rdpdo -s host capture /tmp/region.png 0,0,640x480   # a sub-region
rdpdo -s host capture - > /tmp/piped.png             # to stdout
```

## Hide the cursor first

The mouse pointer can appear in a screenshot and cause spurious differences when
comparing images. Move it off-screen before capturing:

```bash
rdpdo -s host -u user -p pass mouse-hide capture /tmp/clean.png
```

## Capture a series over time

To watch a screen change (an installer, a boot sequence), use `timelapse`:

```bash
rdpdo -s host -u user -p pass timelapse /tmp/frame-{n}.png --interval 1000 --count 30
```

## Notes

`shot` and `capture` decode the same graphics pipeline (EGFX, RemoteFX, and
H.264/AVC420 when OpenH264 is present). See
[codecs and graphics](../explanation/codecs-and-graphics.md) for what is decoded.
