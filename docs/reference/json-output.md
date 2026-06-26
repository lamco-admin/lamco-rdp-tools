# JSON output reference

Both tools accept `--json` and then emit machine-readable JSON instead of the
human-readable text. This page documents the shape each command emits. Optional
fields are omitted when they have no value (for example `color_depth` appears
only when the server reported one), so consumers should treat absent fields as
"not available," not as an error.

`--json` is honored by every `rdpsee` command and by `rdpdo`'s reporting,
measurement, and pixel commands. Commands whose only result is success or
failure (most `rdpdo` action verbs) signal the outcome through the
[exit code](exit-codes.md), not JSON.

## rdpsee

### `scan`

Always an **array**, one object per target (even for a single target):

```json
[
  {
    "server": "192.168.1.10:3389",
    "reachable": true,
    "security": "HYBRID_EX",
    "nla_required": true,
    "egfx_capable": true,
    "restricted_admin": false,
    "redirected_auth": false,
    "extended_client_data": true
  }
]
```

`security` is omitted when the server is unreachable or rejected negotiation; in
the rejection case a `failure` field carries the failure code (for example
`HYBRID_REQUIRED_BY_SERVER`). An unreachable or non-RDP target carries a `note`
instead.

### `cert`

```json
{
  "server": "192.168.1.10:3389",
  "subject": "CN=host",
  "issuer": "CN=host",
  "self_signed": true,
  "not_before": "2026-06-10T03:52:18Z",
  "not_after": "2026-07-10T03:52:18Z",
  "serial": "2C:F7:0F:8C:...",
  "signature_algorithm": "SHA256-RSA",
  "public_key": "RSA",
  "subject_alt_names": ["DNS:host"],
  "sha256_fingerprint": "327438ad7e8e..."
}
```

`subject_alt_names` is omitted when the certificate has none.

### `id`

```json
{
  "server": "192.168.1.10:3389",
  "fingerprint": "rdp_se_Sr/244fe77e6a056182",
  "security": "SSL",
  "nla_required": false,
  "egfx_capable": false,
  "public_key": "RSA",
  "signature_algorithm": "SHA256-RSA",
  "self_signed": true,
  "cert_sha256": "327438ad7e8e..."
}
```

See [the fingerprint scheme](../explanation/fingerprint-scheme.md) for how
`fingerprint` is built.

### `report`

The negotiated capability report. Optional fields (`color_depth`, `compression`,
`egfx_caps`) and an empty `codecs` are omitted when not negotiated.

```json
{
  "connected": true,
  "security_protocol": "TLS",
  "desktop_size": { "width": 1280, "height": 800 },
  "static_channels": ["cliprdr", "drdynvc", "rdpsnd"],
  "egfx_active": true,
  "color_depth": 32,
  "egfx_caps": "V8.1 (AVC420)",
  "codecs": ["RemoteFX"]
}
```

### `shot`

```json
{ "server": "192.168.1.10", "path": "screenshot.png", "width": 1280, "height": 800 }
```

## rdpdo

### `info`

Identical in shape to `rdpsee report` above — both are built from the same
connection report.

### `perf`

```json
{
  "time_to_first_frame_ms": 38,
  "total_frames": 142,
  "avg_fps": 30.1,
  "bandwidth_kbps": 8200.0,
  "frame_interval_p50_ms": 32.0,
  "frame_interval_p99_ms": 110.0,
  "bytes_received": 5242880,
  "bytes_sent": 4096
}
```

`time_to_first_frame_ms`, `avg_fps`, `bandwidth_kbps`, and the percentiles are
omitted until enough frames have been observed to compute them.

### `measure`

```json
{ "label": "login-time", "elapsed_ms": 4200, "matched": true }
```

### `status`

```json
{
  "server": "192.168.1.10:3389",
  "resolution": "1280x800",
  "graphics": "EGFX",
  "frames": 142,
  "avg_fps": 30.1,
  "bytes_rx": 5242880,
  "bytes_tx": 4096,
  "uptime_secs": 5,
  "disconnected": false
}
```

### `watch`

```json
{ "duration_secs": 10, "total_frames": 300, "avg_fps": 30.0 }
```

### `monitor list`

```json
{ "monitors": [{ "id": 1, "primary": true, "width": 1280, "height": 800 }] }
```

### Pixel and matching commands

`pixel`, `assert-pixel`, `find-color`, `checksum`, and `diff` also accept
`--json` and emit a command-specific result (for example `pixel` emits the
sampled color, `find-color` the matched clusters). Run `rdpdo help <command>`
for the exact fields of each.
