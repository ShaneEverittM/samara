# pulse: a tiny uptime monitor with an alert journal

The second hands-on DX trial after `examples/url`, briefed October 8, 2026.
The `url` checker covered one Component, stdin, HTTP, one debounced timer,
and controlled tests. This one exercises competing keyed operations, an
in-flight cap, cancellation, Drain, a two-Component Program joined through a
Port, request and reply from both ends, and a reusable library-style unit.

## The brief as written

A command-line tool that watches several URLs on independent schedules and
journals state changes to a file.

Console commands on stdin:

```
watch <name> <url> [every <seconds>]   # add or replace a target
unwatch <name>
pause <name> | resume <name>
status                                 # one line per target: name, state, streak, last check
history [n]                            # last n journal entries
```

Two Components joined by a Port:

- **Monitor** owns the targets. Each target has its own poll loop built from
  `Command::after` and an HTTP request with a timeout. A target is Unknown,
  Up, or Down, with a consecutive-failure streak. Three failures flip it to
  Down and one success flips it back. It never overlaps two checks on the
  same target, and it caps in-flight checks across all targets at two.
- **Journal** is a reusable library-style unit. It receives `Transition`
  messages, suppresses flapping by recording only a state that has held for
  ten seconds of logical time, and appends one line per confirmed transition
  to a file. It answers a `Recent(n)` request with those lines. The
  `history` command makes Monitor issue that request and print the reply.

Behaviours that make the tests interesting:

- `unwatch`, or `watch` with a changed interval, must bump a per-target
  revision so a late outcome from the old loop is ignored (the keyed
  operation question in the follow-ups).
- With three targets and a cap of two, one check must wait, and controlled
  execution should show a deterministic order.
- A target that flaps every two seconds never reaches the journal. One that
  stays Down for ten seconds produces exactly one line.
- Ctrl-C uses `Shutdown::Drain`, so checks in flight finish and the journal
  flushes before exit. A controlled test runs both Drain and Cancel and
  compares the files.
- Stdin EOF still has no clean answer; note what you want to write there,
  since that feeds the completion and supervision design.

Suggested order: Journal first with its controlled tests, then Monitor with
one target, then several targets with the cap, then the Port wiring and
`history`.

Things to watch for: how much ceremony a two-Component Program needs
compared with `url`, whether a library unit can expose a clean assembly
without leaking its capabilities, and whether anything in the Driver trait
forces policy into the Driver instead of the Component.

## What the brief changed since

- The transition's `at` field was dropped. The time belongs to the moment
  the journal confirms the transition, so the journal asks for it with the
  first-party `ReadSystemTime` effect (ADR-0013) instead of carrying it in
  the message.
- The hand-written `AppendLine` effect and Driver became first-party
  filesystem effects (ADR-0013) and then a permission-typed file resource
  capability, `File<ReadAppend>`, bound to one exact file by the host
  (ADR-0022). The journal appends and reads through that capability.
- "A reusable library-style Component" became an `Assembly` (ADR-0013, 0018):
  `Journal` exports a `Connector<JournalMessage>` and declares no Port. The
  host hands Monitor the Connector.
- `protocol!` and the request trait are gone (ADR-0015, 0016). The vocabulary
  is the hand-written `JournalMessage` enum; `Recent` replies with a
  `Result`, since reads can fail (ADR-0020).
- Capabilities moved out of Component configuration into
  `Component::Requires`, typed on the Program (ADR-0019), so a host that
  forgets a driver fails to compile rather than at `build()`. Component names
  are optional diagnostics (ADR-0021).

## Status

The journal appends transitions and answers `Recent` from the file's
contents. Monitor exists in `main.rs` as a sketch wired to stdin and stdout;
the targets, polling, hold policy, and the console commands are not yet
written.
