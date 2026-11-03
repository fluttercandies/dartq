# F9 split-design — `class_modifier_transitivity`

**Scope of this recon:** read-only. No `crates/**` edits, no cargo, no oracle /
conformance runs. Evidence is (a) the repo's Rust sources at `dc681488`,
(b) `conformance/expected.tsv` (oracle output, committed),
(c) the scratch report (row-level diff, snapshot 2026-10-30 20:14),
(d) the pinned Dart SDK 3.13.7 (the repo's pinned oracle source, `pin.toml
sdk_version = "3.13.7"`, revision `416e33b0`).

Decisions taken autonomously in this recon (no user questions, per brief):
- Read the **3.13.7** SDK tree, not upstream Dart SDK source (main). `the project instructions`
  "版本钉死链" requires the reference source and the oracle to be the same pin;
  the analyzer sources below are line-cited against `dart-sdk-3.13.7`.
- Classified the F9 rows against a **stale** snapshot rather than re-running
  the sweep (the brief forbids oracle runs). §1 states exactly how stale and
  what that does to the yield estimate.
- Treated `base_transitivity` extras as "already closed by #3161" only after
  reconstructing the pre-#3161 code path from `git show 7ff55168`; the proof is
  in §4.A, not assumed.

---

## 0. Headline

**The transitivity rules themselves are already implemented, in full, and are
unit-pinned.** F9 is not a missing-feature family. What was missing were
*emit-arm defects* in the wiring, and the bulk of those landed in `#3161`
(commit `7ff55168`, 2026-10-31 23:35) — **after** the wave-B snapshot that
produced the "538 missing + ~200 extra" numbers.

What is actually left is three narrow, independent sub-seams, none of which is
"implement transitivity":

| # | Sub-seam | Side | Evidence-backed rows (stale snapshot) |
|---|---|---|---:|
| **S1** | clause-error **drive gate** (`_checkClassInheritance`) not modeled | **extra** | ~138 |
| **S2** | modifier facts captured in a **pre-feature library** | **extra** | 71 (co19 `versioning_A02_*`) |
| **S3** | residual missing side after #3161 | **missing** | ≤ 827 stale, ~40 projected live |

S1 and S2 are pure *suppression* work (delete rows). S3 is the only place that
could still be a real logic gap, and it needs one cheap probe before dispatch
(§6).

---

## 1. Why the F9 row counts must be re-censused before dispatch

| fact | value |
|---|---|
| wave-B snapshot mtime | `2026-10-30 20:14` (the scratch report) |
| `#3161` land time | `2026-10-31 23:35` (`7ff55168`) |
| delta | ~27 h — the snapshot predates the fix |

`#3161`'s own commit message records:

```
Before: 285 MISSING / 46 EXTRA / 46 WRONG-SPAN (status.tsv 282/94/188f/280 drift).
...
Residuals: clause-error gate not modeled (...); ~40 non-alias MISSING projected
```

Two consequences:

1. The "538 missing" headline is a **pre-#3161** number and includes the
   alias-rooted resolution defect (A1), the enum drive (A3), the class/mixin
   drive split (A4), the `with`-entry span defect (A5) and the Pass-5
   mis-shaped base row (A6) — all five fixed.
2. The extra side is **not** `~200` uniform rows: it splits cleanly into
   "already fixed by A6" and "still live" (§4).

**Re-census cost:** one `conformance sweep` + the F9 code filter (the same
one-liner used here). Must run before the ticket is sized. Everything below is
sized from the stale data and marked as an upper bound.

Current F9 row census (stale, both codes, from `wave_b_rows.tsv`):

```
missing: 827  = 538 invalid_use_of_type_outside_library + 289 subtype_of_base_or_final_...
extra  : 231  = 185 invalid_use_of_type_outside_library +  46 subtype_of_base_or_final_...

by corpus:
  761 missing / 80 extra   co19 LanguageFeatures/Class-modifiers/*
   66 missing / 151 extra  everything else (diagnostics/*, features/_core/*, parser/src/*)
```

Note the raw split is `827 / 231`, **not** `538 / ~200`: the wave-B report
quoted only the `invalid_use` code in its F9 line and dropped the sibling
family's 289 missing / 46 extra. Any ticket sized off "538" undercounts.

---

## 2. Where the check lives today (implemented, not missing)

Two layers, both present:

**Decision layer — `crates/dartq-resolution/src/modifiers.rs` (1,996 lines, cold, pure).**
Pure decision functions over resolved facts, no I/O, no element model:
`declaration_violations`, `sealed_supertype_violation`,
`final_extends_violation`, `final_implements_candidate`,
`final_mixin_constraint_violation`, `interface_extends_violation`,
`base_implemented_candidate`, `subtype_restriction_violation`,
`nearest_explicit_base_or_final`, `induced_modifier`,
`may_ignore_class_modifiers`, and the `ModifierCode` registry table. The module
doc (:1-88) already cites the upstream decision order line-by-line.

**Wiring layer — `crates/dartq-semantic/src/diagnose/overrides.rs`.**
`check_modifier_use_sites` (:3009) → `plan_modifier_use_sites` (:3195) runs five
passes, one per upstream pass, in upstream source order:

| pass | upstream | dartq | emits |
|---|---|---|---|
| 1 | `_checkForBaseClassOrMixinImplementedOutsideOfLibrary` | `implements` closure walk | `invalid_use_of_type_outside_library` |
| 2 | `_checkForInterfaceClassOrMixinSuperclassOutsideOfLibrary` | extends head | `invalid_use_of_type_outside_library` |
| 3 | `_checkForFinalSupertypeOutsideOfLibrary` | extends → implements walk → `on` | `invalid_use_of_type_outside_library` |
| 4 | `_checkForSealedSupertypeOutsideOfLibrary` | extends ∪ with ∪ implements (class drive) / implements ∪ on (mixin drive) | `invalid_use_of_type_outside_library` |
| 5 | `BaseOrFinalTypeVerifier.checkElement` | extends → implements → with → on, one row per element | `subtype_of_base_or_final_is_not_base_final_or_sealed` (+ the implements-anchored base pair, owned by pass 1) |

Supporting machinery already in place: `resolve_super` (alias-transparent,
#3161 A1), the enum class-drive (#3161 A3/A4), `header_with_entry_spans`
(#3161 A5), `reported_modifier_checks` single-shot latch,
`library_facts` / `LibraryClassModifiers` / `build_class_modifiers`
(`crates/dartq-engine/src/semdiag.rs:553`) for the per-library
`class-modifiers` posture, `FoldHierarchy` (`diagnose/types.rs:5387`) for
effective modifiers incl. sealed-induced flags, and `emit_modifier`
(`diagnose/engine_core.rs:1938`).

Pins already exist: `crates/dartq-semantic/src/diagnose/ticket_3161_tests.rs`
(13 tests, incl. the co19 `*Sealed*Implement` shape),
`crates/dartq-engine/tests/modifier_restrictions.rs`, and
`diagnose/tests.rs:3027-3050`.

**Conclusion: do not dispatch "implement transitivity".** Dispatch suppression
work (S1/S2) plus one probe-then-fix (S3).

---

## 3. The upstream decision order (verbatim, 3.13.7)

Both families are gated differently — **this is the whole of S1**.

### 3.1 The outside-library family (`invalid_use_of_type_outside_library`)

`error_verifier.dart:2327-2370` — the whole block runs only when **all four**
clause-error probes are false:

```dart
if (!_checkForExtendsDisallowedClass(superclass) &&
    !_checkForImplementsClauseErrorCodes(implementsClause) &&
    !_checkForAllMixinErrorCodes(withClause) &&
    !_checkForNoGenerativeConstructorsInSuperclass(superclass)) {
  ...
  _checkForBaseClassOrMixinImplementedOutsideOfLibrary(implementsClause);
  _checkForInterfaceClassOrMixinSuperclassOutsideOfLibrary(superclass, withClause);
  _checkForFinalSupertypeOutsideOfLibrary(superclass, withClause, implementsClause, null);
  _checkForSealedSupertypeOutsideOfLibrary([?superclass, ...?withClause?.mixinTypes,
                                            ...?implementsClause?.interfaces]);
  return true;
}
return false;
```

Mixin drive is a **different, narrower** gate — `error_verifier.dart:7808-7845`:

```dart
if (!_checkForOnClauseErrorCodes(onClause) &&
    !_checkForImplementsClauseErrorCodes(implementsClause)) {
  _checkForBaseClassOrMixinImplementedOutsideOfLibrary(implementsClause);
  _checkForFinalSupertypeOutsideOfLibrary(null, null, implementsClause, onClause);
  _checkForSealedSupertypeOutsideOfLibrary([...?implementsClause?.interfaces,
                                            ...?onClause?.superclassConstraints]);
}
```

Enum rides the class drive with a null superclass
(`visitEnumDeclaration` → `_checkClassInheritance`, `:823-863`).

### 3.2 The subtype-modifier family (`subtype_of_base_or_final_...`)

`BaseOrFinalTypeVerifier` is a **separate verifier**, driven from the resolver
(`resolver.dart:2370` class, `:2384` class type alias / named mixin
application, `:3665` mixin) — **not** inside `_checkClassInheritance`, therefore
**not** subject to the §3.1 gates. That asymmetry is a hard constraint on the
fix: the gate must suppress passes 1–4 and must leave pass 5 alone.

`base_or_final_type_verifier.dart` decision order:

```
checkElement(element, implementsClause):           // :40-74, early-return per family
  supertype        -> _checkSupertypes              (return on first report)
  implementsClause -> _checkInterfaceSupertypes
  element.mixins   -> _checkSupertypes
  element is Mixin -> element.superclassConstraints -> _checkSupertypes

_reportRestrictionError(element, superElement, {implementsNamedType}):   // :178-315
  1. element.library != _definingLibrary            -> return false          (:185-187)
  2. trigger := superElement.isBase || superElement.isFinal
                || (!superElement.library.class_modifiers
                    && element.library.class_modifiers)                       (:189-202)
     if !trigger -> return false
  3. ancestor := _getExplicitlyBaseOrFinalElement(superElement)               (:198, :107-134)
     if ancestor == null -> return false                                      (:204-206)
  4. if _mayIgnoreClassModifiers(ancestor.library) -> return false            (:208-210)
  5. if implementsNamedType != null && superElement.isSealed
        && ancestor.library != element.library && ancestor.isBase:
          baseClassImplementedOutsideOfLibrary / baseMixinImplementedOutsideOfLibrary
          (argument = ANCESTOR's name, anchor = the written implements entry)
          -> return true                                                      (:240-256)
  6. if element.isBase || element.isFinal || element.isSealed -> return false (:258)
     if ancestor.isFinal:
        cross-library ancestor && (superElement.library.class_modifiers
                                   || !ancestor.library.isInSdk
                                   || implementsNamedType != null) -> return false  (:262-279)
        report subtypeOfFinalIsNotBaseFinalOrSealed / mixinSubtypeOfFinalIsNotBase   (:280-294)
     else if ancestor.isBase:
        report subtypeOfBaseIsNotBaseFinalOrSealed  / mixinSubtypeOfBaseIsNotBase    (:295-311)
```

`_getExplicitlyBaseOrFinalElement` (`:107-134`) = preorder walk, **element
itself first**, then supertype → interfaces → mixins → superclassConstraints,
qualifying on `(isBase || isFinal) && !isSealed`. Induced-only carriers do not
qualify. `nearest_explicit_base_or_final` in `modifiers.rs:747` is a faithful
port.

`_mayIgnoreClassModifiers` has **two** upstream copies and dartq keeps both
(`modifiers.rs:223` with the `dart:ffi` carve-out, `:250` without);
`base_or_final_type_verifier.dart:164-173` is the no-carve-out one.

### 3.3 The legality matrix (what a planned fix must agree with)

| super declaration | same library | other library |
|---|---|---|
| `class` (none) | extends/implements/with/on all legal | all legal |
| `base class` / `base mixin` | extends/implements/with legal **iff** the subtype is `base`/`final`/`sealed`; `on` legal | **implements** illegal (`base_class_implemented_outside_of_library` / `base_mixin_...`); extends/with legal (a `base` super may be extended from anywhere — no type-family rule) |
| `interface class` | nothing special | `extends` illegal (`interface_class_extended_outside_of_library`); implements/with legal |
| `final class` | subtype must be `base`/`final`/`sealed` | `extends` illegal, `implements` illegal (with the indirect-final skip, §3.1 pass 3), `on` illegal; subtype row suppressed (carve-out, §3.2 step 6) |
| `sealed class` | subtype legal (subclassable only here) | extends/with/implements/on **all** illegal (`sealed_class_subtype_outside_of_library`), no `_mayIgnoreClassModifiers` gate |
| pre-feature library's declaration with a modifier | modifier is an `experiment_not_enabled` error — the element is **not** base/final/sealed | same (see S2) |
| SDK platform supertype | `_mayIgnoreClassModifiers` may exempt | `dart:ffi` never exempt; other `dart:` exempt only if the use site is SDK or pre-feature |

---

## 4. Sub-seams (evidence first, then the rule)

### 4.A — already closed by `#3161` A6 (do **not** dispatch)

46 of the 231 extras sit in
`features/class-modifiers/language/class_modifiers/base_transitivity/*`
(`base_class_different_library_error_test` 9,
`base_mixin_different_library_error_test` 14,
`base_mixin_class_different_library_error_test` 23).

Observed extra row shape (`wave_b_rows.tsv`):

```
...base_class_different_library_error_test 179:7 27
  "The class 'SimpleSealedExtendImplement' can't be implemented outside of its library because it's a base class."
```

vs. the oracle (`conformance/expected.tsv`, same case):

```
...base_class_different_library_error_test 179:7 10
  "The class 'BaseClass' can't be implemented outside of its library because it's a base class."
```

The extra names the **declaring container** (a0 = `display(container)`, anchor =
`name_range`), length 27 = the container name's length, column 7 = the
declaring name — i.e. exactly pass 5's row shape
(`overrides.rs:3501-3506`). Upstream `:240-256` names the **explicit base
ancestor** and anchors on the **written implements entry**, which is pass 1's
row. `#3161` A6 added the `break` for
`BaseClassImplementedOutsideOfLibrary | BaseMixinImplementedOutsideOfLibrary`
before pass 5 pushes. The fixture (`shared_library_definitions.dart`:
`base class BaseClass {}` … `sealed class SealedExtend extends BaseClass {}`)
confirms the ancestor-naming rule.

**Verdict: closed. Do not re-dispatch; verify in the re-census only.**

### 4.B — S1: the clause-error drive gate (LIVE, extra-side, ~138 rows)

Upstream §3.1 suppresses the entire outside-library block when the clause has a
hard error. dartq has no such gate: `plan_modifier_use_sites` runs passes 1–4
unconditionally.

Measured extras by corpus, and their attribution:

| corpus | extras | shape |
|---|---:|---|
| `features/_core/*` | 71 | `class X extends int/String/Null/Object…`, `with`/`implements` on core types |
| `parser/src/fasta/*` | 31 | recovery shapes: `extends int` / `implements int` / `on int` |
| `diagnostics/{extends,implements}_disallowed_class`, `subtype_of_disallowed_type`, `mixin_super_class_constraint_disallowed_class`, `subtype_of_ffi_class` | 33 | same, the analyzer-test spellings |
| `manual/diagnostic/expected_named_type` | 1 | |
| `syntax_base_mixin_A02_t0{1,2}` | 8 | `extends <base mixin>` — likely `_checkForAllMixinErrorCodes`'s `class_used_as_mixin` arm; classify during the fix |

Representative pin (exact oracle row is *absent*; this is the dartq extra):

```
diagnostics/extends_disallowed_class/ExtendsDisallowedClassTest/test_class_int
  extra ERROR|invalid_use_of_type_outside_library|lib/test.dart|…|3
  "The class 'int' can't be extended outside of its library because it's a final class."
```

**Rule to implement:** before passes 1–4, compute the four clause-error verdicts
for this container and return no findings if any is true. Pass 5 stays
unconditional (§3.2).

**Where the verdicts already exist:** `crates/dartq-semantic/src/diagnose/engine_core.rs:9190`
`finalize_disallowed_subtypes` already walks *exactly* the four upstream clause
lists (`_checkForExtendsDisallowedClass` 4692, the `with` arm of
`_checkForAllMixinErrorCodes` 2446-2463 incl. the `!isDartCoreEnum` conjunct,
`_checkForImplementsClauseErrorCodes` 5365, `_checkForOnClauseErrorCodes` 6724).
The `extends`/`implements`/`with`/`on` classification and the
`NON_SUBTYPABLE_SDK_NAMES` set are already there. **Do not invent a second
predicate** — extract a `clause_error_faces(container) -> ClauseErrors`
helper from that function and consume it from `plan_modifier_use_sites`.

Two of the four upstream probes are *not* covered by `finalize_disallowed_subtypes`:
`_checkForAllMixinErrorCodes`'s non-disallowed arms (mixin-clause errors such as
`class_used_as_mixin`, `mixin_of_non_class`) and
`_checkForNoGenerativeConstructorsInSuperclass`. Their dartq emit sites must be
located in the same pass (grep `ClassUsedAsMixin`,
`NoGenerativeConstructors`); if a site is not reachable from the plan, the
conservative fallback is to gate on what is computable and let the re-census
decide — replacing a fabricated row with a missing row is a net loss, so
**prefer under-gating to over-gating** for the first landed iteration.

### 4.C — S2: modifier facts in a pre-feature library (LIVE, extra-side, 71 rows)

All 71 co19 extras come from the six `versioning_A02_t0{1,2,3,4,6}` cases,
every one of which carries `// @dart=2.19` on the entry file.

Mechanism (traced, not guessed):

1. `tools/conformance/src/materialize.rs:73` — `language_version(case)` takes the
   entry file's `//@dart` override and writes it into the case's
   `package_config.json` `languageVersion` for the **whole case package**
   (`materialize.rs:242-260`). `class_modifiers_lib.dart` has no override of its
   own, so the oracle sees it at 2.19 too.
2. The oracle's committed expectation for `versioning_A02_t03`
   (`expected.tsv`) is **only** `experiment_not_enabled` rows on
   `class_modifiers_lib.dart` (`This requires the 'class-modifiers' language
   feature to be enabled.`) — no modifier rows anywhere.
3. dartq's `container_modifiers` (`diagnose/collector.rs:2548`) copies
   `container.is_base` / `is_final` / `sealed` **straight from the syntax**;
   `collector.rs:2620/2696/2791` set them from `base_kw.is_some()` etc. with no
   feature check. So a 2.19 library still yields a `base` class, and
   `LibraryClassModifiers::is_enabled` never gets a chance to suppress it —
   `may_ignore_class_modifiers` only fires when the *supertype's* library is the
   SDK (`modifiers.rs:227-230`), and a co19 fixture library is not.

**Rule to implement:** the modifier flags an element carries are a function of
its **declaring library's** `class-modifiers` feature state, not of the tokens
alone. When `!LibraryClassModifiers::is_enabled(declaring_lib)`, the effective
flag set for that declaration is empty (the modifier only produced
`experiment_not_enabled`).

**Where — two candidates, pick one (recommendation: B):**

- **A — collector (`diagnose/collector.rs:2548` `container_modifiers`).**
  Correct at the source but the collector has no `LibraryClassModifiers`; it
  would need the per-file posture injected, i.e. a signature change on a hot
  construction path. Rejected for cost.
- **B — `FoldHierarchy::declared` (`diagnose/types.rs:5420`).** Every consumer
  in this family reads modifiers through `FoldHierarchy` (`effective` →
  `declared` → `container_modifiers`). `declared` already resolves the handle to
  `(container, owner)`; adding `owner_library(...)` + a posture lookup and
  masking the flags when disabled is one small, local change that fixes
  passes 1–5 at once. Requires threading `&LibraryClassModifiers` into
  `FoldHierarchy` (it is constructed in `overrides.rs` and in
  `diagnose/types.rs:2937`'s doc-pinned rig). **Recommended.**
- **C — mask only at `subtype_restriction_violation` / the four `*_violation`
  call sites.** Cheapest but duplicated at 5 sites and misses `induced_modifier`
  and `nearest_explicit_base_or_final`. Rejected.

**Blast-radius note (must be part of the ticket):** `base_face()` /
`sealed_face()` / `container_modifiers` have exactly 8 consumer files
(`dartq-commands/src/lints.rs`, `dartq-engine/src/pipeline.rs`,
`dartq-resolution/src/modifiers.rs`, `dartq-semantic/src/{collector,lib}.rs`,
`diagnose.rs`, `diagnose/types.rs`, `diagnose/unused_finalization.rs`).
Masking at `FoldHierarchy::declared` keeps the change inside the semantic
diagnose layer; `unused_finalization.rs:99` reads the same gate and would pick
the fix up (desirable — a pre-feature library's `base` is not a `base`).
`declaration_violations` (declaration-site combos, e.g. `abstract sealed`) is a
*different* family and is **out of scope**; do not mask it in the same change.

### 4.D — S3: residual missing side (needs one probe, then possibly a fix)

The stale snapshot shows 827 missing. `#3161` projected ~40 non-alias residual,
so the live number is unknown but small. The stale data nevertheless contains a
**clean, reproducible anomaly** worth resolving before anything else, because it
is either "already fixed" or a real whole-file suppression bug.

Pattern (all four cases have identical structure, differing only in which
supertype they name):

| case | supertype | expected rows | dartq rows |
|---|---|---:|---:|
| `co19/.../basic_restrictions_A04_t01` | `BaseClass` (other lib) | 19 | 0 |
| `co19/.../basic_restrictions_A04_t02` | `BaseClass` (same lib) | 33 | 0 |
| `co19/.../basic_restrictions_A04_t05` | `FinalClass` (other lib) | 5 | 0 |
| `co19/.../basic_restrictions_A04_t03` | `AbstractBaseClass` (other lib) | 19 | **19** |
| `co19/.../basic_restrictions_A04_t08` | `abstract final` (same lib) | — | matches |

`diff basic_restrictions_A04_t01.dart basic_restrictions_A04_t03.dart` shows
the two bodies are identical except for the supertype names, and both import
the same `class_modifiers_lib.dart`. Yet one emits its full expected set and the
other emits nothing. That is not a per-row predicate difference; it is a
whole-plan suppression. `basic_restrictions_A03_t0{1..20}` behave the same way
(16–17 `invalid_use` rows expected each, all missing).

Candidate causes to probe, in cost order:

1. `deferred_modifier` slot loss / `reported_modifier_checks` double-drain for a
   header that carries **both** a `with` clause and an `implements` clause
   (`A04_t01/t02` both do; `A04_t03` has no plain `with BaseMixin` face).
2. `header_with_entry_spans`'s bounded rescan interacting with a *with-only*
   header (`class ClassWithBaseMixin with BaseMixin {}`), possibly bailing the
   whole plan for the file.
3. The container-level latch keyed on `StringId` being consumed by an earlier,
   span-empty call.

**Probe recipe (30 min, no oracle needed):** run the existing rig
(`report_ops` / `report_ops_imports` in `diagnose/test_rig.rs`) over the two
co19 bodies verbatim, and bisect by deleting declarations until the emission
flips. That separates "whole-file suppression" from "per-declaration". Do this
**before** writing any fix; if the probe shows the rows already emit on HEAD,
S3 collapses to the re-census and the ticket is S1+S2 only.

---

## 5. Dispatch-ready ticket scope

### F9-A — clause-error drive gate (S1)  ← dispatch first, highest certain yield

- **Files:** `crates/dartq-semantic/src/diagnose/engine_core.rs` (extract the
  verdicts from `finalize_disallowed_subtypes`, :9190),
  `crates/dartq-semantic/src/diagnose/overrides.rs`
  (`plan_modifier_use_sites`, :3195; `check_modifier_use_sites`, :3009).
- **Rule:** §3.1. Class drive = four probes; mixin drive = two probes. Gate
  passes 1–4 only.
- **Pin shapes:** the *absence* of
  `ERROR|invalid_use_of_type_outside_library|<file>|<offset>|<len>|"The class 'int' can't be extended outside of its library because it's a final class."`
  on `class X extends int {}`, `class X implements int {}`,
  `mixin M on int {}`, and the positive control that
  `base class B {}` + `class X extends B {}` in one library **still** reports
  `subtype_of_base_or_final_...` (pass 5 un-gated).
- **Yield:** ~138 extras removed (upper bound; stale snapshot).
- **Risk:** over-gating converts an extra into a missing row (net zero on
  Jaccard, but loses information). Under-gate deliberately for iteration 1.

### F9-B — pre-feature modifier facts (S2)

- **Files:** `crates/dartq-semantic/src/diagnose/types.rs`
  (`FoldHierarchy::declared`, :5420 — add the posture mask; thread
  `&LibraryClassModifiers`, which `overrides.rs` already holds as
  `self.class_modifiers`), `crates/dartq-semantic/src/diagnose/overrides.rs`
  (constructor sites only).
- **Rule:** §4.C. Mask to the empty flag set (keep `kind`) when
  `!class_modifiers.is_enabled(declaring_lib)`.
- **Pin shapes:** `versioning_A02_t03` — `class_modifiers_lib.dart` at 2.19 must
  yield **zero** `invalid_use_of_type_outside_library` and zero
  `subtype_of_base_or_final_...`; the only rows are the 33
  `experiment_not_enabled` rows on that file. Same for `versioning_A02_t0{1,2,4,6}`.
- **Yield:** 71 extras.
- **Risk:** `symbol`-level consumers of `ClassModifiers` outside the diagnose
  layer (`dartq-lints`, `dartq-commands/src/lints.rs`, `pipeline.rs` class
  registry) read the *unmasked* facts. Decide explicitly whether to mask at
  `FoldHierarchy` (diagnose-only, safe, recommended for iteration 1) or at
  `container_modifiers` (global, larger blast radius, correct long-term). Record
  the choice in the ticket.

### F9-C — S3 probe + residual closure

- **Files:** probe only, then whatever §4.D names.
- **Gate:** if the probe shows current HEAD already emits the A04_t01/t02/t05
  rows, close as "absorbed by #3161" and spend the slot on the re-census.

### Explicitly NOT in scope

- `crates/dartq-resolution/src/modifiers.rs` — decision layer is correct;
  `nearest_explicit_base_or_final`, `subtype_restriction_violation` and the
  double `_mayIgnoreClassModifiers` copies all match 3.13.7 line for line.
- Declaration-site combination checks (`abstract sealed`, `interface mixin`,
  …) — different code family (`declaration_violations`).
- `subtype_of_disallowed_type` anchoring (dartq anchors at the declaring name,
  upstream at the clause entry) — that is F13's row, not F9's.
- The `experiment_not_enabled` matrix for class modifiers — F19. S2 *depends* on
  the two staying consistent (both derive from the same posture map); note it in
  the ticket but do not fix F19 here.

---

## 6. File-ownership note (for sequencing against live lanes)

Observed at `dc681488` (`git status --short`): dirty = `dartq-parser/src/{members,recovery}.rs`,
`dartq-semantic/src/diagnose/{invocation,row_resolution}.rs`,
`dartq-lints/src/rules/**`. `overrides.rs`, `types.rs`, `engine_core.rs`,
`collector.rs`, `modifiers.rs` are **clean** — no lane holds them right now.

| file | size | class | note |
|---|---:|---|---|
| `crates/dartq-semantic/src/diagnose/overrides.rs` | 8,581 | **family-owned, cold-ish** | the only file with the five passes; not on any other family's list (absent from `f6_split_design.md`'s ownership table). Safe to hand to a single lane. |
| `crates/dartq-semantic/src/diagnose/engine_core.rs` | ~9,600 | **HOT (shared row-path)** | `emit_modifier`, `finalize_disallowed_subtypes`, scope frames — every semantic lane touches it. F9-A needs it only to *read/extract* a helper; sequence behind the in-flight semantic lanes and land as a small extraction. |
| `crates/dartq-semantic/src/diagnose/types.rs` | 6,460 | **HOT — lane #3202** | `FoldHierarchy` lives here; F6-A/F6-E/F6-H/F6-I also own `types.rs`. F9-B must wait for #3202 to land or ride the same lane. |
| `crates/dartq-semantic/src/diagnose.rs` | 6,475 | **HOT** | only needs one `mod ticket_<id>_tests;` line + doc block (§ see the existing `ticket_3161_tests` block at :6429). One-line append at the tail of a pass — coordinate per the "尾部一次接线" rule. |
| `crates/dartq-semantic/src/diagnose/collector.rs` | ~2,800 | warm | only touched if S2 chooses option A (rejected). |
| `crates/dartq-resolution/src/modifiers.rs` | 1,996 | **cold, pure** | expected to need **zero** changes; if the fix wants a predicate there it is a trivial, conflict-free add. |
| `crates/dartq-engine/src/semdiag.rs` | — | warm | only read (`build_class_modifiers`), no edit expected. |
| new `crates/dartq-semantic/src/diagnose/ticket_<id>_tests.rs` | — | **cold, new file** | zero conflict surface. |

Recommended sequencing: **F9-A alone** on `overrides.rs` + a small
`engine_core.rs` extraction (avoid while a semantic lane is mid-flight in
`engine_core.rs`); **F9-B** only after `#3202` frees `types.rs`; **F9-C** probe
can run any time (read-only).

---

## 7. Open points carried forward

1. **Re-census is mandatory.** Every number here is a stale-snapshot upper
   bound; the ticket must start with a sweep and re-split the residual by
   corpus family before sizing.
2. **`syntax_base_mixin_A02_t0{1,2}` (8 extras)** are placed under S1 by shape,
   not by proof — classify them during the S1 fix.
3. **S3's whole-file suppression anomaly** is unexplained by reading; the probe
   in §4.D is the cheapest resolution and may collapse the entire S3 sub-seam.
4. **`#3161`'s own residual note** ("clause-error gate not modeled", "~40
   non-alias MISSING projected") is the prior art for this design: S1 is the
   item it deferred, S3 is the item it projected. Nothing here contradicts it.
