//! Machine-readable JSON schemas for dartq's versioned output faces, plus
//! the lightweight structural validator CI runs against real output (the
//! diagnostics/patch/impact report schemas ship with the version, in-crate,
//! so tooling can check outputs against their published shapes).
//!
//! # Schema freeze (v1)
//!
//! The v1 surface this crate documents is **frozen**: every
//! `SCHEMA_VERSION_*` constant in this crate is 1 and stays 1 for the whole
//! v1 line. `scripts/schema-freeze-sweep.sh` is the enforcement gate — it
//! drives the frozen shapes end-to-end across faces (the CLI machine and
//! JSON faces, the daemon's `initialize` capability map, the `dartq/check`
//! tier answers with the short-id table, the `dartq/fix` intake answer, and
//! the `dartq/assists` document) and fails on any drift. Run it in the
//! serial phase before any release cut.
//!
//! * **v1.x is additive-only.** A new optional field, a new enum spelling,
//!   or a new face keeps the major: the schema document, the validator, and
//!   the sweep grow in the same change as the first emitter.
//! * **What forces v2.** Removing or renaming a field, changing a field's
//!   JSON type or an enum spelling, making a field mandatory, reusing a
//!   field with a new meaning, or moving a version constant — anything a
//!   frozen v1 consumer could break on. A v2 bump moves every affected
//!   `SCHEMA_VERSION_*` and every producer in one release.
//! * **The short-id clause.** The session symbol table's *form* is frozen:
//!   keys are `s1`, `s2`, … assigned in first-seen order; values are the
//!   substituted-out strings (`label|name` entity paths, or the file-path
//!   table's own paths). The *assignment* is session-scoped — an id is
//!   meaningful only alongside the `symbols` table in the same answer, so
//!   consumers resolve through the table and never hardcode an id. Whether
//!   a given answer carries the table stays the emitter's byte-economics
//!   decision and is not a shape break.
//!
//! # Versioning discipline
//!
//! * Every versioned output carries its contract version under `v` —
//!   except the diagnostics envelope, which keeps the oracle's own
//!   `version` key (see "The `version` alias" below), and the
//!   compatibility report, which keeps its long-stamped `schema_version`
//!   (the compatibility face's own frozen dialect).
//! * A breaking output change bumps the major: the domain's
//!   `SCHEMA_VERSION_*` constant moves, and every producer of the face must
//!   move in the same release.
//! * Additive evolution — a new optional field, a new enum spelling — keeps
//!   the major: the schema document and this validator are updated in
//!   lockstep with the first emitter, and the constant stays put. The
//!   validator rejects undeclared fields and unknown spellings, so such a
//!   change cannot land silently: it always forces the documented schema
//!   change first.
//! * Optional fields are omitted rather than null. Two total
//!   columns predate the rule and keep the null spelling — a query
//!   declaration row's `container` (null = top-level; the machine face
//!   prints `-`) and run metrics' `max_rss_bytes` (null = getrusage
//!   unavailable). The validators accept both spellings for exactly those
//!   two fields.
//! * Every entity reference is the stable `label|name` path form; internal
//!   ids never leave the engine. The label is the located file spelling —
//!   root-relative inside the analysis root, absolute outside it — the
//!   same one locator every file column uses; the envelope's optional
//!   `root` (the absolute analysis root) makes the relative tails
//!   unambiguous. The `|name` suffix is the inline symbol reference's
//!   (the impact report's `sym[].sym`); the row-shaped file columns
//!   (`file`, `path`, `target`) carry the bare located spelling. The
//!   validator pins the `label|name` form for the impact report's symbol
//!   refs and the session symbol table's values. The index export's
//!   schema-frozen path columns are the one face still on the older
//!   `package:<name>/<lib-relative>` grammar (index_export.rs). Internal-
//!   only data (e.g. the fix edit's verified original slice, `fix.rs`'s
//!   `Edit.source` — the edit row's declared `src` key carries the
//!   provenance tag, a different value) is never emitted; the validator
//!   rejects anything undeclared.
//! * Enum spellings are the shortest unambiguous form: kebab-case tags for
//!   impact levels, declaration families, and reference relations; the
//!   oracle's UPPERCASE spellings for diagnostic severities and types.
//! * Ranges spell `off` + `len` (the short-vocabulary locator pair). The
//!   oracle-derived faces keep their pinned older spellings (position
//!   objects in the diagnostics envelope); the compat report keeps its
//!   own frozen dialect.
//!
//! # The `version` alias
//!
//! The diagnostics envelope keys its contract version `version`, not
//! the generic `schema_version`, because the envelope is byte-compatible
//! with `dart analyze --format=json`. This is a deliberate, recorded alias —
//! never a reason to add a second key to that envelope. Oracle byte parity
//! outranks naming uniformity everywhere the two touch (the machine line
//! formats likewise carry no version key at all; their contract is the
//! harness parser, not a schema).
//!
//! # Verbosity tiers
//!
//! Every face answers under `--verbosity minimal|compact|full` (default
//! compact). The tiers are part of this frozen contract; the canonical
//! semantics:
//!
//! * **compact** — the shape each producer emits today. It is the baseline
//!   every fixture pins.
//! * **minimal** — the red-line floor: locating fields are never
//!   omissible (`code`/`file`/`off`+`len` or the face's equivalent,
//!   marked ● in the tables). Token cost is trimmed by dropping prose and
//!   convenience spellings, never location.
//! * **full** — compact plus cheaply-available extras (per-face, listed
//!   below). Deep content stays behind ids and dedicated endpoints
//!; `full` never inlines unbounded objects.
//!
//! Payloads carry no verbosity marker (adding one would break the compact
//! byte shape), so each published schema is the **union** of its tiers'
//! field sets: `validate` enforces the union and the red line. The tier-
//! exact shapes are checked by [`validate_at_tier`], which rejects a
//! payload carrying a field its tier must omit (a minimal row with a
//! `problemMessage`, a compact edit with a `rule`, a full report with the
//! minimal-only `counts`). Presence beyond the red line stays the emitter's
//! fixture obligation: the omission conventions make legitimate
//! absences the validator cannot distinguish from tier violations.
//!
//! Diagnostics rows (`dartq-diagnostics` envelope; minimal = `code` +
//! `location`, ~15 tokens):
//!
//! | field              | minimal | compact | full |
//! |--------------------|---------|---------|------|
//! | `code`             | ●       | ●       | ●    |
//! | `location`         | ●       | ●       | ●    |
//! | `severity`         |         | ●       | ●    |
//! | `type`             |         | ●       | ●    |
//! | `problemMessage`   |         | ●       | ●    |
//! | `correctionMessage`|         | opt     | opt  |
//! | `documentation`    |         | opt     | opt  |
//!
//! `full` equals `compact` today (the envelope already carries the complete
//! message and documentation). Related-diagnostic and linked-fix surfaces
//! are full-tier content with no producer yet; when one lands it is
//! an additive optional field. The CLI's diagnostics faces are tier-
//! invariant regardless: the oracle faces (`--format machine|json`) are
//! byte-pinned, and the human listing is the verbose presentation. The
//! tiers bind the emitters that honor `--verbosity` (daemon/RPC faces).
//!
//! Fix patch documents (per edit — `file`/`off`/`len`/`text` are the red
//! line, and the provenance tag never drops either):
//!
//! | field              | minimal | compact | full |
//! |--------------------|---------|---------|------|
//! | `file`             | ●       | ●       | ●    |
//! | `edits[].off`      | ●       | ●       | ●    |
//! | `edits[].len`      | ●       | ●       | ●    |
//! | `edits[].text`     | ●       | ●       | ●    |
//! | `edits[].src`      | ●       | ●       | ●    |
//! | `edits[].anchor`   |         | ●       | ●    |
//! | `edits[].rule`     |         |         | ●    |
//! | `edits[].msg`      |         |         | ●    |
//! | `edits[].priority` |         |         | ●    |
//!
//! `edits[].src` is the row's provenance tag — `builtin-lint` or the
//! plugin's manifest name (`edit_floor`, `dartq-commands` fix.rs); not to
//! be confused with `fix.rs`'s internal `Edit.source` slice, which never
//! leaves the engine. `edits[].stale` may appear in any tier when an
//! applier marks an edit instead of dropping it. `stale` is the one
//! absent-tolerant field left: no writer emits it today, so enabling it is
//! additive.
//!
//! Query answers (`query symbol`; row locators are the red line — `kind` is
//! the row's semantic code):
//!
//! | field                     | minimal | compact | full |
//! |---------------------------|---------|---------|------|
//! | `sym`                     | ●       | ●       | ●    |
//! | `decls[].file`            | ●       | ●       | ●    |
//! | `decls[].kind`            | ●       | ●       | ●    |
//! | `decls[].name`            | ●       | ●       | ●    |
//! | `decls[].line`/`col`      | ●       | ●       | ●    |
//! | `decls[].len`             | ●       | ●       | ●    |
//! | `decls[].container`       |         | ●       | ●    |
//! | `refs[].target`           | ●       | ●       | ●    |
//! | `refs[].kind`             | ●       | ●       | ●    |
//! | `refs[].off`/`len`        | ●       | ●       | ●    |
//! | `refs[].qualified`        |         | ●       | ●    |
//! | `implementations[]`       |         | opt     | opt  |
//! | `deps[]`                  |         | opt     | opt  |
//!
//! `symbols` and `since` are session fields, tier-independent — see "Token
//! economics". The two reserved kind arrays are declared on frozen v1 for
//! the query kinds — `implementations` the subclass/implementation
//! declaration rows (typed on the shared declaration-row shape),
//! `dependencies` the declaring library's own import edges and consumed
//! dependency symbols (`path`, plus the optional `name` column — typed on
//! `dartq-ai::explain`'s `DependencyRow`); `minimal` drops them
//! (non-locating detail sections).
//!
//! Compatibility matrix rows (the verdict + axis identity are the
//! red line — evidence is one `check` away):
//!
//! | field                    | minimal | compact             | full |
//! |--------------------------|---------|---------------------|------|
//! | `matrix[].sdk`           | ●       | ●                   | ●    |
//! | `matrix[].compatible`    | ●       | ●                   | ●    |
//! | `matrix[].first_failure` |         | ● (when errors)     | ●    |
//! | `matrix[].error`         |         | ● (on load failure) | ●    |
//!
//! Explain report rows (the declaration's locators are the red
//! line — `kind` is the row's semantic code. `minimal` drops the section
//! arrays for the `counts` object, so the AI still sees which sections
//! exist and are empty; `full` equals `compact` today):
//!
//! | field                 | minimal | compact | full |
//! |-----------------------|---------|---------|------|
//! | `sym`                 | ●       | ●       | ●    |
//! | `declaration`         | ●       | ●       | ●    |
//! | `declaration.container` |       | ●       | ●    |
//! | section arrays (7)    | counts  | ●       | ●    |
//! | `refs[].qualified`    | (count) | ●       | ●    |
//! | `counts`              | ●       |         |      |
//!
//! Check answer rows (the daemon's `dartq/check`; the row's code + file
//! locator are the red line — `line`/`col`/`len` locate, `file` names the
//! document; `sev` is the lowercase word):
//!
//! | field           | minimal | compact | full |
//! |-----------------|---------|---------|------|
//! | `code`          | ●       | ●       | ●    |
//! | `file`          | ●       | ●       | ●    |
//! | `line`/`col`    | ●       | ●       | ●    |
//! | `len`           | ●       | ●       | ●    |
//! | `sev`           |         | ●       | ●    |
//! | `msg`           |         | ●       | ●    |
//!
//! `full` equals `compact` plus the session economics (`symbols` — the
//! short-id table over the rows' file paths). The two views (disk
//! pipeline, agent overlay) emit the same row shape.
//!
//! Analyze answer rows (the daemon's `dartq/analyze`; the row's code +
//! locator are the red line — the flattened short-vocabulary row, `file`
//! carrying the entry's document URI, `line`/`col` 1-based):
//!
//! | field        | minimal | compact | full |
//! |--------------|---------|---------|------|
//! | `code`       | ●       | ●       | ●    |
//! | `file`       | ●       | ●       | ●    |
//! | `line`/`col` | ●       | ●       | ●    |
//! | `len`        | ●       | ●       | ●    |
//! | `sev`        |         | ●       | ●    |
//! | `msg`        |         | ●       | ●    |
//!
//! `full` equals `compact` plus the session economics (`symbols` over the
//! row `file` values). No writer emits `since` on this face — the pull
//! face has no delta parameter — so none is declared.
//!
//! Index export documents (`dartq index --full`; JSONL — one header line
//! then one document per analyzed file; the document's `path` is the red
//! line and `symbol_count` rides it, so `minimal` lists the documents and
//! their symbol counts; `compact` adds the per-document short-id `symbols`
//! table and the `declarations` rows; `full` adds the `occurrences` and
//! `supertypes` rows):
//!
//! | document field   | minimal | compact | full |
//! |------------------|---------|---------|------|
//! | `v`              | ●       | ●       | ●    |
//! | `path`           | ●       | ●       | ●    |
//! | `symbol_count`   | ●       | ●       | ●    |
//! | `symbols`        |         | ●       | ●    |
//! | `declarations`   |         | ●       | ●    |
//! | `occurrences`    |         |         | ●    |
//! | `supertypes`     |         |         | ●    |
//!
//! The header line counts the stream's emitted rows: `documents` and
//! `declarations` ride every tier; `occurrences` and `supertypes` join
//! only at `full` — the tiers that emit no such rows must not carry the
//! keys, so consumers read "zero exported" apart from "not exported".
//! Full row-shape detail lives in `index_export`'s module docs.
//!
//! Diagnostic explain report (`dartq explain --diagnostic`; the echoed
//! diagnostic is the red line — `compact` adds the registry facts and the
//! related count, `full` adds the fix rows and the per-file line list):
//!
//! | field                 | minimal | compact | full |
//! |-----------------------|---------|---------|------|
//! | `diagnostic`          | ●       | ●       | ●    |
//! | `why`                 |         | ●       | ●    |
//! | `doc`                 |         | opt     | opt  |
//! | `related`             |         | ●       | ●    |
//! | `related.lines`       |         |         | ●    |
//! | `fixes`               |         |         | ●    |
//!
//! Full row-shape detail lives in `diagnostic_explain`'s module docs.
//!
//! Assists documents (`dartq assists` / `dartq/assists`; per assist row —
//! the row's provenance tag and title ride every tier, unlike the fix
//! patch's human-name rows: the writer drops only the priority and the
//! edit anchors at minimal):
//!
//! | field                      | minimal | compact | full |
//! |----------------------------|---------|---------|------|
//! | `assists[].msg`            | ●       | ●       | ●    |
//! | `assists[].src`            | ●       | ●       | ●    |
//! | `assists[].edits[]`        | ● floor | ● floor | ● floor |
//! | `assists[].priority`       |         | ●       | ●    |
//! | `assists[].edits[].anchor` |         | ●       | ●    |
//! | `errors[]`                 | opt     | opt     | opt  |
//!
//! The edits' floor is the fix patch's own — `off`/`len`/`text` plus the
//! provenance tag (`src`, the plugin's manifest name). `errors` rides any
//! tier when a host could not serve the request.
//!
//! Tier-invariant faces: the impact report (every field is a locator or a
//! verdict — nothing prose-like to trim; `since` is its token lever), the
//! run-metrics sidecar (a benchmark artifact, emitted complete or not at
//! all), and the index run summary (four totals off the maintained index —
//! no locator, no prose, nothing to trim).
//!
//! # Token economics
//!
//! Declared now, absent-tolerant, so v1 does not unfreeze when emitters
//! land:
//!
//! * **Session symbol table** — the query and impact envelopes take an
//!   optional `symbols` object: short id → stable `label|name` path. The
//!   session's first answer attaches it; later answers may back-reference
//!   by replacing a row's path-valued string with its short id (still a
//!   string to this validator, by design — resolving ids is the consumer's
//!   table lookup, not a schema concern). The faces whose economics dedup
//!   their own path-valued rows spell the same table id → FILE path — the
//!   values are the very strings substituted out of the rows, a different
//!   value domain than the entity-reference table. [`Domain::Explain`],
//!   [`Domain::Check`], and [`Domain::Analyze`] validate that spelling
//!   (`support::path_session_fields`).
//! * **Delta mode** — a repeat query may carry `since: <epoch>`; the answer
//!   restricted to changed rows keeps its envelope shape and echoes the
//!   cursor in the optional `since` field (absent = full answer).
//! * **Batch queries** — shape-preserving: one envelope per queried symbol,
//!   the JSONL convention the fix face already uses. No aggregate envelope
//!   is defined.
//! * **Null/empty omission** — emitters omit optional fields
//!   instead of sending null/empty; the two legacy null spellings are listed
//!   under the versioning discipline.
//!
//! # Domains
//!
//! * [`Domain::Impact`] — the impact report ([`impact_schema`];
//!   `SCHEMA_VERSION_IMPACT` mirrors `dartq-ai`'s authoritative constant).
//! * [`Domain::Diagnostics`] — the `dart analyze --format=json` envelope
//!   ([`diagnostics_schema`]; the `version` alias above).
//! * [`Domain::FixPatch`] — the patch document ([`fix_patch_schema`]).
//! * [`Domain::Query`] — the query answer ([`query_schema`]).
//! * [`Domain::Explain`] — the explain report
//!   ([`explain_schema`]; `SCHEMA_VERSION_EXPLAIN` mirrors `dartq-ai`'s
//!   authoritative constant).
//! * [`Domain::DiagnosticExplain`] — the `dartq explain --diagnostic`
//!   report ([`diagnostic_explain_schema`];
//!   `SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN` mirrors `dartq-ai`'s constant).
//! * [`Domain::Compat`] — the compatibility report
//!   ([`compat_schema`]; `SCHEMA_VERSION_COMPAT` mirrors `dartq-ai`'s
//!   authoritative constant).
//! * [`Domain::RunMetrics`] — the `--metrics-json` run summary
//!   ([`run_metrics_schema`]).
//! * [`Domain::Index`] — the index run summary ([`index_schema`]).
//! * [`Domain::IndexExport`] — the `dartq index --full` JSONL export's
//!   lines, header and per-file document ([`index_export_schema`];
//!   validated line-by-line — the validator accepts either shape).
//! * [`Domain::Check`] — the daemon's `dartq/check` answer
//!   ([`check_schema`]).
//! * [`Domain::Analyze`] — the daemon's `dartq/analyze` answer
//!   ([`analyze_schema`]).
//! * [`Domain::Assists`] — the assists document ([`assists_schema`]).
//! * The workspace config — `dartq.yaml` ([`config_schema`]) — is
//!   published without a [`Domain`]: it is an input document, and its
//!   enforcement point is the lenient reader in `dartq-commands`, not a
//!   payload validator (the config module's docs carry the division).
//!
//! # What [`validate`] checks — and what it does not
//!
//! The documents are hand-written JSON Schema draft-07 subset documents —
//! the published contract. [`validate`] is a hand-rolled structural checker
//! over the same shapes, not a JSON Schema engine: it parses the payload,
//! pins the version constant, requires the declared fields with their
//! declared JSON types, enforces the exact enum spellings, and rejects
//! undeclared fields. It reports the first violation as a
//! [`SchemaViolation`] (field path + expected vs found). Anything richer
//! (patterns, conditional schemas, per-code registries) is out of scope; the
//! differential harness covers semantics, this covers shape.
//!
//! # CI consumption
//!
//! CI (and the harness) serializes a real output face — or captures it from
//! the CLI/daemon — and calls [`validate`] with the producing [`Domain`];
//! any [`SchemaViolation`] fails the run via its `Display`. A face that
//! answers under a known `--verbosity` is checked with [`validate_at_tier`]
//! and the tier it answered under.
//! `tests/alignment.rs` runs real producers through it;
//! `tests/output_faces.rs` pins the hand-derived fixture shapes with their
//! writer provenance.

use std::fmt;

use serde_json::Value;

mod analyze;
mod assists;
mod check;
mod compat;
mod config;
mod diagnostic_explain;
mod diagnostics;
mod explain;
mod fix_patch;
mod impact;
mod index;
mod index_export;
mod query;
mod run_metrics;
mod support;

pub use analyze::{SCHEMA_VERSION_ANALYZE, analyze_schema};
pub use assists::{SCHEMA_VERSION_ASSISTS, assists_schema};
pub use check::{SCHEMA_VERSION_CHECK, check_schema};
pub use compat::{ERROR_KINDS, SCHEMA_VERSION_COMPAT, compat_schema};
pub use config::{SCHEMA_VERSION_CONFIG, TARGET_KINDS, config_schema};
pub use diagnostic_explain::{
    FIX_SOURCES, RULE_GROUPS, SCHEMA_VERSION_DIAGNOSTIC_EXPLAIN, TRIGGER_KINDS,
    diagnostic_explain_schema,
};
pub use diagnostics::{SCHEMA_VERSION_DIAGNOSTICS, diagnostics_schema};
pub use explain::{SCHEMA_VERSION_EXPLAIN, explain_schema};
pub use fix_patch::{SCHEMA_VERSION_FIX_PATCH, fix_patch_schema};
pub use impact::{SCHEMA_VERSION_IMPACT, impact_schema};
pub use index::{SCHEMA_VERSION_INDEX, index_schema};
pub use index_export::{
    EXPORT_REF_KINDS, SCHEMA_VERSION_INDEX_EXPORT, SUPER_CLAUSES, index_export_schema,
};
pub use query::{DECL_KINDS, REF_KINDS, SCHEMA_VERSION_QUERY, query_schema};
pub use run_metrics::{SCHEMA_VERSION_RUN_METRICS, run_metrics_schema};

/// Which published schema a payload is validated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// The impact report (`dartq-ai`'s `impact` query output).
    Impact,
    /// The `dart analyze --format=json` envelope (`dartq-diagnostics`).
    Diagnostics,
    /// The fix patch document (`dartq fix --format=machine`).
    FixPatch,
    /// The query answer (`dartq query --format=json`).
    Query,
    /// The explain report (`dartq-ai`'s `explain` query output).
    Explain,
    /// The diagnostic explain report (`dartq explain --diagnostic`).
    DiagnosticExplain,
    /// The compatibility report (`dartq compat`).
    Compat,
    /// The `--metrics-json` run summary (`dartq check` / `dartq analyze`).
    RunMetrics,
    /// The index run summary (`dartq index`).
    Index,
    /// The `dartq index --full` export stream's lines (header + per-file
    /// documents — the JSONL face).
    IndexExport,
    /// The daemon's `dartq/check` answer (`schema_version` + machine rows).
    Check,
    /// The daemon's `dartq/analyze` answer (LSP diagnostic entries).
    Analyze,
    /// The assists document (`dartq assists` / `dartq/assists`).
    Assists,
}

/// A verbosity tier — the `--verbosity` spelling a face
/// answered under. Tier exactness is checked by [`validate_at_tier`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    /// The red-line floor: locating fields only — a diagnostic's
    /// `code` + `location`, an edit's `off`/`len`/`text`.
    /// Nothing prose-like may ride along.
    Minimal,
    /// The default tier: each producer's baseline shape — the shape the
    /// compact fixtures pin byte for byte.
    Compact,
    /// Compact plus the cheaply-available extras named per face in the
    /// tier tables above.
    Full,
}

impl fmt::Display for Verbosity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Verbosity::Minimal => "minimal",
            Verbosity::Compact => "compact",
            Verbosity::Full => "full",
        })
    }
}

/// The first structural violation found in a payload, named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaViolation {
    /// Field path of the violation: `schema_version`,
    /// `affected[0].impact`, `diagnostics[0].location.range.start.line`.
    /// Empty for whole-payload failures (JSON syntax, root not an object).
    pub path: String,
    /// What the schema requires at that path.
    pub expected: String,
    /// What the payload actually had there.
    pub found: String,
}

impl fmt::Display for SchemaViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "expected {}, found {}", self.expected, self.found)
        } else {
            write!(
                f,
                "at `{}`: expected {}, found {}",
                self.path, self.expected, self.found
            )
        }
    }
}

impl std::error::Error for SchemaViolation {}

/// Validates a serialized output payload against its domain's published
/// schema: JSON syntax, version constant, required fields and types, exact
/// enum spellings, `label|name` symbol refs, and no undeclared fields.
/// Reports the first violation.
///
/// # Errors
///
/// Returns a [`SchemaViolation`] naming the first check the payload fails;
/// `Ok(())` means the payload matches the published v1 shape.
pub fn validate(payload: &str, domain: Domain) -> Result<(), SchemaViolation> {
    let value = serde_json::from_str::<Value>(payload).map_err(|error| SchemaViolation {
        path: String::new(),
        expected: "a JSON document".to_owned(),
        found: error.to_string(),
    })?;
    match domain {
        Domain::Impact => impact::validate_impact(&value),
        Domain::Diagnostics => diagnostics::validate_diagnostics(&value),
        Domain::FixPatch => fix_patch::validate_fix_patch(&value),
        Domain::Query => query::validate_query(&value),
        Domain::Explain => explain::validate_explain(&value),
        Domain::DiagnosticExplain => diagnostic_explain::validate_diagnostic_explain(&value),
        Domain::Compat => compat::validate_compat(&value),
        Domain::RunMetrics => run_metrics::validate_run_metrics(&value),
        Domain::Index => index::validate_index(&value),
        Domain::IndexExport => index_export::validate_index_export(&value),
        Domain::Check => check::validate_check(&value),
        Domain::Analyze => analyze::validate_analyze(&value),
        Domain::Assists => assists::validate_assists(&value),
    }
}

/// Validates a serialized output payload at a specific verbosity tier:
/// everything [`validate`] checks, plus tier exactness — a payload may not
/// carry a field its tier must omit (crate docs, "Verbosity tiers"). A
/// `minimal` answer therefore carries only the red line plus the face's
/// tier-independent fields; a `compact` answer carries no full-only field.
///
/// Presence beyond the red line is not enforced: the ● cells past the
/// locating floor are the emitter's fixture obligation (the omission
/// conventions make legitimate absences indistinguishable from violations).
/// The tier-invariant faces ([`Domain::Impact`], [`Domain::RunMetrics`],
/// [`Domain::Index`]) ignore the tier — every field is a locator, a
/// verdict, or a summary count.
///
/// # Errors
///
/// Returns a [`SchemaViolation`] naming the first union violation, or the
/// first field the tier must omit.
pub fn validate_at_tier(
    payload: &str,
    domain: Domain,
    tier: Verbosity,
) -> Result<(), SchemaViolation> {
    let value = serde_json::from_str::<Value>(payload).map_err(|error| SchemaViolation {
        path: String::new(),
        expected: "a JSON document".to_owned(),
        found: error.to_string(),
    })?;
    match (domain, tier) {
        (Domain::Impact, _) => impact::validate_impact(&value),
        (Domain::RunMetrics, _) => run_metrics::validate_run_metrics(&value),
        (Domain::Index, _) => index::validate_index(&value),
        (Domain::IndexExport, tier) => index_export::validate_index_export_at_tier(&value, tier),
        (Domain::Diagnostics, tier) => diagnostics::validate_diagnostics_at_tier(&value, tier),
        (Domain::FixPatch, tier) => fix_patch::validate_fix_patch_at_tier(&value, tier),
        (Domain::Query, tier) => query::validate_query_at_tier(&value, tier),
        (Domain::Compat, tier) => compat::validate_compat_at_tier(&value, tier),
        (Domain::Explain, tier) => explain::validate_explain_at_tier(&value, tier),
        (Domain::DiagnosticExplain, tier) => {
            diagnostic_explain::validate_diagnostic_explain_at_tier(&value, tier)
        }
        (Domain::Check, tier) => check::validate_check_at_tier(&value, tier),
        (Domain::Analyze, tier) => analyze::validate_analyze_at_tier(&value, tier),
        (Domain::Assists, tier) => assists::validate_assists_at_tier(&value, tier),
    }
}
