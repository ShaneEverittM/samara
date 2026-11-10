# Responsiveness under sustained load

Source and reproduction commands belong to `shane/native-runtime-spike`;
see the [experiment index](README.md) for the pinned commit and overall conclusion.

The native backend retains its throughput gain while serving quiet Components.
At budget 256, median throughput is **1.82–1.84× Tokio**, with similar median
per-trial p99 response through the quiet Components. Timer Driver resumption
gets later at larger budgets, even when response through update stays similar.
Manual and coroutine Drivers perform similarly: these measurements do not
justify nightly as a performance requirement.

## Loaded results

64 hot Reader Components, one quiet socket reader, and one quiet timer Component.
Throughput is median million completed hot reads/second. Latencies are in
microseconds: the **median of five per-trial p99s**, not a pooled p99 or confidence
interval. Observation is an Effect emitted by the quiet Component's update, so
its timestamp includes message delivery, update, and subsequent effect dispatch.

| Engine | Native budget | M reads/s | Input resume p99 | Input observation p99 | Timer resume p99 | Timer observation p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Tokio | — | 0.564 | 143.4 | 261.0 | 152.8 | 266.8 |
| Manual | 32 | 0.567 | 132.0 | 333.2 | 108.0 | 308.8 |
| Manual | 64 | 0.757 | 94.2 | 228.1 | 92.0 | 213.4 |
| Manual | 128 | 0.920 | 161.2 | 276.3 | 118.6 | 249.0 |
| Manual | 256 | 1.041 | 159.5 | 232.6 | 200.4 | 265.0 |
| Coroutine | 32 | 0.554 | 190.0 | 461.5 | 187.6 | 469.9 |
| Coroutine | 64 | 0.750 | 151.4 | 285.5 | 100.8 | 252.8 |
| Coroutine | 128 | 0.904 | 163.7 | 295.2 | 199.2 | 288.5 |
| Coroutine | 256 | 1.028 | 180.3 | 264.4 | 194.0 | 273.0 |

Budget 64 is also interesting: manual Drivers deliver about 1.34× Tokio's
throughput and lower median latency here. Budget 256 raises timer-resumption p99
to about 200 µs from about 100 µs at 64. This is evidence of a tunable tradeoff,
not enough to choose a production optimum. The native default remains 32.

Variation matters. Selected min–max ranges across the five trials:

| Engine / budget | M reads/s | Input observation p99, µs | Timer resume p99, µs | Worst input / timer observation, µs |
| --- | ---: | ---: | ---: | ---: |
| Tokio | 0.552–0.584 | 206.7–909.9 | 119.0–638.7 | 5,418.6 / 4,872.8 |
| Manual / 64 | 0.723–0.759 | 186.3–263.0 | 90.3–142.1 | 399.0 / 523.5 |
| Manual / 256 | 1.004–1.059 | 219.4–484.8 | 162.1–322.2 | 3,250.4 / 3,344.8 |
| Coroutine / 64 | 0.712–0.763 | 199.1–363.6 | 95.2–192.7 | 3,306.0 / 2,656.9 |
| Coroutine / 256 | 0.865–1.033 | 251.3–701.2 | 184.1–717.6 | 2,378.2 / 2,606.5 |

No probe was lost; these maxima include delayed probes. Five short workstation
trials do not establish a latency bound. Full ranges for all configurations are
in [the aggregate CSV](responsiveness-summary.csv), with p50/p95/p99/max for every
trial in [the trial CSV](responsiveness-trials.csv).

## Idle controls

Removing the hot Readers produced much noisier results, including producer
scheduling jitter. Values below are medians of per-trial p99s in µs; brackets
show the min–max per-trial p99, not individual-event extremes.

| Engine | Input observation | Timer resume | Producer jitter |
| --- | ---: | ---: | ---: |
| Tokio | 1,368.8 [450.6–1,795.5] | 7,826.3 [1,487.9–16,537.5] | 6,803.9 |
| Manual | 460.5 [286.1–1,185.6] | 1,902.8 [843.2–33,353.7] | 1,927.6 |
| Coroutine | 511.2 [30.2–818.8] | 1,796.3 [130.2–3,791.5] | 2,111.5 |

Loaded producer-jitter p99 medians were consistently about 251–256 µs. The idle
outliers remain unattributed; they must not be read as intrinsic executor costs
or subtracted as a fixed baseline. The largest idle observation was about 44 ms.

## Method and limits

Measured 2026-09-25 on Apple M2 Max, 12 logical CPUs, 64 GiB, macOS 26.7
(25G229), aarch64. All performance cases use the same Rust
1.100.0-nightly compiler (34baba539, 2026-08-16), release profile, with no competing
agent builds/tests. Stable compatibility was separately verified on Rust 1.95.0;
this report does not measure stable-versus-nightly compiler performance.

The same actual Samara Program and hot ReadByte Drivers run on current-thread
Tokio or the native backend. A peer refills hot Unix sockets in 4 KiB batches.
Each trial has 200 warmup and 1,000 measured probes of each kind, at 1 ms spacing.
Five rotated trials cover 12 configurations, retaining 144,000 events including
warmup. A separate producer thread writes input probes on an absolute schedule.
Timers retain their absolute deadlines and catch up after stalls. Late samples
are retained, avoiding omission of delayed work.

Input latency starts immediately before the producer writes, excluding its
scheduling jitter; that jitter and scheduled-to-observation latency are recorded
separately. Timer latency starts at its deadline. Resume is measured in the
Driver; observation is an upper bound on response through update, not an exact
update-entry timestamp. Clocks and shared instrumentation stay outside Models.
Timers use finite Driver effects backed by `sleep_until` / `Wait::Until`; this
experiment does **not** directly benchmark `Command::after`.

Hot throughput counts Driver completions over the producer's actual measured
window, not separately timed Model transitions. Instrumentation is shared across
engines; it includes the existing hot-read audit and observation-channel sends.
Formatting, percentile calculation, and file writes happen after each trial.
Allocation counting is disabled. The host pumps native turns or checks Tokio
completion after 5 ms sleeps; scheduler policies are not identical or retuned
for Tokio. Higher native budgets amortize several costs together, so this is
not a profile attributing the gain to one mechanism.

The workload emphasizes tiny local I/O with mostly runnable work and periodic
real suspension. It does not establish behavior for HTTP/TLS, many Sources,
large reducer computations, other machines, or worst-case production loads.
No CPU affinity or power-policy controls were applied. The evidence supports
further native-backend investigation; it does not demonstrate a coroutine
advantage over manual operations or impose a fairness contract.

## Validation and reproduction

The [acceptance contract](https://github.com/ShaneEverittM/samara/blob/a61a3b8afb06a54641fa70f32a9cd1e1494795a7/experiments/coroutine-spike/RESPONSIVENESS_CONTRACT.md) preceded implementation.
Six new tests cover quantiles, invalid input, pure controlled transitions,
absolute deadlines after delays, actual I/O suspension, exactly-once probe
handling, and cancellation cleanup. All 15 experiment tests pass on nightly;
all 13 applicable tests pass on explicit `+stable`. The stable workspace
all-target check and nightly all-target Clippy with warnings denied also pass.
The analysis independently checks all 144,000 raw
events against 120 trial summaries, including every percentile and throughput
calculation. No production runtime implementation or default changed in this
experiment; rollback removes the harness and its documentation/results.

Run from the repository root:

```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo +nightly run --offline --locked --release \
  -p samara-coroutine-spike --example responsiveness --features native-port,nightly-coroutines -- \
  --raw experiments/coroutine-spike/target/responsiveness/samples.csv \
  > experiments/coroutine-spike/responsiveness-trials.csv
python3 experiments/coroutine-spike/analyze_responsiveness.py \
  experiments/coroutine-spike/responsiveness-trials.csv \
  experiments/coroutine-spike/target/responsiveness/samples.csv \
  --output experiments/coroutine-spike/responsiveness-summary.csv
```

Use `+stable` and `--features native-port` to run Tokio/manual cases without
coroutines. Raw events remain locally under ignored `target/responsiveness/`;
the trial and aggregate CSVs are retained with the experiment.
