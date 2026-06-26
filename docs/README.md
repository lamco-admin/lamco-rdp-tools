# lamco-rdp-tools documentation

Documentation for the `rdpsee` (observe) and `rdpdo` (act) RDP tools, organized
by the [Diátaxis](https://diataxis.fr/) framework: tutorials to learn, how-to
guides to get a task done, reference for the facts, and explanation for the
ideas.

## Tutorial — start here

- [Getting started](tutorial/getting-started.md) — your first inspection and
  automation, end to end.

## How-to guides — task-oriented

Observe (`rdpsee`):

- [Scan a subnet for RDP exposure and gate it in CI](how-to/scan-subnet-ci.md)
- [Fingerprint and inventory a server fleet](how-to/fingerprint-inventory.md)
- [Inspect a server's TLS certificate](how-to/inspect-certificate.md)
- [Capture recon screenshots](how-to/capture-screenshots.md)

Act (`rdpdo`):

- [Automate a login and reach the desktop](how-to/automate-login.md)
- [Run a visual-regression check in CI](how-to/visual-regression-ci.md)
- [Record and replay a session](how-to/record-replay.md)
- [Calibrate click coordinates](how-to/calibrate-clicks.md)
- [Verify audio playback](how-to/verify-audio.md)
- [Provision a fresh VM](how-to/provision-vm.md)

## Use cases — scenario-oriented

- [CI/CD visual testing](use-cases/ci-visual-testing.md)
- [Golden-image validation](use-cases/golden-image-validation.md)
- [Security recon and RDP-exposure audit](use-cases/security-recon-audit.md)
- [VDI performance baselining](use-cases/vdi-baselining.md)
- [Accessibility verification](use-cases/accessibility-verification.md)
- [Server fleet inventory](use-cases/fleet-inventory.md)

## Explanation — concepts

- [Observe vs act: the two tools](explanation/observe-vs-act.md)
- [The connection and security model](explanation/connection-and-security.md)
- [Visual matching and needles](explanation/visual-matching-and-needles.md)
- [The rdpsee fingerprint scheme](explanation/fingerprint-scheme.md)
- [Codecs and graphics](explanation/codecs-and-graphics.md)

## Reference — the facts

- Command reference: `man rdpsee`, `man rdpdo`, or `<tool> help [command]`
- [JSON output](reference/json-output.md) — the `--json` shapes each command emits
- [Exit codes](reference/exit-codes.md)

---

For an overview of the toolkit and installation, see the top-level
[README](../README.md). Both tools work against any standards-compliant RDP
server. Website: <https://lamco.ai/products/lamco-rdp-tools/>.
