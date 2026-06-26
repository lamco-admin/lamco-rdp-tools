# Verify audio playback

Confirm that a remote session is actually producing sound, capture the audio for
inspection, or compare it against a reference recording. RDP carries audio on the
RDPSND channel, and `rdpdo` can record and check it; no other RDP automation tool
does this.

## Assert that audio is playing

The simplest check: fail unless non-silent audio arrives within a timeout. Use it
after triggering something that should make a sound:

```bash
rdpdo -s 192.168.1.10 -u user -p pass \
  expectclick /tmp/play-button.png \
  audio-assert-playing 5
```

`audio-assert-playing 5` waits up to five seconds for audio and exits non-zero if
the session stays silent.

## Capture audio to a file

Record the RDPSND stream to a WAV file for an optional duration (seconds):

```bash
rdpdo -s 192.168.1.10 -u user -p pass audio-capture /tmp/out.wav 10
```

## Compare against a reference

Capture once as your reference, then in CI capture again and compare:

```bash
# Reference (once)
rdpdo -s host -u user -p pass audio-capture reference.wav 10

# Check (later) — compare two WAV files, offline
rdpdo audio-verify captured.wav reference.wav 0.9
```

`audio-verify` runs offline (no connection) and compares two WAV files by
correlation; the optional tolerance (here `0.9`) sets how close they must be.

## A full check

Trigger playback, capture it, and verify in one chain:

```bash
rdpdo -s host -u user -p pass \
  expectclick /tmp/play-button.png \
  audio-capture /tmp/captured.wav 10 \
  && rdpdo audio-verify /tmp/captured.wav reference.wav 0.9
```

## Notes

Audio verification opens up testing for conferencing apps, media players, system
notification sounds, and accessibility tools such as screen readers. See the
[accessibility verification use case](../use-cases/accessibility-verification.md).
