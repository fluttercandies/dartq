//! Canonical type interning (spec §4.3).
//!
//! The canonical key of a type is its structure: the table maps structural
//! equality onto a single [`TypeId`], mirroring the analyzer's structural
//! `==` (`type.dart:876-889`; `isStructurallyEqualTo` delegates to `==`,
//! type.dart:1589). Construction must go through the `intern*` entries —
//! the dartq counterpart of the analyzer's warning against low-level
//! `withNullability` manipulation (type.dart:1608-1612) — so that unit-kind
//! nullability, named-member order, and required counts are normalized
//! before a type becomes observable.

use std::cell::RefCell;
use std::cmp::Ordering;
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

/// The substitution map behind a [`TableState::substitute`] walk: either a
/// caller-owned slice, a live zip of parameter names with the argument pool
/// (rule-12 instantiation) that reads replacements straight from the pool
/// inside the walk's single `borrow_mut` — no intermediate map `Vec` — or a
/// live rename of one signature's declared parameters to another's
/// (function-subtype correlation) that interns its replacements on demand.
enum Subst<'a> {
    Slice(&'a [(TypeParamId, TypeId)]),
    Zip {
        params: &'a [TypeParamId],
        args: PoolRange,
    },
    Rename {
        from: PoolRange,
        to: PoolRange,
    },
}

impl Subst<'_> {
    /// Whether `parameter` is a key — the read-only question the
    /// substitution pre-scans ask. The `Rename` form answers from the pool
    /// without interning anything.
    fn has_key(&self, state: &TableState, parameter: TypeParamId) -> bool {
        match self {
            Subst::Slice(map) => map.iter().any(|(p, _)| *p == parameter),
            Subst::Zip { params, args } => params.iter().take(args.len()).any(|p| *p == parameter),
            Subst::Rename { from, .. } => {
                let lo = from.offset();
                state.fn_type_parameters[lo..lo + from.len()].contains(&parameter)
            }
        }
    }

    /// The replacement for `parameter`, if it is a key. The `Zip` form
    /// truncates at the argument range exactly like `zip` does, so a short
    /// range leaves the tail unsubstituted as the eager map would. The
    /// `Rename` form interns its replacement on demand — a `TypeParameter`
    /// over the correlated parameter — so the function-subtype hot path
    /// materializes no intermediate map; interning is canonical, so repeat
    /// hits are O(1) lookups.
    fn replacement(&self, state: &mut TableState, parameter: TypeParamId) -> Option<TypeId> {
        match self {
            Subst::Slice(map) => map.iter().find(|(p, _)| *p == parameter).map(|&(_, r)| r),
            Subst::Zip { params, args } => params
                .iter()
                .take(args.len())
                .position(|p| *p == parameter)
                .map(|index| state.args[args.offset() + index]),
            Subst::Rename { from, to } => {
                let lo = from.offset();
                let index = state.fn_type_parameters[lo..lo + from.len()]
                    .iter()
                    .position(|p| *p == parameter)?;
                let correlated = state.fn_type_parameters[to.offset() + index];
                Some(state.intern_kind(
                    UninternedType::TypeParameter {
                        parameter: correlated,
                    },
                    false,
                ))
            }
        }
    }
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

    /// Re-interns an interned type with the given projection — the table
    /// primitive behind [`crate::TypeSystem::with_nullability`]. A nullability
    /// flip is one bit on the stored structure: the payload pools are already
    /// interned, so a dedup hit returns the existing id allocation-free and a
    /// miss shares them instead of copying. Unit kinds normalize:
    /// `dynamic` returns itself (type.dart:90-92), `Never?` collapses to
    /// `Null` (type.dart:1207-1209).
    #[must_use]
    pub(crate) fn with_nullability(&self, id: TypeId, nullable: bool) -> TypeId {
        self.state.borrow_mut().with_nullability(id, nullable)
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

    /// Orders the names of the named members at two pool indexes — both read
    /// under one borrow, no `String` cloned (the function-subtype walk
    /// compares one name pair per two-pointer step).
    ///
    /// # Panics
    /// Panics when an index is out of range.
    #[must_use]
    pub(crate) fn named_names_cmp(&self, x: usize, y: usize) -> Ordering {
        let st = self.state.borrow();
        st.named[x].name.cmp(&st.named[y].name)
    }

    /// The named member's type at a pool index.
    ///
    /// # Panics
    /// Panics when the index is out of range.
    #[must_use]
    pub(crate) fn named_ty(&self, index: usize) -> TypeId {
        self.state.borrow().named[index].ty
    }

    /// The named member's name at a pool index (cloned: the borrow guard
    /// must not escape; the record-LUB merge is a cold path).
    ///
    /// # Panics
    /// Panics when the index is out of range.
    #[must_use]
    pub(crate) fn named_name(&self, index: usize) -> String {
        self.state.borrow().named[index].name.clone()
    }

    /// The named member's `required` flag at a pool index.
    ///
    /// # Panics
    /// Panics when the index is out of range.
    #[must_use]
    pub(crate) fn named_required(&self, index: usize) -> bool {
        self.state.borrow().named[index].required
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

    /// The five side-pool lengths — `[args, named, signatures, shapes,
    /// fn_type_parameters]`. Test observability for the dedup-rollback
    /// contract: a dedup hit must reclaim the freshly appended payload, so
    /// re-interning known types must leave every length stable.
    #[cfg(test)]
    pub(crate) fn side_pool_lens(&self) -> [usize; 5] {
        let st = self.state.borrow();
        [
            st.args.len(),
            st.named.len(),
            st.signatures.len(),
            st.shapes.len(),
            st.fn_type_parameters.len(),
        ]
    }

    /// Substitutes type parameters throughout `ty`, interning the result.
    ///
    /// `map` entries replace [`TypeKind::TypeParameter`] references. A mapped
    /// replacement unites nullabilities with the node's own `?`: `T?=X!` is
    /// `X?` (type_algebra.dart:97-106, `visitTypeParameterType`:592-602).
    /// Function-declared type parameters are never substituted: ids are
    /// globally unique per declaration, so a binder's parameters cannot
    /// appear as map keys (no capture-avoidance needed).
    #[must_use]
    pub fn substitute(&self, ty: TypeId, map: &[(TypeParamId, TypeId)]) -> TypeId {
        if map.is_empty() {
            return ty;
        }
        self.state.borrow_mut().substitute(ty, &Subst::Slice(map))
    }

    /// Substitutes through `ty` with the function-subtype correlation
    /// (`relateTypeParameters`, subtype.dart:87-96): `from`'s declared
    /// parameters are renamed positionally to `to`'s. The walk reads both
    /// parameter ranges live and interns each replacement on demand, so the
    /// per-query hot path materializes no intermediate map. `from` and `to`
    /// must have equal lengths (the caller checks the signature arities).
    pub(crate) fn substitute_rename(&self, ty: TypeId, from: PoolRange, to: PoolRange) -> TypeId {
        if from.is_empty() {
            return ty;
        }
        self.state
            .borrow_mut()
            .substitute(ty, &Subst::Rename { from, to })
    }

    /// Instantiates a supertype template `edge` by substituting `params`
    /// positionally with the pooled arguments `args` — one call per
    /// supertype edge per interface-walk step (the
    /// `Substitution.fromInterfaceType` of subtype.dart:213-214). The map is
    /// read as a live zip of `params` with the argument pool inside a single
    /// `borrow_mut`, so the walk materializes no intermediate `Vec`.
    ///
    /// # Panics
    /// Panics when `args` is out of range.
    pub(crate) fn instantiate(
        &self,
        edge: TypeId,
        params: &[TypeParamId],
        args: PoolRange,
    ) -> TypeId {
        if params.is_empty() {
            return edge;
        }
        self.state
            .borrow_mut()
            .substitute(edge, &Subst::Zip { params, args })
    }

    /// Collects every type parameter referenced anywhere in `ty`'s structure,
    /// including function-declared ones.
    pub(crate) fn collect_type_parameters(&self, ty: TypeId, out: &mut BTreeSet<TypeParamId>) {
        self.state.borrow().collect(ty, out);
    }
}

impl TableState {
    fn intern_kind(&mut self, ty: UninternedType, nullable: bool) -> TypeId {
        // The mark precedes every pool append (the `Interface` arm appends
        // while constructing the kind), so `commit`'s dedup-hit rollback
        // reclaims the appended tail.
        let mark = self.mark();
        let (kind, nullable) = match ty {
            // `Never?` behaves as `Null` (type.dart:1207-1209): intern it as
            // `Null` so the bottom type stays a single non-`?` shape.
            UninternedType::Never => {
                return self.commit(
                    mark,
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
        self.commit(mark, kind, nullable)
    }

    fn intern_function(
        &mut self,
        sig: UninternedFunction,
        nullable: bool,
    ) -> Result<TypeId, TypeError> {
        // The mark precedes the appends below (parameters, arguments, named
        // members, signature) so a dedup-hit rollback reclaims all of them.
        let mark = self.mark();
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
        Ok(self.commit(mark, TypeKind::Function { signature }, nullable))
    }

    fn intern_record(
        &mut self,
        shape: UninternedRecord,
        nullable: bool,
    ) -> Result<TypeId, TypeError> {
        // The mark precedes the appends below (fields, shape) so a dedup-hit
        // rollback reclaims all of them.
        let mark = self.mark();
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
        Ok(self.commit(mark, TypeKind::Record { shape }, nullable))
    }

    /// Appends the type unconditionally, then deduplicates: a structural hit
    /// rolls everything appended since `mark` back. The mark is taken by the
    /// caller *before* the payload appends (they happen while constructing
    /// `kind`), so the rollback reclaims the appended tail. Safety: nothing
    /// published references that tail — its only referent is the fresh
    /// `types` entry the hit deduplicates away — so truncating it is sound.
    /// Pool writes are cheap; rollback restores the marks exactly.
    fn commit(&mut self, mark: PoolMark, kind: TypeKind, nullable: bool) -> TypeId {
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
        self.publish(hash, id);
        TypeId::from_raw(id)
    }

    /// Links a fresh id under its structural hash: the newest id takes the
    /// primary slot and the previous one joins the overflow bucket — the
    /// dedup-miss tail shared by [`TableState::commit`] and the nullability
    /// flip probe.
    fn publish(&mut self, hash: u64, id: u32) {
        if let Some(previous) = self.interner.insert(hash, id) {
            self.collisions
                .entry(hash)
                .or_insert_with(|| vec![previous])
                .push(id);
        }
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
        self.hash_type_with(id, self.types[id as usize].is_nullable())
    }

    /// Hashes the structure stored at `id` under an overridden nullability
    /// bit — the in-place probe behind [`TableState::with_nullability`].
    /// Reads the existing pool words only; no buffer is built.
    fn hash_type_with(&self, id: u32, nullable: bool) -> u64 {
        let ty = self.types[id as usize];
        let mut h = FNV_OFFSET;
        h = mix_u64(h, discriminant(ty.kind()));
        h = mix_u64(h, u64::from(nullable));
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
        if self.types[left as usize].is_nullable() != self.types[right as usize].is_nullable() {
            return false;
        }
        self.structure_equal(left, right)
    }

    /// Whether `candidate` equals "the structure stored at `id`, projected
    /// `nullable`" — the dedup question of the nullability flip probe, which
    /// pins the flipped bit separately because `id`'s own entry carries the
    /// source projection.
    fn equals_with_nullability(&self, candidate: u32, id: u32, nullable: bool) -> bool {
        self.types[candidate as usize].is_nullable() == nullable
            && self.structure_equal(candidate, id)
    }

    /// Kind-and-payload comparison with the nullability bit excluded — the
    /// shared body of [`TableState::structurally_equal`] and
    /// [`TableState::equals_with_nullability`].
    fn structure_equal(&self, left: u32, right: u32) -> bool {
        let left_type = self.types[left as usize];
        let right_type = self.types[right as usize];
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

    fn substitute(&mut self, ty: TypeId, map: &Subst<'_>) -> TypeId {
        let t = self.types[ty.raw() as usize];
        match t.kind() {
            TypeKind::TypeParameter { parameter } => {
                match map.replacement(self, parameter) {
                    None => ty,
                    Some(replacement) => {
                        // The official substitutor unites nullabilities
                        // (type_algebra.dart:97-106): `T?=X!` is `X?`.
                        // Unit-kind normalization still applies to the
                        // promoted result (`T?=Never` is `Null`,
                        // type.dart:1207-1209; `dynamic`/`Null` keep their
                        // no-suffix projection).
                        let replacement_nullable =
                            self.types[replacement.raw() as usize].is_nullable();
                        if replacement_nullable || !t.is_nullable() {
                            replacement
                        } else {
                            self.with_nullability(replacement, true)
                        }
                    }
                }
            }
            TypeKind::Interface { decl, args } => match self.substitute_args(args, map) {
                None => ty,
                Some(args) => {
                    self.intern_kind(UninternedType::Interface { decl, args }, t.is_nullable())
                }
            },
            TypeKind::Function { signature } => {
                let sig = self.signatures[signature.raw() as usize];
                // Each section materializes only when its read-only pre-scan
                // finds a key; `Option::is_some` doubles as the changed flag
                // (the id comparison lives in the substitute helpers).
                let positional = self.substitute_args(sig.positional, map);
                let named = self.substitute_named(sig.named, map);
                // The return type substitutes unconditionally: a mapped
                // parameter can appear *only* there (`T Function()`), so the
                // fast path must observe it too.
                let return_type = self.substitute(sig.return_type, map);
                if positional.is_some() || named.is_some() || return_type != sig.return_type {
                    let type_parameters = self.fn_type_parameters[sig.type_parameters.offset()
                        ..sig.type_parameters.offset() + sig.type_parameters.len()]
                        .to_vec();
                    self.intern_function(
                        UninternedFunction {
                            return_type,
                            type_parameters,
                            positional: positional.unwrap_or_else(|| self.args_vec(sig.positional)),
                            named: named.unwrap_or_else(|| self.named_vec(sig.named)),
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
                let positional = self.substitute_args(shape.positional, map);
                let named = self.substitute_named(shape.named, map);
                if positional.is_some() || named.is_some() {
                    self.intern_record(
                        UninternedRecord {
                            positional: positional
                                .unwrap_or_else(|| self.args_vec(shape.positional)),
                            named: named.unwrap_or_else(|| self.named_vec(shape.named)),
                        },
                        t.is_nullable(),
                    )
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

    /// Re-interns an interned type under the given projection — the
    /// target-flag generalization of the former `intern_nullable`, serving
    /// both the substitution walk's `T?=X!` union and
    /// [`TypeTable::with_nullability`]. A flip is one bit on the stored
    /// structure: the payload pools are already interned, so the dedup
    /// probe hashes the target in place (no buffer build) and a hit returns
    /// the existing id allocation-free; a miss shares the source's pool
    /// ranges / signature / shape instead of copying them. Unit-kind
    /// normalization still applies: `Never?` collapses to `Null`
    /// (type.dart:1207-1209) and `dynamic`/`void`/`Invalid`/`Null` keep
    /// their fixed no-suffix projection (type.dart:90-92).
    fn with_nullability(&mut self, id: TypeId, nullable: bool) -> TypeId {
        let ty = self.types[id.raw() as usize];
        if ty.is_nullable() == nullable {
            return id;
        }
        match ty.kind() {
            // Requesting the suffix on a fixed-projection kind lands back on
            // the same interned shape.
            TypeKind::Dynamic | TypeKind::Void | TypeKind::Invalid | TypeKind::Null => id,
            // `Never?` is a different interned kind (`Null`), so this
            // direction goes through the normalizing interner.
            TypeKind::Never => self.intern_kind(UninternedType::Never, nullable),
            TypeKind::Interface { .. }
            | TypeKind::TypeParameter { .. }
            | TypeKind::Function { .. }
            | TypeKind::FutureOr { .. }
            | TypeKind::Record { .. } => {
                let raw = id.raw();
                let hash = self.hash_type_with(raw, nullable);
                if let Some(&candidate) = self.interner.get(&hash)
                    && self.equals_with_nullability(candidate, raw, nullable)
                {
                    return TypeId::from_raw(candidate);
                }
                if let Some(bucket) = self.collisions.get(&hash) {
                    for &candidate in bucket {
                        if self.equals_with_nullability(candidate, raw, nullable) {
                            return TypeId::from_raw(candidate);
                        }
                    }
                }
                // Miss: the new type shares the already-interned payload.
                let new_id = u32::try_from(self.types.len()).expect("type pool exceeds u32::MAX");
                self.types.push(Type::new(ty.kind(), nullable));
                self.publish(hash, new_id);
                TypeId::from_raw(new_id)
            }
        }
    }

    /// Copies a pooled argument range.
    fn args_vec(&self, range: PoolRange) -> Vec<TypeId> {
        self.args[range.offset()..range.offset() + range.len()].to_vec()
    }

    /// Copies a pooled named-member range (cloning each `String` name).
    fn named_vec(&self, range: PoolRange) -> Vec<NamedMember> {
        self.named[range.offset()..range.offset() + range.len()].to_vec()
    }

    /// Substitutes through an argument range; `None` when nothing changed.
    /// A read-only pre-scan gates the copy, and the entries are re-read from
    /// the pool by index (pools are append-only, so indexes survive the
    /// interning that substitution may perform), so the no-match path
    /// allocates nothing and the changed path copies once, not twice.
    fn substitute_args(&mut self, range: PoolRange, map: &Subst<'_>) -> Option<Vec<TypeId>> {
        if !self.range_mentions_key(range, map) {
            return None;
        }
        let mut args = Vec::with_capacity(range.len());
        let mut changed = false;
        for index in range.offset()..range.offset() + range.len() {
            let arg = self.args[index];
            let substituted = self.substitute(arg, map);
            changed |= substituted != arg;
            args.push(substituted);
        }
        changed.then_some(args)
    }

    /// Substitutes through a named-member range; `None` when nothing
    /// changed. The copy carries a `String` per member, so it is gated on
    /// the same pre-scan; `Option::is_some` doubles as the changed flag.
    fn substitute_named(&mut self, range: PoolRange, map: &Subst<'_>) -> Option<Vec<NamedMember>> {
        if !self.named_mentions_key(range, map) {
            return None;
        }
        let mut members = self.named_vec(range);
        let mut changed = false;
        for member in &mut members {
            let substituted = self.substitute(member.ty, map);
            if substituted != member.ty {
                member.ty = substituted;
                changed = true;
            }
        }
        changed.then_some(members)
    }

    /// Read-only pre-scan for [`TableState::substitute_args`]: whether any
    /// type in the pooled argument range mentions a substitution key. A miss
    /// proves the substitution cannot change any id — changes originate only
    /// at `TypeParameter` nodes whose parameter is a key — so the caller
    /// skips materializing the copy.
    fn range_mentions_key(&self, range: PoolRange, map: &Subst<'_>) -> bool {
        self.args[range.offset()..range.offset() + range.len()]
            .iter()
            .any(|&arg| self.mentions_key(arg, map))
    }

    /// Same pre-scan over a named-member range.
    fn named_mentions_key(&self, range: PoolRange, map: &Subst<'_>) -> bool {
        self.named[range.offset()..range.offset() + range.len()]
            .iter()
            .any(|member| self.mentions_key(member.ty, map))
    }

    /// Whether `ty`'s reachable structure contains a `TypeParameter` node
    /// whose parameter is a key of `map`. Mirrors
    /// [`TableState::substitute`]'s recursion exactly — function binders are
    /// skipped because their ids are globally unique per declaration and can
    /// never be substitution keys — but touches nothing.
    fn mentions_key(&self, ty: TypeId, map: &Subst<'_>) -> bool {
        match self.types[ty.raw() as usize].kind() {
            TypeKind::TypeParameter { parameter } => map.has_key(self, parameter),
            TypeKind::Interface { args, .. } => self.range_mentions_key(args, map),
            TypeKind::Function { signature } => {
                let sig = &self.signatures[signature.raw() as usize];
                self.mentions_key(sig.return_type, map)
                    || self.range_mentions_key(sig.positional, map)
                    || self.named_mentions_key(sig.named, map)
            }
            TypeKind::FutureOr { base } => self.mentions_key(base, map),
            TypeKind::Record { shape } => {
                let shape = &self.shapes[shape.raw() as usize];
                self.range_mentions_key(shape.positional, map)
                    || self.named_mentions_key(shape.named, map)
            }
            TypeKind::Never
            | TypeKind::Dynamic
            | TypeKind::Void
            | TypeKind::Invalid
            | TypeKind::Null => false,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::TypeDeclId;

    /// Rule-12 instantiation fixture: a one-parameter owner and a one-argument
    /// wrapper edge, with the map values taken from `Owner<int>`'s pool range.
    fn instantiate_fixture() -> (
        TypeTable,
        TypeDeclId, // wrapper edge's declaration
        TypeId,     // int
        TypeId,     // owner param (the substitution key)
        TypeId,     // a foreign parameter (never a key)
        PoolRange,  // argument range of `Owner<int>`
    ) {
        let table = TypeTable::new();
        let (int_decl, owner_decl, wrapper_decl) = (
            TypeDeclId::from_raw(1),
            TypeDeclId::from_raw(2),
            TypeDeclId::from_raw(3),
        );
        let (key, foreign) = (TypeParamId::from_raw(1), TypeParamId::from_raw(2));
        let int = table.intern(
            UninternedType::Interface {
                decl: int_decl,
                args: Vec::new(),
            },
            false,
        );
        let key_ty = table.intern(UninternedType::TypeParameter { parameter: key }, false);
        let foreign_ty = table.intern(UninternedType::TypeParameter { parameter: foreign }, false);
        let current = table.intern(
            UninternedType::Interface {
                decl: owner_decl,
                args: vec![int],
            },
            false,
        );
        let TypeKind::Interface { args, .. } = table.get(current).kind() else {
            panic!("expected an interface type");
        };
        (table, wrapper_decl, int, key_ty, foreign_ty, args)
    }

    /// The dedup-hit rollback pin: re-interning a known compound type reaches
    /// `commit` *after* the payload (arguments, named members, signature or
    /// shape, declared parameters) was appended, so the hit must roll every
    /// side pool back. The pre-fix mark sat inside `commit`, past the
    /// appends, and each dedup hit orphaned the tail — e.g. one `args` slot
    /// per repeated `instantiate` query, forever.
    #[test]
    fn dedup_hit_rolls_back_the_appended_side_pool_tail() {
        let table = TypeTable::new();
        let (decl, param) = (TypeDeclId::from_raw(1), TypeParamId::from_raw(1));
        let int = table.intern(
            UninternedType::Interface {
                decl,
                args: Vec::new(),
            },
            false,
        );
        let fn_input = || UninternedFunction {
            return_type: int,
            type_parameters: vec![param],
            positional: vec![int, int],
            named: vec![NamedMember {
                name: "x".into(),
                ty: int,
                required: false,
            }],
            required_positional: 1,
        };
        let record_input = || UninternedRecord {
            positional: vec![int],
            named: vec![NamedMember {
                name: "y".into(),
                ty: int,
                required: false,
            }],
        };
        let fn_t = table.intern_function(fn_input(), false).expect("valid");
        let record = table.intern_record(record_input(), false).expect("valid");
        let list_int = table.intern(
            UninternedType::Interface {
                decl,
                args: vec![int],
            },
            false,
        );

        // Second interns are pure dedup hits: identical ids, and every side
        // pool restored to its pre-intern length.
        let (len, lens) = (table.len(), table.side_pool_lens());
        let fn_again = table.intern_function(fn_input(), false).expect("valid");
        assert_eq!(fn_again, fn_t);
        assert_eq!(
            table.side_pool_lens(),
            lens,
            "function dedup hit must reclaim the parameter/argument/named/signature tail"
        );
        let record_again = table.intern_record(record_input(), false).expect("valid");
        assert_eq!(record_again, record);
        assert_eq!(
            table.side_pool_lens(),
            lens,
            "record dedup hit must reclaim the argument/named/shape tail"
        );
        let list_again = table.intern(
            UninternedType::Interface {
                decl,
                args: vec![int],
            },
            false,
        );
        assert_eq!(list_again, list_int);
        assert_eq!(
            table.side_pool_lens(),
            lens,
            "interface dedup hit must reclaim the argument tail"
        );
        assert_eq!(table.len(), len, "dedup hits must not grow the type pool");
    }

    /// Real 64-bit structural collisions are impractical to construct from
    /// the outside, so the test fabricates the exact state one produces:
    /// the primary slot for a hash is stolen by a structurally different
    /// type and the true owner moves to the overflow bucket. Re-interning
    /// that owner must miss on the primary slot, disambiguate through the
    /// bucket scan, and dedup instead of appending a twin.
    #[test]
    fn structural_hash_collision_resolves_through_the_overflow_bucket() {
        let mut st = TableState::default();
        let never = st.intern_kind(UninternedType::Never, false);
        let dynamic = st.intern_kind(UninternedType::Dynamic, false);
        let hash = st.hash_type(never.raw());
        st.interner.insert(hash, dynamic.raw());
        st.collisions.insert(hash, vec![never.raw()]);

        let again = st.intern_kind(UninternedType::Never, false);
        assert_eq!(again, never);
        assert_eq!(st.types.len(), 2);
        // The dedup hit must not append to the bucket.
        assert_eq!(st.collisions.get(&hash).map(Vec::len), Some(1));
    }

    /// Instantiation pre-scan, no-match side: an edge whose structure
    /// mentions no map key must come back as the template's own id — no
    /// re-intern, no materialized copies.
    #[test]
    fn instantiate_without_a_key_mention_returns_the_template_id() {
        let (table, wrapper, _int, _key_ty, foreign_ty, args) = instantiate_fixture();
        let edge = table.intern(
            UninternedType::Interface {
                decl: wrapper,
                args: vec![foreign_ty],
            },
            false,
        );
        assert_eq!(
            table.instantiate(edge, &[TypeParamId::from_raw(1)], args),
            edge
        );
    }

    /// Instantiation pre-scan, match side: an edge that does mention the key
    /// substitutes positionally from the argument pool.
    #[test]
    fn instantiate_substitutes_edges_that_mention_the_key() {
        let (table, wrapper, int, key_ty, _foreign_ty, args) = instantiate_fixture();
        let edge = table.intern(
            UninternedType::Interface {
                decl: wrapper,
                args: vec![key_ty],
            },
            false,
        );
        let expected = table.intern(
            UninternedType::Interface {
                decl: wrapper,
                args: vec![int],
            },
            false,
        );
        assert_eq!(
            table.instantiate(edge, &[TypeParamId::from_raw(1)], args),
            expected
        );
    }

    /// Nullability flips dedup against the interned `?` twin: the first flip
    /// in each direction may intern one type, every further flip is the same
    /// id with zero pool growth — a flip is a one-bit change, so the hit path
    /// must be allocation-free. Covers the payload-sharing kinds (parameter,
    /// interface with arguments, function with a named parameter — the kind
    /// whose old path cloned a `String` per member) and the fixed-projection
    /// unit kinds.
    #[test]
    fn flip_nullability_dedups_without_pool_growth() {
        let table = TypeTable::new();
        let decl = TypeDeclId::from_raw(1);
        let t = table.intern(
            UninternedType::TypeParameter {
                parameter: TypeParamId::from_raw(1),
            },
            false,
        );
        let itf_t = table.intern(
            UninternedType::Interface {
                decl,
                args: vec![t],
            },
            false,
        );
        let fn_t = table
            .intern_function(
                UninternedFunction {
                    return_type: t,
                    type_parameters: Vec::new(),
                    positional: vec![t],
                    named: vec![NamedMember {
                        name: "x".into(),
                        ty: t,
                        required: false,
                    }],
                    required_positional: 1,
                },
                false,
            )
            .expect("valid");

        // First flips intern the `?` twins; flipping back returns the
        // original ids.
        let tq = table.with_nullability(t, true);
        let itf_q = table.with_nullability(itf_t, true);
        let fn_q = table.with_nullability(fn_t, true);
        assert_ne!(tq, t);
        assert_ne!(itf_q, itf_t);
        assert_ne!(fn_q, fn_t);
        assert_eq!(table.with_nullability(tq, false), t);
        assert_eq!(table.with_nullability(itf_q, false), itf_t);
        assert_eq!(table.with_nullability(fn_q, false), fn_t);

        // Fixed-projection unit kinds: `dynamic` keeps its no-suffix shape;
        // `Never?` collapses to `Null` once and stays there.
        let dynamic = table.intern(UninternedType::Dynamic, false);
        let never = table.intern(UninternedType::Never, false);
        assert_eq!(table.with_nullability(dynamic, true), dynamic);
        let null = table.with_nullability(never, true);
        assert_ne!(null, never);
        assert_eq!(table.with_nullability(null, true), null);
        assert_eq!(table.with_nullability(null, false), null);

        // Repeated flips in both directions: identical ids, zero pool growth.
        let len = table.len();
        for _ in 0..100 {
            assert_eq!(table.with_nullability(t, true), tq);
            assert_eq!(table.with_nullability(tq, false), t);
            assert_eq!(table.with_nullability(itf_t, true), itf_q);
            assert_eq!(table.with_nullability(itf_q, false), itf_t);
            assert_eq!(table.with_nullability(fn_t, true), fn_q);
            assert_eq!(table.with_nullability(fn_q, false), fn_t);
            assert_eq!(table.with_nullability(dynamic, true), dynamic);
            assert_eq!(table.with_nullability(never, true), null);
        }
        assert_eq!(table.len(), len, "flip dedup must not grow the type pool");
    }
}
