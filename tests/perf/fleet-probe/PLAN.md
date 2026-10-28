# Fleet RSS probe — measurement plan (ADR ticket T1)

Purpose: turn the two unmeasured numbers that gate every fleet-capacity
claim into measured ones:

1. **The idle RSS baseline** — "daemon assembled, no analysis in flight"
   was never measured (the steady-state probe gap the ADR section 6 table
   cites); the budget targets (<= 40 MiB @10k, <= 64 MiB @100k) hang on it.
2. **The sharing ratio phi** — the ADR section 4 memory math assumed
   `phi@10k = 0.4`, `phi@100k = 0.5` (band ±0.15). This probe pins the
   formula
   `phi = 1 - (fleet_total - single_instance) / ((N - 1) * single_instance)`
   with measured aggregates, per plane (idle and analysis peak).

Reference posture: the multi-instance memory design — content-identity
sharing (`ContentId`-keyed machine-global L2/L3 stores) instead of git
worktrees, with idle baseline, sharing ratio phi, and per-segment
decomposition as its measured axes (this probe is that design's T1 ticket).
Everything in this directory is measurement tooling only — no
product code changes.

## Why plain directory copies (worktree-ban compliance)

The repo's discipline bans creating git worktrees for any purpose. The ADR
sharing axis is CONTENT IDENTITY — `ContentId` (blake3 of bytes) keyed,
machine-global L2/L3 stores (`$XDG_CACHE_HOME/dartq` or `~/.cache/dartq`) —
not git mechanics. N byte-identical plain copies of a corpus reproduce the
worktree fleet's memory shape exactly: every instance sees the same file
bytes, produces the same content keys, and lands in the same shared roots.
`prepare.sh` makes the copies with `cp -R`; fidelity cost: none.

## Files

| File | Role |
| --- | --- |
| `prepare.sh` | Copy a fixture corpus into N sibling dirs, with a disk-headroom gate |
| `run-one.sh` | One instance: daemon lifecycle + RSS sampling, one JSON line |
| `run-fleet.sh` | N instances co-resident, fleet timeline, central-store du, fleet JSON |
| `probe_one.py` | The actual driver/sampler behind the two wrappers (LSP over stdio, `ps` subtree sampling, `fleet-sample`/`fleet-summarize` subcommands) |
| `analyze.py` | phi + idle baseline + ADR table refill from results dirs |

## Fixture choice

Historically the mega generated corpora carried the anchor faces (10k and
100k files, 85,952 KiB and 616,112 KiB tree peak in the ADR section 1
anchor table); the repo no longer vendors a corpus. The fleet source is
whatever `DARTQ_PROBE_SOURCE` names: a complete package fixture
(`pubspec.yaml`, `pubspec.lock`, `packages/`, and a relative-path
`.dart_tool/package_config.json`), so each copy analyzes its own bytes —
a pub-cache package checkout (see conformance/realworld.toml) or a
generated tree of the target scale.

### Disk headroom and descope arithmetic

`prepare.sh` requires `N x source_size x 1.1 + 1 GiB` free on the
destination volume and refuses otherwise, printing the largest N that fits.
Reference points (source sizes: 40 MB @10k, 398 MB @100k):

| N | 10k copies | 100k copies |
| --- | --- | --- |
| 10 | ~0.45 GiB | ~4.4 GiB |
| 50 | ~2.2 GiB | ~21.9 GiB |

If a 50x100k layout does not fit the volume, descope N (phi is defined for
any N >= 2; N=10 already pins the ratio) or point `DARTQ_FLEET_ROOT` at a
larger volume. APFS may store the copies as clonefile clones and cost far
less than the table above; the gate stays conservative on purpose.

## Environment contract

| Variable | Meaning | Default |
| --- | --- | --- |
| `DARTQ_BIN` | daemon binary | `<repo>/target/release/dartq` (build: `cargo build --release -p dartq-cli`) |
| `DARTQ_DART_SDK` (or `DART_SDK`) | pinned Dart SDK root (>= 3.13), passed as `--sdk` | required |
| `DARTQ_FLEET_ROOT` | copies + results root | `${TMPDIR:-/tmp}/dartq-fleet` |
| `XDG_CACHE_HOME` | central L2/L3 root base — the machine-global sharing seam | unset (= `~/.cache`) |
| `DARTQ_PROBE_IDLE_S` | idle settle window | 60 |
| `DARTQ_PROBE_DRIVER_TIMEOUT_S` | per-instance phase deadline | 7200 |
| `DARTQ_PROBE_WAIT_TIMEOUT_S` | fleet wait per instance JSON line | 7800 |
| `DARTQ_PROBE_HOLD_TIMEOUT_S` | hold-mode release deadline | 14400 |
| `DARTQ_PROBE_COSETTLE_S` | all-up settle before the fleet tail window | 10 |
| `DARTQ_PROBE_FLEET_INTERVAL_S` | fleet timeline cadence | 1.0 |

Isolation rule (recommended for every run): set
`XDG_CACHE_HOME="$DARTQ_FLEET_ROOT/cache"` so the fleet shares a
probe-local central root — the sharing seam stays machine-global within the
run, the developer's real cache is untouched, and the du measurement is
exactly the run's footprint. Erase the whole study with
`rm -rf "$DARTQ_FLEET_ROOT"`.

## Daemon invocation under measurement

Per instance (`probe_one.py`):

```
dartq daemon <copy-dir> --sdk <SDK> --cache-size-limit 10GiB --trace-events <trace.jsonl>
```

- `--cache-size-limit 10GiB`: the daemon face leaves the L3 content store
  unarmed unless the flag is set; the probe arms it at the standing 10 GiB
  default (the analyze-face posture) so the machine-global L3 plane
  participates.
- `--memory-limit` is intentionally NOT set: the daemon's standing 2048 MiB
  per-context budget is the posture the ADR budget table assumes, and a
  ladder firing would replace natural peaks with budgeted ones. A firing is
  visible as `MemoryDegrade` trace rows and flags the sample in
  `analyze.py`.
- `--trace-events`: one JSONL sink per instance. The ADR's "SnapshotReuse"
  class surfaces in the daemon's trace-event taxonomy as
  `FileRevalidated/gate=content_id_hit` plus `LibraryAnalyzed/from_cache`
  (plus `CycleReused`); `analyze.py` counts these as the cross-instance L2
  warm-hit signal.

No plugins exist in the mega corpora, so the measurement is the engine
plane by construction; sidecar budgeting is tickets #1284/#1295/T6.

## Protocol

### Phases and cadence (per instance)

1. **assembled** — daemon spawned; LSP `initialize` handshake completes;
   baseline sampled (100 ms cadence, last pre-analysis sample).
2. **analysis** — `didOpen` every `*.dart` file under the copy (the full
   package: `lib/`, `packages/*/lib/`, `tests/`), then wait until
   `publishDiagnostics` covered every opened URI (the daemon's real push
   face; identical shape to the existing `tests/plugin-validation`
   driver). Samples at 100 ms -> `analysis_peak` (max) and
   `analysis_steady` (median of the final 5 s of the wave).
3. **idle** — settle `DARTQ_PROBE_IDLE_S` (60 s) with no traffic at 1 s
   cadence -> `idle_after_settle` (last sample) and `idle_tail_mean`
   (mean of the last 10).

Sampling is `ps -axo pid=,ppid=,rss=` walked to the daemon's process
subtree (RSS in KiB on macOS). The 100 ms cadence is a compromise: peaks
shorter than ~100 ms can be missed, but the measured anchors (tens of MiB
over multi-second passes) are far above that floor. The driver process is
the daemon's parent, not a child, so it never pollutes the subtree.

### Cold vs warm

A serial fleet run is a warm chain by design: instance 1's cold pass fills
the central L2/L3; instance k's pass reads the generation instances
1..k-1 wrote (its traces show the warm hits — this is the direct
cross-process reuse measurement the ADR names). For a fully cold study,
point `XDG_CACHE_HOME` at a fresh directory first. The canonical sequence
below runs 1 -> 10 -> 50 in one session (the workday shape: store already
populated), plus one optional cold rerun of the N=10 point.

### Start modes and which phi each pins

- `run-fleet.sh <N> <scale>` (serial, default): instances start one at a
  time, each stays alive (`--hold`). After all N are up + `COSETTLE_S`,
  the fleet timeline's tail median is the co-resident **idle** plane ->
  `phi_idle`. Serial start never overlaps analysis peaks, so it cannot pin
  `phi_peak`.
- `run-fleet.sh --start parallel <N> <scale>`: all N drivers launch at
  once and analysis passes stack; the timeline's maximum is the
  synchronized **peak** plane -> `phi_peak`. (A serial run still prints a
  staggered `phi_peak` bound — only one instance peaks at a time, so it
  under-reports sharing; do not quote it.)

### What a run must record

Per fleet run (all automatic, in `results/<stamp>-<scale>-n<N>-<mode>/`):

- `instance-<i>.json` — one JSON line: the five RSS planes, phase
  durations, opened-file count, trace counts, validity + note;
- `fleet-samples.jsonl` — co-residency timeline (summed subtree RSS, 1 s);
- `fleet-summary.json` — timeline peak + 10 s tail median;
- `fleet.json` — N, scale, start mode, central-store du before/after and
  per-subdir, caveats;
- `trace-<i>.jsonl` (per-instance trace events), `daemon-<i>.log` (daemon
  stderr), `driver-<i>.log` (driver stderr).

Validity gate: a run is quotable only when every instance line has
`valid: true` (publish wave complete, daemon alive throughout) and no
`MemoryDegrade` rows fired (peaks must be natural, not budgeted).

### Central-store measurement and its caveat

`fleet.json` records `du -sk` of the central root before/after the run
plus per-subdir sizes. That is ON-DISK content — the proof that N instances
produce one shared copy of blobs/generations (expected delta ~= one corpus
size, near-total dedup). It is not resident memory. Resident sharing under
today's posture (A1, all in-heap) is nil: RSS summation is honest
private-page accounting, and the measured engine-plane phi is EXPECTED to
be ~0 — that confirms the A2/T3 motivation rather than refuting it. Once
the T3 mmap-backed `SourceStore` lands, RSS double-counts shared
file-backed pages; the re-run must add per-process footprint accounting
(`vmmap --summary` / `footprint` on macOS) and count file-backed pages
once. `fleet.json`'s caveats field carries this reminder.

## Exact commands (canonical sequence)

```bash
# one-time build (release, the binary the samplers measure)
cargo build --release -p dartq-cli

export DARTQ_DART_SDK=/path/to/dart-sdk            # >= 3.13, pinned
export DARTQ_FLEET_ROOT="${TMPDIR:-/tmp}/dartq-fleet"
export XDG_CACHE_HOME="$DARTQ_FLEET_ROOT/cache"    # probe-local shared root

cd tests/perf/fleet-probe

# layout (add --dry-run to preview the headroom math without copying)
./prepare.sh 10k 50
./prepare.sh 100k 10        # descope N per the disk table if needed

# idle plane + warm-hit chain: 1 -> 10 -> 50, serial, warm store
./run-fleet.sh 1 10k
./run-fleet.sh 10 10k
./run-fleet.sh 50 10k       # only if disk/CPU budget allows; N=10 already pins phi

./run-fleet.sh 1 100k
./run-fleet.sh 10 100k

# peak plane (synchronized), the larger N the volume allows
./run-fleet.sh --start parallel 10 10k
./run-fleet.sh --start parallel 10 100k

# optional cold rerun of one point
XDG_CACHE_HOME="$DARTQ_FLEET_ROOT/cache-cold" ./run-fleet.sh 10 10k

# analysis (accepts any number of results dirs)
python3 analyze.py "$DARTQ_FLEET_ROOT"/results/*
```

`run-one.sh` exists for single-point sanity checks and is what
`run-fleet.sh` drives internally:

```bash
./run-one.sh "$DARTQ_FLEET_ROOT/10k/copy-001"          # one JSON line on stdout
./run-one.sh --dry-run "$DARTQ_FLEET_ROOT/10k/copy-001" # arg validation only
```

## Mapping results back to the ADR

- **Idle baseline**: `analyze.py` prints the N=1 idle plane against the
  section 6 targets (<= 40 MiB @10k, <= 64 MiB @100k). PASS/FAIL gates
  regressions per the ADR.
- **phi**: replaces the assumed 0.4/0.5 ±0.15. Under A1 expect ~0
  (in-heap, no RAM sharing); after T3 re-run for the real number.
- **Capacity table**: the refilled table projects A-as-is (`N x P`) and
  A+CAS (`N(1-phi)P + phiP`) at 10/20/50 worktrees, with fits-32GiB /
  fits-64GiB verdicts using the ADR section 6 host-fit rule (engine plane
  + 512 MiB sidecars + 4 GiB OS/toolchain). Unmeasured N rows are marked
  `*` (projected from the measured phi). A red A-as-is verdict with a
  green A+CAS verdict is the quantitative case for prioritizing T3.

## Known limitations

- `ps` RSS granularity: 100 ms cadence can miss sub-100 ms spikes; idle
  plane and anchors are far above it.
- macOS RSS is KiB; on Linux the same `ps` fields are read unchanged.
- The driver holds ~10k didOpen frames in flight (tens of MB); its own RSS
  is excluded from every sample by construction (not a daemon descendant).
- 100k-scale runs are long (the publish wave dominates); the per-instance
  deadline defaults to 2 h — raise `DARTQ_PROBE_DRIVER_TIMEOUT_S` on slow
  hosts.
- Serial start cannot produce the synchronized peak plane; use
  `--start parallel` when quoting phi_peak.
