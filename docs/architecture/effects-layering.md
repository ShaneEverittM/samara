# Effect Layering: Core Effects, Adapters, and Protocols

## Purpose
Define how Samara separates effect execution mechanism from application/protocol policy, while preserving open-set extensibility and simulation support.

## Core Principle
- Runtime owns **mechanism**.
- App/protocol layers own **policy**.

Mechanism examples:
- Sleep/delay scheduling.
- Socket connect/read/write primitives.
- Task lifecycle, cancellation, backpressure, mailbox reinjection.

Policy examples:
- Retry/backoff rules.
- Framing/parsing protocol behavior.
- Reconnect strategy.
- Domain-specific timeout semantics and error mapping.

## Why This Split Exists
If "core effects" include policy, they become large and rigid:
- Effect enums grow without bound.
- Hidden behavior reduces determinism and debuggability.
- Simulation control becomes harder.
- Library reuse declines because assumptions leak from one app into others.

## Layer Model

### Layer 1: Runtime + System Effects (Library Core)
- Provide runtime loop, routing, and supervision.
- Provide stable primitive effect interfaces (`SystemCmd`-style capabilities).
- Provide pluggable backends:
  - Real backend (actual wall-clock and network).
  - Simulated backend (virtual time and controlled I/O).

### Layer 2: Adapters/Protocols (Composable)
- Map app/domain intents onto primitive system effects.
- Map primitive results/failures into structured domain/runtime messages.
- Remain modular by concern (e.g., socket adapter, timer adapter, wire protocol adapter).

### Layer 3: App/Domain Logic
- Pure `update(model, msg) -> (model, cmds)` logic.
- Emits app-level commands (`AppCmd`) without direct runtime/system coupling.

## Recommended Flow
1. `update` emits `AppCmd`.
2. Adapter/protocol layer translates `AppCmd` into primitive system operations.
3. Runtime executes primitive operations via selected backend (real or simulated).
4. Adapter/protocol layer translates outcomes back into `Msg`.
5. Runtime re-enqueues `Msg` into mailbox.

## Contract Rules
- Runtime APIs must not encode protocol-specific policy.
- Adapter/protocol modules must be composable and testable independently.
- Time and I/O access must route through backend interfaces to support faster-than-real-time simulation.
- Any new built-in core effect must be justified as mechanism, not policy.

## Example Command Partition
- `AppCmd`:
  - `StartTelemetryUpload`
  - `RequestFirmwareChunk`
- `SystemCmd`:
  - `Sleep(duration)`
  - `SocketConnect(endpoint)`
  - `SocketWrite(socket_id, bytes)`
  - `SocketRead(socket_id, max_bytes)`

Adapters/protocols bridge between the two.

## Typed-Fallibility API POC
- A compileable prototype for this pattern lives in `src/system_effects.rs`.
- The prototype includes:
  - `SystemEffect` with associated `Output` and `Error` types.
  - `Sleep` with `Error = Infallible`.
  - `SocketConnect` with `Error = SystemIoError`.
  - `ComposedEffect<E, OkMap, ErrMap>` wrapper for mapping system results into envelopes.
  - `EffectRun::{Future, Composed}` for escape-hatch vs structured composition.
