# Getting started

This tutorial walks you through your first session with both tools. By the end
you will have inspected a real RDP server without logging in, then connected and
driven it. It is learning-oriented: follow it in order, on a server you control,
and you will come away knowing how the two tools fit together. For specific
tasks afterward, see the [how-to guides](../README.md#how-to-guides--task-oriented).

You need: a reachable RDP server (a VM with RDP enabled is ideal) and its
address. Some steps use credentials; have a username and password ready for the
later half.

## 1. Install

Download a signed release binary from the
[releases page](https://github.com/lamco-admin/lamco-rdp-tools/releases), or
build from source:

```bash
git clone https://github.com/lamco-admin/lamco-rdp-tools.git
cd lamco-rdp-tools
cargo build --release
```

This produces two binaries, `target/release/rdpsee` and `target/release/rdpdo`.
Put them on your `PATH`, or run them by full path. Confirm they work:

```bash
rdpsee --help
rdpdo --help
```

## 2. Look before you touch: `rdpsee`

Start with the observe tool. It inspects a server and never changes it, so it is
the safe way to learn what you are dealing with. Point it at your server (replace
`192.168.1.10` throughout):

```bash
rdpsee -s 192.168.1.10 scan
```

`scan` does not log in. It speaks only the pre-authentication security
negotiation, and reports what it learned:

```
server:           192.168.1.10:3389
reachable:        yes
security:         SSL
nla (CredSSP):    not required
egfx capable:     no
...
```

This already tells you the server is reachable, which security protocol it
selected, and whether it requires network-level authentication (NLA). You
learned all of that without a single credential.

Now complete a connection and ask for its capabilities:

```bash
rdpsee -s 192.168.1.10 report
```

```
server:     192.168.1.10
security:   TLS
desktop:    1280x800
color:      32-bit
graphics:   EGFX
egfx:       V8.1 (AVC420)
codecs:     RemoteFX
channels:   cliprdr, drdynvc, rdpsnd
```

If your server requires a login, add `-u user -p pass`. Either way, `report`
reads the negotiated session and reports it; it never drives anything.

Add `--json` to any command to get machine-readable output:

```bash
rdpsee -s 192.168.1.10 --json report
```

## 3. Drive the session: `rdpdo`

Now switch to the act tool. Unlike `rdpsee`, which runs one command per
invocation, `rdpdo` takes a **chain** of commands and runs them in order against
a single connection. Take a screenshot:

```bash
rdpdo -s 192.168.1.10 capture /tmp/first.png
```

Open `/tmp/first.png` — that is the server's current screen. (If your server
shows a login screen and refuses anonymous connections, add `-u user -p pass`.)

Now chain a few actions together. The following moves the mouse off-screen so it
does not appear in the shot, types some text, presses Enter, waits, and captures:

```bash
rdpdo -s 192.168.1.10 -u user -p pass \
  mouse-hide type "hello from rdpdo" key enter pause 1 capture /tmp/after.png
```

Each word after the flags is a command: `mouse-hide`, then `type "..."`, then
`key enter`, then `pause 1`, then `capture`. They run left to right on one
connection. Run `rdpdo help` to see all commands grouped by category, or
`rdpdo help capture` for one command's full detail.

## 4. Wait for the screen, then check it

Automation that types blindly is fragile. `rdpdo` can wait for the screen to
settle and assert on what it sees. Save a reference image of a known-good state:

```bash
rdpdo -s 192.168.1.10 -u user -p pass wait-still capture /tmp/desktop.png
```

`wait-still` waits until the screen stops changing before capturing, so you get a
stable frame. Later, you can assert the screen matches that reference:

```bash
rdpdo -s 192.168.1.10 -u user -p pass expect /tmp/desktop.png 0.95 30
```

`expect` waits up to 30 seconds for the screen to match `/tmp/desktop.png` at 95%
similarity, and exits non-zero if it never does. That single line is the heart of
visual testing over RDP.

## Where to go next

You now know the shape of both tools: `rdpsee` observes, `rdpdo` drives, and
`rdpdo` chains commands on one connection. From here:

- To accomplish a specific task, see the [how-to guides](../README.md#how-to-guides--task-oriented).
- To see how teams use these tools, see the [use cases](../README.md#use-cases--scenario-oriented).
- To understand why the tools are split the way they are, read
  [Observe vs act](../explanation/observe-vs-act.md).
- For every command and flag, see `man rdpsee`, `man rdpdo`, and
  `<tool> help <command>`.
