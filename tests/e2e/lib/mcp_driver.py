#!/usr/bin/env python3
"""mcp_driver.py — a scripted MCP stdio client for the dartq e2e suite.

Spawns `dartq mcp --dir TREE --sdk SDK`, drives the newline-delimited
JSON-RPC 2.0 transport through a scripted list of requests, correlates
responses by id (progress notifications arrive interleaved), and prints
one JSON verdict line per scripted expectation. Exits 0 when every
expectation held and the server shut down cleanly on EOF.

Usage: mcp_driver.py BIN TREE SDK SCRIPT_JSON
  SCRIPT_JSON: {"env": {...}, "requests": [
    {"id": 1, "method": "initialize", "params": {}, "expect": {"result.protocolVersion": "2025-06-18"}},
    {"notify": true, "method": "initialized"},
    {"id": 2, "method": "tools/call", "params": {...}, "expect_any": ["isError=false"]},
    ...
  ]}

Expectation grammar (each checked against the correlated response):
  "path.sub": value    — dotted lookup equals the JSON value
  "path": value        — same
  "path exists"        — the dotted path is present
  "path contains=STR"  — the deep JSON dump contains STR
Verdict lines: {"id": N, "check": "...", "pass": bool, "got": ...}.
"""

import json
import os
import re
import select
import subprocess
import sys
import time

# Per-request response deadline: a wedged server fails the leg instead of
# hanging the suite.
RESPONSE_TIMEOUT_S = 120


def lookup(value, dotted):
    # The `result.` prefix names the correlated answer itself (the wire
    # envelope is already unwrapped when expectations run).
    if dotted == "result" or dotted.startswith("result."):
        dotted = dotted[len("result."):] if dotted.startswith("result.") else ""
    if not dotted:
        return value, True
    node = value
    for part in dotted.split("."):
        if isinstance(node, list):
            try:
                node = node[int(part)]
                continue
            except (ValueError, IndexError):
                return None, False
        if not isinstance(node, dict) or part not in node:
            return None, False
        node = node[part]
    return node, True


def check_expectation(answer, spec):
    spec = spec.strip()
    if spec.endswith(" exists"):
        _, present = lookup(answer, spec[: -len(" exists")].strip())
        return present, "present" if present else "absent"
    if " contains=" in spec:
        path, needle = spec.split(" contains=", 1)
        node, present = lookup(answer, path.strip())
        dump = json.dumps(node, sort_keys=True) if present else ""
        return needle in dump, dump[:200]
    if re.match(r"^[\w.]+\s*:", spec):
        path, raw = spec.split(":", 1)
        node, present = lookup(answer, path.strip())
        try:
            want = json.loads(raw.strip())
        except json.JSONDecodeError:
            want = raw.strip()
        if not present:
            return False, "absent"
        return node == want, repr(node)[:200]
    # Otherwise: a python expression over `result` (the correlated
    # answer). The helpers ride the globals dict — generator expressions
    # open a new scope that cannot see eval locals.
    scope = {"__builtins__": {}, "len": len, "any": any, "all": all, "result": answer}
    try:
        return bool(eval(spec, scope)), "expr"
    except Exception as error:  # noqa: BLE001 - a bad expectation fails its check
        return False, f"{type(error).__name__}: {error}"


def main():
    binary, tree, sdk, script_path = sys.argv[1:5]
    script = json.load(open(script_path))

    env = dict(os.environ)
    env.update(script.get("env", {}))
    proc = subprocess.Popen(
        [binary, "mcp", tree, "--sdk", sdk],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
    )

    failures = 0
    notifications = []

    class Lines:
        """Line reader over the raw pipe fd — select() must never sit in
        front of an interpreter buffer (a buffered readline can swallow
        the next line, leaving select watching an empty fd forever)."""

        def __init__(self, fileobj):
            self.fileobj = fileobj
            self.buffer = bytearray()

        def readline(self):
            while b"\n" not in self.buffer:
                ready, _, _ = select.select([self.fileobj], [], [], 0.2)
                if not ready:
                    continue  # the caller enforces the deadline
                chunk = os.read(self.fileobj.fileno(), 65536)
                if not chunk:
                    return None
                self.buffer.extend(chunk)
            line, _, self.buffer = self.buffer.partition(b"\n")
            return line.decode("utf-8", "replace")

    lines = Lines(proc.stdout)

    def read_answers_for(ids):
        """Reads lines until every id in `ids` has a response, under a
        response deadline (a wedged server fails the leg, not the suite)."""
        pending = set(ids)
        answers = {}
        deadline = time.monotonic() + RESPONSE_TIMEOUT_S
        while pending:
            line = lines.readline()
            if line is None:
                break
            if not line.strip():
                continue
            if time.monotonic() > deadline:
                break
            message = json.loads(line)
            if "id" in message and ("result" in message or "error" in message):
                answers[message["id"]] = message
                pending.discard(message["id"])
            elif "method" in message:
                notifications.append(message)
        return answers

    for request in script["requests"]:
        if request.get("notify"):
            wire = {"jsonrpc": "2.0", "method": request["method"]}
            if "params" in request:
                wire["params"] = request["params"]
            proc.stdin.write((json.dumps(wire) + "\n").encode())
            proc.stdin.flush()
            continue

        repeat = request.get("repeat_until")
        tries = repeat["tries"] if repeat else 1
        delay = repeat.get("delay_s", 1.0) if repeat else 0.0
        answer = None
        result = None
        for _ in range(tries):
            wire = {"jsonrpc": "2.0", "method": request["method"], "id": request["id"]}
            if "params" in request:
                wire["params"] = request["params"]
            proc.stdin.write((json.dumps(wire) + "\n").encode())
            proc.stdin.flush()
            answers = read_answers_for([request["id"]])
            answer = answers.get(request["id"])
            if answer is None:
                break
            result = answer.get("result", answer.get("error"))
            if not repeat:
                break
            if all(check_expectation(result, spec)[0] for spec in repeat["when"]):
                break
            time.sleep(delay)

        if answer is None or result is None:
            print(json.dumps({"id": request["id"], "check": "response", "pass": False, "got": "no answer"}))
            failures += 1
            continue
        for spec in request.get("expect", []):
            held, got = check_expectation(result, spec)
            if not held:
                failures += 1
            print(json.dumps({"id": request["id"], "check": spec, "pass": held, "got": got}))

    # Clean shutdown: close stdin, the server exits 0 on EOF.
    proc.stdin.close()
    try:
        code = proc.wait(timeout=30)
    except subprocess.TimeoutExpired:
        proc.kill()
        code = -9
    stderr = proc.stderr.read().decode("utf-8", "replace")
    if code != 0:
        failures += 1
    print(json.dumps({"id": "shutdown", "check": "exit=0", "pass": code == 0, "got": f"rc={code} stderr={stderr[:200]}"}))

    # Progress notifications observed (the progressToken bookends).
    progress = [n for n in notifications if n.get("method") == "notifications/progress"]
    print(json.dumps({"id": "progress", "check": "notifications seen", "pass": len(progress), "got": len(progress)}))

    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
