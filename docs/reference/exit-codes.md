# Exit codes

Both tools use a small set of exit codes so they compose cleanly in shell
scripts and CI pipelines.

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Failure: a command failed, a connection or negotiation error occurred, an assertion did not hold, the server disconnected with `rdpdo` commands still to run, or the overall `--timeout` elapsed |
| `2` | A global flag could not be parsed |
| `3` | `rdpdo` only: the chain ran to the end, but a command marked `soft` failed |

`rdpsee` gating is expressed through the inputs (`rdpsee scan --ci/--expect`),
and its result collapses to 0 or 1.

## rdpsee

Every `rdpsee` command exits `0` on success and `1` on failure or timeout.

`scan` adds conditional gating. By default it always exits `0` once it has
probed the targets (an unreachable target is a reported row, not a failure). With
`--ci` and `--expect`, it instead exits `1` unless **every** target meets the
expectation:

```bash
# Exits 1 if any host in the range is not reachable with TLS + NLA
rdpsee scan 10.0.0.0/24 --ci --expect reachable,tls,nla
```

`--expect` takes a comma-separated list of `reachable`, `tls`, `nla`, `egfx`.

## rdpdo

A `rdpdo` invocation runs a chain of commands on one connection. The chain exits:

- `0` when every command succeeds.
- `1` when any command fails, the command chain can't be parsed, the connection
  cannot be established, the server disconnects while commands are still to
  run, or the overall `--timeout` elapses.
- `3` when the chain runs to the end but a command marked `soft` failed.

Two conditions that used to pass quietly are failures:

- **The server disconnects mid-chain.** The commands after the disconnect can't
  run, so the chain exits `1` with an error naming the last command that ran and
  how many didn't (`the server disconnected after [2] pause 5; 1 command didn't
  run`), and each of them is a failed testcase in `--junit` output. This is a
  hard failure even if the next command is marked `soft`, because the chain
  didn't run to the end. The same holds inside `run`, `boot-sequence` and `play`.
  A disconnect during or after the last command is not an error: nothing was
  left to run, and that command either succeeded or reports its own failure.
- **No screen content arrives.** Until the server's first update is decoded the
  framebuffer is blank. A command that reads the screen fails rather than read
  it: `capture`, `rcapture`, `baseline update`, `baseline check`, `pixel`,
  `checksum`, `assert-checksum`, `assert-pixel`, `find-color`, `timelapse` and
  `calibrate` fail at once, and `expect`, `waitfor`, `expectclick`, `rexpect`,
  `wait-checksum-change`, `wait-pixel` and `measure` first wait up to their own
  timeout for content. `repeat-key` keeps pressing, since its presses may be what
  wakes the screen, but compares nothing until content arrives and fails at its
  timeout or press limit if none has. This is an ordinary command
  failure, so it exits `1`, or `3` when the command is marked `soft`. Commands
  that only send input, and `wait-still` and `wait-change`, which watch update
  traffic rather than pixels, don't need content.

Two chain modifiers change how failure propagates:

- **`soft`** makes the *next* command non-fatal. If it fails, the chain logs the
  failure and continues to the end, and then exits `3` instead of `0` (the
  failure also appears as a failed testcase in `--junit` output). A hard failure
  later in the chain still exits `1`.
- **`retry <N>`** runs the next command up to N times before treating it as a
  failure.

So a chain exits `0` only when every command succeeds, and a caller that wants
to accept soft failures accepts `3` as well.

```bash
# Exits 1 if the desktop never appears, 3 if only the optional banner was missing
rdpdo -s host -u user -p pass \
  soft expect /tmp/banner.png 0.9 5 \
  retry 3 expect /tmp/desktop.png 0.95 60
rc=$?
[ "$rc" = 0 ] || [ "$rc" = 3 ] || exit 1
```

## Using exit codes in CI

Because both tools exit non-zero on any failure, they drop straight into any CI
step:

```bash
rdpsee scan 10.0.0.0/24 --ci --expect tls,nla || exit 1
rdpdo -s host -u "$USER" -p "$PASS" --junit results.xml \
  expect /tmp/desktop.png 0.95 60
```

For richer machine-readable results, combine the exit code with
[`--json`](json-output.md), or, for `rdpdo`, `--junit <path>` to emit a JUnit XML
report where each command is a testcase.
