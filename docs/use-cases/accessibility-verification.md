# Accessibility verification

**Who:** teams that must confirm accessibility features actually work — screen
readers announcing, notification sounds playing, high-contrast themes rendering —
as part of QA or compliance.

**Why these tools:** accessibility checks need both the picture and the sound.
`rdpdo` can assert on the rendered screen *and* verify audio on the RDPSND
channel, which lets it confirm a screen reader is speaking or a notification chime
played — something pixel-only tools cannot do.

## Verify a screen reader is speaking

Trigger an action that should produce spoken feedback, then assert audio plays:

```bash
rdpdo -s "$HOST" -u "$USER" -p "$PASSWORD" \
  expectclick baselines/menu-item.png \
  audio-assert-playing 5
```

To check *what* was spoken, capture the audio and compare it against a reference
recording of the expected announcement:

```bash
rdpdo -s "$HOST" -u "$USER" -p "$PASSWORD" \
  expectclick baselines/menu-item.png \
  audio-capture /tmp/spoken.wav 6
rdpdo audio-verify /tmp/spoken.wav baselines/announcement.wav 0.85
```

## Verify visual accessibility

Assert that a high-contrast or large-text theme renders as expected, using region
matching and pixel checks:

```bash
rdpdo -s "$HOST" -u "$USER" -p "$PASSWORD" \
  expect baselines/high-contrast-desktop.png 0.95 30 \
  assert-pixel 10,10 #000000 10
```

`assert-pixel` confirms a specific pixel is the expected color within a tolerance
— useful for checking a theme's background or an accent color.

## Verify notification sounds

```bash
rdpdo -s "$HOST" -u "$USER" -p "$PASSWORD" \
  expectclick baselines/trigger-notification.png \
  audio-assert-playing 3
```

## Building blocks

- [Verify audio playback](../how-to/verify-audio.md)
- [Run a visual-regression check in CI](../how-to/visual-regression-ci.md)
- [Visual matching and needles](../explanation/visual-matching-and-needles.md)
