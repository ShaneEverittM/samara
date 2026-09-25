# Native Program port results

Source and reproduction commands belong to `shane/native-runtime-spike`;
see the [experiment index](README.md) for the pinned commit and overall conclusion.

The port executes actual Samara Programs through a native command interpreter
and pinned terminal operations. Restoring Samara's machinery removes most of
the stripped loop's large speedup at the original dispatch budget. A larger
budget substantially improves native throughput. Scheduler policy matters much
more here than the choice between manual state machines and compiler coroutines.

## Scope and correctness

The backend reuses Components, kernels, Commands, capabilities, ports, request
tokens, subscription reconciliation, and Layer machinery. It supports
heterogeneous effects, queued messages, batches, sends, notifications,
request/reply deadlines, timers, Sources, and Drain/Cancel. Both native Driver
representations use the same scheduler and type-erasure path.

Validation passed: 11 backend regressions, 6 Program/Layer/coroutine tests, and
3 I/O/transition tests across all five engines. The existing production suite
passed all 325 tests including doctests. Stable workspace all-target checks,
stable manual-port tests, and nightly all-target Clippy with warnings denied
passed. Loopback HTTP tests required permission to bind local fixture listeners.
See [the evidence map](https://github.com/ShaneEverittM/samara/blob/a61a3b8afb06a54641fa70f32a9cd1e1494795a7/experiments/coroutine-spike/PORT_CONTRACT.md#evidence-map).

The port remains owner-driven and Unix-only. It has no cross-thread host handles,
built-in HTTP/TLS Drivers, Future bridge, exact stream bindings, new controlled
scheduler, or full production conformance claim. The HTTP test exercises pure
redirect policy with a fixture Driver. The existing controlled runtime supplies
the behavioral oracle and never invokes these live Drivers.

## Criterion: shared read Program, budget 32

Measured 2026-09-25 on Apple M2 Max, 12 logical CPUs, 64 GiB, macOS 26.7
(25G229), aarch64. Rust 1.100.0-nightly (34baba539, 2026-08-16), LLVM 23.1.0;
Criterion 0.8.2. No other agent builds/tests ran during measurement.

Million byte completions/second; brackets are inverse mean-time 95% confidence
intervals, not predictions for other workloads:

| Components | Input | Tokio Samara | Native manual | Native coroutine |
| ---: | --- | ---: | ---: | ---: |
| 1 | Ready | 0.569 [0.548, 0.584] | 0.624 [0.615, 0.632] | 0.626 [0.617, 0.634] |
| 1 | Pending | 0.575 [0.566, 0.583] | 0.594 [0.570, 0.613] | 0.615 [0.605, 0.625] |
| 64 | Ready | 0.582 [0.578, 0.585] | 0.585 [0.580, 0.589] | 0.535 [0.508, 0.559] |
| 64 | Pending | 0.570 [0.560, 0.577] | 0.574 [0.570, 0.579] | 0.555 [0.541, 0.568] |

The 64-Component coroutine ready case was noisier and slower in this run.
Coroutine/manual differences do not establish an intrinsic advantage for either
representation. Fixed Criterion case order and workstation noise remain limits;
the subsequent rotating CLI run also shows no consistent wall-time win at 32.

All cases use 2,048 reads per Component and pending feed batches of 128 bytes.
Each Reader emits the same ReadByte commands, receives the same typed outcome
Messages, runs the same pure update, and emits the same terminal Report Effect.
There are 20 flat samples per case, 200 ms warmup, 1 s requested measurement,
and 10,000 bootstrap resamples: 240 samples and 1,920 measured iterations total
at budget 32. Expensive cases exceed the requested measurement duration.

Socket/Program/runtime construction precedes timing. Startup, Driver creation,
reads/updates/reports, and shutdown are timed. Checks and final destruction follow
timing. Native readiness registration happens on a parked wait and is timed;
Tokio socket registration happens during setup. Native Report delivery uses a
standard channel and Tokio uses a Tokio channel. These remaining differences,
as well as fairness policy, limit causal attribution. Sources, Layers, and
requests are correctness-tested but are absent from this throughput workload.

## Budget sensitivity: 256 work items per turn

A separate full Criterion run changes only the native dispatch budget. Tokio
runs again as a contemporaneous control. Million completions/second, 95% intervals:

| Components | Input | Tokio Samara | Native manual | Native coroutine |
| ---: | --- | ---: | ---: | ---: |
| 1 | Ready | 0.560 [0.546, 0.573] | 1.147 [1.132, 1.161] | 1.080 [1.064, 1.099] |
| 1 | Pending | 0.519 [0.494, 0.539] | 1.021 [1.006, 1.037] | 1.042 [1.015, 1.067] |
| 64 | Ready | 0.558 [0.538, 0.574] | 1.063 [1.046, 1.078] | 1.062 [1.051, 1.074] |
| 64 | Pending | 0.586 [0.577, 0.594] | 1.024 [0.986, 1.053] | 1.034 [1.018, 1.050] |

This run has another 240 samples and 2,780 measured iterations. The coroutine
port is roughly **1.8–2.0× Tokio throughput** here; manual Drivers are similarly
fast. Three rotating CLI timing runs independently show the same broad gain.
The default remains 32. A turn counts queued work units, not completed reads;
increasing it amortizes reactor polling and per-turn bookkeeping together.
This does not isolate their individual costs.

The tradeoff is up to eight times as many work items between reactor checks
under a continuously runnable workload. This throughput run did not measure
timer/readiness tail latency; the subsequent [responsiveness experiment](RESPONSIVENESS_RESULTS.md)
measures both under sustained load. This is a demonstrated throughput tuning
opportunity, not evidence that 256 is the right production policy or that
coroutines beat Future state machines.

## Allocation and CPU diagnostics

Separate CLI passes count allocation/reallocation calls on the executor thread,
excluding setup and peer-thread allocations. At 64 Components there are about
**11.00 allocations/completion for Tokio and 9.00 for both native variants**
(pending native: 9.01). Operation state and its erased wrapper remain separately
boxed; this port makes no zero-allocation claim.

Executor-thread CPU nanoseconds/completion, medians of three uninstrumented CLI
timing runs at 64 Components and budget 32:

| Input | Tokio | Native manual | Native coroutine |
| --- | ---: | ---: | ---: |
| Ready | 1,406 | 1,021 | 995 |
| Pending | 1,461 | 1,131 | 1,089 |

Lower measured CPU time did not produce a comparable wall-clock gain at 32.
This counter excludes the peer thread and time off the executor thread; it is
a separate diagnostic, not Criterion's sampled statistic.

## Profiling candidates

The interpreter retains boxed Commands, descriptors, outcomes and Messages;
separate operation and Message dispatch; generic component lookup; subscription
reconciliation; and contextual panic handling. It also uses HashMap operation
storage, scans for pending-work diagnostics, and polls Mio once per bounded turn.
Those costs are absent from the stripped control and shared by manual/coroutine
Drivers. These are source-grounded candidates, not profiled cost attribution.

## Reproduction and raw evidence

```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo +nightly bench --offline --locked \
  -p samara-coroutine-spike --bench native_port --features native-port,nightly-coroutines
python3 experiments/coroutine-spike/summarize_criterion.py \
  experiments/coroutine-spike/target/criterion/native_port --group native_port \
  > experiments/coroutine-spike/native-port-criterion-summary.csv
DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo +nightly run --offline --locked --release \
  -p samara-coroutine-spike --features native-port,nightly-coroutines -- \
  --reads 2048 --repeats 3 > experiments/coroutine-spike/native-port-results.csv
# Budget sensitivity; saved under a distinct Criterion group:
DEVELOPER_DIR=/Library/Developer/CommandLineTools SAMARA_NATIVE_BUDGET=256 \
  cargo +nightly bench --offline --locked -p samara-coroutine-spike \
  --bench native_port --features native-port,nightly-coroutines
python3 experiments/coroutine-spike/summarize_criterion.py \
  experiments/coroutine-spike/target/criterion/native_port_budget256 \
  --group native_port_budget256 > experiments/coroutine-spike/native-port-budget256-criterion-summary.csv
# The same CLI command above with --budget 256 produced the additional CSV.
```

[Criterion estimates](native-port-criterion-summary.csv) retain all 12 cases.
[CLI observations](native-port-results.csv) retain timing and allocation passes,
including the original stripped controls. Generated Criterion JSON is ignored
under this package's `target/criterion/` directory.
The larger-budget run has separate [Criterion estimates](native-port-budget256-criterion-summary.csv)
and [CLI observations](native-port-budget256-results.csv).
