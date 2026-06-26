# Visual matching and needles

`rdpdo` automates a screen it cannot read semantically — it has pixels, not a DOM.
Visual matching is how it synchronizes on the UI and asserts on state. This page
explains how matching works and when to use each matching command.

## How a match is scored

Matching compares a reference image against the current screen and scores their
similarity by correlation (a Pearson correlation over pixel values). The score
runs from 0 (unrelated) to 1 (identical). A command **matches** when the score
meets or exceeds a **tolerance** you give it.

Tolerance accepts either form: `0.0`-`1.0` or `1`-`100`. `0.95` and `95` both mean
"95% similar." Lower the tolerance to allow more variation (anti-aliasing, minor
theme differences); raise it to demand a near-exact match.

## Full-screen vs template vs region

The matching commands differ in *what* they compare:

- **`expect <reference>`** waits until the **whole screen** matches a reference
  image. Use it to assert "the desktop is up" or "this dialog is showing."
- **`waitfor <template>`** searches for a smaller **template image anywhere** on
  screen and waits until it is found. Use it to wait for a button or icon to
  appear, regardless of where it lands.
- **`expectclick <template>`** does `waitfor`, then **clicks** the template's
  center. It is the workhorse for driving a UI by its visuals.
- **`rexpect <region> <reference>`** matches a **fixed region** of the screen.
  Use it when you know exactly where something is and want to ignore the rest.

Two more build on these:

- **`repeat-key <key> <template>`** presses a key repeatedly until a template
  matches — for paging through a list or dismissing repeated prompts.
- **`measure <template>`** times how long until a match succeeds and reports
  `elapsed_ms`, the basis of [VDI baselining](../use-cases/vdi-baselining.md).

## Why synchronize on the screen

Automation that waits with `pause` races the UI: too short and it acts before the
screen is ready, too long and it wastes time. Matching lets you wait for the
*actual* state instead. Pair it with `wait-still` (wait until the screen stops
changing) and `mouse-hide` (keep the cursor out of comparisons) for stable,
non-flaky checks.

## Needles: portable match targets

A **needle** is a match target with metadata — the reference image plus the
areas to match and tags — rather than a bare PNG. `rdpdo` reads needles from a
directory (`--needles DIR`) and can filter them by tag (`--tag NAME`). The needle
format is auto-detected: it accepts both the native format and the
[openQA](https://open.qa/) needle format, distinguishing them by their fields. If
your team already has an openQA needle library, `rdpdo` can use it directly.

## Comparing two images offline

`diff` compares two image files without a connection and writes a visual diff,
with `--mode` choosing `highlight`, `side-by-side`, or `heatmap`. Use
`--exclude REGION` to mask volatile areas — clocks, timestamps — that would
otherwise register as differences. This is how you keep CI comparisons stable.

## See also

- [Run a visual-regression check in CI](../how-to/visual-regression-ci.md)
- [CI/CD visual testing](../use-cases/ci-visual-testing.md)
