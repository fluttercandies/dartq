# torture corpus

The complexity-axis complement to `tests/corpus/mega` (the size axis): one
multi-package Dart workspace exercising worst-case semantic shapes, pinned
against `dart analyze` (the oracle of record) by the differential harness.

The corpus is **generated, not hand-written**. Do not edit anything under
`corpus/`; regenerate instead:

```sh
python3 tests/corpus/torture/tools/gen_torture.py --force
```

## Determinism contract

The generator is a pure function of its parameters: no RNG, no clock, no
environment reads. The `--seed` value (default 0) appears only in every
generated file's header comment and in `MANIFEST.json`. Regenerating with
the same seed into a clean directory is byte-identical; the generator
refuses to write into a non-empty directory without `--force`.

## Layout

The corpus root written by the generator is `corpus/` (git-ignored, same
policy as `tests/corpus/mega/`): one root application package `torture` with
a `packages/` dependency tree, a `pubspec.yaml`, and a materialized
`.dart_tool/package_config.json` (relative rootUris, languageVersion 3.13)
exactly like the other analyzer-facing corpora. There are no hosted
dependencies, so no `pub get` is needed to analyze the tree.

Dependency shape: a deep spine (`core_types` -> `mod_provider` ->
`chain_00..07` -> `mix_00..03` -> `modc_00..01` / `extt_00..01` ->
`pat_00..02` -> `gen_00..03` -> `asyncx_00..02` -> `misc_00..02`, plus
`errprobe` off `mod_provider`), so every family head transitively depends on
everything before it. Cross-family references use `package:` imports;
within a package, relative imports.

## Families

| Package(s) | Family | Faces |
| --- | --- | --- |
| `core_types` | shared vocabulary | records, enums, sealed tokens, mixins, generic boxes, annotations |
| `mod_provider` | class modifiers | sealed hierarchy, base/final/interface/abstract-base/mixin-class in one library |
| `chain_00..chain_07` | inheritance chains | 4 chains x 256 levels; level L lives in package `chain_{L mod 8}`, so every extends edge is cross-library; shapes: plain overrides, const-ctor forwarding, interface implementation, generic `<T>` threading |
| `mix_00..mix_03` | mixins | `on` constraints, application order, generic mixins with super-call overrides, `implements` on mixins, cross-package applications of `mix_00` providers |
| `modc_00..modc_01` | cross-library modifier faces | extend base as base/final/sealed, local sealed subclass, implement interface + abstract interface, exhaustive switches over the provider's sealed hierarchy, generic holder over the base class, local base mixin on the provider base |
| `extt_00..extt_01` | extension types | int/String/List/nullable/generic representations, operators, statics, redirecting constructors, Comparable implementations |
| `pat_00..pat_02` | records + patterns | every pattern form in its legal position; refutable patterns only in switches/if-case, irrefutable-only in declarations; exhaustive and defaulted switch faces; cross-library sealed exhaustiveness over `core_types` tokens |
| `gen_00..gen_03` | generics | bounds, F-bounds, multi-parameter + self-referencing bounds, nested instantiation, generic methods/extensions, intersection bounds (`Walk & Talk`), record-typed function typedefs, covariance, generic hierarchies |
| `asyncx_00..asyncx_02` | async gaps | await-in-loop, async*/sync*/yield*, await-for (incl. record streams), try/on/finally interleavings, Completer, Future.wait, switch over awaited values |
| `misc_00..misc_02` | everything else | cascade chains (incl. null-aware), constructor/getter/generic tear-offs, legacy + modern typedefs, enhanced enums (members, mixin, generic), part/part-of pairs, conditional-import facades, flow-sensitive nullability, deferred imports, metadata, operators, callable classes |
| `errprobe` | error probes (separated) | one bounded, certain ERROR-probe per file: modifier violations outside the provider library, type errors, pattern errors, async errors, const errors. This subtree is what keeps the differential gate's oracle side non-vacuous |

Validity policy: families other than `errprobe` emit only code that is valid
Dart 3.13 by construction. The oracle is the ground truth for `errprobe`;
those files are intended to carry a small, fixed number of ERROR-severity
diagnostics each (no error storms).

## Differential gate

`scripts/torture-gate.sh` runs the standard harness differential face over
the generated corpus: ERROR severity, threshold 1.0 (full parity, no frozen
code universe — every ERROR code is judged), oracle = the fvm default
install. Serial phase only; it spawns real `dart analyze` and `dartq
analyze` processes over ~2k files.

```sh
scripts/torture-gate.sh --dry-run          # resolve paths, execute nothing
scripts/torture-gate.sh --label torture-rN # full run; report under
                                           # target/evidence/gate-torture/
```

Regenerate the corpus first if `corpus/` is missing; the gate preflights
`pubspec.yaml`, `.dart_tool/`, and the generator's presence.
