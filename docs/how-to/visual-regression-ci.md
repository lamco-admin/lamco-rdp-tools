# Run a visual-regression check in CI

Assert that a desktop or application looks the way it should, and produce a
report your CI system understands. The pattern is: reach a known state, then
`expect` a reference image, with `--junit` for the report and `--fail-capture`
for evidence on failure.

## Establish the reference

Capture the known-good image once, from a stable frame, and commit it:

```bash
rdpdo -s host -u user -p pass mouse-hide wait-still capture baselines/desktop.png
```

`mouse-hide` keeps the cursor out of the image and `wait-still` ensures the frame
is settled, so the reference is not noisy.

## The CI check

```bash
rdpdo -s "$RDP_HOST" -u "$RDP_USER" -p "$RDP_PASSWORD" \
  --junit results.xml --fail-capture artifacts/ \
  mouse-hide \
  expect baselines/desktop.png 0.95 60
```

- `expect baselines/desktop.png 0.95 60` waits up to 60 seconds for a 95% match
  and fails the run if it never matches.
- `--junit results.xml` writes a JUnit XML report where each command is a
  testcase, so CI shows the result natively.
- `--fail-capture artifacts/` saves a screenshot on any failure, so you can see
  what the screen actually looked like.

The step exits `0` when the screen matches and `1` when it does not, which fails
the CI job.

## Check parts of the screen

To assert a specific element rather than the whole desktop, use `waitfor` (find a
template anywhere) or `rexpect` (match a fixed region):

```bash
rdpdo -s host -u user -p pass \
  waitfor baselines/ok-button.png 0.9 30 \
  rexpect 0,0,300x40 baselines/titlebar.png 0.95 10
```

## Mask volatile areas

Clocks and timestamps change every run. When comparing two images offline, mask
them out with `diff --exclude`:

```bash
rdpdo diff baselines/desktop.png artifacts/actual.png 0.99 \
  --output artifacts/diff.png --exclude 0,0,200x30
```

`diff` runs offline (no connection) and writes a visual diff image; `--mode`
selects `highlight`, `side-by-side`, or `heatmap`.

## Make optional elements non-fatal

Wrap checks that may legitimately be absent in `soft` so they report without
failing the run:

```bash
rdpdo -s host -u user -p pass --junit results.xml \
  soft expect baselines/optional-banner.png 0.9 5 \
  expect baselines/desktop.png 0.95 60
```

## Notes

Tolerance accepts `0.0`-`1.0` or `1`-`100` (`0.95` and `95` are equivalent). For
how matching actually works and when to use `expect` versus `waitfor` versus a
needle, see [visual matching and needles](../explanation/visual-matching-and-needles.md).
