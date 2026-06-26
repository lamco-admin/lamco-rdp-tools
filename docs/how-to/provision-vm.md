# Provision a fresh VM

Take a freshly booted VM from its first screen — a screen-sharing portal prompt,
a lock screen, or a login screen — through to a usable desktop. `rdpdo`'s
provisioning commands handle the compositor-specific steps with profiles, and
`boot-sequence` runs a whole provisioning script with patient timeouts.

## Accept a screen-sharing portal

Some desktops show a portal permission dialog before they will share the screen.
Accept it for the relevant compositor:

```bash
rdpdo -s 192.168.1.10 accept-portal gnome --verify
```

`--verify` confirms the dialog was dismissed. Profiles for each compositor live
under `~/.config/rdpdo/profiles/`; pass `--profile NAME` to select one.

## Unlock a locked screen

```bash
rdpdo -s 192.168.1.10 unlock gnome "$RDP_PASSWORD" --verify
```

## Log in

```bash
rdpdo -s 192.168.1.10 login myuser "$RDP_PASSWORD" --profile gnome --verify
```

See [automate a login](automate-login.md) for the typing-based alternative and
for asserting you reached the desktop.

## Chain the whole sequence

These are ordinary commands, so chain them in the order the VM presents its
screens:

```bash
rdpdo -s 192.168.1.10 \
  accept-portal gnome --verify \
  unlock gnome "$RDP_PASSWORD" --verify \
  login myuser "$RDP_PASSWORD" --profile gnome --verify \
  expect /tmp/desktop.png 0.95 90
```

## Use a boot-sequence script for slow boots

A freshly powered-on VM can take a while to present each screen. `boot-sequence`
runs a provisioning script with longer default timeouts suited to first boot:

```bash
rdpdo -s 192.168.1.10 boot-sequence provision.rdpdo-script
```

Author the script as one command per line (see
[record and replay](record-replay.md) for the script format), and it will tolerate
the delays of a cold boot.

## Notes

Provisioning is the natural front half of [golden-image validation](../use-cases/golden-image-validation.md):
bring the VM up, then assert it booted, logged in, and shows the expected
applications.
