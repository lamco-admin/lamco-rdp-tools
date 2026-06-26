# VDI performance baselining

**Who:** VDI and remote-desktop operators who need objective numbers for login
time, time-to-desktop, and application launch time across a fleet, and want to
watch them over time.

**Why these tools:** `rdpdo` measures *user-visible* timing — how long until the
screen actually shows the desktop or the app window — not just server-side
counters. `measure` times a visual milestone, and `perf` reports the stream's
frame and bandwidth metrics.

## Measure time-to-desktop

`measure` times how long until a reference image matches, and reports it as
`elapsed_ms`:

```bash
rdpdo -s "$VDI_HOST" -u "$USER" -p "$PASSWORD" --json \
  measure baselines/desktop.png 0.9 120 --label time-to-desktop
```

```json
{ "label": "time-to-desktop", "elapsed_ms": 4200, "matched": true }
```

Chain measurements to baseline a whole login-to-app journey:

```bash
rdpdo -s "$VDI_HOST" -u "$USER" -p "$PASSWORD" --json --junit timings.xml \
  measure baselines/desktop.png 0.9 120 --label time-to-desktop \
  expectclick baselines/app-icon.png \
  measure baselines/app-window.png 0.9 60 --label time-to-app
```

## Capture stream metrics

`perf` reports time-to-first-frame, FPS, bandwidth, and frame-interval
percentiles for the session:

```bash
rdpdo -s "$VDI_HOST" -u "$USER" -p "$PASSWORD" --json \
  wait-still pause 5 perf
```

The percentiles (`frame_interval_p50_ms`, `frame_interval_p99_ms`) are the useful
signal for responsiveness: a high p99 means occasional stalls users will feel.

## Baseline the fleet

Run the same measurement across hosts and collect the JSON for trending:

```bash
for host in $(cat vdi-hosts.txt); do
  rdpdo -s "$host" -u "$USER" -p "$PASSWORD" --json \
    measure baselines/desktop.png 0.9 120 --label time-to-desktop
done | jq -s '.' > vdi-baseline.json
```

Compare against last week's run to catch regressions before users report them.

## Building blocks

- [Automate a login](../how-to/automate-login.md)
- [JSON output](../reference/json-output.md) (`measure`, `perf`)
- [Codecs and graphics](../explanation/codecs-and-graphics.md) (what affects frame timing)
