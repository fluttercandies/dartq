//! Canonical type interning (spec §4.3).
//!
//! The canonical key of a type is its structure: the table maps structural
//! equality onto a single [`TypeId`], mirroring the analyzer's structural
//! `==` (`type.dart:876-889`; `isStructurallyEqualTo` delegates to `==`,
//! type.dart:1589). Construction must go through the `intern*` entries —
//! the dartiq counterpart of the analyzer's warning against low-level
//! `withNullability` manipulation (type.dart:1608-1612) — so that unit-kind
//! nullability, named-member order, and required counts are normalized
//! before a type becomes observable.

use std::cell::RefCell;
use std::collections::BTreeSet;

use rustc_hash::FxHashMap;

use crate::error::TypeError;
use crate::ids::{FunctionSigId, RecordShapeId, TypeId, TypeParamId};
use crate::ty::{
    FunctionSigData, NamedMember, PoolRange, RecordShapeData, Type, TypeKind, UninternedFunction,
    UninternedRecord, UninternedType,
};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Interner map keyed by the table's own 64-bit structural hashes
/// (splitmix-strengthened FNV, see [`mix_u64`]). [`FxHashMap`]'s hasher
/// folds each key with one multiply — bijective on `u64`, so buckets see
/// exactly the structural hash's collision profile — at a fraction of
/// keyed-SipHash cost.
type InternerMap<V> = FxHashMap<u64, V>;

/// Interning table for types. Not `Sync`: all interning and lookups go
/// through a single-threaded `RefCell` state (the compiler is
/// single-threaded; the PRD's arena/parallelism story revisits this in M2).
///
/// Append-only pools keep [`TypeId`] and [`PoolRange`] stable for the
/// lifetime of the table. Interned children make structural hashing and
/// equality O(shallow structure): children compare by id because equal
/// children are guaranteed to share one id.
#[derive(Default)]
pub struct TypeTable {
    state: RefCell<TableState>,
}

#[derive(Default)]
struct TableState {
    /// FNV-1a structural hash → the most recent id with that hash. 64-bit
    /// structural hashes almost never collide, so the single slot serves
    /// every lookup in practice; ids beyond the first live in
    /// [`TableState::collisions`].
    interner: InternerMap<u32>,
    /// Structural-hash overflow: hash → *all* ids with that hash (the slot
    /// in `interner` mirrors the bucket's last entry). Disambiguation is
    /// [`TableState::structurally_equal`].
    collisions: InternerMap<Vec<u32>>,
    types: Vec<Type>,
    /// Type arguments, positional parameters, record positional fields.
    args: Vec<TypeId>,
    /// Function named parameters and record named fields (sorted by name).
    named: Vec<NamedMember>,
    signatures: Vec<FunctionSigData>,
    shapes: Vec<RecordShapeData>,
    /// Function-declared type parameters.
    fn_type_parameters: Vec<TypeParamId>,
}

struct PoolMark {
    types: usize,
    args: usize,
    named: usize,
    signatures: usize,
    shapes: usize,
    fn_type_parameters: usize,
}

impl TypeTable {
    /// Creates an empty table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns a non-signature, non-record type and returns its canonical id.
    /// Infallible: the unit kinds normalize their nullability
    /// (`dynamic`/`void`/`Invalid` are stored non-`?`, `Never?` becomes
    /// `Null`) and nothing else here can violate a construction contract.
    #[must_use]
    pub fn intern(&self, ty: UninternedType, nullable: bool) -> TypeId {
        self.state.borrow_mut().intern_kind(ty, nullable)
    }

    /// Interns a function type (`FunctionTypeImpl`, type.dart:100).
    ///
    /// Named parameters are sorted by name into canonical order; the order of
    /// the input is irrelevant to identity.
    ///
    /// # Errors
    /// [`TypeError::RequiredPositionalOutOfBounds`] when
    /// `required_positional` exceeds `positional.len()`;
    /// [`TypeError::DuplicateNamedMember`] when two named parameters share a
    /// name.
    pub fn intern_function(
        &self,
        sig: UninternedFunction,
        nullable: bool,
    ) -> Result<TypeId, TypeError> {
        self.state.borrow_mut().intern_function(sig, nullable)
    }

    /// Interns a record type (`RecordTypeImpl`, type.dart:1296). Named fields
    /// are sorted by name; their `required` flags are normalized to `false`.
    ///
    /// # Errors
    /// [`TypeError::DuplicateNamedMember`] when two fields share a name.
    pub fn intern_record(
        &self,
        shape: UninternedRecord,
        nullable: bool,
    ) -> Result<TypeId, TypeError> {
        self.state.borrow_mut().intern_record(shape, nullable)
    }

    /// Returns the stored type.
    ///
    /// # Panics
    /// Panics when `id` did not come from this table (an out-of-range id is a
    /// program bug, not a recoverable condition).
    #[must_use]
    pub fn get(&self, id: TypeId) -> Type {
        self.state.borrow().types[id.raw() as usize]
    }

    /// Returns the pool entry at a global index (use
    /// [`PoolRange::offset`](crate::PoolRange::offset) + position).
    ///
    /// # Panics
    /// Panics when the index is out of range.
    #[must_use]
    pub fn arg(&self, index: usize) -> TypeId {
        self.state.borrow().args[index]
    }

    /// Copies the pool entries covered by `range` (cold paths only; hot code
    /// indexes with [`TypeTable::arg`]).
    ///
    /// # Panics
    /// Panics when the range is out of bounds.
    #[must_use]
    pub fn args(&self, range: PoolRange) -> Vec<TypeId> {
        let st = self.state.borrow();
        st.args[range.offset()..range.offset() + range.len()].to_vec()
    }

    /// Returns the named member at a global pool index (the `String` is
    /// cloned; internal hot paths access the pool directly under one borrow).
    ///
    /// # Panics
    /// Panics when the index is out of range.
    #[must_use]
    pub fn named_member(&self, index: usize) -> NamedMember {
        self.state.borrow().named[index].clone()
    }

    /// Copies the named members covered by `range`.
    ///
    /// # Panics
    /// Panics when the range is out of bounds.
    #[must_use]
    pub fn named_members(&self, range: PoolRange) -> Vec<NamedMember> {
        let st = self.state.borrow();
        st.named[range.offset()..range.offset() + range.len()].to_vec()
    }

    /// Returns the pooled signature.
    ///
    /// # Panics
    /// Panics when the id did not come from this table.
    #[must_use]
    pub fn signature(&self, id: FunctionSigId) -> FunctionSigData {
        self.state.borrow().signatures[id.raw() as usize]
    }

    /// Returns the pooled record shape.
    ///
    /// # Panics
    /// Panics when the id did not come from this table.
    #[must_use]
    pub fn record_shape(&self, id: RecordShapeId) -> RecordShapeData {
        self.state.borrow().shapes[id.raw() as usize]
    }

    /// Copies the declared type parameters of a function signature.
    ///
    /// # Panics
    /// Panics when the range is out of bounds.
    #[must_use]
    pub fn function_type_parameters(&self, range: PoolRange) -> Vec<TypeParamId> {
        let st = self.state.borrow();
        st.fn_type_parameters[range.offset()..range.offset() + range.len()].to_vec()
    }

    /// Number of distinct interned types.
    #[must_use]
    pub fn len(&self) -> usize {
        self.state.borrow().types.len()
    }

    /// Whether no type has been interned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.borrow().types.is_empty()
    }

    /// Substitutes type parameters throughout `ty`, interning the result.
    ///
    /// `map` entries replace [`TypeKind::TypeParameter`] references.
    /// Function-declared type parameters are never substituted: ids are
    /// globally unique per declaration, so a binder's parameters cannot
    /// appear as map keys (no capture-avoidance needed).
    #[must_use]
    pub fn substitute(&self, ty: TypeId, map: &[(TypeParamId, TypeId)]) -> TypeId {
        self.state.borrow_mut().substitute(ty, map)
    }

    /// Collects every type parameter referenced anywhere in `ty`'s structure,
    /// including function-declared ones.
    pub(crate) fn collect_type_parameters(&self, ty: TypeId, out: &mut BTreeSet<TypeParamId>) {
        self.state.borrow().collect(ty, out);
    }
}

impl TableState {
    fn intern_kind(&mut self, ty: UninternedType, nullable: bool) -> TypeId {
        let (kind, nullable) = match ty {
            // `Never?` behaves as `Null` (type.dart:1207-1209): intern it as
            // `Null` so the bottom type stays a single non-`?` shape.
            UninternedType::Never => {
                return self.commit(
                    if nullable {
                        TypeKind::Null
                    } else {
                        TypeKind::Never
                    },
                    false,
                );
            }
            // The analyzer stores `dynamic` non-`?` (`withNullability` returns
            // `this`, type.dart:90-92); `void`/`Invalid`/`Null` likewise carry
            // no suffix — nullability is semantic, handled by the rules.
            UninternedType::Dynamic => (TypeKind::Dynamic, false),
            UninternedType::Void => (TypeKind::Void, false),
            UninternedType::Invalid => (TypeKind::Invalid, false),
            UninternedType::Null => (TypeKind::Null, false),
            UninternedType::Interface { decl, args } => (
                TypeKind::Interface {
                    decl,
                    args: self.append_args(&args),
                },
                nullable,
            ),
            UninternedType::TypeParameter { parameter } => {
                (TypeKind::TypeParameter { parameter }, nullable)
            }
            UninternedType::FutureOr { base } => (TypeKind::FutureOr { base }, nullable),
        };
        self.commit(kind, nullable)
    }

    fn intern_function(
        &mut self,
        sig: UninternedFunction,
        nullable: bool,
    ) -> Result<TypeId, TypeError> {
        let UninternedFunction {
            return_type,
            type_parameters,
            positional,
            named,
            required_positional,
        } = sig;
        if (required_positional as usize) > positional.len() {
            return Err(TypeError::RequiredPositionalOutOfBounds {
                required: required_positional,
                total: positional.len(),
            });
        }
        let named = normalize_named(named)?;
        let type_parameters = self.append_type_parameters(&type_parameters);
        let positional = self.append_args(&positional);
        let named = self.append_named(&named);
        let signature = self.push_signature(FunctionSigData {
            return_type,
            type_parameters,
            positional,
            named,
            required_positional,
        });
        Ok(self.commit(TypeKind::Function { signature }, nullable))
    }

    fn intern_record(
        &mut self,
        shape: UninternedRecord,
        nullable: bool,
    ) -> Result<TypeId, TypeError> {
        let UninternedRecord { positional, named } = shape;
        // Record fields have no `required` concept.
        let mut named: Vec<NamedMember> = named;
        for member in &mut named {
            member.required = false;
        }
        let named = normalize_named(named)?;
        let positional = self.append_args(&positional);
        let named = self.append_named(&named);
        let shape = self.push_shape(RecordShapeData { positional, named });
        Ok(self.commit(TypeKind::Record { shape }, nullable))
    }

    /// Appends the type unconditionally, then deduplicates: a structural hit
    /// rolls the appends back. Pool writes are cheap; rollback restores the
    /// marks exactly.
    fn commit(&mut self, kind: TypeKind, nullable: bool) -> TypeId {
        let mark = self.mark();
        let id = u32::try_from(self.types.len()).expect("type pool exceeds u32::MAX");
        self.types.push(Type::new(kind, nullable));
        let hash = self.hash_type(id);
        if let Some(&candidate) = self.interner.get(&hash)
            && self.structurally_equal(candidate, id)
        {
            self.rollback(&mark);
            return TypeId::from_raw(candidate);
        }
        if let Some(bucket) = self.collisions.get(&hash) {
            for &candidate in bucket {
                if self.structurally_equal(candidate, id) {
                    self.rollback(&mark);
                    return TypeId::from_raw(candidate);
                }
            }
        }
        if let Some(previous) = self.interner.insert(hash, id) {
            self.collisions
                .entry(hash)
                .or_insert_with(|| vec![previous])
                .push(id);
        }
        TypeId::from_raw(id)
    }

    fn mark(&self) -> PoolMark {
        PoolMark {
            types: self.types.len(),
            args: self.args.len(),
            named: self.named.len(),
            signatures: self.signatures.len(),
            shapes: self.shapes.len(),
            fn_type_parameters: self.fn_type_parameters.len(),
        }
    }

    fn rollback(&mut self, mark: &PoolMark) {
        self.types.truncate(mark.types);
        self.args.truncate(mark.args);
        self.named.truncate(mark.named);
        self.signatures.truncate(mark.signatures);
        self.shapes.truncate(mark.shapes);
        self.fn_type_parameters.truncate(mark.fn_type_parameters);
    }

    fn append_args(&mut self, args: &[TypeId]) -> PoolRange {
        let offset = len_u32(self.args.len());
        self.args.extend_from_slice(args);
        PoolRange::new(offset, len_u32(args.len()))
    }

    fn append_named(&mut self, named: &[NamedMember]) -> PoolRange {
        let offset = len_u32(self.named.len());
        self.named.extend_from_slice(named);
        PoolRange::new(offset, len_u32(named.len()))
    }

    fn append_type_parameters(&mut self, params: &[TypeParamId]) -> PoolRange {
        let offset = len_u32(self.fn_type_parameters.len());
        self.fn_type_parameters.extend_from_slice(params);
        PoolRange::new(offset, len_u32(params.len()))
    }

    fn push_signature(&mut self, sig: FunctionSigData) -> FunctionSigId {
        let id = u32::try_from(self.signatures.len()).expect("signature pool exceeds u32::MAX");
        self.signatures.push(sig);
        FunctionSigId::from_raw(id)
    }

    fn push_shape(&mut self, shape: RecordShapeData) -> RecordShapeId {
        let id = u32::try_from(self.shapes.len()).expect("shape pool exceeds u32::MAX");
        self.shapes.push(shape);
        RecordShapeId::from_raw(id)
    }

    fn hash_type(&self, id: u32) -> u64 {
        let ty = self.types[id as usize];
        let mut h = FNV_OFFSET;
        h = mix_u64(h, discriminant(ty.kind()));
        h = mix_u64(h, u64::from(ty.is_nullable()));
        match ty.kind() {
            TypeKind::Interface { decl, args } => {
                h = mix_u64(h, u64::from(decl.raw()));
                h = self.hash_arg_range(h, args);
            }
            TypeKind::TypeParameter { parameter } => {
                h = mix_u64(h, u64::from(parameter.raw()));
            }
            TypeKind::Function { signature } => {
                let sig = &self.signatures[signature.raw() as usize];
                h = mix_u64(h, u64::from(sig.return_type.raw()));
                h = mix_u64(h, u64::from(sig.required_positional));
                h = self.hash_param_range(h, sig.type_parameters);
                h = self.hash_arg_range(h, sig.positional);
                for member in &self.named[sig.named.offset()..sig.named.offset() + sig.named.len()]
                {
                    h = mix(h, member.name.as_bytes());
                    h = mix_u64(h, u64::from(member.ty.raw()));
                    h = mix_u64(h, u64::from(member.required));
                }
            }
            TypeKind::FutureOr { base } => h = mix_u64(h, u64::from(base.raw())),
            TypeKind::Record { shape } => {
                let shape = &self.shapes[shape.raw() as usize];
                h = self.hash_arg_range(h, shape.positional);
                for member in
                    &self.named[shape.named.offset()..shape.named.offset() + shape.named.len()]
                {
                    h = mix(h, member.name.as_bytes());
                    h = mix_u64(h, u64::from(member.ty.raw()));
                }
            }
            TypeKind::Never
            | TypeKind::Dynamic
            | TypeKind::Void
            | TypeKind::Invalid
            | TypeKind::Null => {}
        }
        h
    }

    fn hash_arg_range(&self, mut h: u64, range: PoolRange) -> u64 {
        for arg in &self.args[range.offset()..range.offset() + range.len()] {
            h = mix_u64(h, u64::from(arg.raw()));
        }
        h
    }

    fn hash_param_range(&self, mut h: u64, range: PoolRange) -> u64 {
        for param in &self.fn_type_parameters[range.offset()..range.offset() + range.len()] {
            h = mix_u64(h, u64::from(param.raw()));
        }
        h
    }

    fn structurally_equal(&self, left: u32, right: u32) -> bool {
        let left_type = self.types[left as usize];
        let right_type = self.types[right as usize];
        if left_type.is_nullable() != right_type.is_nullable() {
            return false;
        }
        // Kind comparison must not use `TypeKind: PartialEq`: the pooled
        // ids of the `Function`/`Record` variants are positions, not values.
        match (left_type.kind(), right_type.kind()) {
            (
                TypeKind::Interface {
                    decl: decl_left,
                    args: args_left,
                },
                TypeKind::Interface {
                    decl: decl_right,
                    args: args_right,
                },
            ) => decl_left == decl_right && self.arg_ranges_equal(args_left, args_right),
            (
                TypeKind::TypeParameter {
                    parameter: left_param,
                },
                TypeKind::TypeParameter {
                    parameter: right_param,
                },
            ) => left_param == right_param,
            (
                TypeKind::Function {
                    signature: left_sig,
                },
                TypeKind::Function {
                    signature: right_sig,
                },
            ) => {
                let left = &self.signatures[left_sig.raw() as usize];
                let right = &self.signatures[right_sig.raw() as usize];
                left.return_type == right.return_type
                    && left.required_positional == right.required_positional
                    && self.param_ranges_equal(left.type_parameters, right.type_parameters)
                    && self.arg_ranges_equal(left.positional, right.positional)
                    && self.named_ranges_equal(left.named, right.named)
            }
            (TypeKind::FutureOr { base: left_base }, TypeKind::FutureOr { base: right_base }) => {
                left_base == right_base
            }
            (TypeKind::Record { shape: left_shape }, TypeKind::Record { shape: right_shape }) => {
                let left = &self.shapes[left_shape.raw() as usize];
                let right = &self.shapes[right_shape.raw() as usize];
                self.arg_ranges_equal(left.positional, right.positional)
                    && self.named_ranges_equal(left.named, right.named)
            }
            (TypeKind::Never, TypeKind::Never)
            | (TypeKind::Dynamic, TypeKind::Dynamic)
            | (TypeKind::Void, TypeKind::Void)
            | (TypeKind::Invalid, TypeKind::Invalid)
            | (TypeKind::Null, TypeKind::Null) => true,
            _ => false,
        }
    }

    fn arg_ranges_equal(&self, x: PoolRange, y: PoolRange) -> bool {
        x.len() == y.len()
            && self.args[x.offset()..x.offset() + x.len()]
                == self.args[y.offset()..y.offset() + y.len()]
    }

    fn param_ranges_equal(&self, x: PoolRange, y: PoolRange) -> bool {
        x.len() == y.len()
            && self.fn_type_parameters[x.offset()..x.offset() + x.len()]
                == self.fn_type_parameters[y.offset()..y.offset() + y.len()]
    }

    fn named_ranges_equal(&self, x: PoolRange, y: PoolRange) -> bool {
        let (xs, ys) = (
            &self.named[x.offset()..x.offset() + x.len()],
            &self.named[y.offset()..y.offset() + y.len()],
        );
        xs.len() == ys.len()
            && xs
                .iter()
                .zip(ys.iter())
                .all(|(a, b)| a.name == b.name && a.ty == b.ty && a.required == b.required)
    }

    fn substitute(&mut self, ty: TypeId, map: &[(TypeParamId, TypeId)]) -> TypeId {
        if map.is_empty() {
            return ty;
        }
        let t = self.types[ty.raw() as usize];
        match t.kind() {
            TypeKind::TypeParameter { parameter } => map
                .iter()
                .find(|(p, _)| *p == parameter)
                .map_or(ty, |&(_, replacement)| replacement),
            TypeKind::Interface { decl, args } => match self.substitute_args(args, map) {
                None => ty,
                Some(args) => {
                    self.intern_kind(UninternedType::Interface { decl, args }, t.is_nullable())
                }
            },
            TypeKind::Function { signature } => {
                let sig = self.signatures[signature.raw() as usize];
                let (positional, positional_changed) =
                    self.substitute_slice_changed(sig.positional, map);
                let mut named =
                    self.named[sig.named.offset()..sig.named.offset() + sig.named.len()].to_vec();
                let mut named_changed = false;
                for member in &mut named {
                    let substituted = self.substitute(member.ty, map);
                    if substituted != member.ty {
                        member.ty = substituted;
                        named_changed = true;
                    }
                }
                if positional_changed || named_changed {
                    let return_type = self.substitute(sig.return_type, map);
                    let type_parameters = self.fn_type_parameters[sig.type_parameters.offset()
                        ..sig.type_parameters.offset() + sig.type_parameters.len()]
                        .to_vec();
                    self.intern_function(
                        UninternedFunction {
                            return_type,
                            type_parameters,
                            positional,
                            named,
                            required_positional: sig.required_positional,
                        },
                        t.is_nullable(),
                    )
                    .expect("substituting types preserves signature well-formedness")
                } else {
                    ty
                }
            }
            TypeKind::FutureOr { base } => {
                let base = self.substitute(base, map);
                self.intern_kind(UninternedType::FutureOr { base }, t.is_nullable())
            }
            TypeKind::Record { shape } => {
                let shape = self.shapes[shape.raw() as usize];
                let (positional, positional_changed) =
                    self.substitute_slice_changed(shape.positional, map);
                let mut named = self.named
                    [shape.named.offset()..shape.named.offset() + shape.named.len()]
                    .to_vec();
                let mut named_changed = false;
                for member in &mut named {
                    let substituted = self.substitute(member.ty, map);
                    if substituted != member.ty {
                        member.ty = substituted;
                        named_changed = true;
                    }
                }
                if positional_changed || named_changed {
                    self.intern_record(UninternedRecord { positional, named }, t.is_nullable())
                        .expect("substituting types preserves record well-formedness")
                } else {
                    ty
                }
            }
            TypeKind::Never
            | TypeKind::Dynamic
            | TypeKind::Void
            | TypeKind::Invalid
            | TypeKind::Null => ty,
        }
    }

    /// Substitutes through an argument range; `None` when nothing changed.
    fn substitute_args(
        &mut self,
        range: PoolRange,
        map: &[(TypeParamId, TypeId)],
    ) -> Option<Vec<TypeId>> {
        let (args, changed) = self.substitute_slice_changed(range, map);
        if changed { Some(args) } else { None }
    }

    fn substitute_slice_changed(
        &mut self,
        range: PoolRange,
        map: &[(TypeParamId, TypeId)],
    ) -> (Vec<TypeId>, bool) {
        let slice: Vec<TypeId> = self.args[range.offset()..range.offset() + range.len()].to_vec();
        let mut changed = false;
        let args = slice
            .into_iter()
            .map(|arg| {
                let substituted = self.substitute(arg, map);
                changed |= substituted != arg;
                substituted
            })
            .collect();
        (args, changed)
    }

    fn collect(&self, ty: TypeId, out: &mut BTreeSet<TypeParamId>) {
        match self.types[ty.raw() as usize].kind() {
            TypeKind::Interface { args, .. } => {
                for arg in &self.args[args.offset()..args.offset() + args.len()] {
                    self.collect(*arg, out);
                }
            }
            TypeKind::TypeParameter { parameter } => {
                out.insert(parameter);
            }
            TypeKind::Function { signature } => {
                let sig = &self.signatures[signature.raw() as usize];
                out.extend(
                    self.fn_type_parameters[sig.type_parameters.offset()
                        ..sig.type_parameters.offset() + sig.type_parameters.len()]
                        .iter()
                        .copied(),
                );
                self.collect(sig.return_type, out);
                for arg in &self.args
                    [sig.positional.offset()..sig.positional.offset() + sig.positional.len()]
                {
                    self.collect(*arg, out);
                }
                for member in &self.named[sig.named.offset()..sig.named.offset() + sig.named.len()]
                {
                    self.collect(member.ty, out);
                }
            }
            TypeKind::FutureOr { base } => self.collect(base, out),
            TypeKind::Record { shape } => {
                let shape = &self.shapes[shape.raw() as usize];
                for arg in &self.args
                    [shape.positional.offset()..shape.positional.offset() + shape.positional.len()]
                {
                    self.collect(*arg, out);
                }
                for member in
                    &self.named[shape.named.offset()..shape.named.offset() + shape.named.len()]
                {
                    self.collect(member.ty, out);
                }
            }
            TypeKind::Never
            | TypeKind::Dynamic
            | TypeKind::Void
            | TypeKind::Invalid
            | TypeKind::Null => {}
        }
    }
}

fn len_u32(len: usize) -> u32 {
    u32::try_from(len).expect("pool exceeds u32::MAX")
}

fn discriminant(kind: TypeKind) -> u64 {
    match kind {
        TypeKind::Never => 0,
        TypeKind::Dynamic => 1,
        TypeKind::Void => 2,
        TypeKind::Invalid => 3,
        TypeKind::Null => 4,
        TypeKind::Interface { .. } => 5,
        TypeKind::TypeParameter { .. } => 6,
        TypeKind::Function { .. } => 7,
        TypeKind::FutureOr { .. } => 8,
        TypeKind::Record { .. } => 9,
    }
}

/// Sorts named members by name (canonical order) and rejects duplicates.
fn normalize_named(mut named: Vec<NamedMember>) -> Result<Vec<NamedMember>, TypeError> {
    named.sort_by(|a, b| a.name.cmp(&b.name));
    for pair in named.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(TypeError::DuplicateNamedMember(pair[0].name.clone()));
        }
    }
    Ok(named)
}

fn mix(mut h: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Mixes one word into the structural hash: a splitmix64 finalizer on
/// `value`, folded into the FNV chain. Word-at-a-time replaces the former
/// byte loop — same role, a fraction of the cost, and stronger avalanche.
/// Only internal ids and flags feed it, so nothing observable changes:
/// buckets are disambiguated by [`TableState::structurally_equal`]
/// regardless of the hash values.
fn mix_u64(h: u64, value: u64) -> u64 {
    let mut z = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z ^= z >> 30;
    z = z.wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (h ^ z).wrapping_mul(FNV_PRIME)
}
