#!/usr/bin/env python3
"""probe_one.py — one-instance RSS probe driver for the T1 fleet study.

Lifecycle (ADR-multi-worktree-posture ticket T1): spawn one dartq daemon in
LSP mode over stdio pointed at one package directory, drive the daemon's
real work face (initialize handshake, didOpen every *.dart file under the
directory, wait for the full publishDiagnostics wave), and sample the RSS of
the daemon's process tree around the three planes the capacity math needs:

  assembled_baseline  — after the initialize handshake, before any didOpen
  analysis_peak       — max subtree RSS during the publish wave
  analysis_steady     — median subtree RSS over the last 5 s of the wave
  idle_after_settle   — last sample after the idle settle window
  idle_tail_mean      — mean of the last 10 idle samples

Sampling is `ps -axo pid=,ppid=,rss=` (macOS/Linux; RSS in KiB) walked to a
subtree from the daemon PID at 100 ms cadence during the analysis pass and
1 s cadence during idle (env-tunable). The driver process itself is the
daemon's parent, not a child, so it is excluded from the subtree by
construction; any daemon-spawned children (plugin sidecars) are included.

Output contract: exactly one JSON line on stdout (schema
`dartq-fleet-probe/1`), everything else on stderr. Exit 0 = valid sample,
3 = invalid (phase timeout, daemon death, missing publishes). The line is
emitted even when invalid. With --hold the driver keeps the daemon alive
after emitting (release via SIGTERM/SIGINT or --hold-timeout) so a fleet
orchestrator can stack instances and measure co-residency.

Subcommands:
  (default) <copy-dir> [options]   run the lifecycle above
  fleet-sample --pids-file F --out F.jsonl [--interval S]
      Sample the summed subtree RSS of every PID listed in F (re-read each
      tick, so PIDs may be appended live) until SIGTERM, the file contains a
      `STOP` line, or --max-seconds elapses. One JSON line per tick.
  fleet-summarize <samples.jsonl> [--tail-window S]
      Reduce a fleet-sample output to peak / tail-median totals, one JSON
      line on stdout.

Stdlib only. No product process is executed by tooling checks; this module
is run only by the measurement phase.
"""

import argparse
import json
import os
import signal
import statistics
import subprocess
import sys
import threading
import time
from pathlib import Path

SCHEMA = "dartq-fleet-probe/1"
FLEET_SAMPLE_SCHEMA = "dartq-fleet-probe-fleet-sample/1"
FLEET_SUMMARY_SCHEMA = "dartq-fleet-probe-fleet-summary/1"

DEFAULT_ANALYSIS_INTERVAL_S = 0.1
DEFAULT_IDLE_INTERVAL_S = 1.0
DEFAULT_IDLE_SETTLE_S = 60.0
DEFAULT_TIMEOUT_S = 7200.0
DEFAULT_HOLD_TIMEOUT_S = 3600.0
STEADY_WINDOW_S = 5.0
PUBLISH_POLL_S = 0.2
MAX_HEADERLESS_BUFFER = 64 * 1024 * 1024


def subtree_rss_kib(root_pid):
    """Sum the RSS (KiB) of the process subtree rooted at root_pid.

    One `ps` snapshot per call; returns 0 when the root is gone.
    """
    try:
        out = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,rss="],
            capture_output=True, text=True, timeout=15, check=False,
        ).stdout
    except (OSError, subprocess.SubprocessError):
        return 0
    procs = {}
    for line in out.splitlines():
        parts = line.split()
        if len(parts) != 3:
            continue
        try:
            pid, ppid, rss = int(parts[0]), int(parts[1]), int(parts[2])
        except ValueError:
            continue
        procs[pid] = (ppid, rss)
    if root_pid not in procs:
        return 0
    children = {}
    for pid, (ppid, _) in procs.items():
        children.setdefault(ppid, []).append(pid)
    total, stack, seen = 0, [root_pid], set()
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        entry = procs.get(pid)
        if entry is None:
            continue
        total += entry[1]
        stack.extend(children.get(pid, ()))
    return total


class Sampler(threading.Thread):
    """Polls subtree RSS for one root PID, phase-tagging every sample."""

    def __init__(self, root_pid):
        super().__init__(daemon=True)
        self.root_pid = root_pid
        self.phase = "boot"
        self.interval = DEFAULT_ANALYSIS_INTERVAL_S
        self.samples = []  # (monotonic_ts, phase, rss_kib)
        self._lock = threading.Lock()
        self._stop = threading.Event()

    def set_phase(self, phase, interval):
        with self._lock:
            self.phase = phase
            self.interval = interval

    def run(self):
        while not self._stop.wait(self.interval):
            rss = subtree_rss_kib(self.root_pid)
            with self._lock:
                self.samples.append((time.monotonic(), self.phase, rss))

    def stop(self):
        self._stop.set()
        self.join(timeout=5)

    def series(self, phase):
        with self._lock:
            return [(t, r) for (t, p, r) in self.samples if p == phase]


class DaemonSession:
    """LSP-over-stdio driver for one daemon process (reader thread model).

    A reader thread parses frames concurrently so the writer never blocks on
    a daemon that is itself blocked writing publishes into a full stdout
    pipe (the 10k-file didOpen burst is tens of MB).
    """

    def __init__(self, proc):
        self.proc = proc
        self.responses = {}
        self.published = set()
        self.eof = threading.Event()
        self._lock = threading.Lock()
        self._next_id = 1
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    def _read_loop(self):
        buf = b""
        stream = self.proc.stdout
        while True:
            chunk = stream.read(65536)
            if not chunk:
                break
            buf += chunk
            while True:
                head = buf.find(b"\r\n\r\n")
                if head < 0:
                    if len(buf) > MAX_HEADERLESS_BUFFER:
                        self.eof.set()
                        return
                    break
                length = None
                for line in buf[:head].decode("ascii", "replace").split("\r\n"):
                    if line.lower().startswith("content-length:"):
                        try:
                            length = int(line.split(":", 1)[1].strip())
                        except ValueError:
                            pass
                if length is None:
                    buf = buf[head + 4:]
                    continue
                total = head + 4 + length
                if len(buf) < total:
                    break
                self._dispatch(buf[head + 4:total])
                buf = buf[total:]
        self.eof.set()

    def _dispatch(self, body):
        try:
            message = json.loads(body.decode("utf-8", "replace"))
        except ValueError:
            return
        if isinstance(message, dict):
            if "id" in message and ("result" in message or "error" in message):
                with self._lock:
                    self.responses[message["id"]] = message
            elif message.get("method") == "textDocument/publishDiagnostics":
                uri = (message.get("params") or {}).get("uri")
                if uri:
                    with self._lock:
                        self.published.add(uri)

    def _send(self, payload):
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
        self.proc.stdin.flush()

    def request(self, method, params, timeout_s):
        with self._lock:
            msg_id = self._next_id
            self._next_id += 1
        self._send({"jsonrpc": "2.0", "id": msg_id, "method": method, "params": params})
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            if self.eof.is_set():
                return None
            with self._lock:
                reply = self.responses.pop(msg_id, None)
            if reply is not None:
                return reply
            time.sleep(0.02)
        return None

    def notify(self, method, params):
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def alive(self):
        return self.proc.poll() is None and not self.eof.is_set()

    def published_uris(self):
        with self._lock:
            return set(self.published)

    def shutdown(self, timeout_s=25.0):
        if self.proc.poll() is not None:
            return
        try:
            self.request("shutdown", {}, timeout_s=10.0)
            self.notify("exit", {})
            self.proc.stdin.close()
            self.proc.wait(timeout=15.0)
            return
        except (OSError, subprocess.SubprocessError, ValueError):
            pass
        for action in (self.proc.terminate, self.proc.kill):
            if self.proc.poll() is None:
                action()
                try:
                    self.proc.wait(timeout=5.0)
                except subprocess.SubprocessError:
                    pass


def count_trace_events(path):
    """Count the trace rows the sharing study reads.

    The ADR's "SnapshotReuse" class surfaces in the current taxonomy as
    FileRevalidated/gate=content_id_hit plus LibraryAnalyzed/from_cache=true;
    CycleReused and MemoryDegrade are the cycle-reuse and budget-ladder rows.
    """
    counts = {
        "content_id_hit": 0,
        "library_analyzed": 0,
        "library_from_cache": 0,
        "cycle_reused": 0,
        "memory_degrade": 0,
    }
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            for line in handle:
                if '"gate":"content_id_hit"' in line:
                    counts["content_id_hit"] += 1
                if '"event":"LibraryAnalyzed"' in line:
                    counts["library_analyzed"] += 1
                    if '"from_cache":true' in line:
                        counts["library_from_cache"] += 1
                if '"event":"CycleReused"' in line:
                    counts["cycle_reused"] += 1
                if '"event":"MemoryDegrade"' in line:
                    counts["memory_degrade"] += 1
    except OSError:
        return None
    return counts


def median(values):
    return statistics.median(values) if values else 0


def rss_stats(sampler):
    assembled = sampler.series("assembled")
    analysis = sampler.series("analysis")
    idle = sampler.series("idle")
    baseline = assembled[-1][1] if assembled else 0
    peak = max((r for _, r in analysis), default=0)
    cutoff = analysis[-1][0] - STEADY_WINDOW_S if analysis else 0.0
    steady = median([r for t, r in analysis if t >= cutoff])
    return {
        "assembled_baseline": baseline,
        "analysis_peak": peak,
        "analysis_steady": steady,
        "idle_after_settle": idle[-1][1] if idle else 0,
        "idle_tail_mean": round(mean([r for _, r in idle[-10:]])) if idle else 0,
        "sample_counts": {
            "assembled": len(assembled),
            "analysis": len(analysis),
            "idle": len(idle),
        },
    }


def mean(values):
    return sum(values) / len(values) if values else 0


def discover_dart_files(root):
    files = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for name in sorted(filenames):
            if name.endswith(".dart"):
                files.append(os.path.join(dirpath, name))
    return files


def run_one(args):
    copy_dir = Path(args.copy_dir).resolve()
    if not copy_dir.is_dir():
        print(f"probe_one: not a directory: {copy_dir}", file=sys.stderr)
        return 2
    if not (copy_dir / "pubspec.yaml").is_file():
        print(f"probe_one: no pubspec.yaml under {copy_dir}", file=sys.stderr)
        return 2
    if not (copy_dir / ".dart_tool" / "package_config.json").is_file():
        print(f"probe_one: no .dart_tool/package_config.json under {copy_dir}", file=sys.stderr)
        return 2

    cmd = [str(args.binary), "daemon", str(copy_dir), "--cache-size-limit", "10GiB"]
    if args.sdk:
        cmd += ["--sdk", str(args.sdk)]
    if args.trace_events:
        cmd += ["--trace-events", str(args.trace_events)]

    stderr_name = args.stderr_log or os.devnull
    started = time.monotonic()
    note = None
    valid = True
    initialize_ms = analysis_ms = None
    result_line = None
    proc = None
    session = None
    sampler = None
    try:
        with open(stderr_name, "ab") as stderr_log:
            proc = subprocess.Popen(
                cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=stderr_log,
            )
            session = DaemonSession(proc)
            sampler = Sampler(proc.pid)
            sampler.start()

            reply = session.request("initialize", {
                "processId": None,
                "rootUri": copy_dir.as_uri(),
                "capabilities": {},
            }, timeout_s=60.0)
            initialize_ms = round((time.monotonic() - started) * 1000)
            if reply is None or reply.get("error") is not None:
                valid = False
                note = "initialize handshake failed or timed out"
            else:
                session.notify("initialized", {})
                sampler.set_phase("assembled", DEFAULT_ANALYSIS_INTERVAL_S)
                time.sleep(0.5)

            files = discover_dart_files(copy_dir) if valid else []
            opened = {Path(p).as_uri() for p in files}
            missing = set(opened)
            if not files:
                valid = False
                note = "no *.dart files found under the copy"
            if valid:
                sampler.set_phase("analysis", DEFAULT_ANALYSIS_INTERVAL_S)
                analysis_started = time.monotonic()
                for path in files:
                    session.notify("textDocument/didOpen", {
                        "textDocument": {
                            "uri": Path(path).as_uri(),
                            "languageId": "dart",
                            "version": 1,
                            "text": Path(path).read_text(encoding="utf-8", errors="replace"),
                        },
                    })
                deadline = time.monotonic() + max(args.timeout, 1.0)
                while True:
                    missing = opened - session.published_uris()
                    if not missing:
                        break
                    if not session.alive():
                        valid = False
                        note = "daemon exited during the analysis pass"
                        break
                    if time.monotonic() >= deadline:
                        valid = False
                        note = (f"publish wave incomplete; {len(missing)} of "
                                f"{len(opened)} files missing")
                        break
                    time.sleep(PUBLISH_POLL_S)
                if valid:
                    analysis_ms = round((time.monotonic() - analysis_started) * 1000)

                sampler.set_phase("idle", DEFAULT_IDLE_INTERVAL_S)
                time.sleep(args.idle_seconds)

            stats = rss_stats(sampler)
            traces = count_trace_events(args.trace_events) if args.trace_events else None
            result_line = {
                "schema": SCHEMA,
                "label": args.label,
                "copy_dir": str(copy_dir),
                "binary": str(args.binary),
                "sdk_root": str(args.sdk) if args.sdk else "",
                "daemon_pid": proc.pid,
                "opened_files": len(opened),
                "phases_ms": {"initialize": initialize_ms, "analysis": analysis_ms},
                "idle_settle_s": args.idle_seconds,
                "rss_kib": {k: v for k, v in stats.items() if k != "sample_counts"},
                "sampling": {
                    "analysis_interval_ms": int(DEFAULT_ANALYSIS_INTERVAL_S * 1000),
                    "idle_interval_ms": int(DEFAULT_IDLE_INTERVAL_S * 1000),
                    "samples": stats["sample_counts"],
                },
                "traces": traces,
                "stderr_log": stderr_name,
                "valid": valid,
                "note": note,
            }
            print(json.dumps(result_line), flush=True)
    except (OSError, ValueError, BrokenPipeError) as error:
        if result_line is None:
            print(json.dumps({
                "schema": SCHEMA, "label": args.label, "copy_dir": str(copy_dir),
                "daemon_pid": 0, "valid": False, "note": f"driver error: {error}",
                "rss_kib": {}, "opened_files": 0,
            }), flush=True)
        print(f"probe_one: driver error: {error}", file=sys.stderr)
        return 3
    finally:
        if session is not None and not args.hold:
            session.shutdown()
        if sampler is not None:
            sampler.stop()
        if proc is not None and proc.poll() is None and not args.hold:
            proc.kill()

    if args.hold:
        release_deadline = time.monotonic() + args.hold_timeout
        print(f"probe_one: holding daemon pid {proc.pid} until SIGTERM/SIGINT "
              f"or --hold-timeout {args.hold_timeout:.0f}s", file=sys.stderr)
        while not args.stop_event.is_set() and time.monotonic() < release_deadline:
            time.sleep(0.2)
        session.shutdown()
        if proc.poll() is None:
            proc.kill()

    return 0 if valid else 3


def fleet_sample(args):
    stop = threading.Event()

    def on_signal(_signum, _frame):
        stop.set()

    signal.signal(signal.SIGTERM, on_signal)
    signal.signal(signal.SIGINT, on_signal)

    deadline = time.monotonic() + args.max_seconds
    with open(args.out, "a", encoding="utf-8") as out:
        while not stop.is_set() and time.monotonic() < deadline:
            pids = []
            try:
                with open(args.pids_file, "r", encoding="utf-8") as handle:
                    for line in handle:
                        line = line.strip()
                        if line == "STOP":
                            stop.set()
                            break
                        if line.isdigit():
                            pids.append(int(line))
            except OSError:
                pass
            roots = [subtree_rss_kib(pid) for pid in pids]
            out.write(json.dumps({
                "schema": FLEET_SAMPLE_SCHEMA,
                "t": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "roots": len(pids),
                "rss_kib_total": sum(roots),
                "rss_kib_per_root": roots,
            }) + "\n")
            out.flush()
            stop.wait(args.interval)
    return 0


def fleet_summarize(args):
    totals = []
    try:
        with open(args.samples, "r", encoding="utf-8") as handle:
            for line in handle:
                try:
                    totals.append((time.monotonic(), json.loads(line)["rss_kib_total"]))
                except (ValueError, KeyError):
                    continue
    except OSError as error:
        print(f"fleet-summarize: cannot read {args.samples}: {error}", file=sys.stderr)
        return 2
    if not totals:
        print(json.dumps({
            "schema": FLEET_SUMMARY_SCHEMA, "samples": 0,
            "rss_kib": {"peak_coresident": 0, "steady_last_window_median": 0},
        }))
        return 0
    end = totals[-1][0]
    tail = [r for t, r in totals if t >= end - args.tail_window]
    print(json.dumps({
        "schema": FLEET_SUMMARY_SCHEMA,
        "samples": len(totals),
        "tail_window_s": args.tail_window,
        "rss_kib": {
            "peak_coresident": max(r for _, r in totals),
            "steady_last_window_median": median(tail),
        },
    }))
    return 0


def main(argv):
    if argv and argv[0] == "fleet-sample":
        parser = argparse.ArgumentParser(prog="probe_one.py fleet-sample")
        parser.add_argument("--pids-file", required=True)
        parser.add_argument("--out", required=True)
        parser.add_argument("--interval", type=float, default=1.0)
        parser.add_argument("--max-seconds", type=float, default=14400.0)
        return fleet_sample(parser.parse_args(argv[1:]))
    if argv and argv[0] == "fleet-summarize":
        parser = argparse.ArgumentParser(prog="probe_one.py fleet-summarize")
        parser.add_argument("samples")
        parser.add_argument("--tail-window", type=float, default=10.0)
        return fleet_summarize(parser.parse_args(argv[1:]))

    parser = argparse.ArgumentParser(
        prog="probe_one.py", description="One-instance dartq daemon RSS probe.")
    parser.add_argument("copy_dir")
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--sdk", type=Path, default=None)
    parser.add_argument("--trace-events", type=Path, default=None)
    parser.add_argument("--stderr-log", default=None)
    parser.add_argument("--label", default="")
    parser.add_argument("--idle-seconds", type=float, default=DEFAULT_IDLE_SETTLE_S)
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_S)
    parser.add_argument("--hold", action="store_true",
                        help="keep the daemon alive after emitting the JSON line")
    parser.add_argument("--hold-timeout", type=float, default=DEFAULT_HOLD_TIMEOUT_S)
    args = parser.parse_args(argv)
    args.stop_event = threading.Event()

    def on_signal(_signum, _frame):
        args.stop_event.set()

    signal.signal(signal.SIGTERM, on_signal)
    signal.signal(signal.SIGINT, on_signal)
    return run_one(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
