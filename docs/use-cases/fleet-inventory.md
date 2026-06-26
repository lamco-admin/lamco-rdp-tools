# Server fleet inventory

**Who:** operators and auditors who need an accurate, repeatable inventory of the
RDP servers on their network — what is there, how each is configured, and what
changed since last time.

**Why these tools:** `rdpsee` reads each server's identity and configuration
without logging in, and emits JSON, so an inventory is a short script plus `jq`.
Because the fingerprint is stable, the same script doubles as a drift detector.

## Collect the inventory

Sweep for reachable hosts, then fingerprint each one:

```bash
# Discover reachable RDP hosts and their posture
rdpsee scan 10.0.0.0/24 --json > scan.json

# Fingerprint each reachable host
jq -r '.[] | select(.reachable) | .server' scan.json | while read -r host; do
  rdpsee -s "$host" --json id
done | jq -s '.' > fleet.json
```

`fleet.json` now holds a fingerprint, security posture, certificate identity, and
key algorithms for every reachable server.

## Group and summarize

```bash
# How many distinct configurations exist, and which hosts share each
jq -r 'group_by(.fingerprint)[] |
  "\(.[0].fingerprint)  \(length) host(s): \([.[].server] | join(", "))"' \
  fleet.json
```

Servers that should be identical but show different fingerprints are
configuration outliers worth investigating.

## Detect change over time

Keep each run and diff against the last accepted baseline:

```bash
diff <(jq -S 'map({server, fingerprint, cert_sha256})' baseline.json) \
     <(jq -S 'map({server, fingerprint, cert_sha256})' fleet.json) \
  && echo "fleet unchanged"
```

- A changed **fingerprint** means a host's security or capability configuration
  moved.
- A changed **cert_sha256** alone means the certificate was reissued.

## Add certificate expiry tracking

Fold in certificate validity to flag soon-to-expire hosts:

```bash
jq -r '.[] | select(.reachable) | .server' scan.json | while read -r host; do
  rdpsee -s "$host" --json cert | jq -c '{server, not_after, self_signed}'
done
```

## Building blocks

- [Fingerprint and inventory a fleet](../how-to/fingerprint-inventory.md)
- [Scan a subnet](../how-to/scan-subnet-ci.md)
- [Inspect a certificate](../how-to/inspect-certificate.md)
- [JSON output](../reference/json-output.md)
