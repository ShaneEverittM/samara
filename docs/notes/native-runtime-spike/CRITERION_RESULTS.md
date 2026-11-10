# Criterion results

Source and reproduction commands belong to `shane/native-runtime-spike`;
see the [experiment index](README.md) for the pinned commit and overall conclusion.

2026-09-25, same Apple M2 Max/macOS 26.7 environment and Rust nightly
1.100.0 (34baba539) as [the exploratory run](RESULTS.md). Criterion 0.8.2,
ordinary Cargo bench/release profile. Minimal Criterion features enable Cargo
bench support only; no Rayon, plots, HTML reports, or async adapter.

## Method

The `read_cycle` target runs all three engines with ready/pending input and
1/64 Components. Each iteration performs 2,048 reads per Component, with pending
batches of 128 and native turn budget 32. This differs from the exploratory
CSV's 4,096-read runs. Throughput is `components * 2048` elements per iteration.

`Bencher::iter_custom` returns the sum of the existing execution-only wall-time
measurements. Socket/runtime/peer setup, Model validation, and final result
destruction occur outside that returned interval. Actual elapsed setup time
still affects Criterion's iteration calibration. All iterations, including
warmup, assert expected Models, reports, released operations/registrations, and
at least one real suspension/resumption per requested pending batch.

Configuration: 20 flat samples per case, 200 ms warmup, 1 s requested measurement,
10,000 bootstrap resamples, 95% confidence. All cases completed. Samara64 needed
about 4.6 s per case to obtain 20 samples and emitted Criterion's expected
insufficient-target-time warning. The suite collected 3,300 measured iterations
across 240 samples, totaling 22.62 s of reported execution time; setup, warmup,
analysis, and compilation are additional. No other agent build/check jobs ran
during measurement.

```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo +nightly bench \
  --offline --locked -p samara-coroutine-spike --bench read_cycle \
  --features nightly-coroutines

python3 experiments/coroutine-spike/summarize_criterion.py \
  experiments/coroutine-spike/target/criterion/read_cycle \
  > experiments/coroutine-spike/criterion-summary.csv
```

Generated full data is under the package's `target/criterion/read_cycle/`,
ignored by Git. The checked-in [summary CSV](criterion-summary.csv) contains
12 extracted rows with sample/iteration counts, mean-time estimates and
confidence intervals, and derived throughput. The extraction script reads
each case's latest `new` results; run the full suite before extraction to avoid
mixing cases from different invocations.

## Estimates

Millions of completions per wall-clock second, **Criterion mean-time estimate
inverted to throughput**, with the corresponding 95% bootstrap interval in
brackets. Throughput interval endpoints reverse the time interval endpoints.
These are not the earlier five-run medians.

| Components | Input | Samara | Manual | Coroutine |
|---:|---|---:|---:|---:|
| 1 | Ready | 0.575 [0.572, 0.579] | 1.306 [1.282, 1.330] | 1.354 [1.345, 1.364] |
| 1 | Pending | 0.569 [0.565, 0.573] | 1.245 [1.229, 1.259] | 1.233 [1.184, 1.273] |
| 64 | Ready | 0.549 [0.538, 0.558] | 2.942 [2.873, 3.013] | 2.987 [2.961, 3.009] |
| 64 | Pending | 0.559 [0.545, 0.569] | 2.924 [2.880, 2.963] | 2.955 [2.927, 2.979] |

The specialized dispatcher remains much faster for this workload. At 64
Components, manual and coroutine intervals overlap in both input modes. These
samples provide no compelling performance reason to prefer coroutines over the
handwritten state machine.

Intervals quantify resampling uncertainty within each case; they do not account
for fixed case order, thermal/frequency drift, OS scheduling, the feeder, or
omitted production behavior. Criterion marked outliers in several cases; they
were not manually removed. No CPU affinity/frequency controls were used.
The direct indexing, inline homogeneous storage, fused completion-to-update
path, and unequal shutdown/trace/capability work remain the same confounders as
the first spike. These data do not isolate Futures versus coroutines or predict
a feature-complete replacement's performance.

The CLI remains useful for CPU time, allocations, parks, and progress; Criterion
adds better wall-time sampling and estimates without changing its timing window.
