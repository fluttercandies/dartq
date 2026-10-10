//! The per-file API signature (PRD §11.3, ticket #282): the canonical hash
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
//!   variable/return type, full formal tables; [`MicroKind::TypeParam`]
//!   entries are skipped — declaration-scoped, not a library face, matching
//!   the library-graph feeding rule);
//! * every named container: kind, type-parameter count, `sealed`, superclass
//!   name, the `with`/`on`/`implements` clause lists, enum constants **in
//!   declaration order** (the
//!   exhaustiveness witness order is semantic), and every member with its
//!   full executable shape;
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
//! maps (memory law, PRD §14).
//!
//! [`SIGNATURE_VERSION`] rides the hash: any change to the rendering or the
//! composition bumps it, invalidating every signature computed by a previous
//! engine build (the PRD §5.6 semantic-version discipline — the engine's
//! `FACTS_SEMANTIC_VERSION` folds `SIGNATURE_VERSION` into its low half, so
//! cached facts invalidate automatically; no separate engine-side bump).

use blake3::Hasher;
use dartq_source::{StringId, StringTable};

use crate::model::{
    Container, ContainerKind, FileElements, Formal, Formals, MemberEntry, MemberKind, MicroKind,
    TopDecl, Ty,
};

/// The signature-composition version (PRD §5.6): bumped with any change to
/// what [`FileElements::api_signature`] hashes. v1: the initial composition-F
/// face (#282). v2: constructor selectors + the unnamed flag per container,
/// and member type-parameter names (#350 — the constructor-undefined faces).
/// v3: the header-residue names per container and the extension faces with
/// their `on` types (#491 S1 + #476 — the clause-attribution fix and the
/// extension-application faces changed the hashed face).
pub const SIGNATURE_VERSION: u32 = 3;

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
        let mut face = Face::new(strings);
        face.field("sig-v");
        face.uint(u64::from(version));
        face.top_decls(self);
        face.containers(self);
        face.extension_faces(self);
        ApiSignature::truncate(&blake3::hash(&face.buf))
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
}

impl<'a> Face<'a> {
    fn new(strings: &'a StringTable) -> Self {
        Self {
            buf: Vec::with_capacity(256),
            names: Vec::new(),
            inner: Vec::new(),
            strings,
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
            Ty::Void => self.field("void"),
            Ty::Named { name, nullable } => {
                self.field(self.strings.resolve(name));
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
    }

    fn top_decls(&mut self, elements: &FileElements) {
        Self::sorted(
            self.strings,
            &mut self.names,
            elements.decls.keys().copied(),
        );
        for i in 0..self.names.len() {
            let name = self.names[i];
            let TopDecl {
                kind,
                type_params,
                var_ty,
                return_ty,
                formals,
            } = &elements.decls[&name];
            if *kind == MicroKind::TypeParam {
                continue;
            }
            self.field("decl");
            self.name(name);
            self.field(match kind {
                MicroKind::Class => "class",
                MicroKind::Enum => "enum",
                MicroKind::Mixin => "mixin",
                MicroKind::Extension => "extension",
                MicroKind::TypeAlias => "typeAlias",
                MicroKind::Prefix | MicroKind::TypeParam => {
                    unreachable!("prefix never captured; type params skipped")
                }
                MicroKind::TopLevelFn => "function",
                MicroKind::TopLevelVar => "variable",
            });
            self.uint(u64::from(*type_params));
            self.ty(*var_ty);
            self.ty(*return_ty);
            match formals {
                Some(formals) => self.formals(formals),
                None => self.field("no-formals"),
            }
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
            let Container {
                kind,
                is_abstract,
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
            } = &elements.containers[&name];
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
            // The representation formals in declaration order (name, type,
            // required bit) — the extension-type face.
            self.usize(primary_formals.len());
            for rep in primary_formals.iter() {
                self.name(rep.formal.name);
                self.ty(rep.formal.ty);
                self.field(if rep.formal.required { "req" } else { "opt" });
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
            // Member names are a hash map: sort by text, then render each
            // member's full executable shape. The nested level sorts into the
            // second scratch so the container loop's iteration state survives.
            Self::sorted(self.strings, &mut self.inner, members.keys().copied());
            for j in 0..self.inner.len() {
                let member_name = self.inner[j];
                self.member("member", member_name, &members[&member_name]);
            }
        }
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

    /// The same declarations in a different source order hash identically —
    /// the canonical-sorted-write contract (acceptance 282.c). The empty
    /// `Alpha` sorts before `Zeppelin`, pinning the nested member sort's
    /// scratch independence (an empty member set must not disturb the
    /// container loop).
    #[test]
    fn reordered_declarations_share_the_signature() {
        let strings = StringTable::new();
        let a = elements_of(
            "class Zeppelin { void m() {} int f = 0; }\nclass Alpha {}\nint top() => 1;\ntypedef T = int;\n",
            &strings,
        );
        let b = elements_of(
            "typedef T = int;\nclass Alpha {}\nint top() => 1;\nclass Zeppelin { int f = 0; void m() {} }\n",
            &strings,
        );
        assert_eq!(a.api_signature(&strings), b.api_signature(&strings));
    }

    /// Comment and body edits keep the signature — the L2 win condition.
    #[test]
    fn comment_and_body_edits_keep_the_signature() {
        let strings = StringTable::new();
        let base = elements_of("int f() => 1;\nclass C {}\n", &strings);
        let commented = elements_of(
            "/* docs */ int f() => 1; // trailing\nclass C {}\n",
            &strings,
        );
        let rebody = elements_of("int f() => 99999;\nclass C {}\n", &strings);
        let expected = base.api_signature(&strings);
        assert_eq!(commented.api_signature(&strings), expected);
        assert_eq!(rebody.api_signature(&strings), expected);
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
}
