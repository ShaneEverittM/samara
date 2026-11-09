# ADR-0017: Declared Clock Capability

- Status: Proposed (title only)
- Date: November 9, 2026
- Deciders: Samara maintainers
- Scope: Making time a declared Program capability, `program.clock()`, so
  that `Command::after` and request deadlines take a `Clock` like effects take
  an `EffectCapability`; a live runtime binds a clock (`bind_tokio_clock`, or
  a scaled or paused clock) or has no time at all; controlled execution keeps
  logical time as the controlled form of the same capability

## Context (to be written)

Time is the one boundary that escaped the capability discipline of ADR-0008:
a Component can schedule a timer or a deadline with nothing declared in its
Config, so a Program's declarations are complete for effects and sources and
silently incomplete for time. Controlled time is one of Samara's most valued
properties for scenario testing; this ADR would make the live side honest
about it too, and let clocks be swapped without touching Components. The
Elm comparison (`Process.sleep` as an effect, `Time.every` as a
subscription) and the cost to `Command::after` call sites are recorded in
the conversation that produced this title and belong here when it is
written.
