//! The per-file API signature (ticket #282): the canonical hash
//! the L2 strong-incremental gate compares. A file whose signature is
//! unchanged across an edit exports the same element face, so its library's
//! importers keep their semantic products — the cascade stops at the edited
//! file (dartq-engine `l1::close` consumes the comparison).
//!
//! # The hashed face (adjudicated composition F, #302)
//!
//! Everything in [`FileElements`] except the capture-time diagnostic logs
//! ([`FileElements::duplicates`], [`FileElements::arity_violations`] — those
//! are per-file diagnostics, recomputed with the file's own facts and never
//! read cross-file):
//!
//! * every top-level declaration (kind, name, type-parameter count, declared
//!   variable/return type, full formal tables; [`MicroKind::TypeParam`] and
//!   [`MicroKind::Prefix`] entries are skipped — declaration-scoped, not a
//!   library face, matching the library-graph feeding rule);
//! * every named container: kind, type-parameter count, `sealed`, superclass
//!   name, the `with`/`on`/`implements` clause lists, enum constants **in
//!   declaration order** (the
//!   exhaustiveness witness order is semantic), the declaration's
//!   annotations (spelled name, argument-list source text, position), and
//!   every member with its full executable shape;
//! * the merged extension-member pool.
//!
//! The engine folds the directive face (the raw directive-region lexemes —
//! import/export/part shapes, prefixes, combinators, library and part-of
//! names) on top, so an export-combinator or membership edit also flips the
//! gate.
//!
//! # Canonicality
//!
//! Declaration maps are hash maps, so every name set is rendered sorted by
//! **resolved name text** (interning order differs between the incremental
//! and full arms — sorting by [`StringId`] would fork the hash). Ordered
//! faces keep their semantic order: positional formals and named formals
//! keep declaration order (positional order is the call-site contract), and
//! enum constants keep declaration order. The rendering writes into one
//! scratch buffer and hashes once — no intermediate strings, no String-keyed
//! maps; the engine's standing memory law).
//!
//! [`SIGNATURE_VERSION`] rides the hash: any change to the rendering or the
//! composition bumps it, invalidating every signature computed by a previous
//! engine build (the semantic-version discipline — the engine's
//! `FACTS_SEMANTIC_VERSION` folds `SIGNATURE_VERSION` into its low half, so
//! cached facts invalidate automatically; no separate engine-side bump).

use blake3::Hasher;
use dartq_source::{StringId, StringTable};

use crate::model::{
    Annotation, ArgPool, Container, ContainerKind, FileElements, Formal, Formals, MemberEntry,
    MemberKind, MicroKind, TopDecl, Ty,
};

/// The signature-composition version: bumped with any change to
/// what [`FileElements::api_signature`] hashes. v1: the initial composition-F
/// face (#282). v2: constructor selectors + the unnamed flag per container,
/// and member type-parameter names (#350 — the constructor-undefined faces).
/// v3: the header-residue names per container and the extension faces with
/// their `on` types (#491 S1 + #476 — the clause-attribution fix and the
/// extension-application faces changed the hashed face). v4: the explicit
/// type arguments of every declared type (#701 — an argument edit is an
/// element-face edit; arguments hash as content in declaration order, so
/// interning order stays irrelevant). v5: the annotation argument-list
/// source text per declaration annotation (#1083 — an argument-only edit is
/// an element-face edit; the text folds as interned content, so interning
/// order stays irrelevant). v6: the member name-token span (#1867 — the
/// context-row anchor face: a move-only member edit flips the gate, so the
/// dependents' cross-file context rows re-anchor) and the #1637 face
/// extension (the formal covariant bit and default-value text, plus the
/// member annotation star — a modifier-, default-, or annotation-only edit
/// is an element-face edit). v7: declared types carry the function-shape
/// variant (`Ty::Function`) — a formal, field, or return type written as a
/// function type hashes its columns (return, formals, parameter count)
/// instead of capturing `Unknown`, so a function-shape-only edit is an
/// element-face edit. v8: the container's const-constructor bit (a
/// `const`-only constructor edit flips the cross-file const-creation
/// faces of every dependent).
pub const SIGNATURE_VERSION: u32 = 8;

/// The 128-bit blake3 digest of a file's canonical element face — the same
/// width family as [`dartq_source::ContentId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ApiSignature([u8; 16]);

impl ApiSignature {
    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Restores the digest a persisted facts entry carries baked (#341):
    /// the loaded signature must be byte-true, or the first post-load edit
    /// would compare against a forged baseline and the L2 gate could
    /// wrongly skip a cascade.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The first 128 bits of a blake3 digest — the `ContentId` truncation.
    fn truncate(digest: &blake3::Hash) -> Self {
        let mut id = [0; 16];
        id.copy_from_slice(&digest.as_bytes()[..16]);
        Self(id)
    }
}

impl FileElements {
    /// The file's canonical API signature at the current
    /// [`SIGNATURE_VERSION`].
    #[must_use]
    pub fn api_signature(&self, strings: &StringTable) -> ApiSignature {
        self.api_signature_with_version(strings, SIGNATURE_VERSION)
    }

    /// The signature at an explicit version — the test seam for the
    /// bump-invalidates-everything discipline.
    #[must_use]
    pub fn api_signature_with_version(&self, strings: &StringTable, version: u32) -> ApiSignature {
        let mut face = Face::new(strings, &self.type_args);
        face.field("sig-v");
        face.uint(u64::from(version));
        face.top_decls(self);
        face.containers(self);
        face.extension_faces(self);
        ApiSignature::truncate(&blake3::hash(&face.buf))
    }

    /// The member-level signature digests (#1561): per library-face name,
    /// the blake3-128 of the exact canonical slice
    /// [`FileElements::api_signature`] renders for that name — a named
    /// container's full face (kind, modifiers, clauses, constructor face,
    /// every member shape, the annotations), or a top-level declaration's
    /// face. Every slice sorts its name sets by resolved text, so a digest
    /// never depends on iteration or interning order, and the entries come
    /// back in map order — the caller orders the pairs as its contract
    /// needs. The slices are content plus the v6 name-token anchor face
    /// (#1867): a member-bearing container's digest tracks its members'
    /// name spans, so a move-only member edit flips it; top-level entries
    /// carry no span and stay position-free. Unnamed extension faces carry
    /// no library-face name; their content rides the file-level digest
    /// alone.
    ///
    /// Derived on demand — one extra face render per unit, one digest per
    /// entry, hashed straight from the scratch buffer. Never persisted: the
    /// resident facts re-derive it in microseconds, so a stored copy would
    /// be redundant state.
    #[must_use]
    pub fn member_digests(&self, strings: &StringTable) -> Vec<(StringId, ApiSignature)> {
        let mut face = Face::new(strings, &self.type_args);
        let mut digests = Vec::with_capacity(self.decls.len() + self.containers.len());
        for (name, decl) in &self.decls {
            // The capture-time filter (the top_decls posture): prefixes and
            // type parameters are declaration-scoped, never a library face.
            if matches!(decl.kind, MicroKind::Prefix | MicroKind::TypeParam) {
                continue;
            }
            face.buf.clear();
            face.top_decl(*name, decl);
            digests.push((*name, ApiSignature::truncate(&blake3::hash(&face.buf))));
        }
        for (name, container) in &self.containers {
            face.buf.clear();
            face.container(*name, container);
            digests.push((*name, ApiSignature::truncate(&blake3::hash(&face.buf))));
        }
        digests
    }

    /// The engine-side composition: the element face folded with the raw
    /// directive-region lexemes (`directive_face` — the caller's capture of
    /// its directive walk; opaque bytes here). Directives ride the signature
    /// because an export-combinator, prefix, or membership edit changes the
    /// file's contribution to its importers' namespaces even when the
    /// element face stands.
    #[must_use]
    pub fn api_signature_with_directives(
        &self,
        strings: &StringTable,
        directive_face: &[u8],
    ) -> ApiSignature {
        let element_face = self.api_signature(strings);
        let mut hasher = Hasher::new();
        hasher.update(element_face.as_bytes());
        hasher.update(directive_face);
        ApiSignature::truncate(&hasher.finalize())
    }
}

/// The canonical rendering buffer: newline-separated tagged fields, written
/// once and hashed once.
struct Face<'a> {
    buf: Vec<u8>,
    /// Sorted-name scratch, outer level (top-level declarations, containers,
    /// the extension pool).
    names: Vec<StringId>,
    /// Sorted-name scratch, nested level (container members).
    inner: Vec<StringId>,
    strings: &'a StringTable,
    /// The unit's argument pool — explicit type arguments hash as their
    /// interned content (never as pool slots, which carry interning order).
    args: &'a ArgPool,
}

impl<'a> Face<'a> {
    fn new(strings: &'a StringTable, args: &'a ArgPool) -> Self {
        Self {
            buf: Vec::with_capacity(256),
            names: Vec::new(),
            inner: Vec::new(),
            strings,
            args,
        }
    }

    fn field(&mut self, tag: &str) {
        self.buf.push(b'\n');
        self.buf.extend_from_slice(tag.as_bytes());
    }

    fn uint(&mut self, value: u64) {
        self.field("n");
        let mut buffer = [0u8; 20];
        let mut cursor = buffer.len();
        let mut rest = value;
        loop {
            cursor -= 1;
            buffer[cursor] = b'0' + (rest % 10) as u8;
            rest /= 10;
            if rest == 0 {
                break;
            }
        }
        self.buf.extend_from_slice(&buffer[cursor..]);
    }

    fn usize(&mut self, value: usize) {
        self.uint(u64::try_from(value).expect("count exceeds u64"));
    }

    /// The resolved text of `name`, as a hash field.
    fn name(&mut self, name: StringId) {
        self.field(self.strings.resolve(name));
    }

    /// Fills `scratch` with `names` sorted by resolved text — the canonical
    /// iteration order that makes interning order irrelevant. The two
    /// scratch lists keep the nested container/member loops from clobbering
    /// each other's iteration state; the pieces are passed disjointly so a
    /// `&mut self` receiver cannot alias the scratch field.
    fn sorted(
        strings: &StringTable,
        scratch: &mut Vec<StringId>,
        names: impl Iterator<Item = StringId>,
    ) {
        scratch.clear();
        scratch.extend(names);
        scratch.sort_unstable_by(|a, b| strings.resolve(*a).cmp(strings.resolve(*b)));
    }

    fn ty(&mut self, ty: Ty) {
        match ty {
            Ty::Unknown => self.field("?"),
            Ty::Null => self.field("Null"),
            Ty::Dynamic => self.field("dynamic"),
            Ty::Never => self.field("Never"),
            Ty::Void => self.field("void"),
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                self.field(self.strings.resolve(name));
                for arg in self.args.get(args) {
                    self.ty(*arg);
                }
                if nullable {
                    self.field("?");
                }
            }
            Ty::Record { shape, nullable } => {
                // The interned shape is canonical (named fields sorted at
                // intern), so the field walk itself is the identity — no
                // re-sort, no slot numbers.
                self.field("record");
                let shape = self.args.record_shape(shape);
                for ty in shape.positional.iter() {
                    self.ty(*ty);
                }
                for (name, ty) in shape.named.iter() {
                    self.name(*name);
                    self.ty(*ty);
                }
                if nullable {
                    self.field("?");
                }
            }
            Ty::Function { shape, nullable } => {
                // The interned shape is canonical (named formals sorted at
                // intern), so the field walk itself is the identity — no
                // re-sort, no slot numbers. The type-parameter count rides
                // (a generic function type is a distinct shape even where
                // its columns coincide).
                self.field("function");
                let shape = self.args.function_shape(shape);
                self.usize(shape.type_params as usize);
                self.ty(shape.return_ty);
                for ty in shape.positional.iter() {
                    self.ty(*ty);
                }
                for (name, ty) in shape.named.iter() {
                    self.name(*name);
                    self.ty(*ty);
                }
                if nullable {
                    self.field("?");
                }
            }
        }
    }

    fn formals(&mut self, formals: &Formals) {
        self.usize(formals.positional.len());
        self.usize(formals.named.len());
        for formal in formals.positional.iter().chain(&formals.named) {
            self.formal(formal);
        }
    }

    fn formal(&mut self, formal: &Formal) {
        self.name(formal.name);
        self.ty(formal.ty);
        self.field(if formal.required { "req" } else { "opt" });
        self.field(if formal.covariant {
            "covariant"
        } else {
            "plain"
        });
        match formal.default_value {
            Some(text) => self.name(text),
            None => self.field("no-default"),
        }
    }

    /// One annotation star (a container's or a member's), written order:
    /// the interned name, the argument group's interned text, and the row's
    /// span. An annotation-only edit moves the meta-getter / override
    /// verdicts, so it must flip the L2 gate.
    fn annotations(&mut self, annotations: &[Annotation]) {
        self.usize(annotations.len());
        for annotation in annotations {
            self.name(annotation.name);
            match annotation.args_text {
                Some(text) => self.name(text),
                None => self.field("no-args"),
            }
            self.uint(u64::from(annotation.range.offset));
            self.uint(u64::from(annotation.range.length));
        }
    }

    fn member(&mut self, tag: &str, name: StringId, entry: &MemberEntry) {
        self.field(tag);
        self.name(name);
        self.field(match entry.kind {
            MemberKind::Method => "method",
            MemberKind::Getter => "getter",
            MemberKind::Setter => "setter",
            MemberKind::Field => "field",
        });
        self.field(if entry.is_static {
            "static"
        } else {
            "instance"
        });
        self.field(if entry.is_abstract {
            "abstract"
        } else {
            "concrete"
        });
        self.field(if entry.is_external {
            "external"
        } else {
            "local"
        });
        self.field(if entry.is_final { "final" } else { "mutable" });
        self.uint(u64::from(entry.type_params));
        for param in &entry.type_param_names {
            self.name(*param);
        }
        self.ty(entry.var_ty);
        self.ty(entry.return_ty);
        match &entry.formals {
            Some(formals) => self.formals(formals),
            None => self.field("no-formals"),
        }
        self.annotations(&entry.annotations);
        // The name-token anchor: a move-only edit (the member repositioned
        // without a face change) is hashed so the L2 cascade re-folds the
        // dependents whose cross-file context rows anchor here.
        self.uint(u64::from(entry.name_range.offset));
        self.uint(u64::from(entry.name_range.length));
    }

    fn top_decls(&mut self, elements: &FileElements) {
        Self::sorted(
            self.strings,
            &mut self.names,
            elements.decls.keys().copied(),
        );
        for i in 0..self.names.len() {
            let name = self.names[i];
            self.top_decl(name, &elements.decls[&name]);
        }
    }

    /// One top-level declaration's slice: the `decl` field through its
    /// formal table. Shared by the file-level render ([`Face::top_decls`])
    /// and the member-level projection ([`FileElements::member_digests`]) —
    /// the per-name digest is the exact slice the file face writes.
    fn top_decl(&mut self, name: StringId, decl: &TopDecl) {
        let TopDecl {
            kind,
            type_params,
            var_ty,
            return_ty,
            formals,
        } = decl;
        // Capture-time filter: prefixes and type parameters are
        // declaration-scoped, never a library face (the collector
        // doesn't capture prefixes). Filtering here keeps the encoding
        // match total by construction — a future capture change
        // degrades to skipping the row, never a panic.
        if matches!(kind, MicroKind::Prefix | MicroKind::TypeParam) {
            return;
        }
        let tag = match kind {
            MicroKind::Class => "class",
            MicroKind::Enum => "enum",
            MicroKind::Mixin => "mixin",
            MicroKind::Extension => "extension",
            MicroKind::TypeAlias => "typeAlias",
            MicroKind::TopLevelFn => "function",
            MicroKind::TopLevelVar => "variable",
            // Filtered above; the arm stays for exhaustiveness and
            // degrades to a skip.
            MicroKind::Prefix | MicroKind::TypeParam => return,
        };
        self.field("decl");
        self.name(name);
        self.field(tag);
        self.uint(u64::from(*type_params));
        self.ty(*var_ty);
        self.ty(*return_ty);
        match formals {
            Some(formals) => self.formals(formals),
            None => self.field("no-formals"),
        }
    }

    fn containers(&mut self, elements: &FileElements) {
        Self::sorted(
            self.strings,
            &mut self.names,
            elements.containers.keys().copied(),
        );
        for i in 0..self.names.len() {
            let name = self.names[i];
            self.container(name, &elements.containers[&name]);
        }
    }

    /// One named container's slice: the `container` field through the
    /// declaration's annotations. Shared by the file-level render
    /// ([`Face::containers`]) and the member-level projection
    /// ([`FileElements::member_digests`]) — the per-name digest is the
    /// exact slice the file face writes.
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn container(&mut self, name: StringId, container: &Container) {
        let Container {
            kind,
            is_abstract,
            is_base,
            is_final,
            is_interface,
            is_mixin_class,
            superclass,
            members,
            type_params,
            sealed,
            with_clause,
            implements_clause,
            on_clause,
            header_residue,
            primary_formals,
            enum_constants,
            constructors,
            has_unnamed_constructor,
            has_const_constructor,
            ctor_formals,
            annotations,
        } = container;
        self.field("container");
        self.name(name);
        self.field(match kind {
            ContainerKind::Class => "class",
            ContainerKind::Enum => "enum",
            ContainerKind::Mixin => "mixin",
            ContainerKind::Extension => "extension",
            ContainerKind::ExtensionType => "extensionType",
        });
        self.uint(u64::from(*type_params));
        self.field(if *is_abstract { "abstract" } else { "concrete" });
        self.field(if *sealed { "sealed" } else { "unsealed" });
        // The declared class-modifier facts: a modifier-only edit moves
        // diagnostics (the restriction families), so it must flip the
        // L2 gate.
        self.field(if *is_base { "base" } else { "plain" });
        self.field(if *is_final { "final" } else { "nonfinal" });
        self.field(if *is_interface {
            "interface"
        } else {
            "noninterface"
        });
        self.field(if *is_mixin_class {
            "mixin-class"
        } else {
            "nominal-class"
        });
        match superclass {
            Some(superclass) => self.name(*superclass),
            None => self.field("no-superclass"),
        }
        // Clause lists in fixed order (with, on, implements), each in
        // written declaration order.
        self.usize(with_clause.len());
        for supertype in with_clause {
            self.name(*supertype);
        }
        self.usize(on_clause.len());
        for supertype in on_clause {
            self.name(*supertype);
        }
        self.usize(implements_clause.len());
        for supertype in implements_clause {
            self.name(*supertype);
        }
        // The unclaimed header names hash too (L2 honesty: an edit that
        // only moves a name between a clause and the residue must flip
        // the gate), sorted by text — residue order is not semantic.
        Self::sorted(
            self.strings,
            &mut self.inner,
            header_residue.iter().copied(),
        );
        self.usize(self.inner.len());
        for j in 0..self.inner.len() {
            self.name(self.inner[j]);
        }
        // The representation formals in declaration order (the
        // extension-type face) — the same encoding the formal tables use.
        self.usize(primary_formals.len());
        for rep in primary_formals {
            self.formal(&rep.formal);
        }
        self.usize(enum_constants.len());
        for constant in enum_constants {
            self.name(*constant);
        }
        // The constructor face: named selectors in stored order, then
        // the unnamed flag.
        self.usize(constructors.len());
        for constructor in constructors {
            self.name(*constructor);
        }
        self.field(if *has_unnamed_constructor {
            "unnamed"
        } else {
            "no-unnamed"
        });
        self.field(if *has_const_constructor {
            "const-ctor"
        } else {
            "no-const-ctor"
        });
        // The constructor formals by lookup spelling (the
        // `unused_element_parameter` constructor face's facts): a hash
        // map, sorted by text.
        Self::sorted(self.strings, &mut self.inner, ctor_formals.keys().copied());
        for j in 0..self.inner.len() {
            let ctor_name = self.inner[j];
            self.name(ctor_name);
            self.formals(&ctor_formals[&ctor_name]);
        }
        // Member names are a hash map: sort by text, then render each
        // member's full executable shape. The nested level sorts into the
        // second scratch so the container loop's iteration state survives.
        Self::sorted(self.strings, &mut self.inner, members.keys().copied());
        for j in 0..self.inner.len() {
            let member_name = self.inner[j];
            self.member("member", member_name, &members[&member_name]);
        }
        // The declaration's metadata (name-level, written order): an
        // annotation-only edit moves the meta-getter verdicts, so it
        // must flip the L2 gate. The argument list folds as its
        // interned source text: an argument-only edit
        // (`@Deprecated('a')` → `@Deprecated('b')`) is an element-face
        // edit). The row's offset stays hashed — a text
        // shift above the annotation moves it and flips the gate, the
        // conservative direction (over-invalidation cascades at most
        // one extra file; under-invalidation could hide verdict
        // changes).
        self.annotations(annotations);
    }

    /// The extension application faces in declaration order: identity (`E`
    /// or the unnamed marker), the `on` type, then every member's full shape
    /// (per-face sorted by text — the face is a hash map).
    fn extension_faces(&mut self, elements: &FileElements) {
        self.usize(elements.extension_faces.len());
        for face in &elements.extension_faces {
            self.field("extension-face");
            match face.name {
                Some(name) => self.name(name),
                None => self.field("<unnamed>"),
            }
            match face.on_type {
                Some(on_type) => self.name(on_type),
                None => self.field("no-on-type"),
            }
            Self::sorted(self.strings, &mut self.inner, face.members.keys().copied());
            for j in 0..self.inner.len() {
                let member_name = self.inner[j];
                self.member("member", member_name, &face.members[&member_name]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_parser::ParserFeatures;
    use dartq_source::FileTable;
    use std::path::Path;

    /// Scans + collects one unit (the library.rs test posture).
    fn elements_of(source: &str, strings: &StringTable) -> FileElements {
        let mut files = FileTable::new();
        let file = files.intern(Path::new("sig.dart"));
        let result = dartq_scanner::scan(
            source,
            dartq_scanner::ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(result.diagnostics.is_empty(), "{source:?}");
        let tokens: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
        collect(
            source,
            tokens,
            ParserFeatures::from_feature_set(FeatureSet::for_language_version(
                LanguageVersion::new(3, 15),
            )),
            strings,
        )
    }

    /// Every projection digest under one library-face name, in the
    /// projection's own order (decl rows before container rows — a named
    /// class answers twice). Text-keyed, so comparisons survive differing
    /// interning tables.
    fn entries_of(elements: &FileElements, strings: &StringTable, name: &str) -> Vec<ApiSignature> {
        elements
            .member_digests(strings)
            .into_iter()
            .filter(|(entry, _)| strings.resolve(*entry) == name)
            .map(|(_, digest)| digest)
            .collect()
    }

    /// The (name text, digest) pairs in canonical comparison order —
    /// sorted by text, then digest, so equal-name rows (a named class's
    /// decl and container rows) total-order across interning tables.
    fn sorted_digests(
        elements: &FileElements,
        strings: &StringTable,
    ) -> Vec<(String, ApiSignature)> {
        let mut pairs = elements
            .member_digests(strings)
            .into_iter()
            .map(|(name, digest)| (strings.resolve(name).to_owned(), digest))
            .collect::<Vec<_>>();
        pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.as_bytes().cmp(b.1.as_bytes())));
        pairs
    }

    /// Top-level declarations reorder freely — the canonical-sorted-write
    /// contract (acceptance 282.c): the face renders decls and containers
    /// sorted by resolved text, so source order never rides the hash. The
    /// reorder is confined below the member-bearing container on purpose
    /// (#1867 v6): a layout change above a member moves its name-token
    /// anchor, and that IS a face move — the member-reorder case below.
    ///
    /// The member probe is a pure same-member-set reorder (exactly `f` and
    /// `m` on both sides, order swapped): members render sorted by text, so
    /// a position-free-member implementation would render byte-identical
    /// faces and `assert_ne` would not fire — its only way to fire is the
    /// moved `m` anchor.
    #[test]
    fn reordered_declarations_share_the_signature() {
        let strings = StringTable::new();
        let a = elements_of(
            "class Alpha { int f = 0; void m() {} }\nclass Zed {}\nint top() => 1;\ntypedef T = int;\n",
            &strings,
        );
        let b = elements_of(
            "class Alpha { int f = 0; void m() {} }\nclass Zed {}\ntypedef T = int;\nint top() => 1;\n",
            &strings,
        );
        assert_eq!(a.api_signature(&strings), b.api_signature(&strings));
        // Reordering members moves their name-token anchors — the v6
        // anchor face flips the file signature by design (#1867): the
        // dependents' cross-file context rows must re-anchor.
        let flipped = elements_of(
            "class Alpha { void m() {} int f = 0; }\nclass Zed {}\nint top() => 1;\ntypedef T = int;\n",
            &strings,
        );
        assert_ne!(
            a.api_signature(&strings),
            flipped.api_signature(&strings),
            "a pure member reorder (same member set) moves name anchors — the v6 face flips"
        );
    }

    /// Body edits keep the signature — the L2 win condition. A comment
    /// before a **member** flips it since v6: the member name-token span
    /// (the cross-file context-row anchor face) hashes, so moving the token
    /// is a face move — the dependents' fingerprint-driven memo miss is
    /// what re-folds them with re-anchored rows (the byte-consistency gate
    /// outranks the comment-churn cascade saving). A comment before a
    /// top-level function keeps it: top-level declarations carry no span
    /// face (their context anchors resolve from the fold's own tables,
    /// same-file, re-folded on any own edit).
    #[test]
    fn body_edits_keep_the_signature_and_member_comment_edits_flip_it() {
        let strings = StringTable::new();
        let base = elements_of("class C { void m() {} }\nint f() => 1;\n", &strings);
        let rebody = elements_of("class C { void m() {} }\nint f() => 99999;\n", &strings);
        let member_comment = elements_of(
            "/* docs */ class C { void m() {} }\nint f() => 1;\n",
            &strings,
        );
        let expected = base.api_signature(&strings);
        assert_eq!(rebody.api_signature(&strings), expected);
        assert_ne!(
            member_comment.api_signature(&strings),
            expected,
            "the moved member name token is an anchor-face move (v6)"
        );
        // The scoping half: a member-free file's top-level token moves
        // freely — top-level declarations carry no span face (their context
        // anchors resolve from the fold's own tables, same-file, re-folded
        // on any own edit).
        let top_only = elements_of("int f() => 1;\n", &strings);
        let top_moved = elements_of("/* docs */ int f() => 1;\n", &strings);
        assert_eq!(
            top_only.api_signature(&strings),
            top_moved.api_signature(&strings)
        );
    }

    /// Face-shaping edits flip the signature: renames, parameter and return
    /// types, member additions, enum witness order, type-parameter counts
    /// (the top-level-function path — the collector's proven capture).
    #[test]
    fn face_shaping_edits_flip_the_signature() {
        let strings = StringTable::new();
        let base = elements_of("int f(int x) => x;\nenum E { a, b }\n", &strings);
        let expected = base.api_signature(&strings);
        for (label, source) in [
            ("rename", "int g(int x) => x;\nenum E { a, b }\n"),
            ("param type", "int f(String x) => 0;\nenum E { a, b }\n"),
            ("return type", "String f(int x) => '';\nenum E { a, b }\n"),
            (
                "added param",
                "int f(int x, int y) => x;\nenum E { a, b }\n",
            ),
            (
                "member added",
                "int f(int x) => x;\nenum E { a, b }\nclass D {}\n",
            ),
            ("witness order", "int f(int x) => x;\nenum E { b, a }\n"),
            ("type params", "int f<T, U>(int x) => x;\nenum E { a, b }\n"),
        ] {
            let edited = elements_of(source, &strings);
            assert_ne!(
                edited.api_signature(&strings),
                expected,
                "{label} must flip the signature"
            );
        }
    }

    /// A container-member body edit keeps the signature; a member-shape edit
    /// flips it (composition F carries member signatures, never bodies).
    #[test]
    fn member_body_stays_member_shape_flips() {
        let strings = StringTable::new();
        let base = elements_of("class C { int m(int x) => x; }\n", &strings);
        let rebody = elements_of("class C { int m(int x) => x + 1; }\n", &strings);
        let reshaped = elements_of("class C { int m(int y) => y; }\n", &strings);
        assert_eq!(base.api_signature(&strings), rebody.api_signature(&strings));
        assert_ne!(
            base.api_signature(&strings),
            reshaped.api_signature(&strings)
        );
    }

    /// The version rides the hash: any bump changes every signature
    /// (acceptance 282.d).
    #[test]
    fn version_bump_changes_every_signature() {
        let strings = StringTable::new();
        let elements = elements_of("int f() => 1;\n", &strings);
        let v1 = elements.api_signature_with_version(&strings, 1);
        let v2 = elements.api_signature_with_version(&strings, 2);
        assert_ne!(v1, v2);
        assert_eq!(v1, elements.api_signature_with_version(&strings, 1));
        // The live composition rides the current version constant.
        let live = elements.api_signature_with_version(&strings, SIGNATURE_VERSION);
        assert_eq!(elements.api_signature(&strings), live);
        assert_eq!(ApiSignature::as_bytes(&live).len(), 16);
    }

    /// The bump-sensitivity seam at the current boundary (facts-consistency
    /// audit 2026-10-17, G1): adjacent versions around
    /// [`SIGNATURE_VERSION`] — today `6` vs `7` — digest the same payload
    /// differently. The version field rides the hashed face's head, so the
    /// next face change's bump invalidates every cached signature by
    /// construction: the one-time-invalidation guarantee the engine's
    /// `FACTS_SEMANTIC_VERSION` fold rides.
    #[test]
    fn adjacent_versions_around_the_current_bump_differ() {
        let strings = StringTable::new();
        let elements = elements_of("class C { void m() {} }\nint f(int x) => x;\n", &strings);
        assert_ne!(
            elements.api_signature_with_version(&strings, SIGNATURE_VERSION),
            elements.api_signature_with_version(&strings, SIGNATURE_VERSION + 1),
            "a SIGNATURE_VERSION bump must invalidate every cached signature"
        );
    }

    /// An explicit type-argument edit flips the signature (the v4 face),
    /// and the same arguments interned through two independent pool
    /// histories hash equal — content rides the hash, never slots
    /// (interning order must stay irrelevant, acceptance 282.c's law).
    /// Each face interns through its own elements' pool — the one-pool-
    /// one-universe contract (`ArgPool` module docs) the signature reads.
    #[test]
    fn argument_edits_flip_and_pool_history_stays_irrelevant() {
        let strings = StringTable::new();
        let (int, list) = (strings.intern("int"), strings.intern("List"));
        let mut elements = FileElements::default();
        let mut decl = TopDecl::bare(MicroKind::TopLevelVar);
        decl.var_ty = Ty::named(list);
        elements.decls = [(strings.intern("f"), decl)].into_iter().collect();
        let bare = elements.api_signature(&strings);

        let with_args = || {
            let mut elements = FileElements::default();
            let mut decl = TopDecl::bare(MicroKind::TopLevelVar);
            decl.var_ty = elements
                .type_args_mut()
                .named(list, false, &[Ty::named(int)]);
            elements.decls = [(strings.intern("f"), decl)].into_iter().collect();
            elements.api_signature(&strings)
        };
        let with_int = with_args();
        assert_ne!(bare, with_int, "an argument edit is an element-face edit");
        assert_eq!(
            with_int,
            with_args(),
            "same content, separate pools: equal face"
        );
    }

    /// Adding an argument list to an annotation flips the signature (the
    /// v5 face): `@A()` carries facts `@A` does not — the meta-getter
    /// verdicts read the argument group.
    #[test]
    fn annotation_argument_add_flips_the_signature() {
        let strings = StringTable::new();
        let bare = elements_of("@A class C {}\n", &strings);
        let added = elements_of("@A('x') class C {}\n", &strings);
        assert_ne!(
            bare.api_signature(&strings),
            added.api_signature(&strings),
            "an added argument list is an element-face edit"
        );
    }

    /// A same-length argument edit flips the signature: the group folds as
    /// its source text, so `'x'` → `'y'` (same shape, same offsets) still
    /// moves the face.
    #[test]
    fn annotation_argument_edit_flips_the_signature() {
        let strings = StringTable::new();
        let base = elements_of("@A('x') class C {}\n", &strings);
        let edited = elements_of("@A('y') class C {}\n", &strings);
        assert_ne!(
            base.api_signature(&strings),
            edited.api_signature(&strings),
            "a same-length argument edit is an element-face edit"
        );
    }

    /// A comment above the annotation moves the row's offset field, which
    /// hashes — the signature flips. Pinned conservative-by-design (#1083):
    /// the row anchors at the official annotation-anchored report position,
    /// and over-invalidation cascades at most one extra file while
    /// under-invalidation could hide verdict changes.
    #[test]
    fn comment_above_annotation_flips_conservatively() {
        let strings = StringTable::new();
        let base = elements_of("@A class C {}\n", &strings);
        let shifted = elements_of("// lead\n@A class C {}\n", &strings);
        assert_ne!(
            base.api_signature(&strings),
            shifted.api_signature(&strings),
            "the offset hash keeps the conservative direction"
        );
    }

    /// Prefix and type-param decl rows are skipped at capture time (never a
    /// library face): a hand-built elements map carrying them renders the
    /// same signature as the clean face — the encoding match stays total by
    /// construction, and a stray capture degrades to a skip, never a panic
    /// (batch G; the previous `unreachable!` arm died on exactly this
    /// input).
    #[test]
    fn prefix_and_type_param_rows_are_skipped() {
        let strings = StringTable::new();
        let mut clean = FileElements::default();
        clean.decls = [(strings.intern("f"), TopDecl::bare(MicroKind::TopLevelFn))]
            .into_iter()
            .collect();
        let expected = clean.api_signature(&strings);

        let mut with_skips = FileElements::default();
        with_skips.decls = [
            (strings.intern("f"), TopDecl::bare(MicroKind::TopLevelFn)),
            (strings.intern("p"), TopDecl::bare(MicroKind::Prefix)),
            (strings.intern("T"), TopDecl::bare(MicroKind::TypeParam)),
        ]
        .into_iter()
        .collect();
        assert_eq!(with_skips.api_signature(&strings), expected);
    }

    /// The projection's canonicality law (the surviving kernel of the old
    /// content-only contract): a digest renders resolved name text plus
    /// face content — never a `StringId` or an iteration slot — so the same
    /// source through two independent interning histories answers the same
    /// digest set. What a digest DOES track since v6 is the name-token
    /// anchor (#1867): moving a member's name token — here a longer
    /// top-level body above it — flips the member-bearing container's
    /// digest while the offset-free top-level entry stands.
    #[test]
    fn member_digests_are_content_canonical_and_track_the_name_anchor() {
        let strings = StringTable::new();
        let source = "int top() => 1;\nclass Alpha { void m() {} }\n";
        let a = elements_of(source, &strings);
        // A second interning history: another name interned first shifts
        // every StringId — the digests must not notice.
        let shifted_table = StringTable::new();
        shifted_table.intern("zzz_unrelated");
        let b = elements_of(source, &shifted_table);
        assert_eq!(
            sorted_digests(&a, &strings),
            sorted_digests(&b, &shifted_table),
            "interning order never rides a digest"
        );
        // The anchor face (#1867 v6): the member's name token moved, the
        // container digest moved with it.
        let moved = elements_of(
            "int top() => 11111;\nclass Alpha { void m() {} }\n",
            &strings,
        );
        assert_ne!(
            sorted_digests(&a, &strings),
            sorted_digests(&moved, &strings),
            "a moved member name token is an anchor move"
        );
        // The projection rows under `top` exist — the equality asserts
        // below must not pass vacuously.
        assert!(!entries_of(&a, &strings, "top").is_empty());
        // Body freedom: the body edit never moves `top` itself.
        assert_eq!(
            entries_of(&a, &strings, "top"),
            entries_of(&moved, &strings, "top"),
            "top-level entries carry no body face"
        );
        // Position freedom: `top` moves below `Alpha` with zero content
        // change and keeps its digest — offsets never ride a top-level
        // entry, so "offset-free" is pinned, not just body-free.
        let reordered = elements_of("class Alpha { void m() {} }\nint top() => 1;\n", &strings);
        assert_eq!(
            entries_of(&a, &strings, "top"),
            entries_of(&reordered, &strings, "top"),
            "a moved top-level declaration keeps its digest — no offset face"
        );
    }

    /// The name rides its digest: the same member face under a different
    /// declaration name digests differently — two symbols that swap faces
    /// cannot pass the impact verdicts as unchanged.
    #[test]
    fn member_digests_distinguish_the_same_face_under_different_names() {
        let strings = StringTable::new();
        let a = elements_of("class C { void m() {} }\n", &strings);
        let b = elements_of("class D { void m() {} }\n", &strings);
        let (da, db) = (a.member_digests(&strings), b.member_digests(&strings));
        assert_ne!(
            da[0].1, db[0].1,
            "same face, different name: distinct digests"
        );
    }

    /// The verdict granularity the projection exists for (#1561): a face
    /// edit under a stable name flips exactly that name's digests; a body
    /// edit keeps them — scoped by the v6 anchor law (#1867): a body edit
    /// that shifts a member's name token (an edit above it) flips that
    /// member's container digest while the offset-free top-level entry
    /// stands.
    #[test]
    fn member_digest_flip_tracks_the_face_not_the_body() {
        let strings = StringTable::new();
        let base = elements_of(
            "int f(int x) => x;\nclass C { int m(int x) => x; }\n",
            &strings,
        );
        // m's body edited in place — no name token moves, every digest
        // stands.
        let m_rebody = elements_of(
            "int f(int x) => x;\nclass C { int m(int x) => x + 1; }\n",
            &strings,
        );
        assert_eq!(
            entries_of(&base, &strings, "f"),
            entries_of(&m_rebody, &strings, "f")
        );
        assert_eq!(
            entries_of(&base, &strings, "C"),
            entries_of(&m_rebody, &strings, "C"),
            "a body edit that moves no name token keeps every digest"
        );
        // f's body edited in place: f's own (offset-free) digest stands,
        // but the longer line shifts everything below — m's name token
        // moved, and its container digest flips with it (#1867).
        let f_rebody = elements_of(
            "int f(int x) => x + 1;\nclass C { int m(int x) => x; }\n",
            &strings,
        );
        assert_eq!(
            entries_of(&base, &strings, "f"),
            entries_of(&f_rebody, &strings, "f"),
            "a top-level body edit keeps the offset-free entry"
        );
        assert_ne!(
            entries_of(&base, &strings, "C"),
            entries_of(&f_rebody, &strings, "C"),
            "the same edit moved m's name token — an anchor move"
        );
        // Face edits at stable offsets flip exactly the touched face.
        let f_reshaped = elements_of(
            "int f(int y) => y;\nclass C { int m(int x) => x; }\n",
            &strings,
        );
        let m_reshaped = elements_of(
            "int f(int x) => x;\nclass C { int m(int y) => y; }\n",
            &strings,
        );
        assert_ne!(
            entries_of(&base, &strings, "f"),
            entries_of(&f_reshaped, &strings, "f"),
            "a face edit flips the touched top-level entry"
        );
        assert_eq!(
            entries_of(&base, &strings, "C"),
            entries_of(&f_reshaped, &strings, "C")
        );
        assert_ne!(
            entries_of(&base, &strings, "C"),
            entries_of(&m_reshaped, &strings, "C"),
            "a face edit flips the touched member's container digest"
        );
        assert_eq!(
            entries_of(&base, &strings, "f"),
            entries_of(&m_reshaped, &strings, "f")
        );
    }
}
