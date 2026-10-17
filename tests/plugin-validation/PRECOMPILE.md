# Sidecar precompile A-B protocol (kernel snapshots + AOT exe pilot)

Ticket #1300, attack-list row 1 of the 2026-10-14/15 sidecar-memory
re-attribution. The sidecar's peak is the VM's in-process CFE compile of the
plugin's import graph, not analysis data: precompiling the entrypoint removes
that compile from the sidecar process entirely. This document is the
measurement protocol for the `dartq_plugin.json` `entrypoint_kind` support
landed with it; all commands below are serial-phase work (they run `dart` and
the built daemon), one process at a time.

## Mechanism under test

A manifest's new `entrypoint_kind` field selects the launch shape
(`crates/dartq-plugin/src/manifest.rs`, spawn branch in
`SpawnSpec::dart`, `crates/dartq-plugin/src/runtime.rs`):

| `entrypoint_kind` | `entrypoint` holds | Spawned argv | What disappears from the child |
| --- | --- | --- | --- |
| `source` (default) | a Dart script | `dart <script>` | — (historical shape) |
| `kernel` | a `.dill` (`dart compile kernel` output) | `dart <path.dill>` — the VM sniffs the kernel magic number and loads the snapshot directly (upstream Dart SDK source, `runtime/bin/dfe.cc`) | the CFE source compile of the whole import graph (~500–850 MiB transient on the worst plugin) and most of the dartdev front-end share |
| `exe` | a self-contained executable (`dart compile exe` output) | the executable, directly — no `dart` front end | additionally the JIT and the dartdev hop |

`scripts/plugin-precompile.sh <plugin-dir> [--aot]` performs one plugin's
compile + manifest flip; the original manifest is preserved once as
`dartq_plugin.json.bak` (never overwritten).

## Expected deltas (re-attribution arithmetic)

Measured anchor (only sidecar number in-tree): the dart_code_linter sidecar
peak censored at its budget, 1,175,601,152 bytes ≈ 1,121 MiB, composed
(static attribution) of:

| Component | Band | Removed by |
| --- | --- | --- |
| A. VM + dartdev + platform-kernel residency, small-program CFE warm-up | ~110–150 MiB | most of the dartdev share (kernel), all of it (exe) |
| B. CFE in-process compile transient over the import graph | ~500–850 MiB | kernel (entirely) |
| C. Retained kernel image + VM symbol/metadata tables | ~40–90 MiB | partially (kernel), reshaped (exe: precompiled image) |
| D. JIT code heap + rule instances + wire state | ~20–40 MiB | JIT portion (exe only) |
| E. Per-cycle analysis data | < 10 MiB | nothing (live data) |

Import graphs (static BFS, source KB) and the derived kernel-snapshot peak
expectation (B scales with source bytes at the 30–70x working-set band; the
one censored measurement implies ~55x):

| Sidecar | Files | Source KB | Expected kernel peak delta |
| --- | --- | --- | --- |
| dart_code_linter-4.4.2 | 1,449 | 16,038 | −500 to −850 MiB |
| solid_lints-2.0.0 | 1,055 | 14,380 | −450 to −760 MiB |
| riverpod_lint-3.1.9 | 946 | 14,256 | −445 to −755 MiB |
| clean_architecture_linter-2.8.0 | 893 | 14,194 | −440 to −750 MiB |
| leancode_lint-27.0.0 | 883 | 13,981 | −435 to −740 MiB |

Post-kernel steady-state expectation: ~200–300 MiB per sidecar (A minus the
dartdev share + C + D). The AOT pilot on dart_code_linter additionally
removes the JIT: expect the steady peak at or below the kernel arm's, the
difference living in D. Startup latency moves too — spawn+handshake sits
inside `cold_full_ms`, and the post-gap respawn round no longer pays the CFE
compile, so both should drop sharply.

All post-fix numbers are bands until this protocol runs; a result outside a
band falsifies the attribution's band placement, not the mechanism.

## Protocol (serial, one daemon instance at a time)

Environment: the pinned SDK root (`DARTQ_DART_SDK`, >= 3.13 — e.g. the fvm
default `$HOME/fvm/default/bin/cache/dart-sdk`) and cargo on PATH. The
snapshot is version-locked to that SDK's VM; recompile after any SDK bump.
`DARTQ_PLUGIN_IDLE_EVICT_MS` is inherited by the daemon and must sit below
the idle gap or the post-gap round measures a still-resident sidecar.

```bash
export DARTQ_DART_SDK="$HOME/fvm/default/bin/cache/dart-sdk"
export DARTQ_PLUGIN_IDLE_EVICT_MS=60000   # < the 90 s gap below

# 1. Baseline — source manifests, all five vendored plugins
scripts/plugin-ab.sh 10 90

# 2. Kernel-precompile the five vendored plugins (serial, one at a time)
for d in tests/plugin-validation/vendor/*-[0-9]*; do
  scripts/plugin-precompile.sh "$d"
done

# 3. Kernel arm — same rig, same corpus, idle-gap arm on
scripts/plugin-ab.sh 10 90

# 4. AOT pilot on the measured worst offender (revert its kernel flip first;
#    the script refuses to double-flip)
d=tests/plugin-validation/vendor/dart_code_linter-4.4.2
cp "$d/dartq_plugin.json.bak" "$d/dartq_plugin.json"
scripts/plugin-precompile.sh "$d" --aot
scripts/plugin-ab.sh 10 90

# 5. Revert the vendored tree
for d in tests/plugin-validation/vendor/*-[0-9]*; do
  cp "$d/dartq_plugin.json.bak" "$d/dartq_plugin.json"
done
pkill -f dartq_serve || true
```

## Reading the results

Per arm, `tests/plugin-validation/results/<stamp>/`:

- `<arm>.log` — the daemon's relayed stderr carries the per-plugin stats
  lines. `peak_rss_bytes` is **the** sidecar channel (the monitor samples
  the child PID directly); the `<arm>.rss` file sums driver+daemon only and
  never contains sidecars.
- `<arm>.json` — `cold_full_ms`, incremental `rounds_ms` percentiles, and
  the post-gap round (`rounds_ms[0]` of the idle-gap phase) = sidecar
  respawn + re-analysis cost.

Comparisons, all present-arm:

1. **Peak**: per-plugin `peak_rss_bytes`, baseline vs kernel vs AOT. The
   baseline dcl reading is censored at its 1,536 MiB budget (the monitor
   kills there); the kernel arm is expected to produce the first uncensored
   reading. Compare against the per-plugin band table above.
2. **Cold**: `cold_full_ms` drops by roughly the sum of the five plugins'
   compile transients (all five spawn inside the cold segment).
3. **Respawn**: post-gap `rounds_ms[0]` drops — a relaunched sidecar loads
   a precompiled entrypoint instead of CFE-compiling its graph.
4. **AOT vs kernel**: steady `peak_rss_bytes` for dart_code_linter
   (difference expected in the 20–40 MiB JIT band) plus any residual
   `cold_full_ms`/respawn movement.

Validity gate (unchanged from the A-B rig): a sample with `restarts >= 1`
or `degraded_cycles >= 1` on any plugin stats line is invalid (the rig
flags it in `*.json.note`); rerun, do not average it in.

## Knobs and notes

- `dart compile kernel` keeps its default `--embed-sources` (source text
  rides inside the `.dill`, ~16 MB on the worst graph — noise against the
  compile transient it removes). `--no-embed-sources` is a size knob if
  dill footprint ever matters; not part of this pilot.
- After precompiling, sidecar argv no longer contains
  `bin/dartq_serve.dart`; the rig's exit-trap `pkill` pattern misses it.
  The protocol's final `pkill -f dartq_serve` covers both spellings.
- The prep script refuses an already-flipped manifest (no duplicate keys);
  revert via the `.bak` copy, as step 4 does for the AOT pilot.
