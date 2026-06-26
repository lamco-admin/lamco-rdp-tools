# Fingerprint and inventory a server fleet

Give each RDP server a stable identity so you can detect configuration drift,
group identical hosts, and recognize when an individual server's certificate
changes. `rdpsee id` computes a JA4-style fingerprint of a server's
configuration alongside the SHA-256 of its exact certificate.

## Fingerprint one server

```bash
rdpsee -s 192.168.1.10 id
```

```
server:       192.168.1.10:3389
fingerprint:  rdp_se_Sr/244fe77e6a056182
security:     SSL
nla:          not required
egfx:         no
public key:   RSA
signature:    SHA256-RSA
self-signed:  yes
cert sha256:  327438ad7e8e...
```

Two readings to keep separate:

- **`fingerprint`** identifies the server's *configuration and posture* (security
  tier, capability flags, certificate algorithm family). Two servers with the
  same fingerprint are configured the same way.
- **`cert sha256`** identifies the *exact instance*. It changes when the
  certificate is reissued, even if the configuration is unchanged.

See [the fingerprint scheme](../explanation/fingerprint-scheme.md) for how to
read the fingerprint string.

## Build an inventory

`rdpsee` runs one target per invocation, so loop over your hosts and collect the
JSON:

```bash
for host in $(cat hosts.txt); do
  rdpsee -s "$host" --json id
done | jq -s '.' > fleet-fingerprints.json
```

Group hosts by configuration:

```bash
jq -r 'group_by(.fingerprint)[] | "\(.[0].fingerprint)  \(length) host(s)"' \
  fleet-fingerprints.json
```

## Detect drift over time

Store today's fingerprints and diff against a known-good baseline on a schedule:

```bash
rdpsee -s 192.168.1.10 --json id | jq '{fingerprint, cert_sha256}' \
  > current.json
diff <(jq -S . baseline.json) <(jq -S . current.json) && echo "unchanged"
```

A changed `fingerprint` means the server's security or capability configuration
moved; a changed `cert_sha256` alone means the certificate was reissued.

## Notes

`id` negotiates TLS but does not authenticate, so you can fingerprint servers you
have no account on. For the certificate's full details (validity, issuer, SANs),
see [inspect a certificate](inspect-certificate.md).
