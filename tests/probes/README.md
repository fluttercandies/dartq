# Probe Packages — oracle fixtures for deferred diagnostic decisions

Two self-contained probe packages whose serial oracle runs adjudicate
deferred diagnostic decisions (setter-arity dual emission; cross-part
duplicate definitions). The fixtures and the expected-face hypotheses
below were authored without executing any tool; the `dart analyze` runs
are the measuring step and are executed later in a serial verification
phase. Each package is a read-only replay target, like the assembled
corpora: no analysis_options.yaml (absence = analyzer defaults with zero
lints enabled, so the baseline analyzer face is measured, not lint
noise), no dependencies, one face per `lib` file, `publish_to: none`.

Oracle machine rows have 8 `|`-separated fields:
`SEVERITY|TYPE|CODE|FILE|LINE|COLUMN|LENGTH|MESSAGE` (uppercase codes;
the dartdev `emitMachineFormat` wire contract the harness machine parser
follows). A run may exit non-zero when rows exist — the stdout rows are
the evidence, the exit code is not.

## B5 — setter arity (`wrong_number_of_parameters_for_setter`)

Package: `tests/probes/b5-set-multi-param/`.

Background. dartq currently carries three emit sites for this code: the
parser class-member path, the parser top-level path, and a semantic fold
fed by the collector's declaration-arity log. The semantic site predates
the parser sites and was kept "until the parser lands it, or the code
double-reports" — the parser has since landed both paths, so the open
question is the oracle's exact row count and code per offending setter:
1 row, 2 rows, which code. The `(code, file)` intersection the gates
compare collapses a double report, so the answer changes code shape, not
gate color — which is exactly why it was never measured. A prior
2026-10-12 oracle probe session measured the class-member arm as a
single live row; this probe pins that durably in-repo and extends to the
top-level arm and the named-parameter boundary.

### Shapes

| File | Source | Role |
| --- | --- | --- |
| `lib/class_setter_two_params.dart` | `class C { set s(a, b) {} }` | core face, class-member path |
| `lib/top_level_setter_two_params.dart` | `set top(a, b) {}` | core face, top-level path |
| `lib/class_setter_one_param.dart` | `class C { set ok(a) {} }` | control: valid setter, must be silent |
| `lib/setter_named_only_param.dart` | `class C { set named({a}) {} }` | boundary: 1 parameter, 0 required-positional |
| `lib/getter_with_formals.dart` | `int get g(a) => 1;` | baseline: both faces known-agreeing single row |

### Hypotheses (per core face; both core files should behave identically)

| # | Oracle face candidate | Grounding | B5 decision it implies |
| --- | --- | --- | --- |
| H-A | exactly 1 row: `ERROR\|SYNTACTIC_ERROR\|WRONG_NUMBER_OF_PARAMETERS_FOR_SETTER`, anchored on the setter name | `AstBuilder._ensureSetterFormalParameter` reports once per setter from both call sites (upstream Dart SDK source, pkg/analyzer/lib/src/fasta/ast_builder.dart:3439 top-level, :6410/:6510 class member); the diagnostic registry pins the code as SYNTACTIC_ERROR. Matches the 2026-10-12 measured class arm. | keep-parser: delete the semantic site (the collector's `ArityViolation::Setter` log and its fold), keep both parser sites, refresh the stale code-table note that still claims the parser lacks the check. |
| H-B | 2 rows, same code, distinct spans (a second, verifier-tier arm also fires) | would contradict the prior measured session; possible if the measured SDK changed | keep-both: mirror the double report; align the second dartq row's span and message to the oracle's second row. |
| H-C | 1 row but a different TYPE or severity (e.g. `COMPILE_TIME_ERROR`) | error-converter remap between tiers | keep-parser, then fix the code table's TYPE/severity to the measured face. |
| H-D | 0 rows (rule dead at the measured SDK) | unlikely given the prior session | remove all three dartq sites; re-audit the prior measurement that motivated the parser sites. |

Control expectations (must hold under every hypothesis):

- `class_setter_one_param.dart` — 0 rows on both faces. Any row = probe
  contamination or a real divergence; report it separately, do not fold
  it into the B5 verdict.
- `getter_with_formals.dart` — exactly 1 row
  `ERROR|SYNTACTIC_ERROR|GETTER_WITH_PARAMETERS` anchored at the open
  paren of the formals (upstream Dart SDK source,
  pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:1689 maps
  `GetterWithFormals` onto the analyzer code `getter_with_parameters`).
  A clean diff here validates the probe machinery end to end.
- `setter_named_only_param.dart` — under H-A the oracle reports 1 row
  (the rule reads `isRequiredPositional`, not the raw count:
  ast_builder.dart:6504-6511), while a raw-count check (`count == 1`)
  stays silent. A measured oracle row plus dartq silence is an
  independent false-negative finding: the parser check must count
  required positional parameters, not all parameters. Oracle silence
  here instead means the rule only fires on plural lists — follow the
  measured face and record the boundary.

### Serial-run command

```bash
cd tests/probes/b5-set-multi-param
# pub get only if the toolchain asks for resolution; the package has no dependencies
dart pub get
dart analyze --format=machine --no-fatal-warnings lib/
```

### Decision rule

Count rows per offending-setter file in the two core faces:

- 1 row (`H-A`) → keep-parser: single-site parity, delete the semantic
  site, fix the stale code-table note.
- 2 rows with distinct spans (`H-B`) → keep-both: align the second row.
- 2 rows with identical spans → keep-both at the `(code, file)` set
  level; document the row-count nuance, no span work needed.
- 0 rows (`H-D`) → remove all dartq sites for the code.

In every surviving oracle row the anchor must be the setter name token;
a different measured anchor means the parser anchor is fixed to match
before (or together with) deleting the semantic site.

## D1 — cross-part duplicate definitions (`duplicate_definition`)

Package: `tests/probes/d1-part-collision/`.

Background. dartq's duplicate scope is per-file: a name duplicated
across two files of one library (defining vs part, or part vs part) is
currently not detected. The engine's member-files fold exists to host
the library-level comparison; what it must emit depends on which files
the oracle reports rows on, and how many rows per collision — that row
shape is what this probe pins.

Mechanism grounding (upstream Dart SDK source,
pkg/analyzer/lib/src/error/duplicate_definition_verifier.dart,
`checkUnit` + `_checkDuplicateFragmentIdentifier`): the verifier walks
each compilation unit in library order — the defining unit first, then
parts in `part` directive order; each unit is checked against the
namespace accumulated from strictly-earlier fragments only, and a
collision reports in the CURRENT unit's file at the colliding
declaration. A top-level variable's implicit getter and implicit setter
checks share the same name token, and a reported token is marked done,
so one variable declaration collapses to at most one row. All
non-setter declaration kinds (classes, enums, mixins, extensions,
extension types, functions, type aliases, variables' implicit getters)
feed one getter namespace; setters feed a separate setter namespace (a
getter and a setter of the same name coexist by design).

Fragment order in this package: `d1.dart` -> `a.dart` -> `b.dart`.

### Shapes

| File | Contents | Role |
| --- | --- | --- |
| `lib/d1.dart` | `part 'a.dart'; part 'b.dart';` + `String libVsPart` + `int d1Unique` | defining library; first occurrence of the defining-vs-part collision |
| `lib/a.dart` | `part of 'd1.dart';` + `int libVsPart` + `int partVsPart` + `int aUnique` | second occurrence of defining-vs-part; first occurrence of part-vs-part |
| `lib/b.dart` | `part of 'd1.dart';` + `String partVsPart` + `int bUnique` | second occurrence of part-vs-part |
| `lib/within_file_dup.dart` | standalone library, `int dupName` then `String dupName` | baseline: the per-file face both faces should already agree on |

### Hypotheses (file -> rows map, per collision face)

| # | Candidate oracle rows (defining-vs-part `libVsPart`) | Candidate oracle rows (part-vs-part `partVsPart`) | Decision it implies for the engine fold |
| --- | --- | --- | --- |
| D-H1 (walk-order rule, source-grounded) | `a.dart`: 1 row at `libVsPart`; `d1.dart`: 0; `b.dart`: 0 | `b.dart`: 1 row at `partVsPart`; others 0 | fold walks member files in library order, seeds a per-library namespace from earlier files, and emits one `duplicate_definition` row per colliding declaration anchored in the file where it occurs (message "The name '{0}' is already defined.", `COMPILE_TIME_ERROR`). |
| D-H2 (defining hosts rows) | `d1.dart`: 1; parts 0 | `d1.dart`: 1; parts 0 | fold attributes every library-level collision row to the defining file regardless of where the later declaration sits. |
| D-H3 (each conflicting unit reports) | `d1.dart`: 1 AND `a.dart`: 1 | `a.dart`: 1 AND `b.dart`: 1 | fold emits one row per member file per collision (scope re-checked per unit without the earlier-only restriction). |
| D-H4 (oracle silent cross-part) | 0 rows anywhere | 0 rows anywhere | D1 is not a divergence: keep the per-file face, record cross-part silence as oracle-matching behavior, close the entry as no-op. |

The two faces together pin both the ordering rule (D-H1 vs D-H2) and
the per-unit emission count (D-H1 vs D-H3) in one run.

Controls and baseline:

- `d1Unique`, `aUnique`, `bUnique` — 0 rows anywhere, in every
  hypothesis. A row here invalidates the run.
- `within_file_dup.dart` — exactly 1 row
  `ERROR|COMPILE_TIME_ERROR|DUPLICATE_DEFINITION` at the second
  `dupName`, on both faces. This validates the probe machinery and
  guards the future fold against double-reporting the already-correct
  within-file face (one row per name per library, not one per file
  pair).

### Serial-run command

```bash
cd tests/probes/d1-part-collision
# pub get only if the toolchain asks for resolution; the package has no dependencies
dart pub get
dart analyze --format=machine --no-fatal-warnings lib/
```

### Decision rule

Map the measured rows onto the hypothesis table and apply that column's
decision. Scope note: this probe pins variable-vs-variable collisions
(one implicit getter/setter pair each side); class/function/enum
collisions share the same official namespace so the pinned file->rows
rule is expected to transfer, but kind-mixed collisions can carry
different official codes — widen the fixture only after the measured
variable-vs-variable shape is folded.

## Adjudication workflow

1. Run both serial commands above (one `dart analyze` per package, no
   parallel oracle runs).
2. Map each measured stdout row onto the hypotheses tables: core faces
   pick the B5 hypothesis; per-file counts pick the D1 hypothesis.
3. Verify the control and baseline expectations first — any violation
   invalidates that package's run (fixture or machinery bug), not the
   decision.
4. Record the measured rows into the decision tickets and apply the
   mapped decision rule from the corresponding section above.
