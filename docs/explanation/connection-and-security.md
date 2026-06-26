# The connection and security model

Both tools speak RDP through the IronRDP protocol stack. Understanding the
connection sequence explains what each `rdpsee` tier can see and why the tools
behave as they do around security.

## The connection sequence

An RDP connection is built in stages:

1. **X.224 security negotiation.** The client sends a request listing the
   security protocols it supports; the server replies selecting one and
   advertising a few capability flags. This happens before any encryption or
   login.
2. **TLS upgrade.** For enhanced security the stream is upgraded to TLS, and the
   server presents its certificate.
3. **CredSSP (NLA), if required.** Network Level Authentication runs credentials
   through CredSSP over the TLS channel before the session proper begins.
4. **The session.** Channels are joined, capabilities are exchanged, and graphics
   begin to flow.

`rdpsee`'s [three tiers](observe-vs-act.md#rdpsees-three-observation-tiers) map
directly onto these stages: `scan` stops after stage 1, `cert`/`id` after stage 2,
`report`/`shot` complete the connection.

## Security protocols

The protocol selected at stage 1 is the server's security posture:

- **Standard RDP security** — legacy, no TLS. `rdpsee scan` reports it as
  `STANDARD_RDP_SECURITY`; `cert` cannot inspect it because there is no TLS.
- **TLS (SSL)** — the stream is encrypted with TLS, the server presents a
  certificate, but NLA is not required.
- **CredSSP / NLA (HYBRID, HYBRID_EX)** — credentials are validated before the
  session starts. `scan` reports `nla_required` when the server selects one of
  these.

`rdpsee report` reports the protocol the connection **actually used**, read back
from the negotiation — which can differ from what a client requests, because a
server may downgrade (request NLA, get plain TLS). `scan` reports the same
selected protocol pre-authentication.

## Pre-auth capability flags

The stage-1 response also advertises a handful of flags that `scan` surfaces
without logging in: whether the dynamic graphics pipeline (EGFX) is offered,
whether restricted-admin mode and redirected authentication are supported, and
whether extended client data is accepted. These describe how the server is
configured, visible before any credential changes hands.

## Channels

A session joins static virtual channels for its subsystems — typically `drdynvc`
(the dynamic-channel transport that carries EGFX), `cliprdr` (clipboard), and
`rdpsnd` (audio). `rdpsee report` lists the channels the connection actually
joined, which is why it can tell you, for example, that audio redirection is
present.

## What this means for the tools

- You can learn a great deal — reachability, protocol, NLA requirement, graphics
  support, certificate, fingerprint — **without credentials**, because stages 1
  and 2 happen before authentication.
- Anything that needs the rendered screen or the full capability set (`report`,
  `shot`, and all of `rdpdo`) requires a connection the server lets you complete.

For how the graphics that flow in stage 4 are decoded, see
[codecs and graphics](codecs-and-graphics.md). For how the stage-1/stage-2
observations become a stable identity, see [the fingerprint scheme](fingerprint-scheme.md).
