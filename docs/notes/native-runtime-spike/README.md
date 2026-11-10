# Native runtime experiment — conclusion

September 25, 2026. Keep Tokio as the implementation for 0.1. It already works
well in these workloads, and its tuning headroom remains unmeasured. Profile
and tune the existing implementation against the same throughput and latency
workload before reconsidering the execution foundation.

The experiment progressed from a stripped dispatcher to a native backend using
actual Samara Programs, then measured responsiveness under sustained load.
The fuller port largely matched Tokio at its original native turn budget of 32.
At budget 256 it delivered about 1.8–2.0× throughput in the measured workloads;
quiet-Component response remained similar, while timer resumption became later.
Manual state machines performed similarly to compiler coroutines, so the
measurements establish no performance reason to require nightly.

We varied native scheduling budgets while leaving Tokio at its existing
settings. These results establish an optimization opportunity, not Tokio's
performance ceiling. A replacement would need a durable advantage after
comparable tuning, sufficient to justify maintaining scheduling, I/O integration,
and ecosystem compatibility. Compiler coroutines remain an implementation
option; adopting them or replacing Tokio is not a 0.1 decision.

## Evidence

- [Original stripped spike](RESULTS.md): exploratory timing, CPU, and allocation
  observations, with substantial differences in work performed.
- [Criterion follow-up](CRITERION_RESULTS.md): sampled throughput estimates for
  that stripped workload.
- [Native Program port](NATIVE_PORT_RESULTS.md): broader semantic coverage,
  budgets 32/256, and allocation diagnostics.
- [Responsiveness experiment](RESPONSIVENESS_RESULTS.md): 64 busy Components,
  quiet input and timer probes, five trials per configuration, and idle controls.

Reports retain their CSV evidence and measurement limits. In particular, the
responsiveness summaries are medians of per-trial p99s, with substantial trial
variation; they are not latency guarantees. All performance cases used the same
nightly compiler, with stable compatibility checked separately.

## Source and reproduction

The implementation, ADR, contracts, tests, and harnesses are preserved on
`shane/native-runtime-spike` at commit
[`a61a3b8afb06a54641fa70f32a9cd1e1494795a7`](https://github.com/ShaneEverittM/samara/tree/a61a3b8afb06a54641fa70f32a9cd1e1494795a7).
Its [experiment README](https://github.com/ShaneEverittM/samara/blob/a61a3b8afb06a54641fa70f32a9cd1e1494795a7/experiments/coroutine-spike/README.md)
describes building and running the experiments.

Run the commands in these reports from the repository root of that branch or
commit. Their generated CSV paths remain under `experiments/coroutine-spike/`;
the recorded copies live here on `shane/road-to-0.1`. Larger raw event and
Criterion trees remain locally under ignored `target/` directories and can be
regenerated. This branch adds only research evidence; production runtime code,
dependencies, and defaults are unchanged.
