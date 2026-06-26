# Calibrate click coordinates

When clicks land slightly off from where you aim, generate a calibration profile
that corrects the offset, then apply it to every coordinate the tool sends.
Coordinate skew shows up on some servers and resolutions as a fixed pixel offset
between the position you ask for and the position the server registers.

## Generate a calibration profile

Run the `calibrate` command. It clicks a grid of known points, measures where
each landed, and writes a correction profile:

```bash
rdpdo -s 192.168.1.10 -u user -p pass calibrate --output ~/.config/rdpdo/calibration/host.json
```

Options:

- `--grid NxN` sets the grid density (default is a 4x4 grid).
- `--quick` runs a faster, coarser pass.
- `--deploy METHOD` controls how the grid target is delivered (clipboard by
  default).
- `--output PATH` writes the profile; omit it to use the default calibration
  directory.

## Apply the profile

Pass `--calibration` on later runs to correct every click and move. Give it a
file path, or `auto` to search `~/.config/rdpdo/calibration/` for a matching
profile:

```bash
rdpdo -s 192.168.1.10 -u user -p pass --calibration auto \
  click 500,300 click center
```

With the profile applied, the coordinates you specify are adjusted before they
are sent, so a click at `500,300` lands at `500,300` on the server.

## When you need it

Most servers do not need calibration. Reach for it when:

- `expectclick` finds a target but the click misses it, or
- clicks at known coordinates consistently land a few pixels off.

Calibration is per server and resolution, so regenerate the profile if you change
the desktop size (`--width`/`--height`).

## Notes

`calibrate` (the command that *produces* a profile) is distinct from
`--calibration` (the flag that *applies* one). Named positions like `center` and
percentage positions like `50%,50%` are corrected the same way as pixel
coordinates once a profile is applied.
