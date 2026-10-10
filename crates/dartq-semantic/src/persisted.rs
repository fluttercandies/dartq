//! The persistence projection of the element model (M3 facts snapshots,
//! PRD §17.2): plain-data mirrors of the [`crate::model`] types whose
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
    ArityDecl, ArityViolation, Container, ContainerKind, DuplicateDecl, FileElements, Formal,
    Formals, MemberEntry, MemberKind, MicroKind, TopDecl, Ty,
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
    pub extension_members: Vec<(u32, PersistedMemberEntry)>,
    pub duplicates: Vec<PersistedDuplicate>,
    pub arity_violations: Vec<PersistedArity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedTy {
    Unknown,
    Null,
    Dynamic,
    Void,
    Named { name: u32, nullable: bool },
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedFormal {
    pub name: u32,
    pub ty: PersistedTy,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedFormals {
    pub positional: Vec<PersistedFormal>,
    pub named: Vec<PersistedFormal>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedMemberEntry {
    pub kind: PersistedMemberKind,
    pub is_static: bool,
    pub type_params: u16,
    pub type_param_names: Vec<u32>,
    pub var_ty: PersistedTy,
    pub return_ty: PersistedTy,
    pub formals: Option<Box<PersistedFormals>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedContainer {
    pub kind: PersistedContainerKind,
    pub superclass: Option<u32>,
    pub members: Vec<(u32, PersistedMemberEntry)>,
    pub type_params: u16,
    pub sealed: bool,
    pub interfaces: Vec<u32>,
    pub enum_constants: Vec<u32>,
    pub constructors: Vec<u32>,
    pub has_unnamed_constructor: bool,
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
}

impl<'a> Slice<'a> {
    fn new(strings: &'a StringTable) -> Self {
        Self {
            strings,
            values: Vec::new(),
            at: rustc_hash::FxHashMap::default(),
        }
    }

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
            Ty::Void => PersistedTy::Void,
            Ty::Named { name, nullable } => PersistedTy::Named {
                name: self.id(name),
                nullable,
            },
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
        }
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
            type_params: entry.type_params,
            type_param_names: self.ids(&entry.type_param_names),
            var_ty: self.ty(&entry.var_ty),
            return_ty: self.ty(&entry.return_ty),
            formals: entry
                .formals
                .as_ref()
                .map(|formals| Box::new(self.formals(formals))),
        }
    }
}

impl PersistedElements {
    /// Projects a captured element model onto the payload form, resolving
    /// every name through `strings`.
    #[must_use]
    pub fn of(elements: &FileElements, strings: &StringTable) -> Self {
        let mut s = Slice::new(strings);
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
        let extension_members = elements
            .extension_members
            .iter()
            .map(|(name, entry)| (s.id(*name), s.member(entry)))
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
            extension_members,
            duplicates,
            arity_violations,
        }
    }

    /// Restores the element model, interning the string slice into `strings`
    /// in order; every name column resolves through the interned ids.
    #[must_use]
    pub fn into_elements(self, strings: &StringTable) -> FileElements {
        let ids: Vec<StringId> = self
            .strings
            .iter()
            .map(|text| strings.intern(text))
            .collect();
        let at_name = |at: u32| ids[at as usize];
        FileElements {
            decls: self
                .decls
                .into_iter()
                .map(|(at, decl)| (at_name(at), decl.into_top_decl(&ids)))
                .collect(),
            containers: self
                .containers
                .into_iter()
                .map(|(at, container)| (at_name(at), container.into_container(&ids)))
                .collect(),
            container_order: self.container_order.iter().map(|&at| at_name(at)).collect(),
            extension_members: self
                .extension_members
                .into_iter()
                .map(|(at, entry)| (at_name(at), entry.into_member_entry(&ids)))
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

    fn into_top_decl(self, ids: &[StringId]) -> TopDecl {
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
            var_ty: into_ty(self.var_ty, ids),
            return_ty: into_ty(self.return_ty, ids),
            formals: self
                .formals
                .map(|formals| Box::new(into_formals(*formals, ids))),
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
            superclass: container.superclass.map(|name| s.id(name)),
            members: container
                .members
                .iter()
                .map(|(name, entry)| (s.id(*name), s.member(entry)))
                .collect(),
            type_params: container.type_params,
            sealed: container.sealed,
            interfaces: s.ids(&container.interfaces),
            enum_constants: s.ids(&container.enum_constants),
            constructors: s.ids(&container.constructors),
            has_unnamed_constructor: container.has_unnamed_constructor,
        }
    }

    fn into_container(self, ids: &[StringId]) -> Container {
        Container {
            kind: match self.kind {
                PersistedContainerKind::Class => ContainerKind::Class,
                PersistedContainerKind::Enum => ContainerKind::Enum,
                PersistedContainerKind::Mixin => ContainerKind::Mixin,
                PersistedContainerKind::Extension => ContainerKind::Extension,
                PersistedContainerKind::ExtensionType => ContainerKind::ExtensionType,
            },
            superclass: self.superclass.map(|at| ids[at as usize]),
            members: self
                .members
                .into_iter()
                .map(|(at, entry)| (ids[at as usize], entry.into_member_entry(ids)))
                .collect(),
            type_params: self.type_params,
            sealed: self.sealed,
            interfaces: self.interfaces.iter().map(|&at| ids[at as usize]).collect(),
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
        }
    }
}

impl PersistedMemberEntry {
    fn into_member_entry(self, ids: &[StringId]) -> MemberEntry {
        MemberEntry {
            kind: match self.kind {
                PersistedMemberKind::Method => MemberKind::Method,
                PersistedMemberKind::Getter => MemberKind::Getter,
                PersistedMemberKind::Setter => MemberKind::Setter,
                PersistedMemberKind::Field => MemberKind::Field,
            },
            is_static: self.is_static,
            type_params: self.type_params,
            type_param_names: self
                .type_param_names
                .iter()
                .map(|&at| ids[at as usize])
                .collect(),
            var_ty: into_ty(self.var_ty, ids),
            return_ty: into_ty(self.return_ty, ids),
            formals: self
                .formals
                .map(|formals| Box::new(into_formals(*formals, ids))),
        }
    }
}

fn into_ty(ty: PersistedTy, ids: &[StringId]) -> Ty {
    match ty {
        PersistedTy::Unknown => Ty::Unknown,
        PersistedTy::Null => Ty::Null,
        PersistedTy::Dynamic => Ty::Dynamic,
        PersistedTy::Void => Ty::Void,
        PersistedTy::Named { name, nullable } => Ty::Named {
            name: ids[name as usize],
            nullable,
        },
    }
}

fn into_formals(formals: PersistedFormals, ids: &[StringId]) -> Formals {
    let into = |formal: PersistedFormal| Formal {
        name: ids[formal.name as usize],
        ty: into_ty(formal.ty, ids),
        required: formal.required,
    };
    Formals {
        positional: formals.positional.into_iter().map(into).collect(),
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
    fn model_round_trips_through_the_payload() {
        let strings = StringTable::new();
        let name = |text: &str| strings.intern(text);
        let range = SourceRange {
            offset: 7,
            length: 3,
        };
        let member = |text: &str| MemberEntry {
            kind: MemberKind::Method,
            is_static: false,
            type_params: 1,
            type_param_names: Box::new([name(text)]),
            var_ty: Ty::Unknown,
            return_ty: Ty::Named {
                name: name("int"),
                nullable: false,
            },
            formals: Some(Box::new(Formals {
                positional: Box::new([Formal {
                    name: name("x"),
                    ty: Ty::Named {
                        name: name("int"),
                        nullable: true,
                    },
                    required: true,
                }]),
                named: Box::new([Formal {
                    name: name("y"),
                    ty: Ty::Unknown,
                    required: false,
                }]),
            })),
        };
        let mut elements = FileElements::default();
        elements.decls.insert(
            name("main"),
            TopDecl {
                kind: MicroKind::TopLevelFn,
                type_params: 0,
                var_ty: Ty::Unknown,
                return_ty: Ty::Void,
                formals: None,
            },
        );
        elements.containers.insert(
            name("C"),
            Container {
                kind: ContainerKind::Class,
                superclass: Some(name("Object")),
                members: [name("m"), name("m=")]
                    .into_iter()
                    .map(|n| (n, member(strings.resolve(n))))
                    .collect(),
                type_params: 2,
                sealed: true,
                interfaces: vec![name("I")],
                enum_constants: Vec::new(),
                constructors: Box::new([name("named")]),
                has_unnamed_constructor: false,
            },
        );
        elements.container_order.push(name("C"));
        elements.extension_members.insert(name("e"), member("e"));
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
        // Members and the extension pool are inside the hashed face; the
        // signature check above covers them. Spot-check one map for shape.
        let restored_c = restored.container(name("C")).expect("container C");
        assert_eq!(restored_c.members.len(), 2);
        assert!(restored_c.members.contains_key(&name("m=")));
    }
}
