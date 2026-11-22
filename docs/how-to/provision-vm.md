# Provision a fresh VM

Take a freshly booted VM from its first screen — a screen-sharing portal prompt,
a lock screen, or a login screen — through to a usable desktop. `rdpdo`'s
provisioning commands handle the compositor-specific steps with profiles, and
`boot-sequence` runs a whole provisioning script with patient timeouts.

## Accept a screen-sharing portal

Some desktops show a portal permission dialog before they will share the screen.
Accept it for the relevant compositor:

```bash
rdpdo -s 192.168.1.10 accept-portal gnome
```

Profiles for each compositor live under `~/.config/rdpdo/profiles/`; pass
`--profile NAME` to select one. `accept-portal` refuses `--verify`: the dialog
closes on Cancel as well as on Share, so the screen can't show which one
happened. Check the grant where it takes effect, for example by connecting to
the server that asked for it.

## Unlock a locked screen

```bash
rdpdo -s 192.168.1.10 unlock gnome "$RDP_PASSWORD" --verify
```

`--verify` fails the command unless the lock screen is gone: it keeps the screen
as it was just before the profile's last input (the Enter that submits the
password), waits up to 15 seconds for the screen to settle, and requires at least
25% of it to have changed, the result not to be a single colour, and the session
to still be connected. A rejected password changes only the prompt, so it fails.

## Log in

```bash
rdpdo -s 192.168.1.10 login myuser "$RDP_PASSWORD" --profile gnome --verify
```

`login --verify` checks the same way that the login screen is gone, and fails
if the server ends the session after the credentials go in, because rdpdo can't
see the result then. See [automate a login](automate-login.md) for the
typing-based alternative and for asserting you reached the desktop.

## Chain the whole sequence

These are ordinary commands, so chain them in the order the VM presents its
screens:

```bash
rdpdo -s 192.168.1.10 \
  accept-portal gnome \
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
