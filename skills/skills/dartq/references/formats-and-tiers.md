# Formats and Verbosity Tiers

## Face Selection

Every output-bearing verb resolves `--format` from the terminal shape when
the flag is absent. An explicit flag always wins — a piped
`--format machine` stays machine; deterministic pipelines never flip.

| Verb | TTY face | Piped face (agent default) | `--format` values |
| --- | --- | --- | --- |
| `analyze` | human groups | JSON envelope | `machine`, `default`, `json` |
| `query` (all faces) | machine rows | JSON lines | `machine`, `json` |
| `explain` | JSON (report's only format) | JSON | `json` |
| `impact` | tier-invariant report | same | (none — shape is fixed) |
| `rename` | machine edit set | same | (none) |
| `fix` | human listing | JSON documents | `default`, `machine` |
| `packages` | aligned table | `PKG`/`BOUNDARY` rows | `human`, `machine`, `json` |
| `resolve` / `complete` | machine rows | JSON | `machine`, `json` |
| `session` / `events` | JSON lines | JSON lines | (none) |

Diagnostic lines go to stdout only; usage and internal failures report on
stderr and keep stdout empty.

## analyze: the Machine Row

`--format machine` prints one row per diagnostic, the byte-parity
differential face:

```
SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH|MESSAGE
```

`--format json` prints one single-line envelope — byte parity with
`dart analyze --format=json` (`version` key; advisories ride stderr).

## analyze: Row Verbosity (`-v`)

Affects the machine face only; `default`/`json` faces are unchanged.

| Tier | Row shape |
| --- | --- |
| `concise` | severity/type/code + span prefix; the message field drops |
| `default` | the oracle machine row verbatim |
| `verbose` | the row plus `correctionMessage` as a ninth field (`-` when absent) |

## Report Verbosity (`-v`) on query / explain / impact / compat

| Tier | Meaning |
| --- | --- |
| `minimal` | red-line fields only: locators and verdicts stay, prose and convenience spellings drop |
| `compact` | the default output shape |
| `full` | currently identical to `compact` (no full-only fields exist yet) |

`impact` is tier-invariant: every field is a locator or a verdict, so all
tiers emit the same bytes.

## Useful Engine Flags (analyze)

| Flag | Effect |
| --- | --- |
| `--fast` | scan + parse only, no engine |
| `--overlay PATH=FILE` | analyze PATH with FILE's bytes (`-` = stdin), repeatable |
| `--memory-limit MiB` | RSS budget; over budget the engine degrades tier-by-tier, each step an INFO `dartq/memory_degraded` row — the diagnostic set stays complete |
| `--cache-size-limit SIZE` | hard cap for the persistent L3 content store (`~/.cache/dartq/`); `512MiB`, `10GiB`, bare bytes; default 10 GiB |
| `--metrics-json PATH` | one metrics JSON object at exit |
| `--trace-events PATH` | JSONL trace sink, one object per engine event |
| `--no-fatal-warnings` / `--fatal-infos` | oracle's fatal-severity toggles |
| `--enable-experiment NAME[,NAME...]` | language experiments (dartdev spelling) |
| `--no-plugins` | skip `analysis_options.yaml` plugin loading |

## Query Row Grammar (machine)

- `DECL|file|container|kind|name|line|col|length` — a declaration row
- `REF|...` — a resolved reference row (kinded on `references`/`callers`)
- `implementations` / `dependencies` carry their own reserved row families
  (IMPL/DEP), which never print at `minimal`

`resolve`/`complete` answer `MISS` / an empty candidate set on an uncovered
position (a non-identifier, an unresolved use, a past-end offset) — never
an error.
