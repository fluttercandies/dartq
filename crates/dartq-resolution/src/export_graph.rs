//! The export/re-export graph diagnostic faces (module `export_graph.rs`):
//! pure decisions over the frozen [`LibraryGraph`] for the export-family
//! red codes, pinned to the analyzer oracle (dart 3.13.7).
//!
//! Six faces live here as inherent [`LibraryGraph`] methods (the crate
//! owns the type; the module split keeps `library_graph.rs` at its graph
//! core), each mirroring one upstream verifier walk:
//!
//! - [`LibraryGraph::ambiguous_exports`] — `ambiguous_export`
//!   (`error_verifier.dart:2593-2621` `_checkForAmbiguousExport`): a
//!   library-wide seen-map of names to elements is accumulated across the
//!   export directives in source order; a directive whose namespace (the
//!   target's export scope under the directive's combinators,
//!   `scope.dart:99-110` `createExportNamespaceForDirective2`) exports a
//!   name already exported as a **different** element reports once, at the
//!   directive, and contributes no further names from that directive. The
//!   same element via any number of paths dedupes silently — that is
//!   `duplicate_export`'s neighbor case, not an ambiguity.
//! - [`LibraryGraph::duplicate_export_directives`] — `duplicate_export`
//!   (`imports_verifier.dart:365-395` `_duplicates`, reported by
//!   `:100-109`): directives with resolved targets are stably sorted by
//!   the target library's URI and walked as adjacent pairs; a pair with
//!   equal target and identical combinator shape reports the directive
//!   that comes **second in source order**, and the walk advances
//!   unconditionally — a run of `n` identical directives reports `n − 1`
//!   times.
//! - [`LibraryGraph::export_internal_library_directives`] —
//!   `export_internal_library` (`error_verifier.dart:4645-4674`): an export
//!   of an internal SDK library (`dart:_…`, `element.dart:7042-7044`
//!   `isInternalSdkLibrary`) from a library that is not itself a
//!   `dart:`-scheme library (`:320` `_isInSystemLibrary`).
//! - [`LibraryGraph::invalid_internal_export_findings`] —
//!   `invalid_export_of_internal_element`(_indirectly)
//!   (`best_practices_verifier.dart:1004-1050` `_checkForInternalExport`),
//!   gated on the exporter being inside its package's public API
//!   (`workspace/pub.dart:543-570` `sourceIsInPublicApi` — under `lib/`,
//!   not under `lib/src/`): the exported **library** carrying `@internal`
//!   reports once (and falls through), then each namespace element that is
//!   `@internal` reports (skipping its signature walk), then each
//!   `@internal` type alias appearing in an executable element's signature
//!   (parameter types, return type, type-parameter bounds) reports
//!   indirectly — one per matching signature type, no early exit.
//! - [`LibraryGraph::deferred_import_exports_extension`] —
//!   `deferred_import_of_extension`
//!   (`error_verifier.dart:1339-1356` + `:4393-4405`): a deferred import
//!   (deferred flag **and** a prefix — the oracle probes
//!   `prefix?.isDeferred`) whose combinator-filtered namespace contains
//!   any extension element.
//! - [`LibraryGraph::shared_deferred_prefix_imports`] —
//!   `shared_deferred_prefix` (`error_verifier.dart:2371-2394`
//!   `_checkDeferredPrefixCollision`, reached from `:4411-4436`
//!   `_checkForDeferredPrefixCollisions`): imports grouped by their `as`
//!   prefix; a group of more than one import reports each deferred member
//!   at its `deferred` keyword. Part files carry no `import` directives,
//!   so a library's imports are exactly one compilation unit's — the
//!   per-library grouping here equals the oracle's per-unit grouping.
//!
//! The element-graph cycle codes (`recursive_interface_inheritance`,
//! `top_level_cycle`, `recursive_compile_time_constant`,
//! `recursive_constructor_redirect`) share the visited-set-bounded walks
//! in [`crate::cycles`]; their descriptors are in [`crate::codes`].
//!
//! # Determinism contract
//!
//! A directive namespace iterates in interned-name order (ascending
//! [`dartq_source::StringId`]). The oracle's namespace is an insertion-
//! ordered map (`scope.dart` builds declaration order + the link fixpoint
//! order), so when a directive has **several** conflicting names the two
//! orders may disagree on which name the single `ambiguous_export` report
//! names — the (severity, code, file) tuple, anchored at the directive, is
//! unaffected, and the order itself is a total function of the input
//! (intern ids are first-encounter order). Findings carry declaring
//! [`FileId`]s; the message URIs render through
//! [`LibraryGraph::library_of_file`], the graph's canonical file → library
//! answer.

use std::collections::hash_map::Entry;

use dartq_source::{FileId, StringId};
use rustc_hash::FxHashMap;

use crate::library_graph::{Combinators, ImportEdge, LibraryGraph, LibraryId};

/// Whether `uri` identifies an internal SDK library — the oracle's
/// `isInternalSdkLibrary` verbatim (`'$uri'.startsWith('dart:_')`,
/// `element.dart:7042-7044`). Canonical identity strings carry the scheme
/// prefix, so the prefix test is the scheme test.
#[must_use]
pub fn is_internal_sdk_library(uri: &str) -> bool {
    uri.starts_with("dart:_")
}

/// One `ambiguous_export` finding: directive `directive` exports `name` as
/// an element different from the one a previous export directive of the
/// same library already exported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AmbiguousExportFinding {
    /// The reporting directive's index into
    /// [`LibraryGraph::export_edges`] — the report anchor (the oracle
    /// reports `at(directive.uri)`).
    pub directive: usize,
    /// The conflicting name — the message's `{0}`.
    pub name: StringId,
    /// The previously exported element's declaring file — with its owning
    /// library, the message's `{1}`.
    pub first: FileId,
    /// This directive's element's declaring file — the message's `{2}`.
    pub second: FileId,
}

/// One `invalid_export_of_internal_element`(_indirectly) finding for a
/// single export directive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InternalExportFinding {
    /// The exported library itself is annotated `@internal` — `{0}` is the
    /// library's display name (the caller renders it from the target).
    Library,
    /// The namespace member `name` is annotated `@internal` — `{0}`.
    Member(StringId),
    /// An `@internal` type alias named `internal` appears in the signature
    /// of the exported member `exported` — the indirectly-exported shape
    /// (`{0}` the alias, `{1}` the member).
    Signature {
        /// The exported executable member's name.
        exported: StringId,
        /// The `@internal` alias named in its signature.
        internal: StringId,
    },
}

impl LibraryGraph {
    /// The `ambiguous_export` findings for `lib`'s export directives — one
    /// per directive whose combinator-filtered namespace exports a name a
    /// previous directive already exported as a different element.
    ///
    /// Mirrors `error_verifier.dart:2593-2621` exactly: the seen-map is
    /// library-wide and lives across directives in source order
    /// (`LibraryVerificationContext._exportedElements`, constructed per
    /// library, `library_analyzer.dart:106-113`); element identity is the
    /// declaring file (a name resolves to one declaration per file); the
    /// same element arriving again re-sets the slot silently (the else
    /// branch) while a **different** element reports once and ends that
    /// directive's contribution — names the conflicting directive would
    /// have exported after the conflicting name are never recorded.
    /// Directives without a resolved target contribute nothing
    /// (`visitExportDirective`'s `libraryExport != null` gate).
    ///
    /// The finding's `{1}`/`{2}` library URIs resolve through
    /// [`Self::library_of_file`] on `first`/`second`.
    pub fn ambiguous_exports(&self, lib: LibraryId) -> Vec<AmbiguousExportFinding> {
        let mut seen: FxHashMap<StringId, FileId> = FxHashMap::default();
        let mut findings = Vec::new();
        for (index, edge) in self.export_edges(lib).iter().enumerate() {
            let Some(target) = edge.target else {
                continue;
            };
            for (name, file) in self.directive_namespace(target, &edge.combinators) {
                match seen.entry(name) {
                    // A different element under the same name: report once
                    // and abandon the rest of this directive's namespace.
                    Entry::Occupied(occupied) if *occupied.get() != file => {
                        findings.push(AmbiguousExportFinding {
                            directive: index,
                            name,
                            first: *occupied.get(),
                            second: file,
                        });
                        break;
                    }
                    // The same element again (another path to it): silent
                    // dedup — the oracle's `_exportedElements[name] =
                    // element` re-set.
                    Entry::Occupied(_) => {}
                    Entry::Vacant(slot) => {
                        slot.insert(file);
                    }
                }
            }
        }
        findings
    }

    /// The `shared_deferred_prefix` import-directive indices for `lib` — the
    /// indices (into [`Self::import_edges`]) of every deferred import that
    /// shares its `as` prefix with at least one other import of the same
    /// library.
    ///
    /// Mirrors `error_verifier.dart:2371-2394` `_checkDeferredPrefixCollision`
    /// (reached from `:4411-4436` `_checkForDeferredPrefixCollisions`) per
    /// compilation unit: imports are grouped by their resolved prefix
    /// element — a library-scoped identity, so the interned `prefix` name is
    /// the group key — and a group of more than one import reports
    /// `shared_deferred_prefix` at the `deferred` keyword of **each**
    /// deferred member, in group-first-occurrence then directive order. An
    /// import without a prefix joins no group (the oracle's
    /// `directive.prefix != null` gate); a lone import under its prefix is
    /// silent (`count > 1` gate). A library's `import` directives can only
    /// live in its defining unit — part files carry just `part of` — so the
    /// per-library grouping here equals the oracle's per-unit grouping.
    ///
    /// The graph edge records deferredness as a flag but not the keyword's
    /// span; the caller anchors each finding at the directive's `deferred`
    /// keyword token (`diag.sharedDeferredPrefix.at(deferredToken)`),
    /// mapping the returned edge index back to its flattened import
    /// directive exactly as the export faces map their ordinals.
    #[must_use]
    pub fn shared_deferred_prefix_imports(&self, lib: LibraryId) -> Vec<usize> {
        // Group import indices by prefix, preserving first-occurrence group
        // order (`prefixToDirectivesMap` is an insertion-ordered map).
        let edges = self.import_edges(lib);
        let mut group_of: FxHashMap<StringId, usize> = FxHashMap::default();
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for (index, edge) in edges.iter().enumerate() {
            let Some(prefix) = edge.prefix else {
                continue;
            };
            match group_of.entry(prefix) {
                Entry::Occupied(occupied) => groups[*occupied.get()].push(index),
                Entry::Vacant(slot) => {
                    slot.insert(groups.len());
                    groups.push(vec![index]);
                }
            }
        }
        let mut findings = Vec::new();
        for members in groups {
            if members.len() <= 1 {
                continue;
            }
            for index in members {
                if edges[index].deferred {
                    findings.push(index);
                }
            }
        }
        findings
    }

    /// The `duplicate_export` directive indices for `lib`: every export
    /// directive that repeats, under the adjacent-pair rule, a previous
    /// export of the same library with an identical combinator shape.
    ///
    /// Mirrors `imports_verifier.dart:365-395` `_duplicates` over the
    /// export list: directives with resolved targets are stably sorted by
    /// the target library's URI (dartq compares [`LibraryId`]s — the graph
    /// interns one node per canonical URI, so identity is URI equality),
    /// then walked as adjacent pairs. A pair with the same target whose
    /// combinators are `areSyntacticallyIdenticalExceptUri`-equal
    /// (`ast.dart` — [`Combinators::written_shapes_equal`] is that walk
    /// over interned ids) reports the member that comes second in source
    /// order, at its URI. The walk advances unconditionally, so a run of
    /// `n` identical directives reports its 2nd … nth members and sorting
    /// can pair directives that are not source-adjacent — both upstream
    /// behaviors. Directives without a resolved target join no pair
    /// (`imports_verifier.dart:78-83`).
    pub fn duplicate_export_directives(&self, lib: LibraryId) -> Vec<usize> {
        let edges = self.export_edges(lib);
        let mut resolved: Vec<usize> = edges
            .iter()
            .enumerate()
            .filter_map(|(index, edge)| edge.target.is_some().then_some(index))
            .collect();
        if resolved.len() < 2 {
            return Vec::new();
        }
        resolved.sort_by_key(|&index| {
            // Key on the interned target id, not the URI text: node ids
            // are assigned in first-intern order, a total order of the
            // input, so the sort is deterministic — the pair *equality*
            // is the URI equality (one node per canonical URI).
            edges[index].target.expect("filtered to resolved targets")
        });
        let mut duplicates = Vec::new();
        let mut previous = resolved[0];
        for &next in &resolved[1..] {
            let same_target = edges[previous].target == edges[next].target;
            if same_target
                && edges[previous]
                    .combinators
                    .written_shapes_equal(&edges[next].combinators)
            {
                // The member that comes second in source order reports.
                duplicates.push(previous.max(next));
            }
            previous = next;
        }
        duplicates
    }

    /// The `export_internal_library` directive indices for `lib`: every
    /// export directive whose resolved target is an internal SDK library
    /// (`dart:_…`), reported while `lib` itself is not a `dart:`-scheme
    /// library.
    ///
    /// Mirrors `error_verifier.dart:4645-4674`: the `_isInSystemLibrary`
    /// suppression (`:320`, `uri.isScheme('dart')`) and the
    /// `isInternalSdkLibrary` target probe (`element.dart:7042-7044`).
    /// The message's `{0}` is the directive's written URI string — the
    /// caller renders it from the directive text.
    pub fn export_internal_library_directives(&self, lib: LibraryId) -> Vec<usize> {
        let exporter_is_sdk = self.uri(lib).starts_with("dart:");
        self.export_edges(lib)
            .iter()
            .enumerate()
            .filter(|(_, edge)| {
                !exporter_is_sdk
                    && edge
                        .target
                        .is_some_and(|target| is_internal_sdk_library(self.uri(target)))
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// The `invalid_export_of_internal_element`(_indirectly) findings for
    /// one export directive (`edge_index` into [`Self::export_edges`] of
    /// `lib`).
    ///
    /// Mirrors `best_practices_verifier.dart:1004-1050`
    /// `_checkForInternalExport` over caller-supplied `@internal` facts
    /// (the metadata faces live in the semantic layer): `in_public_api` is
    /// the exporter's `sourceIsInPublicApi` verdict (under the package's
    /// `lib/`, not under `lib/src/`, `workspace/pub.dart:543-570`) — false
    /// short-circuits to no findings; `target_library_internal` is the
    /// exported library's own `@internal` annotation (reports
    /// [`InternalExportFinding::Library`] and **falls through** to the
    /// member walk); `element_internal(name)` is the member's
    /// `element.isInternal` (`utilities/extensions/element.dart:99-107`);
    /// `signature_alias_internal(name)` lists the `@internal` type aliases
    /// in an executable member's signature, in signature order — parameter
    /// types, return type, then type-parameter bounds, one finding per
    /// matching type with no early exit. An `@internal` member reports
    /// directly and skips its signature walk (the closure's `return`). A
    /// directive without a resolved target yields nothing.
    pub fn invalid_internal_export_findings(
        &self,
        lib: LibraryId,
        edge_index: usize,
        in_public_api: bool,
        target_library_internal: bool,
        element_internal: impl Fn(StringId) -> bool,
        signature_alias_internal: impl Fn(StringId) -> Vec<StringId>,
    ) -> Vec<InternalExportFinding> {
        if !in_public_api {
            return Vec::new();
        }
        let edge = &self.export_edges(lib)[edge_index];
        let Some(target) = edge.target else {
            return Vec::new();
        };
        let mut findings = Vec::new();
        if target_library_internal {
            findings.push(InternalExportFinding::Library);
        }
        for (name, _) in self.directive_namespace(target, &edge.combinators) {
            if element_internal(name) {
                findings.push(InternalExportFinding::Member(name));
                continue;
            }
            for internal in signature_alias_internal(name) {
                findings.push(InternalExportFinding::Signature {
                    exported: name,
                    internal,
                });
            }
        }
        findings
    }

    /// Whether `edge`'s import namespace contains an extension that
    /// survives the combinators — the `deferred_import_of_extension`
    /// decision.
    ///
    /// Mirrors `error_verifier.dart:1339-1356` + `:4393-4405`: gated on
    /// the import carrying a **deferred prefix** (`prefix?.isDeferred` —
    /// the deferred flag alone, without a prefix, never gates), then any
    /// `ExtensionElement` in the combinator-filtered namespace reports
    /// once at the directive's URI. Extension-kind facts key on the
    /// declaration identity, so a re-exported extension answers through
    /// the chain ([`Self::is_extension`]). Unnamed extensions carry no
    /// namespace name (`scope.dart:135-186` skips empty names) and can
    /// never fire this face.
    #[must_use]
    pub fn deferred_import_exports_extension(&self, edge: &ImportEdge) -> bool {
        if !edge.deferred || edge.prefix.is_none() {
            return false;
        }
        let Some(target) = edge.target else {
            return false;
        };
        self.directive_namespace(target, &edge.combinators)
            .into_iter()
            .any(|(name, file)| self.is_extension(file, name))
    }

    /// One directive's namespace: the target's export scope filtered by
    /// the directive's combinators — `createExportNamespaceForDirective2`
    /// (`scope.dart:99-110`: `_getExportMapping` then
    /// `_applyCombinators` in clause order) — as `(name, declaring file)`
    /// pairs in interned-name order (the module's determinism contract).
    fn directive_namespace(
        &self,
        target: LibraryId,
        combinators: &Combinators,
    ) -> Vec<(StringId, FileId)> {
        let mut names: Vec<(StringId, FileId)> = self
            .export_scope(target)
            .filter(|(name, _)| combinators.allows(*name))
            .map(|(name, entry)| (name, entry.file))
            .collect();
        names.sort_unstable_by_key(|&(name, _)| name);
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_graph::{CombinatorClause, ExportEdge};
    use dartq_source::{FileTable, StringTable};
    use std::path::Path;

    /// The shared rig: leaked string table, file table, builder, and
    /// shorthand registration for the shapes the export-graph faces read.
    struct Rig {
        strings: &'static StringTable,
        files: FileTable,
        builder: crate::library_graph::LibraryGraphBuilder<'static>,
    }

    impl Rig {
        fn new() -> Self {
            let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
            Self {
                strings,
                files: FileTable::new(),
                builder: crate::library_graph::LibraryGraphBuilder::new(strings),
            }
        }

        fn file(&mut self, name: &str) -> FileId {
            self.files.intern(Path::new(name))
        }

        fn name(&mut self, text: &str) -> StringId {
            self.strings.intern(text)
        }

        fn declare(&mut self, lib: LibraryId, name: &str, file: FileId) {
            let id = self.name(name);
            self.builder.declare(lib, id, file);
        }

        fn export(&mut self, lib: LibraryId, target: Option<LibraryId>) {
            self.builder.export(
                lib,
                ExportEdge {
                    target,
                    combinators: Combinators::unfiltered(),
                },
            );
        }

        fn export_hidden(&mut self, lib: LibraryId, target: LibraryId, hidden: &str) {
            let hidden = self.name(hidden);
            self.builder.export(
                lib,
                ExportEdge {
                    target: Some(target),
                    combinators: Combinators::new(
                        [CombinatorClause::Hide([hidden].into())],
                        self.strings,
                    ),
                },
            );
        }

        fn deferred_import(&mut self, lib: LibraryId, target: Option<LibraryId>) {
            let prefix = self.name("p");
            self.builder.import(
                lib,
                ImportEdge {
                    target,
                    prefix: Some(prefix),
                    deferred: true,
                    non_library: false,
                    combinators: Combinators::unfiltered(),
                },
            );
        }

        /// One import edge with an explicit prefix spelling and deferred flag
        /// (`None` prefix = a bare import).
        fn import(
            &mut self,
            lib: LibraryId,
            target: Option<LibraryId>,
            prefix: Option<&str>,
            deferred: bool,
        ) {
            let prefix = prefix.map(|p| self.name(p));
            self.builder.import(
                lib,
                ImportEdge {
                    target,
                    prefix,
                    deferred,
                    non_library: false,
                    combinators: Combinators::unfiltered(),
                },
            );
        }
    }

    // ---- ambiguous_export ----

    /// Two libraries export different elements under one name: the second
    /// directive reports once, carrying both declaring files.
    #[test]
    fn ambiguous_two_libraries_same_name_reports_second_directive() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let b = rig.builder.intern("package:x/b.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let file_a = rig.file("a.dart");
        let file_b = rig.file("b.dart");
        rig.declare(a, "Foo", file_a);
        rig.declare(b, "Foo", file_b);
        rig.export(app, Some(a));
        rig.export(app, Some(b));
        let foo = rig.name("Foo");
        let graph = rig.builder.finish();

        let findings = graph.ambiguous_exports(app);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].directive, 1);
        assert_eq!(findings[0].name, foo);
        assert_eq!(findings[0].first, file_a);
        assert_eq!(findings[0].second, file_b);
    }

    /// The same element arriving through two paths dedupes silently — the
    /// oracle's re-set else branch, never an ambiguity.
    #[test]
    fn ambiguous_same_element_through_two_paths_stays_silent() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let relay = rig.builder.intern("package:x/relay.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let file_a = rig.file("a.dart");
        rig.declare(a, "Foo", file_a);
        rig.export(relay, Some(a));
        rig.export(app, Some(a));
        rig.export(app, Some(relay));
        let graph = rig.builder.finish();

        assert!(graph.ambiguous_exports(app).is_empty());
    }

    /// A conflicting directive stops recording its remaining names: `b`'s
    /// second name never enters the seen-map, so a later directive
    /// exporting a different element under that name stays silent — a
    /// would-be conflict with the abandoned name never fires.
    #[test]
    fn ambiguous_conflicting_directive_stops_recording() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let b = rig.builder.intern("package:x/b.dart");
        let other = rig.builder.intern("package:x/other.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let file_a = rig.file("a.dart");
        let file_b = rig.file("b.dart");
        let file_other = rig.file("other.dart");
        // `a` exports only Alpha; `b` exports Alpha and Beta, `other` only
        // Beta — so nothing but the abandoned name can pair with `other`.
        rig.declare(a, "Alpha", file_a);
        rig.declare(b, "Alpha", file_b);
        rig.declare(b, "Beta", file_b);
        rig.declare(other, "Beta", file_other);
        rig.export(app, Some(a));
        rig.export(app, Some(b));
        // If the conflicting directive had recorded Beta(file_b), this
        // directive's Beta(file_other) would report. It must not.
        rig.export(app, Some(other));
        let alpha = rig.name("Alpha");
        let graph = rig.builder.finish();

        let findings = graph.ambiguous_exports(app);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].directive, 1);
        assert_eq!(findings[0].name, alpha);
    }

    /// The seen-map is fed by export directives only: the exporting
    /// library's own local declaration under the same name never
    /// conflicts (the oracle's map holds directive-namespace elements).
    #[test]
    fn ambiguous_locals_do_not_participate() {
        let mut rig = Rig::new();
        let b = rig.builder.intern("package:x/b.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let file_app = rig.file("app.dart");
        let file_b = rig.file("b.dart");
        rig.declare(app, "Foo", file_app);
        rig.declare(b, "Foo", file_b);
        rig.export(app, Some(b));
        let graph = rig.builder.finish();

        assert!(graph.ambiguous_exports(app).is_empty());
    }

    /// A `hide` clause that removes the conflicting name from the
    /// directive's namespace suppresses the report — combinator order is
    /// upstream's `_applyCombinators`.
    #[test]
    fn ambiguous_hidden_name_never_conflicts() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let b = rig.builder.intern("package:x/b.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let file_a = rig.file("a.dart");
        let file_b = rig.file("b.dart");
        rig.declare(a, "Foo", file_a);
        rig.declare(b, "Foo", file_b);
        rig.export(app, Some(a));
        rig.export_hidden(app, b, "Foo");
        let graph = rig.builder.finish();

        assert!(graph.ambiguous_exports(app).is_empty());
    }

    /// First-come-wins direction: after the first directive's element is
    /// recorded, every further conflicting directive reports against the
    /// FIRST element, not against each other.
    #[test]
    fn ambiguous_conflicts_measure_against_the_first_flow() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let b = rig.builder.intern("package:x/b.dart");
        let c = rig.builder.intern("package:x/c.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let file_a = rig.file("a.dart");
        let file_b = rig.file("b.dart");
        let file_c = rig.file("c.dart");
        for (lib, file) in [(a, file_a), (b, file_b), (c, file_c)] {
            rig.declare(lib, "Foo", file);
        }
        rig.export(app, Some(a));
        rig.export(app, Some(b));
        rig.export(app, Some(c));
        let graph = rig.builder.finish();

        let findings = graph.ambiguous_exports(app);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].directive, 1);
        assert_eq!(findings[1].directive, 2);
        assert!(findings.iter().all(|found| found.first == file_a));
    }

    /// Directives without a resolved target contribute no names and join
    /// no conflict.
    #[test]
    fn ambiguous_unresolved_targets_are_skipped() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let file_a = rig.file("a.dart");
        rig.declare(a, "Foo", file_a);
        rig.export(app, Some(a));
        rig.export(app, None);
        let graph = rig.builder.finish();

        assert!(graph.ambiguous_exports(app).is_empty());
    }

    // ---- duplicate_export ----

    /// Identical directives report the member that comes second in source
    /// order — detected across a non-adjacent interleave via the URI sort.
    #[test]
    fn duplicate_exports_report_second_of_identical_pair() {
        let mut rig = Rig::new();
        let x = rig.builder.intern("package:x/x.dart");
        let y = rig.builder.intern("package:x/y.dart");
        let app = rig.builder.intern("package:x/app.dart");
        rig.export(app, Some(x));
        rig.export(app, Some(y));
        rig.export(app, Some(x));
        let graph = rig.builder.finish();

        assert_eq!(graph.duplicate_export_directives(app), [2]);
    }

    /// A run of three identical directives reports its 2nd and 3rd members
    /// — the adjacent-pair walk advances unconditionally.
    #[test]
    fn duplicate_run_of_three_reports_second_and_third() {
        let mut rig = Rig::new();
        let x = rig.builder.intern("package:x/x.dart");
        let app = rig.builder.intern("package:x/app.dart");
        rig.export(app, Some(x));
        rig.export(app, Some(x));
        rig.export(app, Some(x));
        let graph = rig.builder.finish();

        assert_eq!(graph.duplicate_export_directives(app), [1, 2]);
    }

    /// Different combinator shapes — including the clause-structure split
    /// (`show a, b` vs `show a show b`) — and different targets stay
    /// silent; unresolved targets join no pair.
    #[test]
    fn duplicate_distinct_shapes_targets_and_unresolved_stay_silent() {
        let mut rig = Rig::new();
        let x = rig.builder.intern("package:x/x.dart");
        let y = rig.builder.intern("package:x/y.dart");
        let app = rig.builder.intern("package:x/app.dart");
        rig.export(app, Some(x));
        // Different target.
        rig.export(app, Some(y));
        // Same target, different shape: `show a, b` (one clause).
        let names: [StringId; 2] = [rig.name("a"), rig.name("b")];
        rig.builder.export(
            app,
            ExportEdge {
                target: Some(x),
                combinators: Combinators::new(
                    [CombinatorClause::Show(names.into())],
                    rig.strings,
                ),
            },
        );
        // Same target, same names regrouped into two clauses.
        rig.builder.export(
            app,
            ExportEdge {
                target: Some(x),
                combinators: Combinators::new(
                    [
                        CombinatorClause::Show([names[0]].into()),
                        CombinatorClause::Show([names[1]].into()),
                    ],
                    rig.strings,
                ),
            },
        );
        // Unresolved targets never pair.
        rig.export(app, None);
        rig.export(app, None);
        let graph = rig.builder.finish();

        assert!(graph.duplicate_export_directives(app).is_empty());
    }

    // ---- export_internal_library ----

    /// The gate: internal SDK target (`dart:_…`) reports from a
    /// non-`dart:` exporter; a public `dart:` target stays silent; an SDK
    /// exporter is suppressed; unresolved targets are skipped.
    #[test]
    fn export_internal_library_gate() {
        let mut rig = Rig::new();
        let internal = rig.builder.intern("dart:_internal");
        let core = rig.builder.intern("dart:core");
        let app = rig.builder.intern("package:x/app.dart");
        rig.export(app, Some(internal));
        rig.export(app, Some(core));
        rig.export(app, None);
        rig.export(core, Some(internal));
        let graph = rig.builder.finish();

        assert_eq!(graph.export_internal_library_directives(app), [0]);
        assert!(graph.export_internal_library_directives(core).is_empty());
    }

    /// The URI predicate itself: `dart:_` prefix is internal, `dart:`
    /// public and package/file URIs are not.
    #[test]
    fn internal_sdk_library_predicate() {
        assert!(is_internal_sdk_library("dart:_internal"));
        assert!(is_internal_sdk_library("dart:_js_types"));
        assert!(!is_internal_sdk_library("dart:core"));
        assert!(!is_internal_sdk_library("package:x/x.dart"));
        assert!(!is_internal_sdk_library("file:///tmp/x.dart"));
        assert!(!is_internal_sdk_library("darts:core"));
    }

    // ---- invalid_export_of_internal_element ----

    /// Out of the public API the whole face is silent; inside it the
    /// layers report in order — library annotation first (falling through
    /// to the member walk), `@internal` members directly (skipping their
    /// signature walk), and one indirect finding per `@internal` signature
    /// alias.
    #[test]
    fn internal_export_layers_report_in_order() {
        let mut rig = Rig::new();
        let b = rig.builder.intern("package:x/b.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let file_b = rig.file("b.dart");
        rig.declare(b, "Api", file_b);
        rig.declare(b, "Plain", file_b);
        rig.declare(b, "Wrapped", file_b);
        rig.export(app, Some(b));
        let api = rig.name("Api");
        let wrapped = rig.name("Wrapped");
        let alias = rig.name("Alias");
        let graph = rig.builder.finish();

        // Out of the public API: nothing, whatever the facts.
        assert!(graph
            .invalid_internal_export_findings(app, 0, false, true, |_| true, |_| {
                vec![alias]
            })
            .is_empty());

        let findings = graph.invalid_internal_export_findings(
            app,
            0,
            true,
            true,
            |name| name == api,
            |name| (name == wrapped).then(|| vec![alias, alias]).unwrap_or_default(),
        );
        assert_eq!(
            findings,
            vec![
                InternalExportFinding::Library,
                InternalExportFinding::Member(api),
                InternalExportFinding::Signature {
                    exported: wrapped,
                    internal: alias,
                },
                InternalExportFinding::Signature {
                    exported: wrapped,
                    internal: alias,
                },
            ]
        );
    }

    /// An `@internal` member skips its signature walk — no indirect
    /// finding rides a direct one.
    #[test]
    fn internal_export_member_skips_signature_walk() {
        let mut rig = Rig::new();
        let b = rig.builder.intern("package:x/b.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let file_b = rig.file("b.dart");
        rig.declare(b, "Marked", file_b);
        rig.export(app, Some(b));
        let marked = rig.name("Marked");
        let alias = rig.name("Alias");
        let graph = rig.builder.finish();

        let findings = graph.invalid_internal_export_findings(
            app,
            0,
            true,
            false,
            |name| name == marked,
            |_| vec![alias],
        );
        assert_eq!(findings, vec![InternalExportFinding::Member(marked)]);
    }

    /// A directive without a resolved target yields nothing even inside
    /// the public API.
    #[test]
    fn internal_export_unresolved_target_yields_nothing() {
        let mut rig = Rig::new();
        let app = rig.builder.intern("package:x/app.dart");
        rig.export(app, None);
        let any = rig.name("A");
        let graph = rig.builder.finish();

        assert!(graph
            .invalid_internal_export_findings(app, 0, true, true, |_| true, |_| {
                vec![any]
            })
            .is_empty());
    }

    // ---- deferred_import_of_extension ----

    /// The gate: deferred + prefix, and any extension surviving the
    /// combinators — including one arriving through a re-export chain.
    #[test]
    fn deferred_import_extension_gate() {
        let mut rig = Rig::new();
        let ext = rig.builder.intern("package:x/ext.dart");
        let relay = rig.builder.intern("package:x/relay.dart");
        let plain = rig.builder.intern("package:x/plain.dart");
        let app = rig.builder.intern("package:x/app.dart");
        let ext_file = rig.file("ext.dart");
        let plain_file = rig.file("plain.dart");
        let on_int = rig.name("OnInt");
        rig.builder.declare_extension(ext, on_int, ext_file);
        rig.declare(plain, "Helper", plain_file);
        rig.export(relay, Some(ext));
        let prefix = rig.name("p");
        let graph = rig.builder.finish();

        let hit = ImportEdge {
            target: Some(ext),
            prefix: Some(prefix),
            deferred: true,
            non_library: false,
            combinators: Combinators::unfiltered(),
        };
        assert!(graph.deferred_import_exports_extension(&hit));
        // Through a re-export chain the entry still points at the
        // declaring file, so the kind answers at a distance.
        let chained = ImportEdge {
            target: Some(relay),
            ..hit.clone()
        };
        assert!(graph.deferred_import_exports_extension(&chained));
        // Hidden by the combinators: no namespace face carries the
        // extension.
        let hidden = ImportEdge {
            combinators: Combinators::new(
                [CombinatorClause::Hide([on_int].into())],
                rig.strings,
            ),
            ..hit.clone()
        };
        assert!(!graph.deferred_import_exports_extension(&hidden));
        // A target with no extension at all.
        let plain_hit = ImportEdge {
            target: Some(plain),
            ..hit.clone()
        };
        assert!(!graph.deferred_import_exports_extension(&plain_hit));
        // Deferred without a prefix never gates (the oracle probes the
        // prefix element's isDeferred).
        let unprefixed = ImportEdge {
            prefix: None,
            ..hit.clone()
        };
        assert!(!graph.deferred_import_exports_extension(&unprefixed));
        // A plain deferred import of a non-deferred shape: not deferred.
        let eager = ImportEdge {
            deferred: false,
            ..hit
        };
        assert!(!graph.deferred_import_exports_extension(&eager));
    }

    /// An unresolved target contributes no namespace, deferred or not.
    #[test]
    fn deferred_import_unresolved_target_is_false() {
        let mut rig = Rig::new();
        let app = rig.builder.intern("package:x/app.dart");
        let prefix = rig.name("p");
        let graph = rig.builder.finish();
        let edge = ImportEdge {
            target: None,
            prefix: Some(prefix),
            deferred: true,
            non_library: false,
            combinators: Combinators::unfiltered(),
        };
        assert!(!graph.deferred_import_exports_extension(&edge));
    }

    // ---- shared_deferred_prefix (ticket #2389 double-zero pins) ----

    /// An eager and a deferred import sharing one prefix report the deferred
    /// one at its index — the `_checkForDeferredPrefixCollisions` gate.
    #[test]
    fn ticket_2389_shared_deferred_prefix_reports_the_deferred_member() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let b = rig.builder.intern("package:x/b.dart");
        let app = rig.builder.intern("package:x/app.dart");
        rig.import(app, Some(a), Some("p"), false);
        rig.import(app, Some(b), Some("p"), true);
        let graph = rig.builder.finish();

        assert_eq!(graph.shared_deferred_prefix_imports(app), vec![1]);
    }

    /// A lone import under its prefix is silent (`count > 1` gate), deferred
    /// or not.
    #[test]
    fn ticket_2389_shared_deferred_prefix_single_import_is_silent() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let app = rig.builder.intern("package:x/app.dart");
        rig.import(app, Some(a), Some("p"), true);
        let graph = rig.builder.finish();

        assert!(graph.shared_deferred_prefix_imports(app).is_empty());
    }

    /// Two eager imports under one prefix are silent — the collision is about
    /// a *deferred* import sharing its prefix, not a plain duplicate.
    #[test]
    fn ticket_2389_shared_deferred_prefix_two_eager_imports_are_silent() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let b = rig.builder.intern("package:x/b.dart");
        let app = rig.builder.intern("package:x/app.dart");
        rig.import(app, Some(a), Some("p"), false);
        rig.import(app, Some(b), Some("p"), false);
        let graph = rig.builder.finish();

        assert!(graph.shared_deferred_prefix_imports(app).is_empty());
    }

    /// Deferred imports under distinct prefixes never collide.
    #[test]
    fn ticket_2389_shared_deferred_prefix_distinct_prefixes_are_silent() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let b = rig.builder.intern("package:x/b.dart");
        let app = rig.builder.intern("package:x/app.dart");
        rig.import(app, Some(a), Some("p"), true);
        rig.import(app, Some(b), Some("q"), true);
        let graph = rig.builder.finish();

        assert!(graph.shared_deferred_prefix_imports(app).is_empty());
    }

    /// Every deferred member of a colliding group reports, in source order.
    #[test]
    fn ticket_2389_shared_deferred_prefix_multiple_deferred_in_group_all_report() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let b = rig.builder.intern("package:x/b.dart");
        let c = rig.builder.intern("package:x/c.dart");
        let app = rig.builder.intern("package:x/app.dart");
        rig.import(app, Some(a), Some("p"), true);
        rig.import(app, Some(b), Some("p"), false);
        rig.import(app, Some(c), Some("p"), true);
        let graph = rig.builder.finish();

        assert_eq!(graph.shared_deferred_prefix_imports(app), vec![0, 2]);
    }

    /// A prefixless deferred import joins no group (the oracle gates on
    /// `directive.prefix != null`), so a sibling `as p` import stays a lone
    /// member and is silent.
    #[test]
    fn ticket_2389_shared_deferred_prefix_unprefixed_import_joins_no_group() {
        let mut rig = Rig::new();
        let a = rig.builder.intern("package:x/a.dart");
        let b = rig.builder.intern("package:x/b.dart");
        let app = rig.builder.intern("package:x/app.dart");
        rig.import(app, Some(a), None, true);
        rig.import(app, Some(b), Some("p"), false);
        let graph = rig.builder.finish();

        assert!(graph.shared_deferred_prefix_imports(app).is_empty());
    }
}
