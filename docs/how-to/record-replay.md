# Record and replay a session

Record a `rdpdo` run with timing and play it back later, or author the same steps
as a script. Use this to turn a manual sequence into a repeatable artifact.

## Record a run

Add `--record` to any chain to capture the commands and their timing to a
`.rdpdo` file:

```bash
rdpdo -s 192.168.1.10 -u user -p pass --record session.rdpdo \
  type "hello" key enter pause 2 capture /tmp/out.png
```

## Replay it

Play the recording back against a server. An optional speed multiplier runs it
faster or slower than recorded:

```bash
rdpdo -s 192.168.1.10 -u user -p pass play session.rdpdo
rdpdo -s 192.168.1.10 -u user -p pass play session.rdpdo 2.0   # twice as fast
```

## Convert between recording and script

A `.rdpdo` recording (timed, binary-ish) and a `.rdpdo-script` (plain text, one
command per line) are interchangeable. Convert a recording into an editable
script:

```bash
rdpdo convert session.rdpdo setup.rdpdo-script
```

`convert` runs offline (no connection). Edit the script in any text editor, then
run it:

```bash
rdpdo -s 192.168.1.10 -u user -p pass run setup.rdpdo-script
```

## Author a script directly

A script is one command per line, with `#` comments and variable substitution:

```text
# setup.rdpdo-script
waitfor /tmp/login-prompt.png 0.9 30
type "{env:RDP_USER}"
key tab
type-password env:RDP_PASSWORD
key enter
expect /tmp/desktop.png 0.95 60
capture /tmp/{server}-ready.png
```

Variables are expanded at parse time: `{env:VAR}`, `{width}`, `{height}`,
`{server}`. Run it with `run`:

```bash
rdpdo -s 192.168.1.10 run setup.rdpdo-script
```

## Notes

`rdpdo --record`/`play` records *commands*, not pixels: replay re-issues the same
actions, it does not stream a video. That makes recordings small, diffable, and
editable as scripts.
