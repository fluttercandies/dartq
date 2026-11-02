//! #3626 — the `conflicting_generic_interfaces` face.
//!
//! The official check (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/error_verifier.dart`
//! `_checkForConflictingGenerics` :3848-3878) delegates every class-family
//! declaration — class, named mixin application, mixin, enum, extension
//! type; augmentation fragments report on the original declaration only —
//! to `ClassHierarchy.errors` (`pkg/analyzer/lib/src/dart/element/
//! class_hierarchy.dart`): the declaration's interface set is collected by
//! appending the supertype, a mixin's `on` constraints, the `implements`
//! faces, then the `with` faces — each face itself plus the referenced
//! element's own merged interfaces substituted through the face's type
//! arguments (a raw generic face instantiates to bounds) — and every
//! repeated generic element merges through
//! `_ClassInterfaceType.update`: the first occurrence stands, and from the
//! second distinct one on, NORM of both sides feeds `NNBD_TOP_MERGE`
//! (`top_merge.dart`, `normalize.dart`). A merge that cannot proceed
//! (different class elements, a one-sided `?`, structurally different
//! shapes) is the error; `InvalidType` absorbs everything silently. One
//! error per element, in collection order, with the running merged type
//! first and the incoming written type second; the row anchors at the
//! declaration's name token with the official kind word.
//!
//! Name-keyed mirror: faces are scanned from this file's tokens (the same
//! faithful-parse posture `overrides.rs`'s clause scanner keeps — prefixed
//! heads keep their prefix in the element key, an unparseable argument
//! shape degrades to [`Ty::Unknown`], the never-conflicting `InvalidType`),
//! resolution rides `Engine::resolve_clause_face` (a typedef alias — this
//! file's or an imported library's — expands through its right-hand side,
//! substituting the alias's parameters by the written arguments, and the
//! expanded head resolves in the alias's own library so a private RHS
//! keys and renders as the element it names), and the transitive walk
//! recurses into same-file targets only — a foreign container's own
//! clause arguments are invisible to this file's scanner, so its chain
//! stays uncollected (the under-report direction: the direct face still
//! participates, never a fabricated conflict). An arity-mismatched
//! instantiation (a resolution error upstream, `InvalidType` there) stays
//! out of the merge for the same reason.

use super::*;
use crate::model::ArgListId;

/// The clause a face belongs to. The declaration order mirrors the
/// official `_getHierarchy` append order — supertype, mixin `on`
/// constraints, `implements`, `with` — which fixes both the merged-map
/// insertion order (the first-error-wins element) and the message's
/// operand order (probed 3.13.7: `with M0 implements I<int>` reports
/// `'I<int>'` before `'I<dynamic>'`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FaceClause {
    /// `extends S` or a named mixin application's head type.
    Extends,
    /// A mixin's `on` constraint.
    On,
    /// An `implements` face.
    Implements,
    /// A `with` face.
    With,
}

impl FaceClause {
    /// The official processing order.
    const ORDER: [FaceClause; 4] = [
        FaceClause::Extends,
        FaceClause::On,
        FaceClause::Implements,
        FaceClause::With,
    ];
}

/// One scanned clause face: the head's bare-leaf spelling
/// ([`StringId::NONE`] for the `dynamic`/`void` keyword heads), the
/// import prefix when the face was spelled `p.C`, and the written type
/// (arguments interned).
#[derive(Clone, Debug)]
struct Face {
    clause: FaceClause,
    prefix: Option<StringId>,
    head: StringId,
    ty: Ty,
}

/// One collected interface instantiation: the element key (prefix, head)
/// and the written-or-substituted type.
type Iface = ((Option<StringId>, StringId), Ty);

/// The per-element merge state (`_ClassInterfaceType`): the first
/// occurrence raw, the running NORM-merged result from the second
/// distinct occurrence, and the first failure's operand pair.
#[derive(Default)]
struct MergeCell {
    single: Option<Ty>,
    current: Option<Ty>,
    error: Option<(Ty, Ty)>,
}

/// The substitution-chain walk bound (a hierarchy can nest arbitrarily;
/// pathological depth is broken code reporting elsewhere).
const MAX_WALK_DEPTH: u32 = 24;

/// The alias-chain follow bound (`typedef A = B; typedef B = C; …`).
const MAX_ALIAS_HOPS: usize = 8;

impl<'a> Engine<'a> {
    /// The #3626 whole-unit pass: every class-family declaration this file
    /// declares runs the interface-conflict walk. Runs beside
    /// [`Engine::verify_enum_member_set`] — every container is closed and
    /// the rows stage into the same source-ordered usage tail.
    pub(crate) fn verify_generic_interface_conflicts(&mut self) {
        let rows: Vec<(SourceRange, &'static str, String, String, String)> = {
            let faces_by_name = self.scan_conflict_headers();
            // One check per uniquely-named container, at the FIRST
            // declaration site — the official reports on the original
            // declaration, and the model collapses augmentation fragments
            // into one container whose clause faces the scan unions across
            // every same-name header.
            let mut seen: FxHashSet<StringId> = FxHashSet::default();
            let sites: Vec<(StringId, SourceRange)> = self
                .decl_sites
                .iter()
                .filter(|site| matches!(site.kind, DeclKind::Container))
                .filter(|site| seen.insert(site.name))
                .map(|site| (site.name, site.range))
                .collect();
            let mut walk = IfaceWalk {
                eng: self,
                faces_by_name,
                merged: FxHashMap::default(),
                visiting: FxHashSet::default(),
                object: self.intern("Object"),
                future: self.intern("Future"),
                future_or: self.intern("FutureOr"),
                never: self.intern("Never"),
                null: self.intern("Null"),
            };
            let mut rows = Vec::new();
            for (name, name_range) in sites {
                let Some(container) = self
                    .index
                    .get(self.file)
                    .and_then(|unit| unit.container(name))
                else {
                    continue;
                };
                let kind = match container.kind {
                    ContainerKind::Class => "class",
                    ContainerKind::Mixin => "mixin",
                    ContainerKind::Enum => "enum",
                    ContainerKind::ExtensionType => "extension type",
                    // Extensions never join the class-family visit.
                    ContainerKind::Extension => continue,
                };
                let Some(faces) = walk.faces_by_name.get(&name).cloned() else {
                    continue;
                };
                let display = self.strings.resolve(name).to_owned();
                for (first, second) in walk.declaration_conflicts(name, &faces) {
                    let pool = walk.eng.boundary.pool();
                    let t1 = first.render(walk.eng.strings, &pool);
                    let t2 = second.render(walk.eng.strings, &pool);
                    drop(pool);
                    rows.push((name_range, kind, display.clone(), t1, t2));
                }
            }
            rows
        };
        let code = SemanticCode::ConflictingGenericInterfaces;
        for (range, kind, name, t1, t2) in rows {
            let correction = code.correction_message(kind);
            self.report.usage.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message4(kind, &name, &t1, &t2),
                self.file,
                range,
                correction,
            ));
        }
    }

    // —— the header scan ——————————————————————————————————————————————

    /// Scans this file's class-family headers into per-name clause faces
    /// (source order; augmentation fragments append). A header that does
    /// not parse faithfully contributes no faces.
    fn scan_conflict_headers(&self) -> FxHashMap<StringId, Vec<Face>> {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut out: FxHashMap<StringId, Vec<Face>> = FxHashMap::default();
        let mut i = 0;
        while i < toks.len() {
            match lex_at(i) {
                Some("class") | Some("enum") => {
                    i = self.scan_one_conflict_header(i + 1, &mut out);
                }
                // A `mixin class` header scans from its `class` keyword.
                Some("mixin") if lex_at(i + 1) != Some("class") => {
                    i = self.scan_one_conflict_header(i + 1, &mut out);
                }
                // `extension type E(...)`: the representation group sits
                // between the name and the clauses.
                Some("extension") if lex_at(i + 1) == Some("type") => {
                    i = self.scan_one_conflict_header(i + 2, &mut out);
                }
                _ => i += 1,
            }
        }
        out
    }

    /// Parses one declaration header starting at the name: the optional
    /// type-parameter window, an extension type's representation group,
    /// then the clause window until the body opens. Returns the scan
    /// position past the header (a `{` body skipped whole); a header that
    /// does not parse faithfully contributes nothing.
    fn scan_one_conflict_header(
        &self,
        start: usize,
        out: &mut FxHashMap<StringId, Vec<Face>>,
    ) -> usize {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut i = start;
        let Some(name) = self.conflict_intern(i) else {
            return i + 1;
        };
        i += 1;
        // The type-parameter window: balanced `<...>`, contents skipped
        // (bounds never reach the clause window).
        if lex_at(i) == Some("<") {
            match self.angle_end(i) {
                Some(end) => i = end,
                None => return self.skip_conflict_body(i),
            }
        }
        // An extension type's representation formal group.
        if lex_at(i) == Some("(") {
            match self.tokens.matching_end(i) {
                Some(end) => i = end + 1,
                None => return self.skip_conflict_body(i),
            }
        }
        // The clause window.
        let mut faces: Vec<Face> = Vec::new();
        let mut faithful = true;
        while let Some(lex) = lex_at(i) {
            let clause = match lex {
                "{" | ";" => break,
                // A named mixin application's head type.
                "=" | "extends" => FaceClause::Extends,
                "on" => FaceClause::On,
                "implements" => FaceClause::Implements,
                "with" => FaceClause::With,
                _ => {
                    faithful = false;
                    break;
                }
            };
            i = self.scan_conflict_faces(i + 1, clause, &mut faces, &mut faithful);
            if !faithful {
                break;
            }
        }
        if faithful {
            out.entry(name).or_default().extend(faces);
        }
        self.skip_conflict_body(i)
    }

    /// Parses one clause's comma-separated face list into `faces`. Sets
    /// `faithful = false` on a shape the scan cannot parse; the caller
    /// drops the whole header then.
    fn scan_conflict_faces(
        &self,
        start: usize,
        clause: FaceClause,
        faces: &mut Vec<Face>,
        faithful: &mut bool,
    ) -> usize {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut i = start;
        loop {
            match lex_at(i) {
                Some("dynamic") => {
                    faces.push(Face {
                        clause,
                        prefix: None,
                        head: StringId::NONE,
                        ty: Ty::Dynamic,
                    });
                    i += 1;
                }
                Some("void") => {
                    faces.push(Face {
                        clause,
                        prefix: None,
                        head: StringId::NONE,
                        ty: Ty::Void,
                    });
                    i += 1;
                }
                Some(_) => match self.scan_conflict_face(i, clause, faces) {
                    Some(next) => i = next,
                    None => {
                        *faithful = false;
                        return i;
                    }
                },
                None => {
                    *faithful = false;
                    return i;
                }
            }
            match lex_at(i) {
                Some(",") => i += 1,
                _ => return i,
            }
        }
    }

    /// Parses one face — `Head`, `p.Head`, `Head<...>`, `Head?` — pushing
    /// it and returning the position past it; `None` marks the header
    /// unfaithful (a non-identifier head).
    fn scan_conflict_face(
        &self,
        start: usize,
        clause: FaceClause,
        faces: &mut Vec<Face>,
    ) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut i = start;
        let first = self.conflict_intern(i)?;
        i += 1;
        // A prefixed face (`p.C`): the prefix keys the element, the local
        // leaf names it — the same bare-leaf convention the element model
        // keeps.
        let (prefix, head) = if lex_at(i) == Some(".") && self.conflict_intern(i + 1).is_some() {
            let local = self.conflict_intern(i + 1)?;
            i += 2;
            (Some(first), local)
        } else {
            (None, first)
        };
        let mut args: Vec<Ty> = Vec::new();
        if lex_at(i) == Some("<") {
            i = self.scan_conflict_args(i, &mut args)?;
        }
        let nullable = lex_at(i) == Some("?");
        if nullable {
            i += 1;
        }
        let ty = Ty::Named {
            name: head,
            nullable,
            args: if args.is_empty() {
                ArgListId::EMPTY
            } else {
                self.boundary.intern_list(&args)
            },
        };
        faces.push(Face {
            clause,
            prefix,
            head,
            ty,
        });
        Some(i)
    }

    /// Parses a balanced `<...>` type-argument group. A plain argument
    /// recurses (`A<List<int>>`); an argument the clause grammar cannot
    /// parse (a function tail, a record group) degrades to
    /// [`Ty::Unknown`] — the oracle's `InvalidType`, which never
    /// conflicts — by skipping its extent.
    fn scan_conflict_args(&self, start: usize, args: &mut Vec<Ty>) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut i = start;
        if lex_at(i) != Some("<") {
            return None;
        }
        i += 1;
        loop {
            let ty = match lex_at(i)? {
                "dynamic" => {
                    i += 1;
                    Ty::Dynamic
                }
                "void" => {
                    i += 1;
                    Ty::Void
                }
                _ => {
                    let id = self.conflict_intern(i)?;
                    i += 1;
                    let mut nested: Vec<Ty> = Vec::new();
                    if lex_at(i) == Some("<") {
                        i = self.scan_conflict_args(i, &mut nested)?;
                    }
                    let nullable = lex_at(i) == Some("?");
                    if nullable {
                        i += 1;
                    }
                    Ty::Named {
                        name: id,
                        nullable,
                        args: if nested.is_empty() {
                            ArgListId::EMPTY
                        } else {
                            self.boundary.intern_list(&nested)
                        },
                    }
                }
            };
            args.push(ty);
            match lex_at(i)? {
                "," => i += 1,
                ">" => return Some(i + 1),
                // A function-typed or record-typed argument: skip its
                // extent and degrade it — the group stays collectable.
                _ => {
                    i = self.skip_conflict_arg(i)?;
                    args.push(Ty::Unknown);
                    match lex_at(i)? {
                        "," => i += 1,
                        ">" => return Some(i + 1),
                        _ => return None,
                    }
                }
            }
        }
    }

    /// Skips one unparsable type argument (a function tail, a record
    /// group): nested angle and bracket groups whole, stopping at the
    /// `,` or the closing `>` of the enclosing group.
    fn skip_conflict_arg(&self, start: usize) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut i = start;
        let mut depth = 0_usize;
        while i < toks.len() {
            match lex_at(i) {
                Some("<") => depth += 1,
                Some(">") => {
                    if depth == 0 {
                        return Some(i);
                    }
                    depth -= 1;
                }
                Some("(") | Some("[") | Some("{") => {
                    i = self.tokens.matching_end(i)?;
                }
                Some(",") if depth == 0 => return Some(i),
                Some(";") => return None,
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// The position past the balanced `<...>` group open at `i`.
    fn angle_end(&self, start: usize) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut depth = 0_usize;
        let mut i = start;
        while i < toks.len() {
            match lex_at(i) {
                Some("<") => depth += 1,
                Some(">") => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                Some(";") | Some("{") => return None,
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Skips past the header's terminator: a `{` body whole, or the `;`.
    fn skip_conflict_body(&self, start: usize) -> usize {
        let toks = self.tokens.as_slice();
        let lex_at = |i: usize| toks.get(i).map(|_| self.tokens.lexeme(self.source, i));
        let mut i = start;
        while i < toks.len() {
            match lex_at(i) {
                Some("{") => {
                    return self.tokens.matching_end(i).map_or(toks.len(), |close| close + 1)
                }
                Some(";") => return i + 1,
                _ => i += 1,
            }
        }
        toks.len()
    }

    /// The token at `i` interned as an identifier spelling, `None` for
    /// anything the clause grammar cannot treat as a head.
    fn conflict_intern(&self, i: usize) -> Option<StringId> {
        let lexeme = self.tokens.lexeme(self.source, i);
        let first = lexeme.as_bytes().first()?;
        if first.is_ascii_alphabetic() || *first == b'_' || *first == b'$' {
            Some(self.intern(lexeme))
        } else {
            None
        }
    }
}

/// The interface-collection walk: faces per declared name, the memoized
/// merged-interface list per same-file container, and the interned
/// dart:async/core spellings the merge table keys on.
struct IfaceWalk<'e, 'a> {
    eng: &'e Engine<'a>,
    faces_by_name: FxHashMap<StringId, Vec<Face>>,
    merged: FxHashMap<StringId, Option<Vec<Iface>>>,
    visiting: FxHashSet<StringId>,
    object: StringId,
    future: StringId,
    future_or: StringId,
    never: StringId,
    null: StringId,
}

impl<'e, 'a> IfaceWalk<'e, 'a> {
    /// The declaration's conflicts: one operand pair per conflicting
    /// element, in collection order.
    fn declaration_conflicts(&mut self, name: StringId, faces: &[Face]) -> Vec<(Ty, Ty)> {
        let mut cells: Vec<((Option<StringId>, StringId), MergeCell)> = Vec::new();
        self.collect_faces(name, faces, 0, &mut cells);
        cells
            .into_iter()
            .filter_map(|(_, cell)| cell.error)
            .collect()
    }

    /// Appends every face in the official clause order.
    fn collect_faces(
        &mut self,
        name: StringId,
        faces: &[Face],
        depth: u32,
        cells: &mut Vec<((Option<StringId>, StringId), MergeCell)>,
    ) {
        for clause in FaceClause::ORDER {
            for face in faces.iter().filter(|face| face.clause == clause) {
                self.append_face(name, face, depth, cells);
            }
        }
    }

    /// Appends one face the official way: the entry itself, then the
    /// target's merged interfaces substituted through the face's type
    /// arguments.
    fn append_face(
        &mut self,
        writer: StringId,
        face: &Face,
        depth: u32,
        cells: &mut Vec<((Option<StringId>, StringId), MergeCell)>,
    ) {
        if depth > MAX_WALK_DEPTH {
            return;
        }
        // The entry and the recursion target: a type alias expands through
        // its right-hand side (substituting the alias's own parameters by
        // the written arguments), anything else stands as written. The
        // expansion tracks the owner of the last alias followed: the
        // right-hand head resolves in the alias's OWN library — a private
        // RHS (`typedef Public<T> = _Private<T>`) is invisible from the
        // importer's scope, and the official entry is the element itself
        // (`_Private<int>`), never the alias spelling. A prefixed face
        // stays on the container path: `p.X` never names a local alias.
        let mut prefix = face.prefix;
        let mut entry_ty = face.ty;
        let mut target_head = face.head;
        let mut scope: Option<FileId> = None;
        if face.prefix.is_none()
            && let Some((owner, alias)) = self.alias_decl(self.eng.file, face.head)
        {
            entry_ty = self.expand_alias(face.ty, owner, &alias);
            scope = Some(owner);
            for _ in 0..MAX_ALIAS_HOPS {
                let Ty::Named { name, .. } = entry_ty else {
                    break;
                };
                if name == face.head {
                    break;
                }
                let Some((next_owner, next)) = self.alias_decl(owner, name) else {
                    break;
                };
                entry_ty = self.expand_alias(entry_ty, next_owner, &next);
                scope = Some(next_owner);
            }
            match entry_ty {
                Ty::Named { name, .. } => {
                    target_head = name;
                    prefix = None;
                }
                _ => return,
            }
        }
        if target_head == StringId::NONE {
            return;
        }
        let resolved = match scope {
            Some(owner) if owner != self.eng.file => resolve_container(
                self.eng.graph,
                self.eng.namespaces,
                self.eng.index,
                self.scope_lib(owner),
                owner,
                target_head,
            ),
            _ => self
                .eng
                .resolve_clause_face(writer, self.eng.file, self.eng.lib, target_head),
        };
        let Some((target, target_file)) = resolved else {
            // No element: the oracle's `InvalidType` interface — it never
            // conflicts.
            return;
        };
        let params = target.type_param_names.to_vec();
        let mut args = self.ty_args(entry_ty);
        // A raw generic face instantiates to bounds (the element model's
        // own instantiation; an absent bound reads `dynamic`, the Dart
        // reading). An arity-mismatched instantiation is a resolution
        // error upstream — `InvalidType`, never a merge participant.
        if args.is_empty() && !params.is_empty() {
            args = self.bounds_of(target, target_file);
            entry_ty = Ty::Named {
                name: target_head,
                nullable: false,
                args: self.eng.boundary.intern_list(&args),
            };
        }
        // An arity-mismatched instantiation (including arguments on a
        // non-generic element) is `InvalidType` upstream: no entry and no
        // chain. A non-generic element's own instantiation is the merge
        // identity — no entry — but its transitive interfaces still
        // collect (the chain is where `class A implements I<int>` reaches
        // the declaration that extends `A`).
        if args.len() != params.len() {
            return;
        }
        if !params.is_empty() {
            self.merge_entry(cells, (prefix, target_head), entry_ty);
        }
        // The transitive interfaces: same-file targets only (a foreign
        // container's clause arguments are invisible to this scanner); a
        // generic target whose parameter spellings the model does not
        // carry cannot substitute — its chain stays uncollected rather
        // than leaking stale parameter leaves.
        if target_file == self.eng.file
            && !(target.type_params > 0 && params.is_empty())
            && let Some(interfaces) = self.merged_interfaces(target_head, depth)
        {
            for ((ikey_prefix, ikey_head), ty) in interfaces {
                let ty = self.substitute(ty, &params, &args);
                self.merge_entry(cells, (ikey_prefix, ikey_head), ty);
            }
        }
    }

    /// The type-alias declaration `name` resolves to from `scope_file`'s
    /// scope (its own declarations first, then its library namespace —
    /// the resolution order a clause use walks), with the owning file;
    /// `None` for anything but an alias with a captured right-hand side.
    fn alias_decl(&self, scope_file: FileId, name: StringId) -> Option<(FileId, TopDecl)> {
        let index = self.eng.index;
        let (owner, decl) = match lookup_scoped(
            self.eng.graph,
            self.eng.namespaces,
            index,
            self.scope_lib(scope_file),
            scope_file,
            name,
        ) {
            ScopedHit::Local(decl) => (scope_file, decl),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => (file, index.get(file)?.decl(name)?),
            _ => return None,
        };
        (decl.kind == MicroKind::TypeAlias && decl.var_ty != Ty::UNKNOWN)
            .then(|| (owner, decl.clone()))
    }

    /// The library `file` belongs to (the diagnosing library for the
    /// diagnosing file).
    fn scope_lib(&self, file: FileId) -> LibraryId {
        if file == self.eng.file {
            self.eng.lib
        } else {
            self.eng.graph.library_of_file(file).unwrap_or(self.eng.lib)
        }
    }

    /// One alias expansion: the alias's right-hand side with its
    /// parameters instantiated by the face's written arguments (or the
    /// alias's bounds when the face is raw). The alias's own types are
    /// slots of `owner`'s pool and project into the diagnosing universe
    /// before anything reads them.
    fn expand_alias(&self, face_ty: Ty, owner: FileId, alias: &TopDecl) -> Ty {
        let mut args = self.ty_args(face_ty);
        if args.is_empty() && !alias.type_param_names.is_empty() {
            args = if alias.type_param_bounds.len() == alias.type_param_names.len() {
                alias
                    .type_param_bounds
                    .iter()
                    .map(|bound| self.eng.boundary.ty(owner, *bound))
                    .collect()
            } else {
                vec![Ty::Dynamic; alias.type_param_names.len()]
            };
        }
        let rhs = self.eng.boundary.ty(owner, alias.var_ty);
        self.substitute(rhs, &alias.type_param_names, &args)
    }

    /// The target's instantiate-to-bounds argument list (`dynamic` for an
    /// absent bound, the Dart reading), projected out of `owner`'s pool.
    fn bounds_of(&self, target: &Container, owner: FileId) -> Vec<Ty> {
        if target.type_param_bounds.len() == target.type_param_names.len() {
            target
                .type_param_bounds
                .iter()
                .map(|bound| self.eng.boundary.ty(owner, *bound))
                .collect()
        } else {
            vec![Ty::Dynamic; target.type_param_names.len()]
        }
    }

    /// The merged-interface list of a same-file container, memoized —
    /// the official `implementedInterfaces(element)`.
    fn merged_interfaces(&mut self, name: StringId, depth: u32) -> Option<Vec<Iface>> {
        if let Some(hit) = self.merged.get(&name) {
            return hit.clone();
        }
        if !self.visiting.insert(name) {
            // A hierarchy cycle: broken code reporting elsewhere — never
            // cached (the outer computation owns the real result).
            return None;
        }
        let computed = match self.faces_by_name.get(&name).cloned() {
            Some(faces) if depth < MAX_WALK_DEPTH => {
                let mut cells: Vec<((Option<StringId>, StringId), MergeCell)> = Vec::new();
                self.collect_faces(name, &faces, depth + 1, &mut cells);
                Some(
                    cells
                        .into_iter()
                        .map(|(key, cell)| {
                            (key, cell.current.or(cell.single).unwrap_or(Ty::Unknown))
                        })
                        .collect::<Vec<Iface>>(),
                )
            }
            _ => None,
        };
        self.visiting.remove(&name);
        self.merged.insert(name, computed.clone());
        computed
    }

    /// Merges one interface instantiation into its element's cell
    /// (`_ClassInterfaceType.update`).
    fn merge_entry(
        &mut self,
        cells: &mut Vec<((Option<StringId>, StringId), MergeCell)>,
        key: (Option<StringId>, StringId),
        ty: Ty,
    ) {
        let index = match cells.iter().position(|(k, _)| *k == key) {
            Some(index) => index,
            None => {
                cells.push((key, MergeCell::default()));
                cells.len() - 1
            }
        };
        self.update_cell(&mut cells[index].1, ty);
    }

    fn update_cell(&mut self, cell: &mut MergeCell, ty: Ty) {
        if cell.error.is_some() {
            return;
        }
        if cell.current.is_none() {
            match cell.single {
                None => {
                    cell.single = Some(ty);
                    return;
                }
                Some(single) if single == ty => return,
                Some(single) => cell.current = Some(self.norm(single)),
            }
        }
        let Some(current) = cell.current else {
            return;
        };
        match self.top_merge(current, self.norm(ty)) {
            Some(merged) => cell.current = Some(merged),
            None => cell.error = Some((current, ty)),
        }
    }

    /// The written type arguments of a named `Ty` (empty when none).
    fn ty_args(&self, ty: Ty) -> Vec<Ty> {
        if let Ty::Named { args, .. } = ty
            && args != ArgListId::EMPTY
        {
            let pool = self.eng.boundary.pool();
            return pool.get(args).to_vec();
        }
        Vec::new()
    }

    /// Substitutes `params` by `args` inside `ty` (the official
    /// `Substitution.fromInterfaceType`). A record- or function-shaped
    /// slot that mentions a parameter degrades to [`Ty::Unknown`] — the
    /// shape cannot re-intern through the diagnosing pool.
    fn substitute(&self, ty: Ty, params: &[StringId], args: &[Ty]) -> Ty {
        if params.is_empty() {
            return ty;
        }
        match ty {
            Ty::Named {
                name,
                nullable,
                args: list,
            } => {
                if let Some(hit) = params.iter().position(|p| *p == name) {
                    return args.get(hit).copied().unwrap_or(Ty::Unknown);
                }
                if list == ArgListId::EMPTY {
                    return ty;
                }
                // The pool borrow must end before the recursion: a nested
                // generic argument re-interns through `intern_list`, which
                // borrows the same pool mutably.
                let written = self.eng.boundary.pool().get(list).to_vec();
                let substituted: Vec<Ty> = written
                    .into_iter()
                    .map(|arg| self.substitute(arg, params, args))
                    .collect();
                Ty::Named {
                    name,
                    nullable,
                    args: self.eng.boundary.intern_list(&substituted),
                }
            }
            Ty::Record { .. } | Ty::Function { .. } => {
                let mut mentions = false;
                self.eng.boundary.pool().visit_named(ty, &mut |named| {
                    if let Ty::Named { name, .. } = named
                        && params.contains(&name)
                    {
                        mentions = true;
                    }
                });
                if mentions {
                    Ty::Unknown
                } else {
                    ty
                }
            }
            _ => ty,
        }
    }

    // —— NORM (`normalize.dart`) ——————————————————————————————————————

    /// The canonical representation (`normalizeInterfaceType` for the
    /// interface-shaped domain this face meets).
    fn norm(&self, ty: Ty) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable: true,
                args,
            } => {
                // NORM(T?): let S be NORM(T); a top stays, `Never?` and
                // `Null?` are `Null`, a nullable-argument `FutureOr`
                // stays, else `S?`.
                let stripped = self.norm(Ty::Named {
                    name,
                    nullable: false,
                    args,
                });
                if self.is_top(stripped) {
                    return stripped;
                }
                if self.is_never(stripped) || self.is_null(stripped) {
                    return self.bare(self.null);
                }
                if let Ty::Named {
                    name: inner_name,
                    nullable: false,
                    args: inner_args,
                } = stripped
                    && inner_name == self.future_or
                    && inner_args != ArgListId::EMPTY
                {
                    let argument = {
                        let pool = self.eng.boundary.pool();
                        pool.get(inner_args).first().copied()
                    };
                    if argument.is_some_and(|argument| self.is_nullable(argument)) {
                        return stripped;
                    }
                }
                self.flag_nullable(stripped, true)
            }
            Ty::Named {
                name,
                nullable: false,
                args,
            } => {
                // NORM(FutureOr<T>): a top argument collapses to itself,
                // `Object` to itself, `Never` to `Future<Never>`, `Null`
                // to `Future<Null>?`, else `FutureOr<NORM(T)>`.
                if name == self.future_or && args != ArgListId::EMPTY {
                    if let Some(argument) = {
                        let pool = self.eng.boundary.pool();
                        pool.get(args).first().copied()
                    } {
                        let s = self.norm(argument);
                        if self.is_top(s) || s == self.bare(self.object) {
                            return s;
                        }
                        if self.is_never(s) {
                            return Ty::Named {
                                name: self.future,
                                nullable: false,
                                args: self.eng.boundary.intern_list(&[s]),
                            };
                        }
                        if self.is_null(s) {
                            return Ty::Named {
                                name: self.future,
                                nullable: true,
                                args: self.eng.boundary.intern_list(&[s]),
                            };
                        }
                        if s != argument {
                            return Ty::Named {
                                name,
                                nullable: false,
                                args: self.eng.boundary.intern_list(&[s]),
                            };
                        }
                        return ty;
                    }
                }
                // NORM(C<T0..Tn>) = C<NORM(Ti)>.
                if args != ArgListId::EMPTY {
                    let normalized: Vec<Ty> = {
                        let pool = self.eng.boundary.pool();
                        let list = pool.get(args).to_vec();
                        drop(pool);
                        list.iter().map(|arg| self.norm(*arg)).collect()
                    };
                    let interned = self.eng.boundary.intern_list(&normalized);
                    if interned != args {
                        return Ty::Named {
                            name,
                            nullable: false,
                            args: interned,
                        };
                    }
                }
                ty
            }
            // Record and function shapes ride as written — the documented
            // degradation (they cannot re-intern here; nothing in this
            // face's corpus reaches them).
            _ => ty,
        }
    }

    /// A non-nullable bare `name` type (`Object` as written).
    fn bare(&self, name: StringId) -> Ty {
        Ty::Named {
            name,
            nullable: false,
            args: ArgListId::EMPTY,
        }
    }

    /// TOP(T) (`type_system.dart`'s `isTop`): `dynamic`, `void`,
    /// `InvalidType`, `T?` over a top or `Object`, `FutureOr<T>` over a
    /// top.
    fn is_top(&self, ty: Ty) -> bool {
        match ty {
            Ty::Unknown | Ty::Dynamic | Ty::Void => true,
            Ty::Null | Ty::Never => false,
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if nullable {
                    return name == self.object
                        || self.is_top(Ty::Named {
                            name,
                            nullable: false,
                            args,
                        });
                }
                if name == self.future_or && args != ArgListId::EMPTY {
                    let argument = {
                        let pool = self.eng.boundary.pool();
                        pool.get(args).first().copied()
                    };
                    return argument.is_some_and(|argument| self.is_top(argument));
                }
                false
            }
            Ty::Record { .. } | Ty::Function { .. } => false,
        }
    }

    /// Whether the type is nullable (`dynamic`, `Null`, a `?`-flagged
    /// shape).
    fn is_nullable(&self, ty: Ty) -> bool {
        match ty {
            Ty::Dynamic | Ty::Null => true,
            Ty::Named { nullable, .. } => nullable || self.is_null(ty),
            Ty::Record { nullable, .. } | Ty::Function { nullable, .. } => nullable,
            _ => false,
        }
    }

    /// `Never` as written (the bottom type rides the clause scanner as a
    /// named leaf; the variant form answers too).
    fn is_never(&self, ty: Ty) -> bool {
        matches!(ty, Ty::Never) || ty == self.bare(self.never)
    }

    /// `Null` as written (the same two forms).
    fn is_null(&self, ty: Ty) -> bool {
        matches!(ty, Ty::Null) || ty == self.bare(self.null)
    }

    /// Re-flags a named type's `?` (identity for the other shapes).
    fn flag_nullable(&self, ty: Ty, nullable: bool) -> Ty {
        match ty {
            Ty::Named {
                name,
                args,
                nullable: _,
            } => Ty::Named {
                name,
                nullable,
                args,
            },
            other => other,
        }
    }

    // —— NNBD_TOP_MERGE (`top_merge.dart`) ————————————————————————————

    /// Merges two canonical types; `None` is the conflict. `InvalidType`
    /// absorbs, the top-type table decides the keyword pairs, nullability
    /// flags merge only pairwise, and same-element instantiations merge
    /// pairwise.
    fn top_merge(&self, t: Ty, s: Ty) -> Option<Ty> {
        // InvalidType absorbs everything.
        if matches!(t, Ty::Unknown) || matches!(s, Ty::Unknown) {
            return Some(Ty::Unknown);
        }
        match (t, s) {
            (Ty::Dynamic, Ty::Dynamic) => Some(Ty::Dynamic),
            (Ty::Void, Ty::Void) => Some(Ty::Void),
            (Ty::Never, Ty::Never) => Some(Ty::Never),
            (Ty::Null, Ty::Null) => Some(Ty::Null),
            // NNBD_TOP_MERGE(dynamic, void) = Object?; the same Object?
            // answers for the void and dynamic pairs either way.
            (Ty::Dynamic, Ty::Void) | (Ty::Void, Ty::Dynamic) => Some(self.object_question()),
            (a, b)
                if self.is_object_question(a) && matches!(b, Ty::Void | Ty::Dynamic)
                    || self.is_object_question(b) && matches!(a, Ty::Void | Ty::Dynamic) =>
            {
                Some(self.object_question())
            }
            (Ty::Named { .. }, Ty::Named { .. }) => self.top_merge_named(t, s),
            // The bracket shapes: structural identity merges, any
            // difference is undefined (the corpus never reaches them; the
            // structural arm is the documented under-report posture).
            (Ty::Record { .. }, Ty::Record { .. }) | (Ty::Function { .. }, Ty::Function { .. }) => {
                (t == s).then_some(t)
            }
            // One `?` only, or structurally different shapes: undefined.
            _ => None,
        }
    }

    /// `Object?` as a named type.
    fn object_question(&self) -> Ty {
        self.flag_nullable(self.bare(self.object), true)
    }

    /// Whether the type is exactly `Object?`.
    fn is_object_question(&self, ty: Ty) -> bool {
        matches!(ty, Ty::Named { name, nullable: true, args: ArgListId::EMPTY } if name == self.object)
    }

    /// The named arm: nullability flags merge pairwise (one-sided is the
    /// conflict), then same-element instantiations merge pairwise.
    fn top_merge_named(&self, t: Ty, s: Ty) -> Option<Ty> {
        let (t_name, t_nullable, t_args) = named_parts(t)?;
        let (s_name, s_nullable, s_args) = named_parts(s)?;
        match (t_nullable, s_nullable) {
            (true, true) => {
                let merged = self.top_merge_named_parts(t_name, t_args, s_name, s_args)?;
                Some(self.flag_nullable(merged, true))
            }
            (false, false) => self.top_merge_named_parts(t_name, t_args, s_name, s_args),
            // NNBD_TOP_MERGE(T?, T) is undefined.
            (true, false) | (false, true) => None,
        }
    }

    /// Same-element instantiations: element identity first (the name-keyed
    /// mirror of the element check), then pairwise arguments — an empty
    /// argument list (a non-generic element) is the identity.
    fn top_merge_named_parts(
        &self,
        t_name: StringId,
        t_args: ArgListId,
        s_name: StringId,
        s_args: ArgListId,
    ) -> Option<Ty> {
        if t_name != s_name {
            return None;
        }
        if t_args == ArgListId::EMPTY {
            return Some(Ty::Named {
                name: t_name,
                nullable: false,
                args: t_args,
            });
        }
        let (t_list, s_list) = {
            let pool = self.eng.boundary.pool();
            (pool.get(t_args).to_vec(), pool.get(s_args).to_vec())
        };
        if t_list.len() != s_list.len() {
            return None;
        }
        let mut merged = Vec::with_capacity(t_list.len());
        for (t_arg, s_arg) in t_list.iter().zip(s_list.iter()) {
            merged.push(self.top_merge(*t_arg, *s_arg)?);
        }
        Some(Ty::Named {
            name: t_name,
            nullable: false,
            args: self.eng.boundary.intern_list(&merged),
        })
    }
}

/// The named parts of a [`Ty::Named`].
fn named_parts(ty: Ty) -> Option<(StringId, bool, ArgListId)> {
    match ty {
        Ty::Named {
            name,
            nullable,
            args,
        } => Some((name, nullable, args)),
        _ => None,
    }
}
