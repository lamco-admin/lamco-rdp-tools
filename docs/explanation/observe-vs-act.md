# Observe vs act: the two tools

lamco-rdp-tools is split into two binaries on purpose. Understanding why makes
both easier to use and explains why some things belong to one tool and not the
other.

## The boundary: lifecycle, not features

The seam between the tools is **what they do to the server**, not which features
they have:

- **`rdpsee` observes.** It inspects a server and never changes it. It may
  authenticate (its `report` and `shot` commands complete a session), but only to
  read.
- **`rdpdo` acts.** It completes an authenticated session and mutates remote
  state: it injects input, writes the clipboard, resizes the display, provisions.

That single rule tells you which tool to reach for. "What is this server?" is
`rdpsee`. "Make this server do something" is `rdpdo`.

## They share one core

Both binaries are built from one library. The connection, session, graphics
decode, capture, and reporting code is compiled into each. Where `rdpsee` needs a
read-only capability that `rdpdo` also has — completing a connection, taking a
screenshot, reporting capabilities — it reuses the same building blocks rather
than reimplementing them. So the two tools behave identically where they overlap:
the same connection flags, the same `--json`, the same help style.

## Two interaction models

The tools differ in how an invocation is shaped, because their jobs differ:

- **`rdpsee` runs one command per invocation:** `rdpsee <command> [target]`. An
  inspection is a single question with a single answer.
- **`rdpdo` runs a chain:** `rdpdo -s host <cmd> <cmd> <cmd>` executes its
  commands in order against one persistent connection. Automation is a sequence,
  and a sequence on one connection is the natural unit.

## rdpsee's three observation tiers

Observation has depth, and `rdpsee`'s commands sit at three tiers of how far into
the connection they go:

1. **Connectionless** — `scan` speaks only the pre-authentication security
   negotiation. No TLS, no login.
2. **TLS handshake, no auth** — `cert` and `id` complete the TLS handshake to read
   the certificate, but never authenticate.
3. **Completed session** — `report` and `shot` complete the connection (and
   authenticate only if you give credentials), then read capabilities or a frame.

The deeper the tier, the more it sees, and the more access it needs. `scan` works
against any reachable server; `report` needs a connection it is allowed to
complete.

## Why this matters in practice

The split keeps each tool's surface honest. `rdpsee` will never type into a
server or change its state, so it is safe to point at production or at hosts you
are auditing. `rdpdo` is the tool you reach for when you intend to drive. The
[security recon use case](../use-cases/security-recon-audit.md) leans entirely on
`rdpsee`; the [CI visual testing use case](../use-cases/ci-visual-testing.md)
leans on `rdpdo`. Most real workflows use both: inspect first, then act.
