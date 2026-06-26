# Automate a login and reach the desktop

Drive an RDP login screen to a logged-in desktop, then confirm you arrived. Two
approaches: the `login` command with a compositor profile, or typing the
credentials directly. Both end by waiting for the desktop with `expect`.

## Option A: type the credentials directly

When you know the login screen's layout, type into it. A common shape is
username, Tab, password, Enter:

```bash
rdpdo -s 192.168.1.10 \
  type "myuser" key tab type-password env:RDP_PASSWORD key enter \
  expect /tmp/desktop.png 0.95 60
```

- `type-password` reads the password from a source (`env:VAR`, `file:PATH`, or a
  literal) and types it **without** logging it.
- `expect /tmp/desktop.png 0.95 60` waits up to 60 seconds for the screen to
  match a reference image of the desktop at 95% similarity, and fails if it never
  does. That is how you assert the login actually succeeded.

Capture the reference image once, from a known-good logged-in session:

```bash
rdpdo -s host -u user -p pass wait-still capture /tmp/desktop.png
```

## Option B: the `login` command with a profile

For desktops with a known compositor, `login` uses a provisioning profile to
place the credentials correctly:

```bash
rdpdo -s 192.168.1.10 login myuser "$RDP_PASSWORD" --profile gnome --verify
```

`--verify` checks that the login took effect. Profiles live under
`~/.config/rdpdo/profiles/`.

## Make it robust

Login screens render at their own pace. Wait for the prompt before typing, and
retry the desktop check:

```bash
rdpdo -s 192.168.1.10 \
  waitfor /tmp/login-prompt.png 0.9 30 \
  type "myuser" key tab type-password env:RDP_PASSWORD key enter \
  retry 3 expect /tmp/desktop.png 0.95 60
```

- `waitfor` waits until a template image appears anywhere on screen.
- `retry 3` runs the next command up to three times before failing.

## Notes

Keep the password out of your shell history and process list by using
`type-password env:VAR` or `login user "$VAR"` with the value in an environment
variable, not a literal on the command line. To reach a login screen on a fresh
VM that first shows a portal or lock screen, see
[provision a fresh VM](provision-vm.md).
