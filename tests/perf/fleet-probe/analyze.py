#!/usr/bin/env python3
"""analyze.py — pin the sharing ratio phi and the idle baseline from
fleet-probe measurement runs (ADR-multi-worktree-posture ticket T1).

Reads the artifacts written by run-one.sh / run-fleet.sh (one or more
results directories, each holding instance-*.json files plus an optional
fleet.json) and prints, per scale:

  1. the measured inputs (per-instance and fleet RSS planes, warm hits);
  2. the sharing ratio phi per plane:
         phi = 1 - (fleet_total - single) / ((N - 1) * single)
     where `single` is the N=1 run's instance line (falling back to the
     fleet's own per-instance median, flagged) and fleet_total is the
     co-resident aggregate from the fleet timeline — the tail-median for
     the idle plane, the timeline peak for the peak plane;
  3. the ADR memory-math table (section 4) re-filled with measured values:
     A-as-is (N x P) vs A+CAS projection (N(1-phi)P + phiP) for 10/20/50
     worktrees, with fits-32GiB / fits-64GiB verdicts. The verdict rule is
     the ADR section 6 host-fit rule: engine plane + 512 MiB sidecar budget
     + 4096 MiB OS/agent-toolchain allowance against the host total;
  4. the idle-RSS baseline verdict against the ADR per-instance budget
     targets (<= 40 MiB @10k, <= 64 MiB @100k).

Reading the measured phi: under today's posture (A1, all analysis state
in-heap, no cross-process RAM sharing) the expected engine-plane value is
phi ~= 0 — that is the confirmation, not a failure. The ADR's 0.4/0.5
figures are the POST-T3 projection this probe is designed to re-measure
once the CAS-backed SourceStore lands (re-run unchanged; RSS then
double-counts shared file-backed pages, so also record per-process
footprint accounting on that re-run — see PLAN.md). Disk-side sharing is
measured separately by the central-store du delta (expected ~= one
corpus copy, i.e. near-total dedup) and the trace warm-hit counts.

Stdlib only. Exits 0 when at least one run was read, 2 otherwise.
"""

import json
import math
import os
import sys

IDLE_TARGET_MIB = {"1k": 40.0, "10k": 40.0, "100k": 64.0}
WORKTREE_ROWS = (10, 20, 50)
SIDECAR_BUDGET_MIB = 512.0
OS_ALLOWANCE_MIB = 4096.0
HOSTS = (("32GiB", 32768.0), ("64GiB", 65536.0))

FLEET_KIB = "fleet_rss_kib"


def kib_to_mib(value):
    return value / 1024.0


def fmt_mib(value):
    if value is None or (isinstance(value, float) and math.isnan(value)):
        return "MISSING"
    if value >= 1024.0:
        return f"{value / 1024.0:.2f} GiB"
    return f"{value:.1f} MiB"


def load_instance(path):
    try:
        with open(path, "r", encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) and data.get("schema") == "dartq-fleet-probe/1" else None


def load_fleet(path):
    try:
        with open(path, "r", encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) and data.get("schema") == "dartq-fleet-probe-fleet/1" else None


def load_run(path):
    """One results directory (fleet run) or one instance JSON file."""
    if os.path.isdir(path):
        instances = []
        for name in sorted(os.listdir(path)):
            if name.startswith("instance-") and name.endswith(".json"):
                data = load_instance(os.path.join(path, name))
                if data is not None:
                    instances.append(data)
        fleet = load_fleet(os.path.join(path, "fleet.json"))
        if not instances and fleet is None:
            return None
        n = fleet["N"] if fleet else len(instances)
        scale = fleet["scale"] if fleet else infer_scale(path, instances)
        start = fleet.get("start_mode", "unknown") if fleet else "unknown"
        return {"path": path, "instances": instances, "fleet": fleet,
                "n": n, "scale": scale, "start": start}
    data = load_instance(path)
    if data is None:
        return None
    return {"path": path, "instances": [data], "fleet": None, "n": 1,
            "scale": infer_scale(path, [data]), "start": "single"}


def find_runs(path):
    """A results dir (or its parent: one-level expansion), or one instance file."""
    run = load_run(path)
    if run is not None:
        return [run]
    if os.path.isdir(path):
        runs = []
        for name in sorted(os.listdir(path)):
            sub = os.path.join(path, name)
            if os.path.isdir(sub):
                run = load_run(sub)
                if run is not None:
                    runs.append(run)
        return runs
    return []


def infer_scale(path, instances):
    text = str(path)
    for scale in ("100k", "10k", "1k"):
        if scale in text:
            return scale
    return "unknown"


def rss(line, key):
    value = (line.get("rss_kib") or {}).get(key, 0)
    return float(value) if value else None


def fleet_rss(fleet, key):
    value = (fleet.get(FLEET_KIB) or {}).get(key, 0)
    return float(value) if value else None


def phi(fleet_total, single, n):
    if not fleet_total or not single or n < 2:
        return None
    return 1.0 - (fleet_total - single) / ((n - 1) * single)


def fits(value_mib, host_mib):
    return "fits" if value_mib + SIDECAR_BUDGET_MIB + OS_ALLOWANCE_MIB <= host_mib else "NO"


def valid_instances(run):
    return [line for line in run["instances"] if line.get("valid")]


def median(values):
    values = sorted(values)
    if not values:
        return None
    mid = len(values) // 2
    return values[mid] if len(values) % 2 else (values[mid - 1] + values[mid]) / 2.0


def resolve_reference(runs):
    """Pick the single-instance reference (peak, idle, source).

    Prefers a dedicated N=1 run; falls back to the per-instance median of
    the largest fleet run (flagged, since a warm-chain fleet member is not
    a cold-start measurement).
    """
    singles = [r for r in runs if r["n"] == 1 and valid_instances(r)]
    if singles:
        line = valid_instances(singles[-1])[0]
        return (rss(line, "analysis_peak"), rss(line, "idle_after_settle"),
                f"N=1 run {singles[-1]['path']}")
    fleets = sorted(
        (r for r in runs if r["n"] > 1 and valid_instances(r) and r["fleet"]),
        key=lambda r: r["n"])
    if fleets:
        inst = valid_instances(fleets[-1])
        peaks = [rss(l, "analysis_peak") for l in inst if rss(l, "analysis_peak")]
        idles = [rss(l, "idle_after_settle") for l in inst if rss(l, "idle_after_settle")]
        source = (f"per-instance median of the N={fleets[-1]['n']} fleet run "
                  f"(fallback — no N=1 run)")
        return (median(peaks) if peaks else None,
                median(idles) if idles else None, source)
    return None, None, "MISSING"


def analyze_scale(runs, scale):
    print(f"\n=== scale {scale} ===")
    single_peak, single_idle, single_source = resolve_reference(runs)
    fleet_runs = sorted(
        (r for r in runs if r["n"] > 1 and valid_instances(r) and r["fleet"]),
        key=lambda r: r["n"])

    print(f"single-instance reference: peak={fmt_mib(kib_to_mib(single_peak) if single_peak else None)} "
          f"idle={fmt_mib(kib_to_mib(single_idle) if single_idle else None)}  [{single_source}]")

    phis = {"peak": None, "idle": None}
    for run in fleet_runs:
        inst = valid_instances(run)
        peaks = [rss(l, "analysis_peak") for l in inst if rss(l, "analysis_peak")]
        idles = [rss(l, "idle_after_settle") for l in inst if rss(l, "idle_after_settle")]
        ref_peak = single_peak or median([p for p in peaks if p])
        ref_idle = single_idle or median([i for i in idles if i])
        fallback = "" if single_peak else " (median fallback)"

        fleet_idle = fleet_rss(run["fleet"], "steady_last_window_median")
        phi_idle = phi(fleet_idle, ref_idle, run["n"])
        fleet_peak = fleet_rss(run["fleet"], "peak_coresident")
        phi_peak = phi(fleet_peak, ref_peak, run["n"])

        print(f"\nrun N={run['n']} start={run['start']} [{run['path']}]")
        print(f"  per-instance peak: min={fmt_mib(kib_to_mib(min(peaks)) if peaks else None)} "
              f"median={fmt_mib(kib_to_mib(median(peaks)) if peaks else None)} "
              f"max={fmt_mib(kib_to_mib(max(peaks)) if peaks else None)}")
        print(f"  per-instance idle: min={fmt_mib(kib_to_mib(min(idles)) if idles else None)} "
              f"median={fmt_mib(kib_to_mib(median(idles)) if idles else None)} "
              f"max={fmt_mib(kib_to_mib(max(idles)) if idles else None)}")
        print(f"  fleet co-resident: peak={fmt_mib(kib_to_mib(fleet_peak) if fleet_peak else None)} "
              f"idle(tail median)={fmt_mib(kib_to_mib(fleet_idle) if fleet_idle else None)}")
        print(f"  phi_idle = {phi_idle:.3f}{fallback}" if phi_idle is not None
              else "  phi_idle = MISSING")
        if run["start"] == "parallel":
            print(f"  phi_peak = {phi_peak:.3f} (synchronized peak plane){fallback}"
                  if phi_peak is not None else "  phi_peak = MISSING")
        else:
            print(f"  phi_peak bound = {phi_peak:.3f} (staggered start: only one instance "
                  f"peaks at a time; run --start parallel for the synchronized plane)"
                  if phi_peak is not None else "  phi_peak = MISSING")
        if phi_idle is not None and not 0.0 <= phi_idle <= 1.0:
            print("  WARNING: phi_idle outside [0,1] — measurement noise or plane mismatch")

        if phis["idle"] is None or run["n"] == max(r["n"] for r in fleet_runs):
            phis["idle"] = phi_idle
        if phis["peak"] is None:
            phis["peak"] = phi_peak

        first, last = inst[0], inst[-1]
        t_first = first.get("traces") or {}
        t_last = last.get("traces") or {}
        print(f"  warm hits (content_id_hit): instance-1={t_first.get('content_id_hit')} "
              f"instance-{len(inst)}={t_last.get('content_id_hit')}; "
              f"LibraryAnalyzed from_cache: {t_first.get('library_from_cache')}/"
              f"{t_first.get('library_analyzed')} -> {t_last.get('library_from_cache')}/"
              f"{t_last.get('library_analyzed')}")
        degraded = [(i + 1, (l.get("traces") or {}).get("memory_degrade", 0))
                    for i, l in enumerate(inst)]
        hot = [f"#{i}:{n}" for i, n in degraded if n]
        if hot:
            print(f"  WARNING: memory ladder fired (budgeted peaks, not natural): {', '.join(hot)}")

    if fleet_runs:
        central = fleet_runs[-1]["fleet"].get("central_store") or {}
        before = central.get("du_kib_before", 0) or 0
        after = central.get("du_kib_after", 0) or 0
        print(f"  central store du: before={before} KiB after={after} KiB "
              f"(delta {after - before} KiB) subdirs={central.get('subdirs_kib', {})}")

    phis = {k: v for k, v in phis.items() if v is not None}
    if not single_peak:
        print("\nADR table: SKIPPED — no single-instance reference for this scale")
        return

    phi_peak_txt = f"{phis['peak']:.3f}" if "peak" in phis else "MISSING"
    phi_idle_txt = f"{phis['idle']:.3f}" if "idle" in phis else "MISSING"
    print(f"\nADR memory-math table refilled (phi_peak={phi_peak_txt}, "
          f"phi_idle={phi_idle_txt}; '*' = projected row, N not measured)")
    header = (f"{'worktrees':>9} | {'A as-is peak':>13} | {'A+CAS peak':>13} | "
              f"{'A as-is idle':>13} | {'A+CAS idle':>13} | {'fits 32GiB':>10} | {'fits 64GiB':>10}")
    print(header)
    print("-" * len(header))
    measured_ns = {r["n"] for r in fleet_runs}
    for n in WORKTREE_ROWS:
        asis_peak = kib_to_mib(single_peak) * n
        asis_idle = kib_to_mib(single_idle) * n if single_idle else None
        if "peak" in phis:
            # ADR section 4 form: N(1-phi)P + phiP = N*P - (N-1)*phi*P —
            # degenerates to the as-is column at phi=0
            cas_peak_txt = fmt_mib(n * kib_to_mib(single_peak)
                                   - (n - 1) * phis["peak"] * kib_to_mib(single_peak))
        else:
            cas_peak_txt = "MISSING"
        if "idle" in phis and single_idle:
            cas_idle_txt = fmt_mib(n * kib_to_mib(single_idle)
                                   - (n - 1) * phis["idle"] * kib_to_mib(single_idle))
        else:
            cas_idle_txt = "MISSING"
        mark = "" if n in measured_ns else "*"
        v32 = f"{fits(asis_peak, HOSTS[0][1])}/{fits(asis_idle, HOSTS[0][1]) if asis_idle else '-'}"
        v64 = f"{fits(asis_peak, HOSTS[1][1])}/{fits(asis_idle, HOSTS[1][1]) if asis_idle else '-'}"
        print(f"{n:>8}{mark} | {fmt_mib(asis_peak):>13} | {cas_peak_txt:>13} | "
              f"{fmt_mib(asis_idle):>13} | {cas_idle_txt:>13} | {v32:>10} | {v64:>10}")
    print("verdict rule: engine plane + 512 MiB sidecar budget + 4096 MiB OS/toolchain "
          "allowance vs host total (ADR section 6)")


def main(argv):
    if not argv:
        print(__doc__)
        return 2
    runs = []
    for path in argv:
        found = find_runs(path)
        if not found:
            print(f"analyze.py: skipping {path} — no fleet-probe artifacts found", file=sys.stderr)
            continue
        runs.extend(found)
    if not runs:
        print("analyze.py: no readable runs", file=sys.stderr)
        return 2

    by_scale = {}
    for run in runs:
        by_scale.setdefault(run["scale"], []).append(run)

    for scale, scale_runs in sorted(by_scale.items()):
        analyze_scale(scale_runs, scale)
        target = IDLE_TARGET_MIB.get(scale)
        if target:
            _, idle, _ = resolve_reference(scale_runs)
            if idle is not None:
                idle_mib = kib_to_mib(idle)
                verdict = "PASS" if idle_mib <= target else "FAIL"
                print(f"\nidle baseline @1 instance: {idle_mib:.1f} MiB vs ADR target "
                      f"<= {target:.0f} MiB — {verdict}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
