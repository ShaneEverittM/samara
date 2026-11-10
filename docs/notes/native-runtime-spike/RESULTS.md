# Exploratory local spike results

Source and reproduction commands belong to `shane/native-runtime-spike`;
see the [experiment index](README.md) for the pinned commit and overall conclusion.

2026-09-25, Samara base `b2002ff`, final source pinned in the index. No production
runtime changes. Raw samples: [results.csv](results.csv).

These are the original five-run CLI medians. The subsequent
[Criterion run](CRITERION_RESULTS.md) adds sampled estimates and confidence
intervals; the original measurements and CSV are retained as the exploratory baseline.

## Configuration

- Apple M2 Max, 12 logical CPUs, 64 GiB RAM; macOS 26.7 (25G229), arm64.
- `rustc 1.100.0-nightly (34baba539 2026-08-16)`, LLVM 23.1.0;
  Cargo 1.100.0-nightly (8a0d8afba 2026-08-15).
- Cargo's ordinary release profile; same binary/compiler for every engine.
  Locked Mio 1.2.2 (default features disabled), Tokio 1.49.0, libc 0.2.189.
- 4,096 reads per Component; 1 or 64 Components; one outstanding read each.
  Pending feed supplies 128 bytes per request; native turn budget is 32 reads.
  Samara/Tokio retain their own scheduling budgets.
- One warmup per engine/workload, five timed repetitions with rotating engine
  order, then one separate allocation-counting repetition. Build/check jobs
  finished before timing. Active workstation, no CPU affinity or frequency
  control. Peer startup and its reusable buffer are synchronized before timing.

```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo +nightly run \
  --offline --release -p samara-coroutine-spike --features nightly-coroutines \
  -- --reads 4096 --repeats 5 > experiments/coroutine-spike/results.csv
```

## Measurements

Medians of timed samples. Throughput is millions of completed read/transition
cycles per wall-clock second. CPU is **executor-thread** ns per completion,
excluding the peer thread. Allocation calls include reallocations and are from
the separate counting pass; they do not measure bytes or peak memory.

| Components | Input | Engine | M completions/s | CPU ns/completion | Allocations/completion |
|---:|---|---|---:|---:|---:|
| 1 | Ready | Samara | 0.520 | 1547.6 | 11.0054 |
| 1 | Ready | Manual | 1.262 | 441.4 | 0.0000 |
| 1 | Ready | Coroutine | 1.254 | 459.6 | 0.0000 |
| 1 | Pending | Samara | 0.565 | 1395.1 | 11.0059 |
| 1 | Pending | Manual | 1.285 | 414.0 | 0.0005 |
| 1 | Pending | Coroutine | 1.284 | 411.3 | 0.0005 |
| 64 | Ready | Samara | 0.564 | 1420.9 | 11.0023 |
| 64 | Ready | Manual | 3.041 | 322.5 | 0.0000 |
| 64 | Ready | Coroutine | 3.018 | 325.2 | 0.0000 |
| 64 | Pending | Samara | 0.570 | 1433.5 | 11.0025 |
| 64 | Pending | Manual | 2.917 | 337.9 | 0.0003 |
| 64 | Pending | Coroutine | 2.864 | 343.8 | 0.0003 |

For 64 Components, timed throughput ranges were 0.558–0.567 / 2.942–3.058 /
2.962–3.025 M/s (Samara / manual / coroutine, ready), and 0.562–0.574 /
2.809–2.966 / 2.826–2.983 M/s (pending). The single-Component samples last only
a few milliseconds and have substantially more relative noise; these are
screening measurements, not statistical confidence intervals.

Each pending run parked and resumed exactly 32 times per Component: 32 total
for one Component, 2,048 for 64. No batch exists before the initiating empty
read; each request follows a path that returns Pending or yields Wait. The peer
can win the race with physical reactor parking, so these counts establish real
suspension/resumption boundaries, not a kernel sleep for every wait.
In ready mode, Tokio still initially returned WouldBlock/Pending 1 time for one
Component and 60 times for 64, despite preloaded data: its readiness observation
must first be populated. The native direct-read path had zero ready-mode parks.
The WouldBlock counter records the public read attempt, not kernel syscalls.

At the global midpoint, every native Component had completed 2,048 reads.
Samara ready input also had 2,048 each; pending input ranged from 2,041 to 2,064
across repetitions. This samples I/O completion progress; each Model can lag
its Driver by at most one read. It is not a starvation or tail-latency bound.

All runs checked their final counts and order-sensitive checksums against the
same deterministic byte streams. Focused tests also passed for all engines,
ready/pending input, 1/64 Components, and suspended cancellation. Cancellation
tests verified operation destruction and registration cleanup, then supplied
more bytes and observed no completion or terminal Model report afterward.
Stable tests, stable Clippy, nightly tests, nightly Clippy, and formatting passed.

## Interpretation and limits

This specialized dispatcher is faster for this workload. At 64 Components,
coroutines reached about 5.4x the baseline's ready throughput and 5.0x its pending
throughput. Manual state machines were essentially as fast; the small differences
do not establish an advantage for either native representation. The useful
compiler result is that the coroutine retains Driver context inline without a
per-operation allocation or a Model reference.

This is **not Future-versus-Coroutine evidence**. Important unequal work:

- Samara uses general Component/active-effect lookup tables, capability/type
  checks, boxed Commands and Messages, task/channel delivery, trace/lifecycle
  accounting, and real shutdown. Native code uses direct socket token indexing,
  homogeneous inline operation slots, and immediate completion-to-update
  delivery. The experiment cannot attribute the gain to task handoff alone.
- Native storage is preallocated during setup. Samara's post-spawn runtime
  allocations are counted. Samara emits a terminal typed Report Effect/channel
  per Component to expose the Model safely; native code already owns the Model
  vector. Both include operation startup and cleanup, but their cleanup duties
  differ. See the [experiment README](https://github.com/ShaneEverittM/samara/blob/a61a3b8afb06a54641fa70f32a9cd1e1494795a7/experiments/coroutine-spike/README.md) for the full list of omitted production features.
- Both native controls use the same bounded runnable queue and raw socket
  reads. Samara uses its actual scheduler and Tokio readiness API. Native
  single-Component versus 64-Component scaling includes different reactor
  amortization; it does not establish how a general runtime would scale.
- The peer/channel feeding policy is shared. Pending mode includes its wall
  delays; batching amortizes them but cannot remove that influence. Peer CPU
  and allocations are excluded. Tiny pending native allocation counts are
  compatible with feeder-channel block allocation, not per-read coroutines;
  this run did not collect allocation stack traces.
- Every engine includes audit atomics/Arc operations and midpoint sampling.
  The counting allocator's disabled TLS check remains in timed runs. There is
  no HTTP/TLS, heterogeneous workload, cross-Component messaging, timer, Layer,
  controlled execution, or production-equivalent cancellation in this spike.

The result justifies further investigation of storage/routing/scheduling costs
in Samara. It does not justify a nightly migration. A next experiment should
equalize more of the production bookkeeping (or progressively remove measured
costs from the existing path) before choosing a new execution foundation.
