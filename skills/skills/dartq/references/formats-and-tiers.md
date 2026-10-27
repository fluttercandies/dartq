# Formats and Verbosity Tiers

## Face Selection

Every output-bearing verb carries one `--format <lon|json|text>` axis whose
default is the constant `lon` — never the terminal's shape, so a piped run
and a TTY run print the same bytes. An invalid spelling is the usage error
(exit 64).

| Verb | `lon` (default) | `json` | `text` |
| --- | --- | --- | --- |
| `analyze` | `{v diags:[…]}` document | oracle envelope (byte parity with `dart analyze --format=json`) | grouped human listing |
| `query` (all faces) | the vocabulary answer, one line per row set | frozen v1 envelope | indented projection |
| `explain` | the vocabulary report | frozen v1 report | indented projection |
| `impact` / `compat` | the vocabulary report | frozen v1 report | indented projection |
| `rename` | `{v … new_name edits:[{file off len}]}` | frozen document | indented projection |
| `fix` / `assists` | the vocabulary patch documents, one line per file | frozen documents | the human listing |
| `packages` | `{v root cfg? pkgs bounds}` | one JSON object per line | aligned table + hints |
| `resolve` / `complete` | the vocabulary answer envelope | the daemon envelope | indented projection |
| `session` / `events` | one vocabulary row per line | one JSON row per line | identity lines / indented rows |
| `debug` | the daemon answer projected, one line | the answer verbatim | indented `status`; other faces ride lon |

Diagnostic lines go to stdout only; usage and internal failures report on
stderr and keep stdout empty.

## Exit Codes

| Code | Meaning | Faces |
| --- | --- | --- |
| 0 | success — includes analyze clean/info-only, zero-result query answers, a rename position miss (`target:null`, empty edit set) | all |
| 1 | analyze fatal infos (`--fatal-infos`); a failed test run; a debug domain failure (one `{"error":{code,msg}}` line) | `analyze`, `test`, `debug` |
| 2 | analyze fatal warnings (fatal by default; `--no-fatal-warnings` clears) | `analyze` |
| 3 | analyze errors present | `analyze` |
| 4 | internal failure — a caught panic, an analysis error, or the debug proxy never reaching the engine (no resident session, transport) | one-shot faces, `debug` |
| 64 | usage — unknown flag or subcommand, invalid values, retired spellings (`--format machine`, `--format default`), rename edits outside the workspace | all |
| 65 | the runner's data-error code, passed through | `test` |
| 70 | internal defect reported through a protocol channel — the `mcp` stdio server's transport failure (the resident session server exits with it on its own failures) | `mcp`, resident server |
| 79 | no tests found (the runner's own code, passed through) | `test` |

## The lon Face and the Field Vocabulary

`lon` is canonical [Language Object Notation v1] — the JSON data model with
record tables (`{users:[id name;1 Ada;2 Bob]}`), quote-free safe strings,
and no commas between bare array items. The lon face prints the answer's
fields through the short snake_case vocabulary (the same word list the
daemon's `dartq/*` answers and the MCP content payloads carry):

`v` (schema_version) · `root` · `diags` · `sev` · `msg` · `doc` · `sym` ·
`decls` · `refs` · `deps` · `off` · `len` · `col` · `sig` · `target` ·
`files` · `tests` · `envs` · `lines` · `sig_changed` · `bases` · `ifaces` ·
`ctors` · `new_name` · `age_s` · `stale_binary` · `text` · `src` · `role` ·
`before` · `after` · `ts` · `decl_kind` · `pkgs` · `bounds` · `cfg` ·
`isolates` · `main` · `bps` · `session_id`

The json face keeps the long frozen spellings (`schema_version`,
`declarations`, `references`, …) — jq consumers and the schema contracts
are untouched.

[Language Object Notation v1]: https://github.com/fluttercandies/lon/blob/main/spec/lon-v1.md

## analyze: the Three Faces

- `lon` (default): `{v:1 root? diags:[{code sev file line col len msg}]}` —
  one document, one flat row per diagnostic (`sev` is `error`/`warning`/
  `info`). `root` (the absolute analysis root) rides only when rows exist;
  `file` is root-relative inside it, absolute outside (SDK, pub cache);
  the row carries no `doc` — `explain` and the json face serve the URL.
  Workspace advisories ride stderr as `Info • code • message` lines.
- `json`: one single-line envelope — byte parity with
  `dart analyze --format=json` (`version` key, absolute paths; every
  first-party lint row carries `documentation`:
  `https://dart.dev/diagnostics/<name>` when published, else
  `https://dart.dev/lints/<name>`; advisories ride stderr).
- `text`: a leading `root: <abs>` line when rows exist, then diagnostics
  grouped by file under located headings, `sev code line:col msg` rows,
  the count tail (`No issues found!` / `N issues found.`).

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
| `--metrics-json PATH` | one metrics JSON object at exit — the phase keys `discovery_ms`, `sdk_ms`, `assembly_ms`, `analyze_ms` (strictly the engine pass), `lints_ms`, `render_ms` beside `total_ms`: always present, integer ms (0 when the face lacks the phase), non-overlapping sub-intervals whose sum stays ≤ `total_ms` |
| `--trace-events PATH` | JSONL trace sink, one object per engine event |
| `--no-fatal-warnings` / `--fatal-infos` | oracle's fatal-severity toggles |
| `--enable-experiment NAME[,NAME...]` | language experiments (dartdev spelling) |
| `--no-plugins` | skip `analysis_options.yaml` plugin loading |

## Query Row Shapes

The lon answer is `{v sym decls refs}` — decl rows
`{file kind name container? line col len sig?}`, ref rows
`{file kind off len target qualified?}` (`implementations` / `dependencies`
carry their own arrays, which never print at `minimal`).

`resolve`/`complete` answer a null `hit` / an empty candidate set on an
uncovered position (a non-identifier, an unresolved use, a past-end offset)
— never an error.

File spellings: `query`/`explain`/`impact`/`compat` spell every file
column through the same locator the analyze/check faces use —
root-relative inside the analysis root, absolute outside it (SDK,
pub cache, another package's checkout) — and carry the envelope's
`root:<absolute>` once whenever the answer has file fields. No `package:`
URIs on these faces (the `index --full` export's frozen grammar is the
one exception).
