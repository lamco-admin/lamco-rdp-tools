# Scan a subnet for RDP exposure and gate it in CI

Find every RDP server in an address range, read its pre-authentication security
posture, and fail a CI job when a host does not meet your security baseline. No
credentials are involved: `rdpsee scan` only speaks the security negotiation.

## Scan a range

Pass one or more targets. A target is a host, `host:port`, or an IPv4 CIDR range:

```bash
rdpsee scan 10.0.0.0/24
```

A single target prints a detail block; multiple targets (or a CIDR) print a
compact sweep table:

```
TARGET                 REACH  SECURITY               NLA  EGFX
10.0.0.5:3389          yes    HYBRID_EX              yes  yes
10.0.0.6:3389          yes    STANDARD_RDP_SECURITY  no   no
10.0.0.7:3389          no     -                      -    -
```

CIDR ranges are `/16` to `/32` (capped at 65536 hosts) and are probed
concurrently, so a `/24` finishes quickly.

## Capture the result as JSON

For inventory or further processing, add `--json` (always an array):

```bash
rdpsee scan 10.0.0.0/24 --json > rdp-exposure.json
```

## Gate a CI job

Use `--ci` with `--expect` to require a baseline of every target. `--expect`
takes a comma-separated list of `reachable`, `tls`, `nla`, `egfx`. With `--ci`,
`rdpsee` exits non-zero unless **every** target meets the expectation:

```bash
# Fail if any reachable host allows non-TLS or non-NLA connections
rdpsee scan 10.0.0.0/24 --ci --expect tls,nla
```

A typical CI step:

```yaml
- name: RDP exposure baseline
  run: rdpsee scan 10.0.0.0/24 --ci --expect tls,nla
```

The step fails (exit `1`) and names the offending hosts when one falls short.

## Variations

- **Specific hosts instead of a range:** `rdpsee scan host-a host-b:3390`.
- **Require enhanced graphics too:** add `egfx` to `--expect`.
- **One host, full detail:** drop `--ci` and pass a single target to see the
  full flag set (restricted-admin, redirected-auth, extended-client-data) that
  the table omits.

## Notes

`scan` reports a server's *selected* security protocol and pre-auth capability
flags. To go deeper on a single host — its certificate, a stable fingerprint, or
its full negotiated capabilities — see
[inspect a certificate](inspect-certificate.md),
[fingerprint a fleet](fingerprint-inventory.md), and `rdpsee report`.
