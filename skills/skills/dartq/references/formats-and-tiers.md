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

## The lon Face and the Field Vocabulary

`lon` is canonical [Language Object Notation v1] — the JSON data model with
record tables (`{users:[id name;1 Ada;2 Bob]}`), quote-free safe strings,
and no commas between bare array items. The lon face prints the answer's
fields through the short snake_case vocabulary (the same word list the
daemon's `dartq/*` answers and the MCP content payloads carry):

`v` (schema_version) · `diags` · `sev` · `msg` · `doc` · `sym` · `decls` ·
`refs` · `deps` · `off` · `len` · `col` · `sig` · `target` · `files` ·
`tests` · `envs` · `lines` · `sig_changed` · `bases` · `ifaces` · `ctors` ·
`new_name` · `age_s` · `stale_binary` · `text` · `src` · `before` ·
`after` · `ts` · `decl_kind` · `pkgs` · `bounds` · `cfg` · `isolates` ·
`main` · `bps` · `session_id`

The json face keeps the long frozen spellings (`schema_version`,
`declarations`, `references`, …) — jq consumers and the schema contracts
are untouched.

[Language Object Notation v1]: https://github.com/fluttercandies/lon/blob/main/spec/lon-v1.md

## analyze: the Three Faces

- `lon` (default): `{v:1 diags:[{code sev file line col len msg doc?}]}` —
  one document, one flat row per diagnostic (`sev` is `ERROR`/`WARNING`/
  `INFO`, `doc` the published docs URL when the registry marks one);
  workspace advisories ride stderr as `Info • code • message` lines.
- `json`: one single-line envelope — byte parity with
  `dart analyze --format=json` (`version` key; advisories ride stderr).
- `text`: diagnostics grouped by file, `sev code line:col msg` rows, the
  count tail (`No issues found!` / `N issues found.`).

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

## Query Row Shapes

The lon answer is `{v sym decls refs}` — decl rows
`{file kind name container? line col len sig?}`, ref rows
`{file kind off len target qualified?}` (`implementations` / `dependencies`
carry their own arrays, which never print at `minimal`).

`resolve`/`complete` answer a null `hit` / an empty candidate set on an
uncovered position (a non-identifier, an unresolved use, a past-end offset)
— never an error.
