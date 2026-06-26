# Security recon and RDP-exposure audit

**Who:** security teams, penetration testers, and operators who need to know what
RDP is exposed on a network and whether it meets a security baseline.

**Why these tools:** `rdpsee` learns a server's security posture *before*
authenticating — it speaks only the negotiation. You can sweep a network, read
each server's selected protocol and capability flags, inspect certificates, and
fingerprint hosts, all without a single credential.

## The workflow

1. **Sweep** the address space to find RDP endpoints and their posture.
2. **Gate** against a baseline so a non-compliant host is an actionable failure.
3. **Drill in** on individual hosts: certificate, fingerprint, screen.
4. **Track** results over time to catch drift.

## Sweep and baseline

```bash
# Inventory every RDP host in the range
rdpsee scan 10.0.0.0/24 --json > exposure.json

# Fail an audit job if any host allows non-TLS or non-NLA connections
rdpsee scan 10.0.0.0/24 --ci --expect tls,nla
```

`scan` reports reachability, the selected security protocol, whether NLA is
required, and pre-auth flags such as restricted-admin and redirected-auth — the
posture that matters for exposure.

## Drill into a host

```bash
rdpsee -s 10.0.0.5 cert      # certificate: issuer, validity, self-signed, SANs
rdpsee -s 10.0.0.5 id        # stable fingerprint + exact-cert SHA-256
rdpsee -s 10.0.0.5 shot recon.png   # what is on screen, no login where allowed
```

`shot` without credentials captures a login screen where the server permits
anonymous connections — useful for confirming what an exposed host reveals.

## Track drift

Fingerprint the fleet on a schedule and diff against a baseline; a changed
fingerprint means a host's security configuration moved. See
[fingerprint and inventory a fleet](../how-to/fingerprint-inventory.md).

## Scope and ethics

Run this only against systems you are authorized to assess. The tools are passive
observers at the protocol level, but scanning and screenshotting hosts you do not
own may be unlawful.

## Building blocks

- [Scan a subnet and gate it in CI](../how-to/scan-subnet-ci.md)
- [Inspect a certificate](../how-to/inspect-certificate.md)
- [Fingerprint and inventory a fleet](../how-to/fingerprint-inventory.md)
- [Capture recon screenshots](../how-to/capture-screenshots.md)
- [The connection and security model](../explanation/connection-and-security.md)
