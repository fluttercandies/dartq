//! The persistence projection of the element model (the M3 facts
//! snapshots): plain-data mirrors of the [`crate::model`] types whose
//! `StringId`s are projected onto indexes into one per-entry string slice.
//!
//! Arena ids are process-local and [`StringId`] has no numeric constructor,
//! so the mirrors carry `u32` slice indexes instead; [`PersistedElements::
//! into_elements`] re-interns the slice in order and every column resolves
//! through the resulting ids. The slice holds each referenced text once —
//! the compact-facts discipline: no token payloads, no source bytes, only
//! the columns the model already holds.

use serde::{Deserialize, Serialize};

use dartq_source::{SourceRange, StringId, StringTable};

use crate::model::{
    Annotation, ArgPool, ArityDecl, ArityViolation, Container, ContainerKind, DuplicateDecl,
    ExtensionFace, FileElements, Formal, Formals, MemberEntry, MemberKind, MicroKind, RepFormal,
    TopDecl, Ty,
};

/// A span projection: `SourceRange`'s two public columns (`SourceRange`
/// itself carries no serde impl and is foreign to every writer of this
/// module).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedRange {
    pub offset: u32,
    pub length: u32,
}

impl PersistedRange {
    /// Projects a source span.
    #[must_use]
    pub const fn of(range: SourceRange) -> Self {
        Self {
            offset: range.offset,
            length: range.length,
        }
    }

    /// Restores the source span.
    #[must_use]
    pub const fn source_range(self) -> SourceRange {
        SourceRange {
            offset: self.offset,
            length: self.length,
        }
    }
}

/// The per-unit element model in payload form. Map columns are entry lists
/// keyed by string-slice index; hash-map iteration order is deterministic
/// (seedless hasher, same key set), so equal models encode equally.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedElements {
    /// The interned texts every `u32` name column indexes.
    pub strings: Vec<String>,
    pub decls: Vec<(u32, PersistedTopDecl)>,
    pub containers: Vec<(u32, PersistedContainer)>,
    /// Container names in declaration order (the exhaustiveness witness
    /// order).
    pub container_order: Vec<u32>,
    pub extension_faces: Vec<PersistedExtensionFace>,
    pub duplicates: Vec<PersistedDuplicate>,
    pub arity_violations: Vec<PersistedArity>,
}

/// The declared-type column. The `Named` arguments ride as nested content
/// (the model's own canonical order), never as pool slots — slots carry
/// interning order and would fork the payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedTy {
    Unknown,
    Null,
    Dynamic,
    /// Additive after the format shipped: a pre-`Never` payload could not
    /// capture the bottom type (no producer existed), so the absent variant
    /// decodes nothing differently (#539 additive-field discipline).
    Never,
    Void,
    Named {
        name: u32,
        nullable: bool,
        /// Additive after the format shipped: `default` so payloads
        /// persisted before the slot existed decode argument-less — exact,
        /// since a pre-slot binary could not capture arguments (the #539
        /// additive-field discipline; the #701 argument slot).
        #[serde(default)]
        args: Vec<PersistedTy>,
    },
    /// A record type: the positional fields then the named `(name, field)`
    /// pairs, content-keyed exactly like `Named` (the #1511 face; a
    /// pre-record payload could not produce the variant, so the additive
    /// arm decodes nothing differently).
    Record {
        nullable: bool,
        positional: Vec<PersistedTy>,
        named: Vec<(u32, PersistedTy)>,
    },
    /// A function type: the nullable flag, the type-parameter count, the
    /// return type, the positional formals, and the named `(name, formal)`
    /// pairs, content-keyed exactly like `Record` (the function-shape
    /// face; a pre-function payload could not produce the variant, so the
    /// additive arm decodes nothing differently).
    Function {
        nullable: bool,
        type_params: u16,
        return_ty: Box<PersistedTy>,
        positional: Vec<PersistedTy>,
        named: Vec<(u32, PersistedTy)>,
    },
}

impl Default for PersistedTy {
    /// The additive-field decode default (`PersistedExtensionFace::on_ty`):
    /// a pre-slot payload could not capture the `on` clause's arguments —
    /// `Unknown` suppresses the argument-level gate the field feeds, the
    /// under-report direction.
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedMicroKind {
    Class,
    Enum,
    Mixin,
    Extension,
    TypeAlias,
    Prefix,
    TopLevelFn,
    TopLevelVar,
    TypeParam,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedMemberKind {
    Method,
    Getter,
    Setter,
    Field,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedContainerKind {
    Class,
    Enum,
    Mixin,
    Extension,
    ExtensionType,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedFormal {
    pub name: u32,
    pub ty: PersistedTy,
    pub required: bool,
    /// Additive after the format shipped: `default` so payloads persisted
    /// before the columns existed decode modifier-less and default-less —
    /// exact, since a pre-slot binary captured neither (the #539
    /// additive-field discipline).
    #[serde(default)]
    pub covariant: bool,
    #[serde(default)]
    pub default_value: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedFormals {
    pub positional: Vec<PersistedFormal>,
    pub named: Vec<PersistedFormal>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[expect(clippy::struct_excessive_bools)] // flags mirror the official member modifier set
pub struct PersistedMemberEntry {
    pub kind: PersistedMemberKind,
    pub is_static: bool,
    pub is_abstract: bool,
    pub is_external: bool,
    pub is_final: bool,
    pub type_params: u16,
    pub type_param_names: Vec<u32>,
    pub var_ty: PersistedTy,
    pub return_ty: PersistedTy,
    pub formals: Option<Box<PersistedFormals>>,
    /// Additive after the format shipped: `default` so payloads persisted
    /// before the field existed decode with no member metadata (the #539
    /// additive-field discipline; the member annotation star).
    #[serde(default)]
    pub annotations: Vec<PersistedAnnotation>,
    /// Additive after the format shipped: `default` so payloads persisted
    /// before the field existed decode span-less (the #539
    /// additive-field discipline; the member's name-token anchor — the
    /// context-row faces resolve through it, and a span-less member degrades
    /// those rows to plain rows).
    #[serde(default)]
    pub name_range: Option<(u32, u32)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[expect(clippy::struct_excessive_bools)] // flags mirror the official container modifier set
pub struct PersistedContainer {
    pub kind: PersistedContainerKind,
    pub is_abstract: bool,
    /// Additive after the format shipped: `default` so payloads persisted
    /// before the fields existed decode with no declared modifier flags
    /// (the #539 additive-field discipline; the #600 class-modifier facts).
    #[serde(default)]
    pub is_base: bool,
    #[serde(default)]
    pub is_final: bool,
    #[serde(default)]
    pub is_interface: bool,
    #[serde(default)]
    pub is_mixin_class: bool,
    pub superclass: Option<u32>,
    pub members: Vec<(u32, PersistedMemberEntry)>,
    pub type_params: u16,
    pub sealed: bool,
    pub with_clause: Vec<u32>,
    pub implements_clause: Vec<u32>,
    pub on_clause: Vec<u32>,
    pub header_residue: Vec<u32>,
    pub primary_formals: Vec<PersistedRepFormal>,
    pub enum_constants: Vec<u32>,
    pub constructors: Vec<u32>,
    pub has_unnamed_constructor: bool,
    /// Some declared constructor carries `const` (the const-creation faces'
    /// resolved-container fact). `default` decodes older payloads as
    /// `false`; the facts-shape bump keeps such payloads from ever being
    /// served.
    #[serde(default)]
    pub has_const_constructor: bool,
    /// Additive after the format shipped: `default` so payloads persisted
    /// before the field existed decode with no constructor formals (the
    /// #539 additive-field discipline).
    #[serde(default)]
    pub ctor_formals: Vec<(u32, Box<PersistedFormals>)>,
    /// Additive after the format shipped: `default` so payloads persisted
    /// before the field existed decode with no declaration metadata (the
    /// #539 additive-field discipline; the #705 annotation substrate).
    #[serde(default)]
    pub annotations: Vec<PersistedAnnotation>,
}

/// One declaration annotation in payload form: the dot-joined name id, the
/// annotation's own span (`@` through the name's last segment), and the
/// argument group's interned source text when one parsed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedAnnotation {
    pub name: u32,
    pub offset: u32,
    pub length: u32,
    /// Additive after the format shipped: `None` so payloads persisted
    /// before the field existed decode argument-less (the #539
    /// additive-field discipline).
    #[serde(default)]
    pub args_text: Option<u32>,
}

/// One extension application face in payload form (the implicit-application
/// candidates of the unit, declaration order).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedExtensionFace {
    pub name: Option<u32>,
    pub on_type: Option<u32>,
    /// Additive after the format shipped (the #539 discipline): the `on`
    /// type was written nullable — the applicability/specificity split
    /// reads it.
    #[serde(default)]
    pub on_type_nullable: bool,
    /// Additive after the format shipped (the #539 discipline): the `on`
    /// clause's full type with its argument list — the applicability
    /// gate's argument-level comparison. A pre-slot payload decodes to
    /// `Unknown`, the argument-less direction the gate already suppresses
    /// on.
    #[serde(default)]
    pub on_ty: PersistedTy,
    /// Additive after the format shipped: the extension's own
    /// type-parameter spellings (`on List<T>` keeps `[T]`) — an `on`
    /// argument naming one instantiates against the receiver rather than
    /// comparing concretely.
    #[serde(default)]
    pub type_param_names: Vec<u32>,
    pub members: Vec<(u32, PersistedMemberEntry)>,
}

/// One extension-type representation formal in payload form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedRepFormal {
    pub name: u32,
    pub ty: PersistedTy,
    pub required: bool,
    pub name_offset: u32,
    pub name_length: u32,
    /// Additive after the format shipped (the #539 discipline): the
    /// embedded formal's modifier bit and default-value text.
    #[serde(default)]
    pub covariant: bool,
    #[serde(default)]
    pub default_value: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedTopDecl {
    pub kind: PersistedMicroKind,
    pub type_params: u16,
    pub var_ty: PersistedTy,
    pub return_ty: PersistedTy,
    pub formals: Option<Box<PersistedFormals>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedArityViolation {
    Operator {
        name: u32,
        expected: u16,
        found: u16,
    },
    OperatorMinus {
        found: u16,
    },
    Setter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedArity {
    pub violation: PersistedArityViolation,
    pub range: PersistedRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedDuplicate {
    pub name: u32,
    pub range: PersistedRange,
}

/// The per-entry string slice under construction: every referenced
/// `StringId` gets one slot, first-use ordered.
struct Slice<'a> {
    strings: &'a StringTable,
    values: Vec<String>,
    at: rustc_hash::FxHashMap<StringId, u32>,
    /// The unit's argument pool: `Named` columns resolve their slots to
    /// nested content through it.
    type_args: &'a ArgPool,
}

impl<'a> Slice<'a> {
    fn new(strings: &'a StringTable, type_args: &'a ArgPool) -> Self {
        Self {
            strings,
            values: Vec::new(),
            at: rustc_hash::FxHashMap::default(),
            type_args,
        }
    }

    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn id(&mut self, id: StringId) -> u32 {
        if let Some(&at) = self.at.get(&id) {
            return at;
        }
        self.values.push(self.strings.resolve(id).to_owned());
        let at = self.values.len() as u32 - 1;
        self.at.insert(id, at);
        at
    }

    fn ids(&mut self, ids: &[StringId]) -> Vec<u32> {
        ids.iter().map(|&id| self.id(id)).collect()
    }

    fn ty(&mut self, ty: &Ty) -> PersistedTy {
        match *ty {
            Ty::Unknown => PersistedTy::Unknown,
            Ty::Null => PersistedTy::Null,
            Ty::Dynamic => PersistedTy::Dynamic,
            Ty::Never => PersistedTy::Never,
            Ty::Void => PersistedTy::Void,
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                let args = self.type_args.get(args).to_vec();
                PersistedTy::Named {
                    name: self.id(name),
                    nullable,
                    args: args.iter().map(|arg| self.ty(arg)).collect(),
                }
            }
            Ty::Record { shape, nullable } => {
                // The shape interns its named set sorted by the producer's
                // StringId — canonical per producer universe, not byte-stable
                // across producers (intern order varies by run). Decode
                // re-interns canonically, so identity survives: a differently
                // ordered payload costs a spurious cache miss, never a wrong hit.
                let (positional, named) = {
                    let shape = self.type_args.record_shape(shape);
                    (
                        shape.positional.to_vec(),
                        shape
                            .named
                            .iter()
                            .map(|(n, t)| (*n, *t))
                            .collect::<Vec<_>>(),
                    )
                };
                PersistedTy::Record {
                    nullable,
                    positional: positional.iter().map(|ty| self.ty(ty)).collect(),
                    named: named
                        .into_iter()
                        .map(|(name, ty)| (self.id(name), self.ty(&ty)))
                        .collect(),
                }
            }
            Ty::Function { shape, nullable } => {
                // The shape interns its named set sorted by the producer's
                // StringId — canonical per producer universe, not byte-stable
                // across producers (intern order varies by run). Decode
                // re-interns canonically, so identity survives: a differently
                // ordered payload costs a spurious cache miss, never a wrong hit.
                let (type_params, return_ty, positional, named) = {
                    let shape = self.type_args.function_shape(shape);
                    (
                        shape.type_params,
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape
                            .named
                            .iter()
                            .map(|(n, t)| (*n, *t))
                            .collect::<Vec<_>>(),
                    )
                };
                PersistedTy::Function {
                    nullable,
                    type_params,
                    return_ty: Box::new(self.ty(&return_ty)),
                    positional: positional.iter().map(|ty| self.ty(ty)).collect(),
                    named: named
                        .into_iter()
                        .map(|(name, ty)| (self.id(name), self.ty(&ty)))
                        .collect(),
                }
            }
        }
    }

    fn formals(&mut self, formals: &Formals) -> PersistedFormals {
        PersistedFormals {
            positional: formals.positional.iter().map(|f| self.formal(f)).collect(),
            named: formals.named.iter().map(|f| self.formal(f)).collect(),
        }
    }

    fn formal(&mut self, formal: &Formal) -> PersistedFormal {
        PersistedFormal {
            name: self.id(formal.name),
            ty: self.ty(&formal.ty),
            required: formal.required,
            covariant: formal.covariant,
            default_value: formal.default_value.map(|text| self.id(text)),
        }
    }

    /// One annotation star (a container's or a member's) in written order.
    fn annotations(&mut self, annotations: &[Annotation]) -> Vec<PersistedAnnotation> {
        annotations
            .iter()
            .map(|annotation| PersistedAnnotation {
                name: self.id(annotation.name),
                offset: annotation.range.offset,
                length: annotation.range.length,
                args_text: annotation.args_text.map(|text| self.id(text)),
            })
            .collect()
    }

    fn member(&mut self, entry: &MemberEntry) -> PersistedMemberEntry {
        PersistedMemberEntry {
            kind: match entry.kind {
                MemberKind::Method => PersistedMemberKind::Method,
                MemberKind::Getter => PersistedMemberKind::Getter,
                MemberKind::Setter => PersistedMemberKind::Setter,
                MemberKind::Field => PersistedMemberKind::Field,
            },
            is_static: entry.is_static,
            is_abstract: entry.is_abstract,
            is_external: entry.is_external,
            is_final: entry.is_final,
            type_params: entry.type_params,
            type_param_names: self.ids(&entry.type_param_names),
            var_ty: self.ty(&entry.var_ty),
            return_ty: self.ty(&entry.return_ty),
            formals: entry
                .formals
                .as_ref()
                .map(|formals| Box::new(self.formals(formals))),
            annotations: self.annotations(&entry.annotations),
            name_range: (entry.name_range.offset != u32::MAX)
                .then(|| (entry.name_range.offset, entry.name_range.length)),
        }
    }
}

impl PersistedElements {
    /// Projects a captured element model onto the payload form, resolving
    /// every name through `strings`.
    #[must_use]
    pub fn of(elements: &FileElements, strings: &StringTable) -> Self {
        let mut s = Slice::new(strings, &elements.type_args);
        let decls = elements
            .decls
            .iter()
            .map(|(name, decl)| (s.id(*name), PersistedTopDecl::of(decl, &mut s)))
            .collect();
        let containers = elements
            .containers
            .iter()
            .map(|(name, container)| (s.id(*name), PersistedContainer::of(container, &mut s)))
            .collect();
        let container_order = s.ids(&elements.container_order);
        let extension_faces = elements
            .extension_faces
            .iter()
            .map(|face| PersistedExtensionFace {
                name: face.name.map(|name| s.id(name)),
                on_type: face.on_type.map(|name| s.id(name)),
                on_type_nullable: face.on_type_nullable,
                on_ty: s.ty(&face.on_ty),
                type_param_names: s.ids(&face.type_param_names),
                members: face
                    .members
                    .iter()
                    .map(|(name, entry)| (s.id(*name), s.member(entry)))
                    .collect(),
            })
            .collect();
        let duplicates = elements
            .duplicates
            .iter()
            .map(|duplicate| PersistedDuplicate {
                name: s.id(duplicate.name),
                range: PersistedRange::of(duplicate.range),
            })
            .collect();
        let arity_violations = elements
            .arity_violations
            .iter()
            .map(|arity| PersistedArity {
                violation: match arity.violation {
                    ArityViolation::Operator {
                        name,
                        expected,
                        found,
                    } => PersistedArityViolation::Operator {
                        name: s.id(name),
                        expected,
                        found,
                    },
                    ArityViolation::OperatorMinus { found } => {
                        PersistedArityViolation::OperatorMinus { found }
                    }
                    ArityViolation::Setter => PersistedArityViolation::Setter,
                },
                range: PersistedRange::of(arity.range),
            })
            .collect();
        Self {
            strings: s.values,
            decls,
            containers,
            container_order,
            extension_faces,
            duplicates,
            arity_violations,
        }
    }

    /// Restores the element model, interning the string slice into `strings`
    /// in order; every name column resolves through the interned ids. The
    /// `Named` argument columns re-intern into the model's fresh pool, so
    /// equal contents land on equal slots regardless of payload order.
    #[must_use]
    pub fn into_elements(self, strings: &StringTable) -> FileElements {
        let ids: Vec<StringId> = self
            .strings
            .iter()
            .map(|text| strings.intern(text))
            .collect();
        let at_name = |at: u32| ids[at as usize];
        let mut type_args = ArgPool::default();
        FileElements {
            decls: self
                .decls
                .into_iter()
                .map(|(at, decl)| (at_name(at), decl.into_top_decl(&ids, &mut type_args)))
                .collect(),
            containers: self
                .containers
                .into_iter()
                .map(|(at, container)| {
                    (at_name(at), container.into_container(&ids, &mut type_args))
                })
                .collect(),
            container_order: self.container_order.iter().map(|&at| at_name(at)).collect(),
            extension_faces: self
                .extension_faces
                .into_iter()
                .map(|face| ExtensionFace {
                    name: face.name.map(&at_name),
                    on_type: face.on_type.map(&at_name),
                    on_type_nullable: face.on_type_nullable,
                    on_ty: into_ty(face.on_ty, &ids, &mut type_args),
                    type_param_names: face
                        .type_param_names
                        .into_iter()
                        .map(&at_name)
                        .collect(),
                    members: face
                        .members
                        .into_iter()
                        .map(|(at, entry)| {
                            (at_name(at), entry.into_member_entry(&ids, &mut type_args))
                        })
                        .collect(),
                })
                .collect(),
            duplicates: self
                .duplicates
                .iter()
                .map(|duplicate| DuplicateDecl {
                    name: at_name(duplicate.name),
                    range: duplicate.range.source_range(),
                })
                .collect(),
            arity_violations: self
                .arity_violations
                .iter()
                .map(|arity| ArityDecl {
                    violation: match arity.violation {
                        PersistedArityViolation::Operator {
                            name,
                            expected,
                            found,
                        } => ArityViolation::Operator {
                            name: at_name(name),
                            expected,
                            found,
                        },
                        PersistedArityViolation::OperatorMinus { found } => {
                            ArityViolation::OperatorMinus { found }
                        }
                        PersistedArityViolation::Setter => ArityViolation::Setter,
                    },
                    range: arity.range.source_range(),
                })
                .collect(),
            type_args,
        }
    }
}

impl PersistedTopDecl {
    fn of(decl: &TopDecl, s: &mut Slice<'_>) -> Self {
        Self {
            kind: match decl.kind {
                MicroKind::Class => PersistedMicroKind::Class,
                MicroKind::Enum => PersistedMicroKind::Enum,
                MicroKind::Mixin => PersistedMicroKind::Mixin,
                MicroKind::Extension => PersistedMicroKind::Extension,
                MicroKind::TypeAlias => PersistedMicroKind::TypeAlias,
                MicroKind::Prefix => PersistedMicroKind::Prefix,
                MicroKind::TopLevelFn => PersistedMicroKind::TopLevelFn,
                MicroKind::TopLevelVar => PersistedMicroKind::TopLevelVar,
                MicroKind::TypeParam => PersistedMicroKind::TypeParam,
            },
            type_params: decl.type_params,
            var_ty: s.ty(&decl.var_ty),
            return_ty: s.ty(&decl.return_ty),
            formals: decl
                .formals
                .as_ref()
                .map(|formals| Box::new(s.formals(formals))),
        }
    }

    fn into_top_decl(self, ids: &[StringId], pool: &mut ArgPool) -> TopDecl {
        TopDecl {
            kind: match self.kind {
                PersistedMicroKind::Class => MicroKind::Class,
                PersistedMicroKind::Enum => MicroKind::Enum,
                PersistedMicroKind::Mixin => MicroKind::Mixin,
                PersistedMicroKind::Extension => MicroKind::Extension,
                PersistedMicroKind::TypeAlias => MicroKind::TypeAlias,
                PersistedMicroKind::Prefix => MicroKind::Prefix,
                PersistedMicroKind::TopLevelFn => MicroKind::TopLevelFn,
                PersistedMicroKind::TopLevelVar => MicroKind::TopLevelVar,
                PersistedMicroKind::TypeParam => MicroKind::TypeParam,
            },
            type_params: self.type_params,
            var_ty: into_ty(self.var_ty, ids, pool),
            return_ty: into_ty(self.return_ty, ids, pool),
            formals: self
                .formals
                .map(|formals| Box::new(into_formals(*formals, ids, pool))),
        }
    }
}

impl PersistedContainer {
    fn of(container: &Container, s: &mut Slice<'_>) -> Self {
        Self {
            kind: match container.kind {
                ContainerKind::Class => PersistedContainerKind::Class,
                ContainerKind::Enum => PersistedContainerKind::Enum,
                ContainerKind::Mixin => PersistedContainerKind::Mixin,
                ContainerKind::Extension => PersistedContainerKind::Extension,
                ContainerKind::ExtensionType => PersistedContainerKind::ExtensionType,
            },
            is_abstract: container.is_abstract,
            is_base: container.is_base,
            is_final: container.is_final,
            is_interface: container.is_interface,
            is_mixin_class: container.is_mixin_class,
            superclass: container.superclass.map(|name| s.id(name)),
            members: container
                .members
                .iter()
                .map(|(name, entry)| (s.id(*name), s.member(entry)))
                .collect(),
            type_params: container.type_params,
            sealed: container.sealed,
            with_clause: s.ids(&container.with_clause),
            implements_clause: s.ids(&container.implements_clause),
            on_clause: s.ids(&container.on_clause),
            header_residue: s.ids(&container.header_residue),
            primary_formals: container
                .primary_formals
                .iter()
                .map(|rep| PersistedRepFormal {
                    name: s.id(rep.formal.name),
                    ty: s.ty(&rep.formal.ty),
                    required: rep.formal.required,
                    name_offset: rep.name_range.offset,
                    name_length: rep.name_range.length,
                    covariant: rep.formal.covariant,
                    default_value: rep.formal.default_value.map(|text| s.id(text)),
                })
                .collect(),
            enum_constants: s.ids(&container.enum_constants),
            constructors: s.ids(&container.constructors),
            has_unnamed_constructor: container.has_unnamed_constructor,
            has_const_constructor: container.has_const_constructor,
            ctor_formals: container
                .ctor_formals
                .iter()
                .map(|(name, formals)| (s.id(*name), Box::new(s.formals(formals))))
                .collect(),
            annotations: s.annotations(&container.annotations),
        }
    }

    fn into_container(self, ids: &[StringId], pool: &mut ArgPool) -> Container {
        Container {
            kind: match self.kind {
                PersistedContainerKind::Class => ContainerKind::Class,
                PersistedContainerKind::Enum => ContainerKind::Enum,
                PersistedContainerKind::Mixin => ContainerKind::Mixin,
                PersistedContainerKind::Extension => ContainerKind::Extension,
                PersistedContainerKind::ExtensionType => ContainerKind::ExtensionType,
            },
            is_abstract: self.is_abstract,
            is_base: self.is_base,
            is_final: self.is_final,
            is_interface: self.is_interface,
            is_mixin_class: self.is_mixin_class,
            superclass: self.superclass.map(|at| ids[at as usize]),
            members: self
                .members
                .into_iter()
                .map(|(at, entry)| (ids[at as usize], entry.into_member_entry(ids, pool)))
                .collect(),
            type_params: self.type_params,
            sealed: self.sealed,
            with_clause: self
                .with_clause
                .iter()
                .map(|&at| ids[at as usize])
                .collect(),
            implements_clause: self
                .implements_clause
                .iter()
                .map(|&at| ids[at as usize])
                .collect(),
            on_clause: self.on_clause.iter().map(|&at| ids[at as usize]).collect(),
            header_residue: self
                .header_residue
                .iter()
                .map(|&at| ids[at as usize])
                .collect(),
            primary_formals: self
                .primary_formals
                .into_iter()
                .map(|rep| RepFormal {
                    formal: Formal {
                        name: ids[rep.name as usize],
                        ty: into_ty(rep.ty, ids, pool),
                        required: rep.required,
                        covariant: rep.covariant,
                        default_value: rep.default_value.map(|at| ids[at as usize]),
                    },
                    name_range: SourceRange {
                        offset: rep.name_offset,
                        length: rep.name_length,
                    },
                })
                .collect(),
            enum_constants: self
                .enum_constants
                .iter()
                .map(|&at| ids[at as usize])
                .collect(),
            constructors: self
                .constructors
                .iter()
                .map(|&at| ids[at as usize])
                .collect(),
            has_unnamed_constructor: self.has_unnamed_constructor,
            has_const_constructor: self.has_const_constructor,
            ctor_formals: self
                .ctor_formals
                .into_iter()
                .map(|(at, formals)| {
                    (
                        ids[at as usize],
                        Box::new(into_formals(*formals, ids, pool)),
                    )
                })
                .collect(),
            annotations: self
                .annotations
                .into_iter()
                .map(|annotation| Annotation {
                    name: ids[annotation.name as usize],
                    range: SourceRange {
                        offset: annotation.offset,
                        length: annotation.length,
                    },
                    args_text: annotation.args_text.map(|at| ids[at as usize]),
                })
                .collect(),
        }
    }
}

impl PersistedMemberEntry {
    fn into_member_entry(self, ids: &[StringId], pool: &mut ArgPool) -> MemberEntry {
        MemberEntry {
            kind: match self.kind {
                PersistedMemberKind::Method => MemberKind::Method,
                PersistedMemberKind::Getter => MemberKind::Getter,
                PersistedMemberKind::Setter => MemberKind::Setter,
                PersistedMemberKind::Field => MemberKind::Field,
            },
            is_static: self.is_static,
            is_abstract: self.is_abstract,
            is_external: self.is_external,
            is_final: self.is_final,
            type_params: self.type_params,
            type_param_names: self
                .type_param_names
                .iter()
                .map(|&at| ids[at as usize])
                .collect(),
            var_ty: into_ty(self.var_ty, ids, pool),
            return_ty: into_ty(self.return_ty, ids, pool),
            formals: self
                .formals
                .map(|formals| Box::new(into_formals(*formals, ids, pool))),
            annotations: self
                .annotations
                .into_iter()
                .map(|annotation| Annotation {
                    name: ids[annotation.name as usize],
                    range: SourceRange {
                        offset: annotation.offset,
                        length: annotation.length,
                    },
                    args_text: annotation.args_text.map(|at| ids[at as usize]),
                })
                .collect(),
            name_range: self
                .name_range
                .map_or(MemberEntry::MISSING_NAME_RANGE, |(offset, length)| {
                    SourceRange { offset, length }
                }),
        }
    }
}

fn into_ty(ty: PersistedTy, ids: &[StringId], pool: &mut ArgPool) -> Ty {
    match ty {
        PersistedTy::Unknown => Ty::Unknown,
        PersistedTy::Null => Ty::Null,
        PersistedTy::Dynamic => Ty::Dynamic,
        PersistedTy::Never => Ty::Never,
        PersistedTy::Void => Ty::Void,
        PersistedTy::Named {
            name,
            nullable,
            args,
        } => {
            let args: Vec<Ty> = args
                .into_iter()
                .map(|arg| into_ty(arg, ids, pool))
                .collect();
            pool.named(ids[name as usize], nullable, &args)
        }
        PersistedTy::Record {
            nullable,
            positional,
            named,
        } => {
            let positional: Vec<Ty> = positional
                .into_iter()
                .map(|ty| into_ty(ty, ids, pool))
                .collect();
            let named: Vec<(StringId, Ty)> = named
                .into_iter()
                .map(|(name, ty)| (ids[name as usize], into_ty(ty, ids, pool)))
                .collect();
            pool.record(&positional, &named, nullable)
        }
        PersistedTy::Function {
            nullable,
            type_params,
            return_ty,
            positional,
            named,
        } => {
            let return_ty = into_ty(*return_ty, ids, pool);
            let positional: Vec<Ty> = positional
                .into_iter()
                .map(|ty| into_ty(ty, ids, pool))
                .collect();
            let named: Vec<(StringId, Ty)> = named
                .into_iter()
                .map(|(name, ty)| (ids[name as usize], into_ty(ty, ids, pool)))
                .collect();
            pool.function(type_params, return_ty, &positional, &named, nullable)
        }
    }
}

fn into_formals(formals: PersistedFormals, ids: &[StringId], pool: &mut ArgPool) -> Formals {
    let mut into = |formal: PersistedFormal| Formal {
        name: ids[formal.name as usize],
        ty: into_ty(formal.ty, ids, pool),
        required: formal.required,
        covariant: formal.covariant,
        default_value: formal.default_value.map(|at| ids[at as usize]),
    };
    Formals {
        positional: formals.positional.into_iter().map(&mut into).collect(),
        named: formals.named.into_iter().map(into).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MemberEntry;

    /// A representative model — every column populated — survives the
    /// payload round trip: the API signature (which hashes the whole face)
    /// is reproduced byte-exact, and the capture-time logs compare equal.
    #[test]
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn model_round_trips_through_the_payload() {
        let strings = StringTable::new();
        let name = |text: &str| strings.intern(text);
        let range = SourceRange {
            offset: 7,
            length: 3,
        };
        let mut elements = FileElements::default();
        // Declared types with nested columns ride the round trip: the
        // member's return type is `List<int>` (a nested payload column),
        // the positional formal's type is `String Function(int)?` (the
        // function-shape columns), the named formal's `int` stays
        // argument-less (the reserved empty slot).
        let int = Ty::named(name("int"));
        let pool = elements.type_args_mut();
        let list_int = pool.named(name("List"), false, std::slice::from_ref(&int));
        let string = Ty::named(name("String"));
        let string_fn = pool.function(0, string, std::slice::from_ref(&int), &[], true);
        let member = move |text: &str| MemberEntry {
            kind: MemberKind::Method,
            is_static: false,
            is_abstract: false,
            is_external: false,
            is_final: false,
            type_params: 1,
            type_param_names: Box::new([name(text)]),
            var_ty: Ty::Unknown,
            return_ty: list_int,
            formals: Some(Box::new(Formals {
                positional: Box::new([Formal {
                    name: name("x"),
                    ty: string_fn,
                    required: true,
                    covariant: true,
                    default_value: None,
                }]),
                named: Box::new([Formal {
                    name: name("y"),
                    ty: Ty::Unknown,
                    required: false,
                    covariant: false,
                    default_value: Some(name("3")),
                }]),
            })),
            annotations: Box::new([Annotation {
                name: name("override"),
                range,
                args_text: None,
            }]),
            name_range: range,
        };
        elements.decls = [(
            name("main"),
            TopDecl {
                kind: MicroKind::TopLevelFn,
                type_params: 0,
                var_ty: Ty::Unknown,
                return_ty: Ty::Void,
                formals: None,
            },
        )]
        .into_iter()
        .collect();
        elements.containers = [(
            name("C"),
            Container {
                kind: ContainerKind::Class,
                is_abstract: true,
                is_base: true,
                is_final: false,
                is_interface: false,
                is_mixin_class: false,
                superclass: Some(name("Object")),
                members: [name("m"), name("m=")]
                    .into_iter()
                    .map(|n| (n, member(strings.resolve(n))))
                    .collect(),
                type_params: 2,
                sealed: true,
                with_clause: Vec::new(),
                implements_clause: vec![name("I")],
                on_clause: Vec::new(),
                header_residue: vec![name("R")],
                primary_formals: Box::new([RepFormal {
                    formal: Formal {
                        name: name("rep"),
                        ty: Ty::named(name("int")),
                        required: true,
                        covariant: false,
                        default_value: None,
                    },
                    name_range: range,
                }]),
                enum_constants: Vec::new(),
                constructors: Box::new([name("named")]),
                has_unnamed_constructor: false,
                has_const_constructor: false,
                ctor_formals: [(
                    name("named"),
                    Box::new(Formals {
                        positional: Box::new([]),
                        named: Box::new([]),
                    }),
                )]
                .into_iter()
                .collect(),
                annotations: Box::new([Annotation {
                    name: name("sealed"),
                    range: SourceRange {
                        offset: 0,
                        length: 7,
                    },
                    args_text: Some(name("('v')")),
                }]),
            },
        )]
        .into_iter()
        .collect();
        elements.container_order.push(name("C"));
        elements.extension_faces.push(ExtensionFace {
            name: Some(name("E")),
            on_type: Some(name("int")),
            on_type_nullable: false,
            on_ty: Ty::named(name("int")),
            type_param_names: Box::new([]),
            members: [(name("e"), member("e"))].into_iter().collect(),
        });
        elements.duplicates.push(DuplicateDecl {
            name: name("d"),
            range,
        });
        elements.arity_violations.push(ArityDecl {
            violation: ArityViolation::Operator {
                name: name("=="),
                expected: 1,
                found: 2,
            },
            range,
        });

        let payload = PersistedElements::of(&elements, &strings);
        let bytes = serde_json::to_vec(&payload).expect("serialize");
        let decoded: PersistedElements = serde_json::from_slice(&bytes).expect("deserialize");
        let restored = decoded.into_elements(&strings);

        assert_eq!(
            restored.api_signature(&strings),
            elements.api_signature(&strings),
            "the hashed element face survives the round trip"
        );
        assert_eq!(restored.decls, elements.decls);
        assert_eq!(restored.duplicates, elements.duplicates);
        assert_eq!(restored.arity_violations, elements.arity_violations);
        assert_eq!(restored.container_order, elements.container_order);
        // Members, the extension faces, and the representation formals are
        // inside the hashed face; the signature check above covers them.
        // Spot-check one map for shape.
        let restored_c = restored.container(name("C")).expect("container C");
        assert_eq!(restored_c.members.len(), 2);
        assert!(restored_c.members.contains_key(&name("m=")));
        assert_eq!(restored_c.annotations[0].name, name("sealed"));
        let restored_face = &restored.extension_faces[0];
        assert_eq!(restored_face.name, Some(name("E")));
        assert_eq!(restored_face.on_type, Some(name("int")));
        assert_eq!(restored_c.primary_formals[0].formal.name, name("rep"));
        // The argument column round trips as content and re-interns onto a
        // canonical slot: equal id, equal display (`List<int>`).
        let restored_member = &restored_c.members[&name("m")];
        assert_eq!(restored_member.return_ty, list_int);
        assert_eq!(
            restored_member
                .return_ty
                .render(&strings, restored.type_args()),
            "List<int>"
        );
        // The function-shape columns ride the same way: equal id, equal
        // display (`String Function(int)?`).
        let restored_formals = restored_member.formals.as_deref().expect("formals");
        assert_eq!(restored_formals.positional[0].ty, string_fn);
        assert_eq!(
            restored_formals.positional[0]
                .ty
                .render(&strings, restored.type_args()),
            "String Function(int)?"
        );
        // The #1637 capture columns ride the payload: the member's star,
        // the formals' covariant bits, and the default-value texts.
        assert_eq!(restored_member.annotations.len(), 1);
        assert_eq!(restored_member.annotations[0].name, name("override"));
        assert_eq!(restored_member.annotations[0].args_text, None);
        assert!(restored_formals.positional[0].covariant);
        assert_eq!(restored_formals.positional[0].default_value, None);
        assert!(!restored_formals.named[0].covariant);
        assert_eq!(restored_formals.named[0].default_value, Some(name("3")));
        // The name-token anchor rides too: the round trip preserves it
        // exactly (the context-row faces resolve through it).
        assert_eq!(restored_member.name_range, range);
    }

    /// The restore path normalizes the payload's name rows to the map's
    /// sorted invariant (#1778): a payload whose rows arrive in arbitrary
    /// order (a foreign-produced or hand-edited payload) decodes to the
    /// identical model — row order, hashed face, and map equality — so the
    /// restored side never carries the payload's accident of order.
    #[test]
    fn restore_normalizes_shuffled_name_rows() {
        let strings = StringTable::new();
        let name = |text: &str| strings.intern(text);
        let member = || MemberEntry {
            kind: MemberKind::Method,
            is_static: false,
            is_abstract: false,
            is_external: false,
            is_final: false,
            type_params: 0,
            type_param_names: Box::new([]),
            var_ty: Ty::Unknown,
            return_ty: Ty::Void,
            formals: Some(Box::new(Formals {
                positional: Box::new([]),
                named: Box::new([]),
            })),
            annotations: Box::default(),
            name_range: MemberEntry::MISSING_NAME_RANGE,
        };
        let mut elements = FileElements::default();
        elements.decls = [
            (name("zeta"), TopDecl::bare(MicroKind::TopLevelFn)),
            (name("alpha"), TopDecl::bare(MicroKind::TopLevelVar)),
        ]
        .into_iter()
        .collect();
        elements.containers = [(
            name("C"),
            Container {
                kind: ContainerKind::Class,
                is_abstract: false,
                is_base: false,
                is_final: false,
                is_interface: false,
                is_mixin_class: false,
                superclass: None,
                members: [(name("b"), member()), (name("a"), member())]
                    .into_iter()
                    .collect(),
                type_params: 0,
                sealed: false,
                with_clause: Vec::new(),
                implements_clause: Vec::new(),
                on_clause: Vec::new(),
                header_residue: Vec::new(),
                primary_formals: Box::new([]),
                enum_constants: Vec::new(),
                constructors: Box::new([]),
                has_unnamed_constructor: true,
                has_const_constructor: false,
                ctor_formals: [(
                    name("C"),
                    Box::new(Formals {
                        positional: Box::new([]),
                        named: Box::new([]),
                    }),
                )]
                .into_iter()
                .collect(),
                annotations: Box::new([]),
            },
        )]
        .into_iter()
        .collect();
        elements.container_order.push(name("C"));

        // The payload's name rows deliberately leave in the opposite of the
        // map's order: restore must normalize, not preserve.
        let mut payload = PersistedElements::of(&elements, &strings);
        payload.decls.reverse();
        payload.containers[0].1.members.reverse();
        payload.containers[0].1.ctor_formals.reverse();
        let bytes = serde_json::to_vec(&payload).expect("serialize");
        let decoded: PersistedElements = serde_json::from_slice(&bytes).expect("deserialize");
        let restored = decoded.into_elements(&strings);

        assert_eq!(restored.decls, elements.decls, "decl rows normalize sorted");
        let restored_c = restored.container(name("C")).expect("container C");
        let member_ids: Vec<StringId> = restored_c.members.iter().map(|(key, _)| *key).collect();
        let mut sorted = member_ids.clone();
        sorted.sort_unstable();
        assert_eq!(member_ids, sorted, "member rows iterate ascending by id");
        assert_eq!(
            restored.api_signature(&strings),
            elements.api_signature(&strings),
            "the hashed element face is payload-order free"
        );
    }
}
