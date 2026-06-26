# CI/CD visual testing

**Who:** teams shipping desktop applications, OS images, or installers who want to
catch visual and behavioral regressions automatically.

**Why these tools:** no other RDP automation tool combines input injection, visual
matching, and CI-native output. `rdpdo` drives a real session over the network
and asserts on what the screen actually shows, with no agent installed in the
guest.

## The workflow

1. **Capture baselines** from a known-good build, once, and commit them.
2. **On every change**, boot the build, drive it to the state under test, and
   `expect` the screen to match the baseline.
3. **Report** through JUnit XML so CI shows pass/fail natively, and save a
   screenshot on failure for triage.

## A pipeline step

```bash
# Confirm the server is up and secure before testing it
rdpsee scan "$RDP_HOST" --ci --expect reachable,tls

# Drive the app and assert it looks right
rdpdo -s "$RDP_HOST" -u "$RDP_USER" -p "$RDP_PASSWORD" \
  --junit results.xml --fail-capture artifacts/ \
  mouse-hide \
  waitfor baselines/app-loaded.png 0.9 60 \
  expectclick baselines/new-doc.png \
  type "regression test" \
  expect baselines/document-typed.png 0.95 30
```

The step fails the job if any assertion does not hold, emits `results.xml` for
the CI test view, and drops a screenshot into `artifacts/` on failure.

## Why it is reliable

- `mouse-hide` keeps the cursor out of comparisons.
- `wait-still`/`waitfor` synchronize on the screen instead of guessing with
  `pause`, so the test does not race the UI.
- `diff --exclude` masks clocks and timestamps that would otherwise flap.
- `soft` marks legitimately-optional elements non-fatal.

## Building blocks

- [Run a visual-regression check in CI](../how-to/visual-regression-ci.md)
- [Automate a login](../how-to/automate-login.md)
- [Visual matching and needles](../explanation/visual-matching-and-needles.md)
- [Exit codes](../reference/exit-codes.md) and [JSON output](../reference/json-output.md)
