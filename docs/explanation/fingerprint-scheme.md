# The rdpsee fingerprint scheme

`rdpsee id` produces a short string that identifies an RDP server's
configuration. It is modeled on the JA4 family of fingerprints: a readable,
categorical prefix plus a hash of the full observable feature set. This page
explains what the string means and how it differs from the certificate hash.

## Two identities

`id` reports two different things, and the difference matters:

- **`fingerprint`** identifies the server's **configuration and posture**: its
  security tier, capability flags, and certificate algorithm family. Two servers
  with the same fingerprint are configured the same way, even if they are
  different machines.
- **`cert_sha256`** identifies the **exact instance**: the SHA-256 of this
  server's specific certificate. It changes when the certificate is reissued,
  even if nothing else about the configuration changed.

So a changed fingerprint means the configuration moved; a changed `cert_sha256`
alone means the certificate was renewed.

## The fingerprint format

```
rdp_<sec><caps>_<signing><key>/<hash>
```

Reading each field:

| Field | Meaning |
|---|---|
| `<sec>` | Security tier: `q`=HYBRID_EX, `h`=HYBRID, `s`=SSL/TLS, `r`=standard RDP |
| `<caps>` | Capability flags, any of: `g`=EGFX, `a`=restricted-admin, `d`=redirected-auth, `e`=extended-client-data (`-` when none) |
| `<signing>` | `S`=self-signed, `C`=CA-signed |
| `<key>` | Server key type: `r`=RSA, `e`=ECDSA, `d`=Ed25519, `x`=other |
| `<hash>` | 16 hex characters: a truncated SHA-256 over the raw protocol bits and certificate algorithm OIDs |

For example, `rdp_se_Sr/244fe77e6a056182` reads as: SSL/TLS security, the `e`
(extended-client-data) flag set, a self-signed certificate, an RSA key, and the
feature hash `244fe77e6a056182`.

## Why a hash and a prefix

The categorical prefix is human-readable: you can glance at two fingerprints and
see how they differ. The hash captures the full feature set — including the exact
protocol bits and certificate algorithm OIDs — independent of any name
formatting, so it is stable and comparable even when the readable tags collapse
detail. Together they give a fingerprint that is both legible and precise.

## Using it

Because the fingerprint is stable, it powers two workflows:

- **Grouping** — hosts that should be identical should share a fingerprint;
  outliers are configuration drift.
- **Drift detection** — store a baseline and compare on a schedule.

See [fingerprint and inventory a fleet](../how-to/fingerprint-inventory.md) and
[server fleet inventory](../use-cases/fleet-inventory.md) for the workflows.
