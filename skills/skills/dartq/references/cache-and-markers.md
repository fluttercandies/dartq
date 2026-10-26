# Test Cache and Drift Markers

## The Cache Key

A suite replays when the key of an earlier green run matches. The key covers:

- the suite's **import closure** (file bytes),
- **run-time inputs**: absolute paths spelled in literals (their bytes or
  their absence), relative path literals, the environment variables the file
  reads by literal key (only those values enter the key),
- **resolution** (`.dart_tool/package_config.json`),
- **toolchain** (dartq binary, Dart SDK, and for Flutter the SDK/engine
  stamps),
- **runner arguments** and the **environment**.

The store is per user; every worktree of the same content shares it.
Failures are never stored.

## Why a Suite Refuses to Replay

A suite is never cached when its closure:

- reads the **wall clock** (`DateTime.now`, `DateTime.timestamp`) — dates
  move, an expiry green today can be red tomorrow;
- touches the **network** (`Socket`, `HttpClient`, `WebSocket`, ...) or
  **child processes** (`Process.run`/`runSync`/`start`) or **stdin**;
- reads the host's identity (`Platform.localHostname`) or native code
  (`dart:ffi`);
- imports a **drift package** whose API is a drifting source: `clock`,
  `dio`, `grpc`, `http`, `http2`, `io`, `process`, `process_run`, `shelf`,
  `web_socket`, `web_socket_channel`;
- reads the environment through a shape the extractor cannot name —
  iterating `Platform.environment`, computed keys, aliased maps.

Not drift (replayable): `Random`, `Stopwatch` durations, `pid` — a replayed
outcome is one sample of the same distribution a fresh run draws.

A suite that observes how it was launched (`Platform.script`,
`Platform.executableArguments`) is always run under the stock loader — it
would see the difference between the stock bootstrap and the shared group.

A suite deriving absolute locations (`.absolute`,
`resolveSymbolicLinks`, `package:path`'s `absolute`/`canonicalize`,
`StackTrace.current`) is keyed on its checkout's absolute root — so it
replays within one checkout but not across worktrees, unless attested
portable (below).

## The Markers (exact spelling)

All three are `//` line comments anywhere in the file. The spelling is
literal: `dartq: no-cache` — a colon, one space, the word.
`dartq:no-cache` (no space) does **not** match; `dartq: no-cacheable`
(a longer word) does **not** match; extra leading slashes (`///`) and a
trailing reason do match.

### `// dartq: no-cache`

The suite reaching this file always runs. The honest opt-out for suites the
key cannot bound.

### `// dartq: deterministic`

The author vouches that this **file's own** drifting reads never reach a
test verdict (a log timestamp, an analyzer modification stamp). Drift in
imported files still blocks the cache — only this file's reads are vouched
for. Do not add this to silence an unknown: a wrong attestation replays a
stale verdict.

### `// dartq: portable`

The absolute locations this file derives reach no verdict except relative
to the package root — the outcome is the same in every checkout of the same
content. This un-keys the checkout root, letting a location-deriving suite
replay across worktrees.

## Deciding Which Marker

| Symptom | Marker |
| --- | --- |
| Test hits network/clock and its verdict depends on it | none — let it run; or mock the source |
| Clock read only feeds a log line, never a verdict | `// dartq: deterministic` |
| Absolute path only locates fixture files under the package root | `// dartq: portable` |
| Cannot bound what the suite reads | `// dartq: no-cache` |

## Host-Wide Memory Admission

Every dartq test run leases its memory from one host-wide pool shared by
all dartq processes of the user — parallel runs across worktrees throttle
together instead of swapping the machine. A runner silent for 30 minutes
(default; `DARTQ_TEST_IDLE_TIMEOUT` in seconds overrides) is killed; a
runner that keeps reporting is never cut short.
