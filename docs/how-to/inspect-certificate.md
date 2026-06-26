# Inspect a server's TLS certificate

Read the certificate an RDP server presents during the TLS handshake, without
logging in. Use this to check expiry, confirm the issuer, read the subject
alternative names, or record the SHA-256 fingerprint.

## Inspect a certificate

```bash
rdpsee -s 192.168.1.10 cert
```

```
server:       192.168.1.10:3389
subject:      CN=host
issuer:       CN=host
self-signed:  yes
not before:   2026-06-10T03:52:18Z
not after:    2026-07-10T03:52:18Z
serial:       2C:F7:0F:8C:...
signature:    SHA256-RSA
public key:   RSA
subject alt:  DNS:host
sha256:       327438ad7e8e...
```

`rdpsee cert` negotiates an enhanced-security protocol and completes the TLS
handshake, then reports the certificate. It does not authenticate.

If the server offers only standard RDP security (no TLS), there is nothing to
inspect and the command fails with a message saying so.

## Check expiry in a script

Pull the validity window as JSON and act on it:

```bash
rdpsee -s 192.168.1.10 --json cert | jq -r .not_after
```

To warn on certificates expiring soon, compare `not_after` against a threshold in
your monitoring job, or alert when `self_signed` is `true` on a host that should
present a CA-issued certificate.

## Variations

- **Confirm the SANs:** `rdpsee -s host --json cert | jq .subject_alt_names` —
  useful when a client rejects a connection over a name mismatch.
- **Record the fingerprint:** `rdpsee -s host --json cert | jq -r .sha256_fingerprint`
  to pin or compare the exact certificate.

## Notes

The negotiated TLS version and cipher suite are not yet reported; that detail is
pending an upstream IronRDP change. For the certificate's role in identifying a
server, see [fingerprint a fleet](fingerprint-inventory.md).
