# Incremental edit-sequence corpus

Curated L2 edit sequences for the incremental-consistency gate (every
incremental step must match a fresh full recompute byte-wise; ticket #1082,
serving the frozen #147 verification order). One directory
per sequence:

- a minimal pure-Dart package (`pubspec.yaml` + `lib/**.dart`, no external
  dependencies), and
- `edits.jsonl` — the edit sequence the harness replay driver applies.

Sequence kinds, per the incremental axes (splices plus the
file delete/create axes):

- `comment-only` — text changes inside a comment; signature-invariant.
- `signature-rename` — renames a function and a parameter at definition and
  call sites; flips the API-signature face.
- `export-combinator-flip` — adds/removes `show` combinators on an
  `export`; flips the export face and changes the diagnostic set.
- `mixed-roundtrip` — ten steps interleaving the kinds above across three
  files, ending back at the original text (round-trip stability).
- `delete-create` — deletes a file another file imports (the importer
  churns unresolved-reference diagnostics), re-creates it with evolved
  content, then splices inside it and at the importer's call site; proves
  the file-set and splice axes interleave. Ends in an evolved state by
  design (every other sequence ends byte-identical to its original).

## Base-corpus sequences

The directories whose name starts with `sig-` / `struct-` / `chain-` /
`recovery-` replay against the real corpora instead of a mini package: a
one-line `base` file names the repo-relative workspace the sequence edits
(`tests/corpus/complex`, or the generated `tests/corpus/torture/corpus` —
regenerate it with the torture generator if absent). `edits.jsonl` paths
are relative to that base. The replay driver copies the corpus before any
edit lands, so the base tree is never written; the gate preflights the
base's `pubspec.yaml` and `.dart_tool/package_config.json` instead of
materializing them. Every base-corpus sequence ends byte-identical to its
starting tree, and every one carries diagnostic-bearing steps (definition-
first renames leave importers unresolved mid-sequence), so none can pass
vacuously.

Kinds, one directory prefix each:

- `sig-*` (signature) — renames or arity changes at a definition site,
  with the importer/override/call-site fix steps as explicit sequence
  steps: `sig-money-display` (4 files across the pricing/reporting/event
  string surfaces), `sig-sku-shortform` (8 call sites in 4 files, half
  fixed then reversed), `sig-restock-evaluate` (cross-package interface
  rename stock_policy -> corpus_complex plus a barrel hide/show flip),
  `sig-chain-describe-arities` (one torture chain root, 256 overrides
  across 8 packages, optional- then required-parameter waves).
- `struct-*` (structure) — member and class add/remove over real
  hierarchies: `struct-product-sealed-cycle` (new sealed subclass breaks
  the part file's exhaustive switch; object-pattern member churn),
  `struct-catalog-extract-tables` (const tables extracted into a new file
  with barrel export and import churn), `struct-picker-abstract-member`
  (abstract getter removal breaks an override, a super call, and a
  downstream loop in one file), `struct-chaind-member-arities` (field +
  method removal and a payload arity change across the chain_d pair).
- `chain-*` (cross-file chained invalidation) — export/import/part faces:
  `chain-deep-graph-export` (deep_mid show/hide flip + a deep getter
  rename rippling down the import graph), `chain-deferred-headline`
  (renames through a `deferred as` namespace, signature and parameters),
  `chain-torture-wire-anchors` (wire.dart import/anchor churn passing
  through unused-import warnings and undefined-class errors),
  `chain-misc-part-detach` (part declaration detached twice on a
  generated m007/part_impl_7 pair).
- `recovery-*` (error recovery) — broken->fixed->broken round trips:
  `recovery-ledger-members` (3->2->1->0->1->2->3 diagnostics),
  `recovery-dock-duplicate` (duplicate_definition resolved then
  re-formed), `recovery-missing-uri-relink` (both missing URIs created,
  wired, broken, and removed again), `recovery-errprobe-returns`
  (torture errprobe fixtures: return-type pair plus a sealed-switch
  exhaustiveness climb).
- `mega-x-generated` — replays a compact sequence over the generated
  `tests/corpus/mega-x/1k` tree (corpus-gen `--scale 1k --seed 0`):
  comment-noise splice pair on `lib/main.dart`, a remove/create
  round-trip on an imported facet stub, and a class-rename wave across
  two links of the core inheritance chain. The base is generated output,
  so the gate's materialization preflight needs the tree regenerated and
  its `.dart_tool/package_config.json` materialized first
  (`scripts/mega-x-run.sh` does both on its way to leg X4). Every step
  pair is self-inverting, so the sequence ends byte-identical to its
  starting tree.

## Edits format

JSONL, one step per line, blank lines skipped. Three step forms; the
parser lives in the differential harness
(`tests/harness/src/replay_edit.rs`, `parse_edits`):

In-file splice:

```json
{"file":"lib/main.dart","line":3,"column":8,"length":2,"new_text":"x"}
```

- `file` — corpus-relative path with plain `/`-separated segments.
- `line`/`column` — 1-based; `column` in UTF-16 code units.
- `length` — UTF-16 code units to replace (`0` = pure insertion).
- `new_text` — replacement text (`""` = deletion).

File delete:

```json
{"remove":"lib/legacy.dart"}
```

- `remove` — the corpus-relative file to delete; the driver errors if it
  is neither in the package nor created by an earlier step.

File create:

```json
{"create":"lib/legacy.dart","text":"const int legacyValue = 2;\n"}
```

- `create` — the corpus-relative file to create; the driver errors if it
  already exists.
- `text` — the new file's full content (an empty string is a valid empty
  file; JSON strings are UTF-8 by construction).

The three forms interleave freely within one sequence. Splice steps apply
sequentially to the evolving text: each step's coordinates address the
file state after the previous step. Every sequence here is pure ASCII, so
UTF-16 columns equal character columns. All sequences except
`delete-create` (which ends evolved by design) end with every file
byte-identical to its original text.

## Consumption

`scripts/incremental-gate.sh` enumerates the sequence directories and runs
`harness replay` over each: one live `dartq daemon` session applies the
sequence through overlay changes and file-set notifications (the
incremental face) while the same workspace — the materialized disk copy
the daemon itself runs on — is re-analyzed fresh per step (the
full-recompute face); every step must compare byte-equal after canonical
normalization.
The gate materializes a run-time copy of each self-contained package and
writes the pub-generated files there (`pubspec.lock`,
`.dart_tool/package_config.json`), so this directory stays
free of machine-specific artifacts. Base-corpus sequences skip that step
and point the replay at the named workspace, which must carry its own
analyzer inputs.
