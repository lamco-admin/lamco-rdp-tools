# Golden-image validation

**Who:** platform and infrastructure teams who build VM or desktop images with
Packer, Ansible, or similar, and need to prove an image actually boots and works
before it is promoted.

**Why these tools:** building an image is not the same as verifying it. `rdpdo`
boots the image over RDP and confirms the desktop comes up, login works, and the
expected applications are present — the things a build log cannot tell you.

## The workflow

1. **Build** the image with your existing pipeline.
2. **Boot** a VM from the candidate image.
3. **Provision** it from first screen to desktop.
4. **Assert** the desktop and key applications are present.
5. **Promote** the image only if every assertion passes.

## A validation run

```bash
# Bring a freshly booted VM to a logged-in desktop, patiently
rdpdo -s "$VM_HOST" \
  boot-sequence provision.rdpdo-script

# Assert the desktop and a key app launched correctly
rdpdo -s "$VM_HOST" -u "$VM_USER" -p "$VM_PASSWORD" \
  --junit validation.xml --fail-capture evidence/ \
  mouse-hide \
  expect baselines/desktop.png 0.95 120 \
  expectclick baselines/app-icon.png \
  expect baselines/app-window.png 0.95 60
```

A non-zero exit blocks promotion; `validation.xml` records exactly which check
failed and `evidence/` shows what the screen looked like.

## Front half: provisioning

A cold-booted image often shows a portal prompt, a lock screen, or a login screen
first. The `boot-sequence` script handles those with first-boot-friendly
timeouts. See [provision a fresh VM](../how-to/provision-vm.md).

## Building blocks

- [Provision a fresh VM](../how-to/provision-vm.md)
- [Automate a login](../how-to/automate-login.md)
- [Run a visual-regression check in CI](../how-to/visual-regression-ci.md)
