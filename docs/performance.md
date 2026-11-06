# Performance measurement

The checked-in short diagnostic measures a complete HTTP read: token lookup in
PostgreSQL, object lookup, OpenFGA check and data read. It uses closed-loop
clients, one connection per request, reports actual completions, errors, raw
latencies, clock and environment. It does not establish a capacity or SLO.

`paired-benchmark.py` is a same-binary A/A calibration. Six randomized,
balanced AB/BA blocks each contain warm-up and measured requests with four
closed-loop workers. It retains all samples and computes a paired
within-run bootstrap interval of median-latency ratios. This is an exploratory
noise check, not a regression gate: one runner cannot estimate between-VM
variation, samples within a block are not independent hosts, and the generator
itself may bottleneck. The manual workflow is never scheduled.

For a comparison, pin both binaries, image digests, schema, model, dataset,
toolchain, workload and host; validate correctness and zero unexpected errors
for both; randomize balanced order in nearby blocks; measure enough independent
blocks and fresh runs; report effect and uncertainty relative to a
predeclared practical threshold. Do not compare the old demo with this service:
their authentication, persistence and operation semantics differ.

GitHub-hosted runners are VMs. First use A/A repetitions to learn whether their
noise can resolve a proposed effect. A smartphone can provide physical ARM
hardware, but Android scheduling, thermal throttling, battery/charging, CPU clusters,
Termux/proot or virtualization and process interference affect results.
Hardware and workload are not equivalent to x86 cloud VMs. No device settings,
processes or benchmark runner have been changed or installed on the phone.

## Example calibration

The [2026-11-06 GitHub Actions run](https://github.com/qqrm/openfga-axum-lab/actions/runs/42250134346)
recorded six A/A blocks with four closed-loop clients and 120 measured requests
per arm per block. It reported zero errors, a geometric mean of block-level
B/A median-latency ratios of 1.003, and a within-run bootstrap interval of
0.916–1.075. The raw artifact has limited
retention. These numbers describe only this VM and run; they do not establish a
regression threshold, cross-run variability, or production capacity.
