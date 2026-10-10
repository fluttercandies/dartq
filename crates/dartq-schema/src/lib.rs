//! Machine-readable JSON schemas for dartq's versioned output faces, plus
//! the lightweight structural validator CI runs against real output (PRD
//! §15.7: the diagnostics/patch/impact report schemas ship with the version,
//! in-crate, so tooling can check outputs against their published shapes).
//!
//! # Versioning discipline (PRD §15.7)
//!
//! * Every versioned output carries its contract version under
//!   `schema_version` — except the diagnostics envelope, which keeps the
//!   oracle's own `version` key (see "The `version` alias" below).
//! * A breaking output change bumps the major: the domain's
//!   `SCHEMA_VERSION_*` constant moves, and every producer of the face must
//!   move in the same release.
//! * Additive evolution — a new optional field, a new enum spelling — keeps
//!   the major: the schema document and this validator are updated in
//!   lockstep with the first emitter, and the constant stays put. The
//!   validator rejects undeclared fields and unknown spellings, so such a
//!   change cannot land silently: it always forces the documented schema
//!   change first.
//! * Optional fields are omitted rather than null (§15.8.5). Two total
//!   columns predate the rule and keep the null spelling — a query
//!   declaration row's `container` (null = top-level; the machine face
//!   prints `-`) and run metrics' `max_rss_bytes` (null = getrusage
//!   unavailable). The validators accept both spellings for exactly those
//!   two fields.
//! * Every entity reference is the stable `label|name` path form; internal
//!   ids never leave the engine. The validator pins the `label|name` form
//!   for the impact report's symbol refs and the session symbol table's
//!   values. Internal-only data (e.g. the fix edit's verified original
//!   slice, `fix.rs`'s `Edit.source`) is rejected as an undeclared field.
//! * Enum spellings are the shortest unambiguous form: kebab-case tags for
//!   impact levels, declaration families, and reference relations; the
//!   oracle's UPPERCASE spellings for diagnostic severities and types.
//! * §15.8.5 pins new faces' ranges to `[startOffset, endOffset)` pairs.
//!   The oracle-derived faces keep their pinned older spellings (position
//!   objects in the diagnostics envelope, `offset`+`length` in query/fix
//!   rows); a face designed under §15.8 adopts the pair form at its schema
//!   review.
//!
//! # The `version` alias
//!
//! The diagnostics envelope keys its contract version `version`, not
//! §15.7's generic `schema_version`, because the envelope is byte-compatible
//! with `dart analyze --format=json`. This is a deliberate, recorded alias —
//! never a reason to add a second key to that envelope. Oracle byte parity
//! outranks naming uniformity everywhere the two touch (the machine line
//! formats likewise carry no version key at all; their contract is the
//! harness parser, not a schema).
//!
//! # Verbosity tiers (PRD §15.1, §15.8.1)
//!
//! Every face answers under `--verbosity minimal|compact|full` (default
//! compact). The tiers are part of this frozen contract; the canonical
//! semantics:
//!
//! * **compact** — the shape each producer emits today. It is the baseline
//!   every fixture pins.
//! * **minimal** — the red-line floor (§15.8): locating fields are never
//!   omissible (`code`/`file`/`range` or the face's equivalent, marked ● in
//!   the tables). Token cost is trimmed by dropping prose and convenience
//!   spellings, never location.
//! * **full** — compact plus cheaply-available extras (per-face, listed
//!   below). Deep content stays behind ids and dedicated endpoints
//!   (§15.8.6); `full` never inlines unbounded objects.
//!
//! Payloads carry no verbosity marker (adding one would break the compact
//! byte shape), so each published schema is the **union** of its tiers'
//! field sets: `validate` enforces the union and the red line, while tier
//! exactness (a minimal answer omits exactly the tier-optional fields) is
//! the emitter's test obligation.
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
//! are §15.8.1 full-tier content with no producer yet; when one lands it is
//! an additive optional field. The CLI's diagnostics faces are tier-
//! invariant regardless: the oracle faces (`--format machine|json`) are
//! byte-pinned, and the human listing is the verbose presentation. The
//! tiers bind the emitters that honor `--verbosity` (daemon/RPC faces).
//!
//! Fix patch documents (§15.6; per edit — `file`/`offset`/`length`/
//! `replacement` are the red line):
//!
//! | field                 | minimal | compact | full |
//! |-----------------------|---------|---------|------|
//! | `file`                | ●       | ●       | ●    |
//! | `edits[].offset`      | ●       | ●       | ●    |
//! | `edits[].length`      | ●       | ●       | ●    |
//! | `edits[].replacement` | ●       | ●       | ●    |
//! | `edits[].anchor`      |         | ●       | ●    |
//! | `edits[].rule`        |         |         | ●    |
//!
//! `edits[].stale` may appear in any tier when an applier marks an edit
//! instead of dropping it (§15.6). `rule` and `stale` are declared
//! absent-tolerant today: the machine writer emits neither (the human face
//! carries the rule; stale edits are skipped), so enabling them is additive.
//!
//! Query answers (`query symbol`; row locators are the red line — `kind` is
//! the row's semantic code):
//!
//! | field                          | minimal | compact | full |
//! |--------------------------------|---------|---------|------|
//! | `name`                         | ●       | ●       | ●    |
//! | `declarations[].file`          | ●       | ●       | ●    |
//! | `declarations[].kind`          | ●       | ●       | ●    |
//! | `declarations[].name`          | ●       | ●       | ●    |
//! | `declarations[].line`/`column` | ●       | ●       | ●    |
//! | `declarations[].length`        | ●       | ●       | ●    |
//! | `declarations[].container`     |         | ●       | ●    |
//! | `references[].target_file`     | ●       | ●       | ●    |
//! | `references[].kind`            | ●       | ●       | ●    |
//! | `references[].offset`/`length` | ●       | ●       | ●    |
//! | `references[].qualified`       |         | ●       | ●    |
//!
//! `symbols` and `since` are session fields, tier-independent — see "Token
//! economics".
//!
//! Compatibility matrix rows (§15.5; the verdict + axis identity are the
//! red line — evidence is one `check` away):
//!
//! | field                    | minimal | compact             | full |
//! |--------------------------|---------|---------------------|------|
//! | `matrix[].sdk`           | ●       | ●                   | ●    |
//! | `matrix[].compatible`    | ●       | ●                   | ●    |
//! | `matrix[].first_failure` |         | ● (when errors)     | ●    |
//! | `matrix[].error`         |         | ● (on load failure) | ●    |
//!
//! Explain report rows (§15.3; the declaration's locators are the red
//! line — `kind` is the row's semantic code. `minimal` drops the section
//! arrays for the `counts` object, so the AI still sees which sections
//! exist and are empty; `full` equals `compact` today):
//!
//! | field                    | minimal | compact | full |
//! |--------------------------|---------|---------|------|
//! | `symbol`                 | ●       | ●       | ●    |
//! | `declaration`            | ●       | ●       | ●    |
//! | `declaration.container`  |         | ●       | ●    |
//! | section arrays (7)       | counts  | ●       | ●    |
//! | `references[].qualified` | (count) | ●       | ●    |
//! | `counts`                 | ●       |         |      |
//!
//! Tier-invariant faces: the impact report (every field is a locator or a
//! verdict — nothing prose-like to trim; `since` is its token lever) and
//! the run-metrics sidecar (a benchmark artifact, emitted complete or not
//! at all).
//!
//! # Token economics (PRD §15.8)
//!
//! Declared now, absent-tolerant, so v1 does not unfreeze when emitters
//! land:
//!
//! * **Session symbol table** — the query and impact envelopes take an
//!   optional `symbols` object: short id → stable `label|name` path. The
//!   session's first answer attaches it; later answers may back-reference
//!   by replacing a row's path-valued string with its short id (still a
//!   string to this validator, by design — resolving ids is the consumer's
//!   table lookup, not a schema concern).
//! * **Delta mode** — a repeat query may carry `since: <epoch>`; the answer
//!   restricted to changed rows keeps its envelope shape and echoes the
//!   cursor in the optional `since` field (absent = full answer).
//! * **Batch queries** — shape-preserving: one envelope per queried symbol,
//!   the JSONL convention the fix face already uses. No aggregate envelope
//!   is defined.
//! * **Null/empty omission** (§15.8.5) — emitters omit optional fields
//!   instead of sending null/empty; the two legacy null spellings are listed
//!   under the versioning discipline.
//!
//! # Domains
//!
//! * [`Domain::Impact`] — the §15.4 impact report ([`impact_schema`];
//!   `SCHEMA_VERSION_IMPACT` mirrors `dartq-ai`'s authoritative constant).
//! * [`Domain::Diagnostics`] — the `dart analyze --format=json` envelope
//!   ([`diagnostics_schema`]; the `version` alias above).
//! * [`Domain::FixPatch`] — the §15.6 patch document ([`fix_patch_schema`]).
//! * [`Domain::Query`] — the §15.2 query answer ([`query_schema`]).
//! * [`Domain::Explain`] — the §15.3 explain report
//!   ([`explain_schema`]; `SCHEMA_VERSION_EXPLAIN` mirrors `dartq-ai`'s
//!   authoritative constant).
//! * [`Domain::Compat`] — the §15.5 compatibility report
//!   ([`compat_schema`]; `SCHEMA_VERSION_COMPAT` mirrors `dartq-ai`'s
//!   authoritative constant).
//! * [`Domain::RunMetrics`] — the `--metrics-json` run summary
//!   ([`run_metrics_schema`]).
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
//! any [`SchemaViolation`] fails the run via its `Display`.
//! `tests/alignment.rs` runs real producers through it;
//! `tests/output_faces.rs` pins the hand-derived fixture shapes with their
//! writer provenance.

use std::fmt;

use serde_json::Value;

mod compat;
mod diagnostics;
mod explain;
mod fix_patch;
mod impact;
mod query;
mod run_metrics;
mod support;

pub use compat::{ERROR_KINDS, SCHEMA_VERSION_COMPAT, compat_schema};
pub use diagnostics::{SCHEMA_VERSION_DIAGNOSTICS, diagnostics_schema};
pub use explain::{SCHEMA_VERSION_EXPLAIN, explain_schema};
pub use fix_patch::{SCHEMA_VERSION_FIX_PATCH, fix_patch_schema};
pub use impact::{SCHEMA_VERSION_IMPACT, impact_schema};
pub use query::{DECL_KINDS, REF_KINDS, SCHEMA_VERSION_QUERY, query_schema};
pub use run_metrics::{SCHEMA_VERSION_RUN_METRICS, run_metrics_schema};

/// Which published schema a payload is validated against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// The §15.4 impact report (`dartq-ai`'s `impact` query output).
    Impact,
    /// The `dart analyze --format=json` envelope (`dartq-diagnostics`).
    Diagnostics,
    /// The §15.6 fix patch document (`dartq fix --format=machine`).
    FixPatch,
    /// The §15.2 query answer (`dartq query --format=json`).
    Query,
    /// The §15.3 explain report (`dartq-ai`'s `explain` query output).
    Explain,
    /// The §15.5 compatibility report (`dartq compat`).
    Compat,
    /// The `--metrics-json` run summary (`dartq check` / `dartq analyze`).
    RunMetrics,
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
        Domain::Compat => compat::validate_compat(&value),
        Domain::RunMetrics => run_metrics::validate_run_metrics(&value),
    }
}
