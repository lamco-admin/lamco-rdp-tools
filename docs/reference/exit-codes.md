# Exit codes

Both tools use a simple two-value exit status so they compose cleanly in shell
scripts and CI pipelines.

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Failure: a command failed, a connection or negotiation error occurred, an assertion did not hold, or the overall `--timeout` elapsed |

There is no third "partial" or "CI" code; gating is expressed through the inputs
(`rdpsee scan --ci/--expect`), and the result still collapses to 0 or 1.

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
- `1` when any command fails, the connection cannot be established, or the
  overall `--timeout` elapses.

Two chain modifiers change how failure propagates:

- **`soft`** makes the *next* command non-fatal. If it fails, the chain logs the
  failure and continues; a soft failure does **not** change the exit code (it
  does appear as a failed testcase in `--junit` output).
- **`retry <N>`** runs the next command up to N times before treating it as a
  failure.

So a chain succeeds (exit `0`) when all of its non-soft commands succeed.

```bash
# Exits 1 only if the desktop never appears; the optional banner is non-fatal
rdpdo -s host -u user -p pass \
  soft expect /tmp/banner.png 0.9 5 \
  retry 3 expect /tmp/desktop.png 0.95 60
```

## Using exit codes in CI

Because both tools collapse to 0/1, they drop straight into any CI step:

```bash
rdpsee scan 10.0.0.0/24 --ci --expect tls,nla || exit 1
rdpdo -s host -u "$USER" -p "$PASS" --junit results.xml \
  expect /tmp/desktop.png 0.95 60
```

For richer machine-readable results, combine the exit code with
[`--json`](json-output.md), or, for `rdpdo`, `--junit <path>` to emit a JUnit XML
report where each command is a testcase.
