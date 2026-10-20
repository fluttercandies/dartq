//! The M1 element model: what one compilation unit declares, in interned-id
//! form (ticket #119, implementing the four-essential model adjudicated in
//! #131).
//!
//! A [`FileElements`] records exactly four things about its unit:
//!
//! 1. the top-level name set ([`FileElements::decl`]) with a
//!    [`MicroKind`] tag — the adjudicated closed set {class, enum, mixin,
//!    extension, typeAlias, prefix, topLevelFn, topLevelVar, typeParam}
//!    (`prefix` never originates here; it enters a library face from import
//!    edges, see [`crate::library`]);
//! 2. per-container member name sets ([`FileElements::container`]) —
//!    method/getter/setter/field, each with a static/instance bit;
//! 3. direct superclass **name** edges ([`Container::superclass`]) — no
//!    resolved type at M1; `p.S` keeps the bare name `S`;
//! 4. one formal-parameter table per executable
//!    ([`TopDecl::formals`], [`MemberEntry::formals`]) — consumers
//!    `undefined_named_parameter` (named membership) and
//!    `argument_type_not_assignable` (parameter nullability).
//!
//! Batch 2 of the diagnostic tickets (#121) adds the declaration-side facts
//! the wrong-number and static-nullability codes decide on: type-parameter
//! counts, declared [`Ty`]s (fields, variables, formals, return types), and
//! the declaration-arity violation log ([`FileElements::arity_violations`],
//! the [`DuplicateDecl`] pattern: capture-time log, diagnose-time fold).
//!
//! # Shape discipline
//!
//! * Names are [`StringId`]s; lookups are binary searches over dense sorted
//!   rows ([`NameMap`]) — no string work, no per-name hash buckets, and a
//!   name-keyed row costs exactly its key + value bytes. A model is sized
//!   entirely by its unit's declarations; nothing grows after capture.
//! * Duplicate names keep the **first** entry: a scope's names are unique by
//!   construction, and duplicates are the diagnostic tickets' `duplicate_*`
//!   business, not a modeling event.
//! * Setters are stored under their `name=` spelling, matching the
//!   resolution layer's export-scope convention
//!   (`summary2/library_builder.dart:136-140`). Top-level getters/setters
//!   fold into `topLevelFn` (the adjudicated kind set has no finer tag);
//!   class-level setters are tagged [`MemberKind::Setter`].
//! * Type parameters land in the per-unit name set tagged
//!   [`MicroKind::TypeParam`] (the adjudicated set includes the tag). They
//!   are declaration-scoped in truth; consumers must not treat them as
//!   top-level declarations.

use dartq_source::{SourceRange, StringId, StringTable, saturate_slot_u32};
use rustc_hash::FxHashMap;
use std::borrow::Cow;
use std::hash::{Hash, Hasher};

/// The M1 static type: the nullability dimension plus a name — the minimal
/// shape the static-nullability codes decide on (the analyzer's `InvalidType`
/// suppresses every question it
/// touches, `subtype.dart:258-260`).
///
/// This is the M1 stand-in for the `dartq-types` interned lattice: explicit
/// type arguments ride along (the #611 verdict-A slot — a canonical
/// [`ArgListId`] handle so `Ty` stays `Copy`), but there is still no
/// hierarchy, no inference — declared types only. When the M2 type engine
/// wires in, `Ty` folds into `TypeId` and `assignable_from` becomes
/// `TypeSystem::is_assignable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ty {
    /// No declared type (or a shape M1 does not model): every check
    /// suppresses. Mirrors the oracle's `InvalidType` (rule 0c).
    Unknown,
    /// `Null` — the type of the `null` literal. Semantically nullable
    /// without a suffix (`type.dart:1276`).
    Null,
    /// `dynamic` — top type, semantically nullable (`type.dart:33`).
    Dynamic,
    /// `Never` — the bottom type: the static type of a `throw` expression
    /// (the resolver records `NeverTypeImpl`,
    /// `resolver_visitor.dart` `visitThrowExpression`) and assignable to
    /// every target (`subtype.dart` Right Bottom). Never an annotation.
    Never,
    /// `void` — top type for the return matrix; never an expression type.
    Void,
    /// A named type as written (`int`, `C`, `p.C` keeps `C`, a type
    /// parameter keeps its name), with the syntactic `?` flag. External
    /// names (`int`, `String`) stay here — the nullability rules never
    /// need their members.
    Named {
        /// The declared name as written (display id).
        name: StringId,
        /// The trailing `?` was present.
        nullable: bool,
        /// The explicit type arguments, canonically interned
        /// ([`ArgListId::EMPTY`] when absent). Equal lists share one id,
        /// so derived equality is structural — `List<int> ==
        /// List<int>` across interning orders.
        args: ArgListId,
    },
    /// A record type as written (`(int, String)`, `(int, {String s})?`):
    /// the positional field list plus the named field set, canonically
    /// interned ([`RecordShapeId`]). Recovered comma holes ride the field
    /// list as [`Ty::Unknown`] (`(int, ,)` binds `(int, InvalidType)` —
    /// the official recovery splices an `InvalidType` field, so the shape
    /// stays a record rather than collapsing into a whole-type invalid).
    Record {
        /// The interned field shape (positional order + named set).
        shape: RecordShapeId,
        /// The trailing `?` was present.
        nullable: bool,
    },
    /// A function type as written (`int Function(String)`,
    /// `void Function()?`), canonically interned ([`FnShapeId`]). The bare
    /// `Function` head without a parameter group is a named type (the
    /// parser fires a plain type completion for it), never this variant.
    Function {
        /// The interned shape (return type + formal columns).
        shape: FnShapeId,
        /// The trailing `?` was present.
        nullable: bool,
    },
}

impl Ty {
    /// The every-check-suppresses sentinel.
    pub const UNKNOWN: Ty = Ty::Unknown;

    /// A non-nullable named type without type arguments — the argument-less
    /// shape every pre-#701 construction site keeps ([`ArgPool::named`] is
    /// the with-arguments constructor).
    #[must_use]
    pub const fn named(name: StringId) -> Ty {
        Ty::Named {
            name,
            nullable: false,
            args: ArgListId::EMPTY,
        }
    }

    /// Renders the type in the official display form (`List<int>?`): the
    /// head name, the explicit arguments `, `-joined, then the `?`
    /// projection (the analyzer's `getDisplayName`, type.dart:1425-1467).
    /// Record fields and named formals display in name-byte order — a
    /// deterministic total order over the interned texts, never the pool's
    /// intern-arrival [`StringId`] order (see [`display_named`]).
    /// [`Ty::Unknown`] renders empty — the diagnoser's message slots for
    /// unknown declared types.
    #[must_use]
    pub fn render(&self, strings: &StringTable, args: &ArgPool) -> String {
        match *self {
            Ty::Unknown => String::new(),
            Ty::Null => "Null".to_string(),
            Ty::Dynamic => "dynamic".to_string(),
            Ty::Never => "Never".to_string(),
            Ty::Void => "void".to_string(),
            Ty::Record { shape, nullable } => {
                let shape = args.record_shape(shape);
                let mut text = String::from("(");
                let mut first = true;
                for ty in shape.positional.iter() {
                    if !first {
                        text.push_str(", ");
                    }
                    first = false;
                    // A `Ty::Unknown` field is the recovery's `InvalidType`
                    // splice — inside a record shape it is the only way the
                    // slot exists, so the display takes the oracle's name
                    // (`(int, InvalidType)`, the official `getDisplayString`).
                    text.push_str(&render_record_field(*ty, strings, args));
                }
                if !shape.named.is_empty() {
                    if !first {
                        text.push_str(", ");
                    }
                    text.push('{');
                    for (index, (name, ty)) in
                        display_named(&shape.named, strings).iter().enumerate()
                    {
                        if index > 0 {
                            text.push_str(", ");
                        }
                        text.push_str(&render_record_field(*ty, strings, args));
                        text.push(' ');
                        text.push_str(strings.resolve(*name));
                    }
                    text.push('}');
                } else if shape.positional.len() == 1 {
                    // A lone positional field keeps its trailing comma —
                    // `(int,)` is the official `RecordType` display.
                    text.push(',');
                }
                text.push(')');
                if nullable {
                    text.push('?');
                }
                text
            }
            Ty::Function { shape, nullable } => {
                // The official `FunctionType` display: `T Function(...)`,
                // the type-parameter group rides uncaptured (the shape
                // carries the count, not the names — the count is hashed
                // and persisted, the display under-reports it).
                let shape = args.function_shape(shape);
                let mut text = shape.return_ty.render(strings, args);
                text.push_str(" Function(");
                for (index, ty) in shape.positional.iter().enumerate() {
                    if index > 0 {
                        text.push_str(", ");
                    }
                    text.push_str(&render_record_field(*ty, strings, args));
                }
                if !shape.named.is_empty() {
                    if !shape.positional.is_empty() {
                        text.push_str(", ");
                    }
                    text.push('{');
                    for (index, (name, ty)) in
                        display_named(&shape.named, strings).iter().enumerate()
                    {
                        if index > 0 {
                            text.push_str(", ");
                        }
                        text.push_str(&render_record_field(*ty, strings, args));
                        text.push(' ');
                        text.push_str(strings.resolve(*name));
                    }
                    text.push('}');
                }
                text.push(')');
                if nullable {
                    text.push('?');
                }
                text
            }
            Ty::Named {
                name,
                nullable,
                args: list,
            } => {
                let mut text = strings.resolve(name).to_owned();
                let list = args.get(list);
                if !list.is_empty() {
                    text.push('<');
                    for (index, arg) in list.iter().enumerate() {
                        if index > 0 {
                            text.push_str(", ");
                        }
                        text.push_str(&arg.render(strings, args));
                    }
                    text.push('>');
                }
                if nullable {
                    text.push('?');
                }
                text
            }
        }
    }
}

/// One record field's display: the recovered comma hole is the analyzer's
/// `InvalidType` splice and renders under its own name (`(int,
/// InvalidType)`); every other field renders its ordinary shape.
fn render_record_field(ty: Ty, strings: &StringTable, args: &ArgPool) -> String {
    match ty {
        Ty::Unknown => "InvalidType".to_string(),
        _ => ty.render(strings, args),
    }
}

/// The named pairs in display order: byte order over the interned texts.
///
/// The pool stores the pairs sorted by [`StringId`] — intern-arrival order,
/// because the string table is shared across the parallel fan-out, so which
/// worker interned a name first decides the ids. Sorting on the ids at
/// intern therefore buys within-run canonicality (equal name sets share one
/// slot) but not across-run stability: the same shape renders differently
/// between runs. The display derives its own total order from the texts —
/// the only scheduling-independent key — and borrows the slice untouched
/// when it is already in that order.
fn display_named<'a>(
    named: &'a [(StringId, Ty)],
    strings: &StringTable,
) -> Cow<'a, [(StringId, Ty)]> {
    if named
        .windows(2)
        .all(|pair| strings.resolve(pair[0].0) <= strings.resolve(pair[1].0))
    {
        Cow::Borrowed(named)
    } else {
        let mut sorted = named.to_vec();
        sorted.sort_by(|a, b| strings.resolve(a.0).cmp(strings.resolve(b.0)));
        Cow::Owned(sorted)
    }
}

impl Ty {
    /// `type_system.dart:1921-` `isStrictlyNonNullable`: false for
    /// `dynamic`, `void`, `Null`, the `?` projection, and unknowns; true
    /// for `Never` (the bottom type declares no null) — the
    /// `invalid_null_aware_operator` receiver test.
    #[must_use]
    pub const fn is_strictly_non_nullable(self) -> bool {
        matches!(self, Ty::Never)
            || matches!(
                self,
                Ty::Named {
                    nullable: false,
                    ..
                } | Ty::Record {
                    nullable: false,
                    ..
                } | Ty::Function {
                    nullable: false,
                    ..
                }
            )
    }

    /// The M1 `isAssignableTo` (`type_system.dart:891-931`): `None` means
    /// the M1 facts cannot decide (oracle would consult the type lattice or
    /// flow analysis) — the caller suppresses, the under-report direction.
    /// `Some(false)` is decidable ONLY from nullability, never from two
    /// distinct named types (a subclass relation could hold).
    #[must_use]
    pub fn assignable_from(self, from: Ty) -> Option<bool> {
        match (from, self) {
            // Unknowns suppress both ways (rule 0c); `void` sources are
            // `use_of_void_result`'s business (unfrozen).
            (Ty::Unknown | Ty::Void, _) | (_, Ty::Unknown) => None,
            // Right Bottom (`subtype.dart`): `Never <: T` for every T —
            // the throw expression's type assigns everywhere.
            (Ty::Never, _) => Some(true),
            // Right Top (`subtype.dart:263-267`): everything assigns into
            // `dynamic`/`void`/`T?` — including `Null` and `void` sources;
            // and `dynamic` assigns into everything (`type_system.dart:930`,
            // non-strict-casts mode — the `strict-casts` overlay rejects the
            // downcast side at the diagnoser, [`crate::diagnose`]'s
            // `Engine::assignable`).
            (Ty::Dynamic, _)
            | (
                _,
                Ty::Dynamic
                | Ty::Void
                | Ty::Named { nullable: true, .. }
                | Ty::Record { nullable: true, .. }
                | Ty::Function { nullable: true, .. },
            ) => Some(true),
            // Left Null (`subtype.dart:349-369`): `Null <: T1` holds only
            // for `T1 = Null` (Right Top covers that) or the Right Top
            // targets above.
            (Ty::Null, Ty::Null) => Some(true),
            // Only `Null` (and `Never`) assign into `Null`; and Left
            // Nullable (`subtype.dart:389-392`): `S? <: T` needs
            // `Null <: T` — false for a non-nullable named, record, or
            // function target.
            (Ty::Null, _)
            | (_, Ty::Null)
            | (
                Ty::Named { nullable: true, .. }
                | Ty::Record { nullable: true, .. }
                | Ty::Function { nullable: true, .. },
                Ty::Named {
                    nullable: false, ..
                }
                | Ty::Record {
                    nullable: false, ..
                }
                | Ty::Function {
                    nullable: false, ..
                },
            ) => Some(false),
            // A named source against the `Never` target: only `Never` (and
            // the top faces above) subtypes it — the subclass facts are the
            // M1 model's blind spot, so the pair suppresses.
            (_, Ty::Never) => None,
            // A record source against a record target: Left Nullable above
            // already rejected a `?` source into a non-`?` target, so a
            // surviving pair has a non-nullable source or a nullable
            // target. Equal interned shapes assign outright — the
            // `(int, InvalidType)` self-assignment stays silent; distinct
            // shapes need the field-wise pass the boundary pool feeds
            // ([`crate::diagnose`]'s `Engine::assignable` record arm). A
            // record against a named head suppresses (`None`) — the
            // `Record <: Object` face is the M2 lattice's, not this
            // slot's.
            (
                Ty::Record {
                    shape: a,
                    nullable: a_nullable,
                },
                Ty::Record {
                    shape: b,
                    nullable: b_nullable,
                },
            ) => {
                if a == b && (!a_nullable || b_nullable) {
                    Some(true)
                } else {
                    None
                }
            }
            // A function source against a function target: the same
            // equal-shape cut. Distinct shapes need the contravariant
            // parameter / covariant return pass (`subtype.dart:86-171`) —
            // the M1 model refuses it (undecidable ⇒ suppress), the same
            // posture [`FnSig`]'s structural cut documents.
            (
                Ty::Function {
                    shape: a,
                    nullable: a_nullable,
                },
                Ty::Function {
                    shape: b,
                    nullable: b_nullable,
                },
            ) => {
                if a == b && (!a_nullable || b_nullable) {
                    Some(true)
                } else {
                    None
                }
            }
            // Cross-family pairs the nullability arms did not settle stay
            // undecided — a named head can still be `Object` (the only
            // non-`Record`/non-`Function` supertype), so `Some(false)`
            // would lie.
            (Ty::Record { .. }, Ty::Named { .. })
            | (Ty::Named { .. }, Ty::Record { .. })
            | (Ty::Function { .. }, Ty::Named { .. })
            | (Ty::Named { .. }, Ty::Function { .. })
            | (Ty::Function { .. }, Ty::Record { .. })
            | (Ty::Record { .. }, Ty::Function { .. }) => None,
            // Same name, both non-nullable: assignable when the written
            // argument lists coincide — canonical interning makes the slot
            // comparison structural (`List<int>` vs `List<int>` across
            // interning orders), and the argument-less fast path is the
            // reserved-slot constant. Differing arguments need the pool to
            // compare element-wise; this pool-less method hands the pair to
            // the caller — [`crate::diagnose`]'s `Engine::assignable` runs
            // the covariant element rule through the boundary pool — and
            // every other caller gets the under-report direction.
            (
                Ty::Named {
                    name: a,
                    args: a_args,
                    ..
                },
                Ty::Named {
                    name: b,
                    nullable: false,
                    args: b_args,
                },
            ) => {
                if a == b && a_args == b_args {
                    Some(true)
                } else {
                    None
                }
            }
        }
    }
}

/// Identity of an interned record-type field shape inside an [`ArgPool`]
/// — [`Ty::Record`]'s slot. The same canonical-interning contract as
/// [`ArgListId`]: equal shapes share one id, so derived `Eq`/`Hash` on
/// `Ty` stays structural.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RecordShapeId(u32);

impl RecordShapeId {
    /// The reserved empty record `()` — zero positional, zero named.
    pub const EMPTY: RecordShapeId = RecordShapeId(0);

    /// The pool slot — canonical, equal written shapes share one id.
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// One interned record-type shape: the positional field types in source
/// order plus the named field set. Named fields canonicalize sorted by
/// name — record-type identity is a name *set* (`({int a, int b})` ==
/// `({int b, int a})`), so equal shapes intern to one slot regardless of
/// written order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordShape {
    /// Positional field types in source order.
    pub positional: Box<[Ty]>,
    /// Named fields `(name, type)`, sorted by [`StringId`] at intern.
    pub named: Box<[(StringId, Ty)]>,
}

/// Identity of an interned function-type shape inside an [`ArgPool`] —
/// [`Ty::Function`]'s slot. The same canonical-interning contract as
/// [`ArgListId`]/[`RecordShapeId`]: equal shapes share one id, so derived
/// `Eq`/`Hash` on `Ty` stays structural.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FnShapeId(u32);

impl FnShapeId {
    /// The reserved degenerate shape (`dynamic Function()` — nothing
    /// interned points at it; slot `0` keeps the pool's reserved-slot
    /// discipline).
    pub const EMPTY: FnShapeId = FnShapeId(0);

    /// The pool slot — canonical, equal written shapes share one id.
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// One interned function-type shape: the type-parameter count, the return
/// type, the positional formal types in source order, and the named
/// formal set. Named formals canonicalize sorted by name — named-formal
/// identity is a name *set* (the subtype rule matches named formals by
/// name, `subtype.dart:86-171`), so equal shapes intern to one slot
/// regardless of written order. Positional formals carry no names: they
/// are display decoration, not type identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnShape {
    /// The type-parameter count (`<T, U>` group).
    pub type_params: u16,
    /// The declared return type ([`Ty::Dynamic`] for a bare
    /// `Function(...)` head — the implicit return).
    pub return_ty: Ty,
    /// Positional formal types in source order.
    pub positional: Box<[Ty]>,
    /// Named formals `(name, type)`, sorted by [`StringId`] at intern.
    pub named: Box<[(StringId, Ty)]>,
}

/// Identity of an interned type-argument list inside an [`ArgPool`] —
/// [`Ty::Named`]'s argument slot (#611 verdict A: a pool handle keeps `Ty`
/// `Copy`, and canonical interning decides equality, so the slot never
/// degrades into position-comparing `PoolRange`-style fake equality).
///
/// Slot `0` is the reserved empty list: argument-less named types
/// ([`Ty::named`]) carry it without touching any pool, so the empty face is
/// one constant and the pre-slot constructors keep their exact identity.
/// `Eq`/`Hash` on this newtype *is* structural list equality — the pool
/// only hands out ids through canonical interning, the same contract as
/// `dartq_types::TypeId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArgListId(u32);

impl ArgListId {
    /// The reserved empty argument list.
    pub const EMPTY: ArgListId = ArgListId(0);
}

/// The canonical interner of type-argument lists backing
/// [`Ty::Named`]'s slot. Equal lists share one [`ArgListId`] regardless of
/// interning order or pool history, so derived equality on `Ty` is
/// structural (`List<int> == List<int>`) without any string or deep-list
/// comparison on the query path — the O(1) id comparison the interned
/// lattice uses.
///
/// One pool belongs to one model universe ([`FileElements::type_args`]):
/// ids are canonical *within their pool*, so [`Ty`] values compare equal
/// only inside the universe that interned their arguments. Arguments are
/// captured bottom-up (inner lists first), so an element's own slot is
/// already canonical when its list interns — no recursion at intern or
/// dedup time.
#[derive(Clone, Debug)]
pub struct ArgPool {
    /// Interned lists; slot `0` is the reserved empty list.
    lists: Vec<Box<[Ty]>>,
    /// Structural hash → every slot with that hash. A hit is confirmed by
    /// slice equality before reuse (64-bit hashes collide rarely; the
    /// bucket keeps the answer exact rather than probabilistic).
    at: FxHashMap<u64, Vec<u32>>,
    /// Interned record shapes; slot `0` is the reserved empty record `()`.
    records: Vec<RecordShape>,
    /// Structural hash → record slots, same confirmed-bucket discipline.
    record_at: FxHashMap<u64, Vec<u32>>,
    /// Interned function-type shapes; slot `0` is the reserved degenerate
    /// shape.
    functions: Vec<FnShape>,
    /// Structural hash → function slots, same confirmed-bucket discipline.
    function_at: FxHashMap<u64, Vec<u32>>,
}

impl Default for ArgPool {
    fn default() -> Self {
        Self {
            lists: vec![Box::new([])],
            at: FxHashMap::default(),
            records: vec![RecordShape {
                positional: Box::new([]),
                named: Box::new([]),
            }],
            record_at: FxHashMap::default(),
            functions: vec![FnShape {
                type_params: 0,
                return_ty: Ty::Dynamic,
                positional: Box::new([]),
                named: Box::new([]),
            }],
            function_at: FxHashMap::default(),
        }
    }
}

impl ArgPool {
    /// Interns `args` canonically and returns the named type
    /// `name<...>` (with its `?` when `nullable`) — the with-arguments
    /// constructor; [`Ty::named`] is its argument-less constant shortcut
    /// and produces the identical value for an empty `args`.
    pub fn named(&mut self, name: StringId, nullable: bool, args: &[Ty]) -> Ty {
        let args = self.intern(args);
        Ty::Named {
            name,
            nullable,
            args,
        }
    }

    /// Interns one list and returns its canonical slot. The empty list is
    /// the reserved slot `0` and never interns; a dedup hit reuses the
    /// stored slot without appending.
    pub(crate) fn intern(&mut self, args: &[Ty]) -> ArgListId {
        if args.is_empty() {
            return ArgListId::EMPTY;
        }
        let hash = hash_args(args);
        if let Some(slots) = self.at.get(&hash) {
            for &slot in slots {
                if &*self.lists[slot as usize] == args {
                    return ArgListId(slot);
                }
            }
        }
        let slot = saturate_slot_u32(self.lists.len());
        self.lists.push(Box::from(args));
        self.at.entry(hash).or_default().push(slot);
        ArgListId(slot)
    }

    /// The interned list at `id`.
    ///
    /// # Panics
    /// Panics when `id` did not come from this pool (an out-of-range slot
    /// is a program bug, not a recoverable condition).
    #[must_use]
    pub fn get(&self, id: ArgListId) -> &[Ty] {
        &self.lists[id.0 as usize]
    }

    /// Interns one record field shape canonically (named fields sort by
    /// name before hashing) and returns the record type `(...)` — with its
    /// `?` when `nullable`.
    pub fn record(&mut self, positional: &[Ty], named: &[(StringId, Ty)], nullable: bool) -> Ty {
        Ty::Record {
            shape: self.intern_record(positional, named),
            nullable,
        }
    }

    /// Interns the field shape and returns its canonical slot. `()` is the
    /// reserved slot `0` and never interns. Named fields canonicalize by
    /// name before hashing — record-type identity is a name set, so
    /// `({int a, int b})` and `({int b, int a})` share one slot.
    pub(crate) fn intern_record(
        &mut self,
        positional: &[Ty],
        named: &[(StringId, Ty)],
    ) -> RecordShapeId {
        if positional.is_empty() && named.is_empty() {
            return RecordShapeId::EMPTY;
        }
        let mut sorted;
        let named = if named.windows(2).all(|pair| pair[0].0 <= pair[1].0) {
            named
        } else {
            sorted = named.to_vec();
            sorted.sort_by_key(|(name, _)| *name);
            sorted.as_slice()
        };
        let hash = hash_record(positional, named);
        if let Some(slots) = self.record_at.get(&hash) {
            for &slot in slots {
                let shape = &self.records[slot as usize];
                if &*shape.positional == positional && &*shape.named == named {
                    return RecordShapeId(slot);
                }
            }
        }
        let slot = saturate_slot_u32(self.records.len());
        self.records.push(RecordShape {
            positional: Box::from(positional),
            named: Box::from(named),
        });
        self.record_at.entry(hash).or_default().push(slot);
        RecordShapeId(slot)
    }

    /// The interned record shape at `id`.
    ///
    /// # Panics
    /// Panics when `id` did not come from this pool.
    #[must_use]
    pub fn record_shape(&self, id: RecordShapeId) -> &RecordShape {
        &self.records[id.0 as usize]
    }

    /// Interns one function-type shape canonically (named formals sort by
    /// name before hashing) and returns the function type — with its `?`
    /// when `nullable`.
    pub fn function(
        &mut self,
        type_params: u16,
        return_ty: Ty,
        positional: &[Ty],
        named: &[(StringId, Ty)],
        nullable: bool,
    ) -> Ty {
        Ty::Function {
            shape: self.intern_function(type_params, return_ty, positional, named),
            nullable,
        }
    }

    /// Interns the function shape and returns its canonical slot. The
    /// degenerate `dynamic Function()` is the reserved slot `0` and never
    /// interns. Named formals canonicalize by name before hashing — the
    /// named-formal set is type identity, so `({int a})` and `({int b})`
    /// are distinct shapes while reorders share one slot.
    pub(crate) fn intern_function(
        &mut self,
        type_params: u16,
        return_ty: Ty,
        positional: &[Ty],
        named: &[(StringId, Ty)],
    ) -> FnShapeId {
        if type_params == 0 && return_ty == Ty::Dynamic && positional.is_empty() && named.is_empty()
        {
            return FnShapeId::EMPTY;
        }
        let mut sorted;
        let named = if named.windows(2).all(|pair| pair[0].0 <= pair[1].0) {
            named
        } else {
            sorted = named.to_vec();
            sorted.sort_by_key(|(name, _)| *name);
            sorted.as_slice()
        };
        let hash = hash_function(type_params, return_ty, positional, named);
        if let Some(slots) = self.function_at.get(&hash) {
            for &slot in slots {
                let shape = &self.functions[slot as usize];
                if shape.type_params == type_params
                    && shape.return_ty == return_ty
                    && &*shape.positional == positional
                    && &*shape.named == named
                {
                    return FnShapeId(slot);
                }
            }
        }
        let slot = saturate_slot_u32(self.functions.len());
        self.functions.push(FnShape {
            type_params,
            return_ty,
            positional: Box::from(positional),
            named: Box::from(named),
        });
        self.function_at.entry(hash).or_default().push(slot);
        FnShapeId(slot)
    }

    /// The interned function shape at `id`.
    ///
    /// # Panics
    /// Panics when `id` did not come from this pool.
    #[must_use]
    pub fn function_shape(&self, id: FnShapeId) -> &FnShape {
        &self.functions[id.0 as usize]
    }

    /// Number of interned non-empty argument lists.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lists.len() - 1
    }

    /// Whether no non-empty list has been interned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lists.len() == 1
    }
}

/// Folds every element's canonical identity — name, nullability, and its
/// own (already interned) argument slot — into one 64-bit hash. Children
/// are canonical, so their slot hashes their whole structure; no recursion
/// here.
fn hash_args(args: &[Ty]) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    for arg in args {
        arg.hash(&mut hasher);
    }
    hasher.finish()
}

/// The canonical-shape hash of one record type: positional children in
/// order, then the (already sorted) named pairs. Children are canonical,
/// so the id fold needs no recursion.
fn hash_record(positional: &[Ty], named: &[(StringId, Ty)]) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    positional.hash(&mut hasher);
    for (name, ty) in named {
        name.hash(&mut hasher);
        ty.hash(&mut hasher);
    }
    hasher.finish()
}

/// The canonical-shape hash of one function type: the parameter count, the
/// return type, positional children in order, then the (already sorted)
/// named pairs. Children are canonical, so the id fold needs no recursion.
fn hash_function(
    type_params: u16,
    return_ty: Ty,
    positional: &[Ty],
    named: &[(StringId, Ty)],
) -> u64 {
    let mut hasher = rustc_hash::FxHasher::default();
    type_params.hash(&mut hasher);
    return_ty.hash(&mut hasher);
    positional.hash(&mut hasher);
    for (name, ty) in named {
        name.hash(&mut hasher);
        ty.hash(&mut hasher);
    }
    hasher.finish()
}

/// The micro-kind tag of a name in a library scope (adjudicated closed set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicroKind {
    /// A class (including named mixin applications `class C = A with B;`).
    Class,
    /// An enum declaration.
    Enum,
    /// A mixin declaration.
    Mixin,
    /// An extension or extension-type declaration.
    Extension,
    /// A typedef (either style).
    TypeAlias,
    /// An import prefix (`import 'u' as p;`) — faces only, never captured
    /// from a unit.
    Prefix,
    /// A top-level function, getter, or setter (getters/setters under their
    /// lookup spelling: plain `name`, setters as `name=`).
    TopLevelFn,
    /// A top-level variable.
    TopLevelVar,
    /// A type parameter (declaration-scoped in truth; see the module docs).
    TypeParam,
}

/// The kind of a class member.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    /// A method (operator names keep their operator spelling).
    Method,
    /// A getter, under its plain name.
    Getter,
    /// A setter, under its `name=` spelling.
    Setter,
    /// A field (enum constants count as static fields).
    Field,
}

/// The declaration family of a named container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    /// A class or named mixin application.
    Class,
    /// An enum declaration.
    Enum,
    /// A mixin declaration.
    Mixin,
    /// An extension declaration (named or unnamed).
    Extension,
    /// An extension-type declaration.
    ExtensionType,
}

/// One formal parameter: name, declared type, and the flags the override
/// faces read (`ast_builder.dart:6283`: a setter passes only when the
/// single parameter is required-positional).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Formal {
    /// The parameter name.
    pub name: StringId,
    /// The declared type (`Unknown` when absent).
    pub ty: Ty,
    /// Required-positional (false for optional positional and named).
    pub required: bool,
    /// The `covariant` keyword was present (`covariant int x`): the
    /// parameter can narrow an inherited parameter's type, so a
    /// super-forwarding override body over it is not trivially removable
    /// (unnecessary_overrides.dart `_isCovariant`).
    pub covariant: bool,
    /// The default value's verbatim source text (the `= 3` expression, the
    /// legacy `: 3` separator's operand included as written), interned —
    /// the override-signature rendering renders it back (the parent's
    /// written default is inherited semantics, not re-derivable from the
    /// type). `None` when the formal carries no default or recovery
    /// dropped the capture.
    pub default_value: Option<StringId>,
}

/// The formal-parameter table of one executable: positional parameters in
/// declaration order (required group first), then named parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Formals {
    /// Positional parameters, declaration order.
    pub positional: Box<[Formal]>,
    /// Named parameters, declaration order.
    pub named: Box<[Formal]>,
}

impl Formals {
    /// The named formal under `name`, for the named-argument checks.
    #[must_use]
    pub fn named_formal(&self, name: StringId) -> Option<&Formal> {
        self.named.iter().find(|f| f.name == name)
    }
}

/// The structural function-type face of a callable declaration: return type
/// plus formal table, viewed over the model's own rows — no new storage, no
/// interning; every [`FnSig`] is a borrowed view a query composes on demand
/// (the declaration tables *are* the side table).
///
/// This is the tear-off substrate the lint layer's lambda-replacement rules
/// read (`unnecessary_lambdas.dart:208`
/// `typeSystem.isAssignableTo(invocationType, nodeType)` over function
/// types): written function-type *annotations* ride [`Ty::Function`] —
/// nested and generic forms included, carried by its interned
/// [`FnShape`] — while [`FnSig`] remains the declaration-row view for
/// callable declarations: it composes a *declaration's* formals from the
/// declaration tables, a fact no written annotation carries.
///
/// # Which shapes compose
///
/// * [`FnSig::of_top_decl`] — a [`MicroKind::TopLevelFn`] with a captured
///   formal table (a plain top-level function). A `None`-formals top-level
///   getter under-reports; top-level setters are spelled `name=` and are
///   never tear-off targets (the contract passes invocation callees only).
/// * [`FnSig::of_member`] — a [`MemberKind::Method`] with a captured formal
///   table. Setters never compose (not tear-off targets); fields and
///   getters carry no formal table.
/// * [`FnSig::new`] — any caller-held [`Formals`] (a constructor face, a
///   rule-side written signature).
/// * Local functions compose from nothing: their formals are not modeled —
///   every query under-reports `None`.
///
/// # Assignability (the silent-on-unprovable direction)
///
/// [`FnSig::assignable_from`] is the structural cut of the official
/// function subtyping (`subtype.dart:86-171` `_isFunctionSubtypeOf`, the
/// core `TypeSystem.isAssignableTo` runs over function types) in
/// three-valued form: `Some(true)` only when every position is *provably*
/// compatible from written declarations; `None` where the declared facts
/// cannot decide (uncaptured or hierarchy-crossing leaves, generics);
/// `Some(false)` where a position is decidable-incompatible. The consuming
/// rule treats `None` and `Some(false)` alike — stay silent, the
/// under-report posture (upstream bails the report whenever assignability
/// fails or cannot be established).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FnSig<'a> {
    /// The callable's own type-parameter count (`T Function<T>(T)`): any
    /// non-zero count on either side makes assignability undecidable (the
    /// `relateTypeParameters` fresh-substitution path, `subtype.dart:88-92`,
    /// needs bounds and inference).
    pub type_params: u16,
    /// The declared return type ([`Ty::Unknown`] when uncaptured — the
    /// return comparison then under-reports).
    pub return_ty: Ty,
    /// The formal-parameter table, borrowed from the model's rows.
    pub formals: &'a Formals,
}

impl<'a> FnSig<'a> {
    /// The raw composition: the caller's own return face and formal table.
    #[must_use]
    pub const fn new(type_params: u16, return_ty: Ty, formals: &'a Formals) -> Self {
        FnSig {
            type_params,
            return_ty,
            formals,
        }
    }

    /// The signature of a top-level function declaration; `None` for every
    /// other micro-kind and for executables without a captured formal
    /// table (top-level getters).
    #[must_use]
    pub fn of_top_decl(decl: &'a TopDecl) -> Option<Self> {
        match (decl.kind, decl.formals.as_deref()) {
            (MicroKind::TopLevelFn, Some(formals)) => Some(FnSig {
                type_params: decl.type_params,
                return_ty: decl.return_ty,
                formals,
            }),
            _ => None,
        }
    }

    /// The signature of a method declaration; `None` for setters (never
    /// tear-off targets), fields, and getters (no formal table).
    #[must_use]
    pub fn of_member(entry: &'a MemberEntry) -> Option<Self> {
        match (entry.kind, entry.formals.as_deref()) {
            (MemberKind::Method, Some(formals)) => Some(FnSig {
                type_params: entry.type_params,
                return_ty: entry.return_ty,
                formals,
            }),
            _ => None,
        }
    }

    /// The structural assignability cut: whether `from` (the replacement —
    /// a tear-off) can stand where `self` (the position's expected type —
    /// the lambda's signature) stands, i.e. the official
    /// `isAssignableTo(from, self)` over function types
    /// (`unnecessary_lambdas.dart:208`).
    ///
    /// Rule shape (`_isFunctionSubtypeOf`, `subtype.dart:86-171`, with `f`
    /// = `from`, `g` = `self`): covariant return (:98-100), contravariant
    /// formals — `isSubtypeOf(gParameter.type, fParameter.type)` (:127,
    /// :137) — required-count and named-set coverage (:157-170), named
    /// matching with required-flag agreement (:136-137). Every leaf runs
    /// through [`Ty::assignable_from`], so its `None` (undecided from
    /// written facts — hierarchy questions, uncaptured types) propagates:
    /// a `true` is only ever provable, never guessed.
    ///
    /// Deviations, all under-reporting:
    ///
    /// * generics on either side answer `None` (the fresh-substitution
    ///   relate, `subtype.dart:88-92`, is outside the declared facts);
    /// * a `void` *target* return is Right Top (`subtype.dart:263-267`) —
    ///   `Some(true)` outright; a `void` *source* return stays undecided
    ///   ([`Ty::Void`] sources are `use_of_void_result`'s business);
    /// * the lint's `parametersMatch` fallback (a permissive heuristic that
    ///   only adds reports upstream, `unnecessary_lambdas.dart:107-149`) is
    ///   deliberately absent.
    #[must_use]
    pub fn assignable_from(self, from: Self) -> Option<bool> {
        if self.type_params > 0 || from.type_params > 0 {
            return None;
        }
        // Covariant return; a `void` target is Right Top.
        let ret = match self.return_ty {
            Ty::Void => Some(true),
            _ => self.return_ty.assignable_from(from.return_ty),
        };
        // The verdict propagates: a decidable-false return face fails the
        // signature (a nullability-narrowed tear-off), an undecided one
        // suppresses.
        match ret {
            Some(true) => {}
            Some(false) => return Some(false),
            None => return None,
        }

        // Positional formals: paired walk, then the leftovers.
        let (src, dst) = (&from.formals.positional, &self.formals.positional);
        for (s, d) in src.iter().zip(dst.iter()) {
            if s.required && !d.required {
                // A required source formal cannot face an optional target
                // slot (:116-121 — f required needs g required).
                return Some(false);
            }
            // Contravariant leaf (:127): the target's formal must assign
            // into the source's.
            match s.ty.assignable_from(d.ty) {
                Some(true) => {}
                Some(false) => return Some(false),
                None => return None,
            }
        }
        if src.len() > dst.len() {
            // Leftover source positionals: optional ones may extend the
            // face; required ones cannot (:157-165 — only required
            // leftovers fail).
            if src[dst.len()..].iter().any(|formal| formal.required) {
                return Some(false);
            }
        } else if dst.len() > src.len() {
            // The replacement must accept every target formal (:167-170).
            return Some(false);
        }

        // Named formals: name-matched, required flags must agree, the same
        // contravariant leaf. The source may add optional named formals the
        // target lacks (:141-146); the target may lack nothing (:167-170).
        // Declaration order differs from upstream's sorted walk, so the
        // coverage checks run both ways over name lookups.
        for s in from.formals.named.iter() {
            match self.formals.named_formal(s.name) {
                None => {
                    if s.required {
                        return Some(false);
                    }
                }
                Some(d) => {
                    if s.required && !d.required {
                        return Some(false);
                    }
                    match s.ty.assignable_from(d.ty) {
                        Some(true) => {}
                        Some(false) => return Some(false),
                        None => return None,
                    }
                }
            }
        }
        if self
            .formals
            .named
            .iter()
            .any(|d| from.formals.named_formal(d.name).is_none())
        {
            return Some(false);
        }
        Some(true)
    }
}

/// A name-keyed map as dense rows sorted by [`StringId`] (#1778): the
/// per-file and per-container name faces (top-level declarations, named
/// containers, container members, constructor formals, extension members).
/// The small-map shape these faces had before — one `FxHashMap` each —
/// spends a bucket array plus control bytes per map, and at the 10⁵-file
/// face hundreds of thousands of live maps carry that overhead in aggregate
/// (~30-80 MiB); a boxed sorted slice costs exactly the key + value bytes,
/// and a `u32`-keyed binary search answers the same lookups.
///
/// Construction runs through [`NameMapBuilder`] (capture) or
/// `FromIterator` (restore) — both sort ascending and keep the **first**
/// entry of a duplicate name, the capture-side keep-first rule (duplicates
/// are the `duplicate_definition` log's business, not a modeling event).
/// A finished map is immutable: iteration is deterministic ascending-id
/// order, and every row is sealed at capacity (`into_boxed_slice`, the
/// #1777 precedent).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameMap<V> {
    rows: Box<[(StringId, V)]>,
}

impl<V> Default for NameMap<V> {
    fn default() -> Self {
        Self { rows: Box::new([]) }
    }
}

impl<V> NameMap<V> {
    /// The value under `name`.
    #[must_use]
    pub fn get(&self, name: &StringId) -> Option<&V> {
        let at = self.rows.binary_search_by_key(name, |(key, _)| *key);
        at.ok().map(|at| &self.rows[at].1)
    }

    /// Whether `name` is present.
    #[must_use]
    pub fn contains_key(&self, name: &StringId) -> bool {
        self.rows
            .binary_search_by_key(name, |(key, _)| *key)
            .is_ok()
    }

    /// The number of rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The names, ascending by id.
    pub fn keys(&self) -> impl Iterator<Item = &StringId> {
        self.rows.iter().map(|(key, _)| key)
    }

    /// The values in row order.
    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.rows.iter().map(|(_, value)| value)
    }

    /// The rows (name, value) in ascending-id order.
    pub fn iter(&self) -> std::slice::Iter<'_, (StringId, V)> {
        self.rows.iter()
    }
}

impl<V> std::ops::Index<&StringId> for NameMap<V> {
    type Output = V;

    fn index(&self, name: &StringId) -> &V {
        let at = self
            .rows
            .binary_search_by_key(name, |(key, _)| *key)
            .expect("name present in NameMap");
        &self.rows[at].1
    }
}

impl<'a, V> IntoIterator for &'a NameMap<V> {
    type Item = &'a (StringId, V);
    type IntoIter = std::slice::Iter<'a, (StringId, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}

impl<V> FromIterator<(StringId, V)> for NameMap<V> {
    fn from_iter<I: IntoIterator<Item = (StringId, V)>>(rows: I) -> Self {
        let mut rows: Vec<_> = rows.into_iter().collect();
        // A stable sort keeps the first entry of a duplicate name first —
        // the same keep-first rule the capture-side builder applies.
        rows.sort_by_key(|(name, _)| *name);
        rows.dedup_by_key(|row| row.0);
        Self {
            rows: rows.into_boxed_slice(),
        }
    }
}

/// The capture-side construction half of [`NameMap`]: sorted-insert rows
/// with hash-map entry semantics (vacant insert, occupied read) at the
/// same O(log n) lookup. [`NameMapBuilder::finish`] seals the rows into the
/// immutable map at exact capacity.
#[derive(Clone, Debug)]
pub(crate) struct NameMapBuilder<V> {
    rows: Vec<(StringId, V)>,
}

impl<V> Default for NameMapBuilder<V> {
    fn default() -> Self {
        Self { rows: Vec::new() }
    }
}

impl<V> NameMapBuilder<V> {
    /// The entry under `name`: [`Entry::Occupied`] carries the existing
    /// value (the duplicate checks read it), [`Entry::Vacant`] inserts at
    /// the sorted position.
    pub(crate) fn entry(&mut self, name: StringId) -> Entry<'_, V> {
        match self.rows.binary_search_by_key(&name, |(key, _)| *key) {
            Ok(at) => Entry::Occupied(&self.rows[at].1),
            Err(at) => Entry::Vacant(VacantEntry {
                rows: &mut self.rows,
                name,
                at,
            }),
        }
    }

    /// Inserts `value` under `name`, replacing an existing row — the
    /// hash-map `insert` semantics the constructor-formals table relies on
    /// (a re-declared selector's later formal list wins).
    pub(crate) fn insert(&mut self, name: StringId, value: V) {
        match self.rows.binary_search_by_key(&name, |(key, _)| *key) {
            Ok(at) => self.rows[at].1 = value,
            Err(at) => self.rows.insert(at, (name, value)),
        }
    }

    /// The existing value under `name`, inserting `make()`'s row when
    /// vacant — the `entry().or_insert` shape.
    pub(crate) fn or_insert_with(&mut self, name: StringId, make: impl FnOnce() -> V) -> &mut V {
        let at = match self.rows.binary_search_by_key(&name, |(key, _)| *key) {
            Ok(at) => at,
            Err(at) => {
                self.rows.insert(at, (name, make()));
                at
            }
        };
        &mut self.rows[at].1
    }

    /// Seals the rows into the immutable map at exact capacity.
    pub(crate) fn finish(self) -> NameMap<V> {
        NameMap {
            rows: self.rows.into_boxed_slice(),
        }
    }
}

/// The vacant half of [`NameMapBuilder::entry`].
pub(crate) struct VacantEntry<'a, V> {
    rows: &'a mut Vec<(StringId, V)>,
    name: StringId,
    at: usize,
}

impl<V> VacantEntry<'_, V> {
    /// Inserts the row at the sorted position.
    pub(crate) fn insert(self, value: V) {
        self.rows.insert(self.at, (self.name, value));
    }
}

/// The result of [`NameMapBuilder::entry`].
pub(crate) enum Entry<'a, V> {
    /// The name is present; the existing value (keep-first: capture logs a
    /// duplicate instead of replacing).
    Occupied(&'a V),
    /// The name is absent.
    Vacant(VacantEntry<'a, V>),
}

/// One representation-field capture of an extension type's primary
/// constructor (`extension type E(int x)` — the `int x` formal). The
/// semantic representation checks (`error_verifier.dart:5340-5460`
/// `_checkForExtensionTypeRepresentationErrorCodes` and the self-reference
/// check at :5343-5354) read the name, its span, and the declared type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepFormal {
    /// The formal's facts (name, declared type, required bit).
    pub formal: Formal,
    /// Source span of the formal's name token (the emission anchor).
    pub name_range: SourceRange,
}

/// One extension declaration's application face: the identity the implicit
/// member application resolves against (`extension_member_resolver.dart:85-128`
/// `findExtension` + `applicable_extensions.dart:157-171` `applicableTo`).
/// Named and unnamed extensions both produce one; extension types do not
/// (their members surface on the extension type's own container face).
#[derive(Clone, Debug)]
pub struct ExtensionFace {
    /// The declaring extension's name; `None` for an unnamed extension (the
    /// ambiguity message renders the `on` type instead,
    /// `extension_member_resolver.dart:119-124`).
    pub name: Option<StringId>,
    /// The `on` clause type name as written (a qualified `on p.T` keeps the
    /// bare `T`, the model's `p.S` convention). `None` when the clause never
    /// produced an honest name (missing clause recovery, synthetic
    /// reference) — the face can never apply and its members are suppressed
    /// from application (the oracle's `extendedType` fails to resolve; an
    /// inapplicable extension never gates an `undefined_*`).
    pub on_type: Option<StringId>,
    /// The `on` type was written nullable (`on C?`). Applicability splits
    /// on it: a non-nullable `on` type never matches a nullable receiver
    /// (`String?` is not a subtype of `String` — `applicableTo`'s subtype
    /// predicate), and against a non-nullable receiver the non-nullable
    /// `on` type is the more specific resolution.
    pub on_type_nullable: bool,
    /// The `on` clause's full interned type (`on Map<LookupName, FieldItem>`
    /// keeps its argument list): the applicability gate's argument-level
    /// comparison — the oracle's `_getApplicable` instantiates the extended
    /// type against the receiver and rejects a face whose arguments cannot
    /// match (`Map<LookupName, FieldItem>` never applies to a
    /// `Map<LookupName, MethodItem>` receiver). [`Ty::Unknown`] when the
    /// clause never produced an honest type; [`Ty::Named`] with an empty
    /// argument list for a bare head (`on int`) — the head-level gate alone
    /// decides those.
    pub on_ty: Ty,
    /// The extension's own type-parameter spellings (`extension E<T> on
    /// List<T>` keeps `[T]`), in declaration order. An `on_ty` argument that
    /// names a type parameter instantiates against the receiver's argument
    /// (the oracle's `applicableTo` inference), so the argument-level gate
    /// only compares the concrete arguments.
    pub type_param_names: Box<[StringId]>,
    /// The extension's members under their lookup spellings (setters under
    /// `name=`), static bit preserved.
    pub members: NameMap<MemberEntry>,
}

/// One class member: kind, static/instance bit, and its executable facts.
/// The modifier bits are the declaration's own flat flags — orthogonal in
/// the grammar, so they stay flat here.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberEntry {
    /// The member kind.
    pub kind: MemberKind,
    /// `static` member (enum constants are static).
    pub is_static: bool,
    /// The `abstract` modifier was present (abstract fields).
    pub is_abstract: bool,
    /// The `external` modifier was present (external members).
    pub is_external: bool,
    /// The field group's `final` modifier (`final int x;`); false for every
    /// other member shape. The abstract-completeness check splits an
    /// abstract field's missing halves with it (a final field has no
    /// setter).
    pub is_final: bool,
    /// The member's own type parameters (`void m<T>()`); 0 when absent.
    pub type_params: u16,
    /// The member's own type-parameter names, in declaration order (the
    /// `wrong_number_of_type_arguments_function` display renders them).
    pub type_param_names: Box<[StringId]>,
    /// Declared type of a field (getters/setters/methods: [`Ty::Unknown`]).
    pub var_ty: Ty,
    /// Declared return type of a method/getter/setter (fields:
    /// [`Ty::Unknown`]).
    pub return_ty: Ty,
    /// The formal-parameter table; `None` = the member declares no
    /// parameter list shape worth modeling (fields) — a definite "none",
    /// not an unknown.
    pub formals: Option<Box<Formals>>,
    /// The member's own metadata, in written order (name-level,
    /// [`Annotation`]) — the `@override` presence the override families
    /// read and the meta-getter faces the annotation recognition composes
    /// per member. Members of every kind claim their star (a field group's
    /// star rides each group member, `@meta int a, b;` annotates both);
    /// constructors are not [`MemberEntry`] rows and stay out of this
    /// face.
    pub annotations: Box<[Annotation]>,
    /// The name token's span (UTF-16) in the member's own file — the
    /// declaration-site anchor the context-row faces resolve (the
    /// `invalid_override` family's "The member being overridden." row rides
    /// it, cross-file through the member walk's defining file). The model
    /// otherwise keeps no declaration spans; this one rides the entry so
    /// the boundary projection carries the anchor with the entry it
    /// deep-copies. [`Self::MISSING_NAME_RANGE`] where no token stands
    /// (a payload restored before the face shipped).
    pub name_range: SourceRange,
}

impl MemberEntry {
    /// The missing name-span sentinel: the offset is never a valid
    /// position, so consumers must test before use — never anchor at it.
    pub const MISSING_NAME_RANGE: SourceRange = SourceRange {
        offset: u32::MAX,
        length: 0,
    };
}

/// One named container declaration with its member face and superclass edge.
#[derive(Clone, Debug)]
#[expect(clippy::struct_excessive_bools)] // flags mirror the official modifier set
pub struct Container {
    /// The declaration family.
    pub kind: ContainerKind,
    /// The `abstract` modifier was present on a class-family header
    /// (classes and named mixin applications; always false for the other
    /// kinds, which cannot be abstract). The abstract-completeness check
    /// reads it (concrete classes only).
    pub is_abstract: bool,
    /// The `base` keyword was present on the header (classes, mixins, mixin
    /// classes). The class-modifier restriction checks read it
    /// (`dartq_resolution::modifiers`).
    pub is_base: bool,
    /// The `final` keyword was present on a class-family header.
    pub is_final: bool,
    /// The `interface` keyword was present on a class-family header.
    pub is_interface: bool,
    /// The declaration is the `mixin class` form.
    pub is_mixin_class: bool,
    /// The direct superclass name (`extends S` / a named mixin application's
    /// head type), name-level; `None` when absent or unrecoverable.
    pub superclass: Option<StringId>,
    /// The member name set, ascending by id ([`NameMap`]).
    pub members: NameMap<MemberEntry>,
    /// The container's own type-parameter count (`class C<T, U>`); 0 when
    /// non-generic. The wrong-number-of-type-arguments decisions read it.
    pub type_params: u16,
    /// The `sealed` modifier was present (the exhaustiveness splitter,
    /// `types/sealed.dart`).
    pub sealed: bool,
    /// The `with` mixin-application names as written (class/enum application
    /// order; a named mixin application's `with` list included). Sealed at
    /// the container's close (`into_boxed_slice`) — a finished container
    /// never grows, so the `Vec` header's capacity field would be dead
    /// weight per row (#2246).
    pub with_clause: Box<[StringId]>,
    /// The `implements` interface names as written (interface edges); sealed
    /// at the container's close like every clause column.
    pub implements_clause: Box<[StringId]>,
    /// The `on` constraint names of a mixin, as written (constraint checks,
    /// not supertype edges); empty for every other container kind.
    pub on_clause: Box<[StringId]>,
    /// Header-window type names no clause event claimed: the `extends`
    /// overflow types the oracle throws away (`ast_builder.dart:3972-3982`
    /// `handleClassExtends` "throw them away for now and pick the first
    /// one") and recovery residue. Never consulted as super-faces; hashed
    /// into the API signature so an L2-relevant edit cannot hide here.
    pub header_residue: Box<[StringId]>,
    /// An extension type's primary-constructor formals, in declaration
    /// order (the representation-field family's facts); empty elsewhere.
    pub primary_formals: Box<[RepFormal]>,
    /// An enum's constants in declaration order (the exhaustiveness
    /// witness order follows it, `types/enum.dart`); empty elsewhere;
    /// sealed at the container's close.
    pub enum_constants: Box<[StringId]>,
    /// The named constructor selectors the container declares (`A.named`
    /// keeps `named`). Unnamed constructors and the implicit default are
    /// [`Container::has_unnamed_constructor`]'s business.
    pub constructors: Box<[StringId]>,
    /// The container can be constructed without a selector: no explicit
    /// constructor exists (the implicit default), or an unnamed one was
    /// declared. A named-only declaration (`A.named();`) drops this to
    /// `false` — the `undefined_constructor_in_initializer` /
    /// `undefined_enum_constructor` unnamed-selector queries read it.
    pub has_unnamed_constructor: bool,
    /// Some declared constructor or factory carries the `const` modifier.
    /// The const-creation faces read it on the RESOLVED container, so a
    /// const constructor declared in another library answers like an
    /// in-file one (the oracle resolves the creation's element, whatever
    /// unit declares it).
    pub has_const_constructor: bool,
    /// Constructor formals by lookup spelling: the selector for a named
    /// constructor (`A.named` → `named`), the container's own name for the
    /// unnamed face (`A()` → `A` — the same spelling a creation's folded
    /// head row carries). The `unused_element_parameter` constructor face
    /// reads it (`unused_local_elements_verifier.dart:780-792` walks every
    /// formal list, constructors included).
    pub ctor_formals: NameMap<Box<Formals>>,
    /// The declaration's metadata, in written order (name-level,
    /// [`Annotation`]). The meta-getter recognition (`crate::annotation`)
    /// reads it; members' own metadata is not part of the container face.
    pub annotations: Box<[Annotation]>,
}

/// One declaration annotation, name-level: the annotation's spelled name,
/// its own span, and the argument group's verbatim source text. The
/// meta-getter faces the recognition helper reads ([`crate::annotation`])
/// need presence and name only; the argument text rides the API signature,
/// so an argument-only edit flips the L2 gate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Annotation {
    /// The name as written: the head identifier for a bare `@sealed`, the
    /// dot-joined spelling for a qualified `@meta.sealed`.
    pub name: StringId,
    /// The annotation's own span, `@` through the name's last segment
    /// (argument groups are outside it; the official annotation-anchored
    /// reports anchor the `@` sign).
    pub range: SourceRange,
    /// The argument group as written (`('a', b: 2)`), `(` through `)`,
    /// interned verbatim — spacing inside the group is part of the face
    /// (the conservative direction). `None` when the annotation carries no
    /// argument group, or recovery dropped the capture (the argument-less
    /// direction).
    pub args_text: Option<StringId>,
}

/// One top-level declaration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopDecl {
    /// The micro-kind tag.
    pub kind: MicroKind,
    /// The declaration's own type parameters; 0 when non-generic.
    pub type_params: u16,
    /// Declared type of a top-level variable (executables:
    /// [`Ty::Unknown`]).
    pub var_ty: Ty,
    /// Declared return type of a top-level function/getter/setter.
    pub return_ty: Ty,
    /// The formal-parameter table; `None` for non-executables.
    pub formals: Option<Box<Formals>>,
}

impl TopDecl {
    /// A bare name tag with no executable facts (containers, variables
    /// whose type was not captured, type parameters).
    #[must_use]
    pub const fn bare(kind: MicroKind) -> TopDecl {
        TopDecl {
            kind,
            type_params: 0,
            var_ty: Ty::UNKNOWN,
            return_ty: Ty::UNKNOWN,
            formals: None,
        }
    }
}

/// A declaration-arity violation captured at the declaration site: the
/// `wrong_number_of_parameters_for_operator` / `_setter` fold log (the
/// [`DuplicateDecl`] pattern — hash-map order must not reach the report).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArityViolation {
    /// A non-`-` operator with a fixed expected count
    /// (`error_verifier.dart:5688-5727`): `[]=`→2, `[]`/binary/`==`→1,
    /// `~`→0.
    Operator {
        /// The operator spelling as declared.
        name: StringId,
        /// The expected parameter count.
        expected: u16,
        /// The declared parameter count.
        found: u16,
    },
    /// `operator -` with more than one parameter (the minus template takes
    /// only the found count: `... should declare 0 or 1 parameter, but {0}
    /// found.`).
    OperatorMinus {
        /// The declared parameter count.
        found: u16,
    },
    /// A setter whose parameter list is not exactly one required-positional
    /// parameter (`ast_builder.dart:6283-6291`).
    Setter,
}

/// A capture-time arity violation: the code choice plus the name token's
/// span (the oracle reports at the declaration's name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArityDecl {
    /// The violation.
    pub violation: ArityViolation,
    /// Source span of the declaration's name token.
    pub range: SourceRange,
}

/// A second declaration whose name was already taken in its scope, recorded
/// at capture time (analyzer `duplicate_definition` granularity:
/// `duplicate_definition_verifier.dart` — top-level and class-member scopes
/// keep getters and setters apart, class members compare within their
/// static/instance bit, type parameters compare per list, and a duplicate is
/// diagnosed at the *second* declaration). The model keeps the first entry;
/// this log is the diagnostic surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DuplicateDecl {
    /// The colliding name (lookup spelling: setters under `name=`).
    pub name: StringId,
    /// Source span of the duplicate (second) declaration's name token.
    pub range: SourceRange,
}

/// Everything one compilation unit contributes to the semantic model.
///
/// Built by [`crate::collector::collect`]; queried per library through
/// [`crate::library`].
///
/// Every column is exact-sized: the maps seal through [`NameMap::finish`]
/// and the order/log columns move into boxed slices at the collector's
/// finish seam, so a finished model carries no `Vec` growth slack for its
/// remaining lifetime (`Arc<FileElements>` behind residency — #2246).
#[derive(Clone, Debug, Default)]
pub struct FileElements {
    pub(crate) decls: NameMap<TopDecl>,
    pub(crate) containers: NameMap<Container>,
    pub(crate) container_order: Box<[StringId]>,
    pub(crate) extension_faces: Box<[ExtensionFace]>,
    pub(crate) duplicates: Box<[DuplicateDecl]>,
    pub(crate) arity_violations: Box<[ArityDecl]>,
    /// The unit's canonical argument pool: every [`Ty::Named`] slot of
    /// this model indexes into it. One pool per model universe — ids from
    /// another unit's pool are meaningless here.
    pub(crate) type_args: ArgPool,
}

impl FileElements {
    /// The top-level declaration under `name`, private names included.
    #[must_use]
    pub fn decl(&self, name: StringId) -> Option<&TopDecl> {
        self.decls.get(&name)
    }

    /// The named container under `name` (unnamed extensions are absent —
    /// their members surface in [`FileElements::extension_faces`]).
    #[must_use]
    pub fn container(&self, name: StringId) -> Option<&Container> {
        self.containers.get(&name)
    }

    /// The container names in declaration order (`containers` itself is a
    /// hash map; the exhaustiveness face needs the source order for the
    /// sealed-split witness order).
    #[must_use]
    pub fn container_order(&self) -> &[StringId] {
        &self.container_order
    }

    /// Every extension application face of the unit, in declaration order
    /// (named and unnamed extensions; extension types never produce one).
    /// The implicit-application checks filter by `on` type and report the
    /// ambiguity over the applicable candidates
    /// ([`ExtensionFace`]).
    #[must_use]
    pub fn extension_faces(&self) -> &[ExtensionFace] {
        &self.extension_faces
    }

    /// Whether any extension face of the unit declares `name` under its
    /// lookup spelling — the conservative gate for expression-typing paths
    /// where the receiver type is unknown (the resolve-side analog of the
    /// old merged pool: any-declaration suppresses, never false-positives).
    #[must_use]
    pub fn has_extension_member(&self, name: StringId) -> bool {
        self.extension_faces
            .iter()
            .any(|face| face.members.contains_key(&name))
    }

    /// Every top-level declaration with its name, in arbitrary order.
    ///
    /// Consumers feeding the library graph must skip
    /// [`MicroKind::TypeParam`] entries: type parameters are
    /// declaration-scoped, not library faces.
    pub fn declarations(&self) -> impl Iterator<Item = (StringId, &TopDecl)> {
        self.decls.iter().map(|(name, decl)| (*name, decl))
    }

    /// Every named container with its name, in arbitrary order.
    pub fn containers(&self) -> impl Iterator<Item = (StringId, &Container)> {
        self.containers.iter().map(|(name, c)| (*name, c))
    }

    /// Every duplicate (second) declaration captured in this unit, in source
    /// order. The keep-first rule of the name maps is unchanged; this log is
    /// what `duplicate_definition` is emitted from.
    #[must_use]
    pub fn duplicates(&self) -> &[DuplicateDecl] {
        &self.duplicates
    }

    /// Every declaration-arity violation captured in this unit, in source
    /// order; folded into the report by [`mod@crate::diagnose`].
    #[must_use]
    pub fn arity_violations(&self) -> &[ArityDecl] {
        &self.arity_violations
    }

    /// The unit's canonical argument pool ([`Ty::Named`] slots index into
    /// it); capture feeds it through [`ArgPool::named`].
    #[must_use]
    pub fn type_args(&self) -> &ArgPool {
        &self.type_args
    }

    /// The pool, mutable — the capture side's intern entry.
    pub fn type_args_mut(&mut self) -> &mut ArgPool {
        &mut self.type_args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_source::StringTable;
    use std::hash::{DefaultHasher, Hash, Hasher};

    /// `int`, `String`, `List`, `Map` over one shared table/pool pair.
    struct Fix {
        strings: StringTable,
        pool: ArgPool,
        int: StringId,
        string: StringId,
        list: StringId,
        map: StringId,
    }

    impl Fix {
        fn new() -> Self {
            let strings = StringTable::new();
            let intern = |text: &str| strings.intern(text);
            Self {
                int: intern("int"),
                string: intern("String"),
                list: intern("List"),
                map: intern("Map"),
                strings,
                pool: ArgPool::default(),
            }
        }

        fn render(&self, ty: Ty) -> String {
            ty.render(&self.strings, &self.pool)
        }
    }

    /// Argument-less construction never touches the pool, and the
    /// with-arguments constructor returns the identical value for an empty
    /// list — the pre-slot construction face is preserved bit for bit.
    #[test]
    fn argument_less_types_never_touch_the_pool() {
        let fix = Fix::new();
        let mut pool = ArgPool::default();
        let plain = Ty::named(fix.int);
        let via_pool = pool.named(fix.int, false, &[]);
        assert_eq!(plain, via_pool);
        assert_eq!(via_pool, pool.named(fix.int, false, &[]));
        assert!(pool.is_empty(), "the empty list must never intern");
    }

    /// Equal argument lists share one slot regardless of interning order —
    /// derived `PartialEq`/`Hash` on `Ty` is structural equality
    /// (`List<int> == List<int>`), the canonical-intern contract.
    #[test]
    fn equal_arguments_intern_to_one_slot() {
        let fix = Fix::new();
        let mut pool = ArgPool::default();
        let int = Ty::named(fix.int);
        let first = pool.named(fix.list, false, &[int]);
        let second = pool.named(fix.list, false, &[int]);
        assert_eq!(first, second);
        assert_eq!(pool.len(), 1, "the second intern must dedup");

        let (mut left, mut right) = (DefaultHasher::new(), DefaultHasher::new());
        first.hash(&mut left);
        second.hash(&mut right);
        assert_eq!(left.finish(), right.finish());
    }

    /// Different arguments, argument count, or nullability are different
    /// types: the slot compares, never collapses.
    #[test]
    fn different_arguments_are_different_types() {
        let fix = Fix::new();
        let mut pool = ArgPool::default();
        let (int, string) = (Ty::named(fix.int), Ty::named(fix.string));
        let list_int = pool.named(fix.list, false, &[int]);
        assert_ne!(list_int, pool.named(fix.list, false, &[string]));
        assert_ne!(
            list_int,
            pool.named(fix.list, false, &[int, string]),
            "arity is part of the identity"
        );
        assert_ne!(
            list_int,
            pool.named(fix.list, true, &[int]),
            "the ? projection is part of the identity"
        );
    }

    /// Nested arguments (`Map<String, List<int>>`) dedup through the
    /// inner slots: re-interning the same nest shares every slot, and a
    /// one-bit inner difference (`List<int?>`) forks the outer slot.
    #[test]
    fn nested_arguments_are_canonical() {
        let fix = Fix::new();
        let mut pool = ArgPool::default();
        let (int, string) = (Ty::named(fix.int), Ty::named(fix.string));

        let list_int = pool.named(fix.list, false, &[int]);
        let first = pool.named(fix.map, false, &[string, list_int]);

        let list_int_again = pool.named(fix.list, false, &[int]);
        let second = pool.named(fix.map, false, &[string, list_int_again]);
        assert_eq!(first, second);

        let list_intq = pool.named(fix.list, true, &[int]);
        assert_ne!(first, pool.named(fix.map, false, &[string, list_intq]));
        assert_eq!(
            pool.len(),
            3,
            "slots: [int], [String, List<int>] (the twin deduped), [String, List<int>?]"
        );
    }

    /// The display face renders the official `Name<Arg, ...>?` form. Every
    /// type interns through the harness's own pool — the same pool the
    /// render reads — per the one-pool-one-universe contract.
    #[test]
    fn display_renders_the_argument_face() {
        let mut fix = Fix::new();
        let (int, string) = (Ty::named(fix.int), Ty::named(fix.string));
        let list_int = fix.pool.named(fix.list, false, &[int]);
        let map = fix.pool.named(fix.map, false, &[string, list_int]);

        assert_eq!(fix.render(Ty::named(fix.int)), "int");
        assert_eq!(fix.render(list_int), "List<int>");
        assert_eq!(fix.render(map), "Map<String, List<int>>");
        let nullable_list = fix.pool.named(fix.list, true, &[int]);
        assert_eq!(fix.render(nullable_list), "List<int>?");
        assert_eq!(fix.render(Ty::Dynamic), "dynamic");
        assert_eq!(fix.render(Ty::Unknown), "");
    }

    /// The same-name assignability face consults the slot: identical
    /// written arguments answer `Some(true)` regardless of interning
    /// order (canonical ids), differing arguments defer to the
    /// pool-owning caller (`None` here — never a decidable false), and
    /// every nullability verdict is unchanged.
    #[test]
    fn same_name_assignability_compares_the_argument_slot() {
        let mut fix = Fix::new();
        let (int, string) = (Ty::named(fix.int), Ty::named(fix.string));
        let list_int = fix.pool.named(fix.list, false, &[int]);
        // A second intern of the same written list lands on one slot.
        let list_int_again = fix.pool.named(fix.list, false, &[int]);
        let list_string = fix.pool.named(fix.list, false, &[string]);
        let list_intq = fix.pool.named(fix.list, true, &[int]);

        assert_eq!(list_int_again.assignable_from(list_int), Some(true));
        assert_eq!(list_int.assignable_from(list_int_again), Some(true));
        assert_eq!(list_string.assignable_from(list_int), None);
        assert_eq!(list_int.assignable_from(list_string), None);

        // Nullability still decides on its own, arguments aside.
        assert_eq!(list_intq.assignable_from(list_int), Some(true));
        assert_eq!(list_int.assignable_from(list_intq), Some(false));
        // Argument-less named pairs keep the pre-slot identity.
        assert_eq!(
            Ty::named(fix.list).assignable_from(Ty::named(fix.list)),
            Some(true)
        );

        // Nested arguments compare through their own canonical slots.
        let nested_a = fix.pool.named(fix.map, false, &[string, list_int]);
        let nested_b = fix.pool.named(fix.map, false, &[string, list_int_again]);
        assert_eq!(nested_b.assignable_from(nested_a), Some(true));
        let nested_c = fix.pool.named(fix.map, false, &[string, list_string]);
        assert_eq!(nested_c.assignable_from(nested_a), None);
    }

    /// Builds a `Formals` from (name, ty, required) triples over one
    /// string table.
    fn formals_of(
        strings: &StringTable,
        positional: &[(&str, Ty, bool)],
        named: &[(&str, Ty, bool)],
    ) -> Formals {
        let build = |specs: &[(&str, Ty, bool)]| {
            specs
                .iter()
                .map(|&(name, ty, required)| Formal {
                    name: strings.intern(name),
                    ty,
                    required,
                    covariant: false,
                    default_value: None,
                })
                .collect::<Box<[Formal]>>()
        };
        Formals {
            positional: build(positional),
            named: build(named),
        }
    }

    /// Assignability is covariant in the return: identical and
    /// nullability-widened returns answer `Some(true)`, a nullability-
    /// narrowed return answers `Some(false)`, and a hierarchy question
    /// (`int` vs `String`) stays undecided.
    #[test]
    fn fn_sig_return_is_covariant() {
        let mut fix = Fix::new();
        let (int, string) = (Ty::named(fix.int), Ty::named(fix.string));
        let intq = fix.pool.named(fix.int, true, &[]);
        let void_formals = formals_of(&fix.strings, &[], &[]);
        fn sig_of(ret: Ty, formals: &Formals) -> FnSig<'_> {
            FnSig::new(0, ret, formals)
        }

        // Identical and nullability-widened returns are provable.
        assert_eq!(
            sig_of(int, &void_formals).assignable_from(sig_of(int, &void_formals)),
            Some(true)
        );
        assert_eq!(
            sig_of(intq, &void_formals).assignable_from(sig_of(int, &void_formals)),
            Some(true)
        );
        // A nullability-narrowed return is decidable-false.
        assert_eq!(
            sig_of(int, &void_formals).assignable_from(sig_of(intq, &void_formals)),
            Some(false)
        );
        // Unrelated names stay undecided (the under-report direction).
        assert_eq!(
            sig_of(int, &void_formals).assignable_from(sig_of(string, &void_formals)),
            None
        );
    }

    /// Assignability is contravariant in the formals (upstream
    /// `isSubtypeOf(gParameter.type, fParameter.type)`): the tear-off may
    /// accept a wider input than the position ever passes; the unsound
    /// direction is decidable-false; hierarchy questions stay silent.
    #[test]
    fn fn_sig_formals_are_contravariant() {
        let mut fix = Fix::new();
        let int = Ty::named(fix.int);
        let intq = fix.pool.named(fix.int, true, &[]);
        let string = Ty::named(fix.string);
        let int_formals = formals_of(&fix.strings, &[("x", int, true)], &[]);
        let intq_formals = formals_of(&fix.strings, &[("x", intq, true)], &[]);
        let string_formals = formals_of(&fix.strings, &[("x", string, true)], &[]);
        fn sig_of(formals: &Formals) -> FnSig<'_> {
            FnSig::new(0, Ty::Void, formals)
        }

        // Position `int Function(int)`, replacement `int Function(int?)`:
        // the tear-off accepts a wider input than the position ever passes.
        let lambda = sig_of(&int_formals);
        assert_eq!(lambda.assignable_from(sig_of(&intq_formals)), Some(true));
        // The unsound direction — a plain-`int` formal cannot serve a
        // position that may pass `int?` — is decidable-false.
        let accepting_lambda = sig_of(&intq_formals);
        assert_eq!(
            accepting_lambda.assignable_from(sig_of(&int_formals)),
            Some(false)
        );
        // Unrelated formal types: undecided, silent.
        let string_lambda = sig_of(&string_formals);
        assert_eq!(string_lambda.assignable_from(sig_of(&int_formals)), None);
    }

    /// A `void` target return is Right Top (every return assigns in); an
    /// uncaptured (`Unknown`) position stays silent — written function-type
    /// annotations are poisoned to `Unknown`, so such faces never report.
    #[test]
    fn fn_sig_void_target_return_is_right_top_and_unknown_stays_silent() {
        let fix = Fix::new();
        let void_formals = formals_of(&fix.strings, &[], &[]);
        let int = Ty::named(fix.int);
        let void_target = FnSig::new(0, Ty::Void, &void_formals);
        assert_eq!(
            void_target.assignable_from(FnSig::new(0, int, &void_formals)),
            Some(true)
        );
        // A void *source* return: undecided (the under-report direction).
        let int_target = FnSig::new(0, int, &void_formals);
        assert_eq!(int_target.assignable_from(void_target), None);
        // An uncaptured return poisons the comparison.
        assert_eq!(
            int_target.assignable_from(FnSig::new(0, Ty::Unknown, &void_formals)),
            None
        );
    }

    /// Generic signatures answer `None` regardless of shape: the official
    /// relate runs a fresh-substitution path with bounds, outside the
    /// declared facts.
    #[test]
    fn fn_sig_generics_stay_silent() {
        let fix = Fix::new();
        let int = Ty::named(fix.int);
        let formals = formals_of(&fix.strings, &[("x", int, true)], &[]);
        let generic = FnSig::new(1, int, &formals);
        let plain = FnSig::new(0, int, &formals);
        assert_eq!(generic.assignable_from(plain), None);
        assert_eq!(plain.assignable_from(generic), None);
        assert_eq!(generic.assignable_from(generic), None);
    }

    /// Positional arity: required-count agreement, optional-extends-face,
    /// and the must-accept-every-target-formal rule.
    #[test]
    fn fn_sig_positional_arity_rules() {
        let fix = Fix::new();
        let int = Ty::named(fix.int);
        let void_ret = Ty::Void;
        // The source may add optional positionals.
        let one_req = formals_of(&fix.strings, &[("x", int, true)], &[]);
        let req_opt = formals_of(&fix.strings, &[("x", int, true), ("y", int, false)], &[]);
        let lambda = FnSig::new(0, void_ret, &one_req);
        assert_eq!(
            lambda.assignable_from(FnSig::new(0, void_ret, &req_opt)),
            Some(true)
        );
        // A required source formal without a target slot cannot.
        let req_req = formals_of(&fix.strings, &[("x", int, true), ("y", int, true)], &[]);
        assert_eq!(
            lambda.assignable_from(FnSig::new(0, void_ret, &req_req)),
            Some(false)
        );
        // A required source formal cannot face an optional target slot.
        let opt_slot = formals_of(&fix.strings, &[("x", int, false)], &[]);
        let req_one = formals_of(&fix.strings, &[("x", int, true)], &[]);
        assert_eq!(
            FnSig::new(0, void_ret, &opt_slot).assignable_from(FnSig::new(0, void_ret, &req_one)),
            Some(false)
        );
        // …while an optional source formal may face a required slot.
        assert_eq!(
            FnSig::new(0, void_ret, &req_one).assignable_from(FnSig::new(0, void_ret, &opt_slot)),
            Some(true)
        );
        // Target extras: the replacement must accept every target formal.
        assert_eq!(
            FnSig::new(0, void_ret, &req_opt).assignable_from(FnSig::new(0, void_ret, &one_req)),
            Some(false)
        );
    }

    /// Named formals match by name: required flags must agree, the source
    /// may add optional named formals, the target may lack nothing.
    #[test]
    fn fn_sig_named_matching_rules() {
        let fix = Fix::new();
        let int = Ty::named(fix.int);
        let void_ret = Ty::Void;
        let named_x = formals_of(&fix.strings, &[], &[("x", int, false)]);
        let lambda = FnSig::new(0, void_ret, &named_x);
        // Same shape.
        assert_eq!(
            lambda.assignable_from(FnSig::new(0, void_ret, &named_x)),
            Some(true)
        );
        // Source's extra optional named formal is fine.
        let x_y = formals_of(&fix.strings, &[], &[("x", int, false), ("y", int, false)]);
        assert_eq!(
            lambda.assignable_from(FnSig::new(0, void_ret, &x_y)),
            Some(true)
        );
        // Source's extra required named formal is not.
        let x_yreq = formals_of(&fix.strings, &[], &[("x", int, false), ("y", int, true)]);
        assert_eq!(
            lambda.assignable_from(FnSig::new(0, void_ret, &x_yreq)),
            Some(false)
        );
        // Target's extra named formal: the replacement cannot accept it.
        assert_eq!(
            FnSig::new(0, void_ret, &x_y).assignable_from(FnSig::new(0, void_ret, &named_x)),
            Some(false)
        );
        // Required-flag agreement on a matched name.
        let x_req = formals_of(&fix.strings, &[], &[("x", int, true)]);
        assert_eq!(
            FnSig::new(0, void_ret, &x_req).assignable_from(FnSig::new(0, void_ret, &named_x)),
            Some(true)
        );
        assert_eq!(
            FnSig::new(0, void_ret, &named_x).assignable_from(FnSig::new(0, void_ret, &x_req)),
            Some(false)
        );
        // Different names are a coverage failure both ways.
        let named_z = formals_of(&fix.strings, &[], &[("z", int, false)]);
        assert_eq!(
            lambda.assignable_from(FnSig::new(0, void_ret, &named_z)),
            Some(false)
        );
    }

    /// Composition gates: only top-level functions and methods with a
    /// captured formal table compose; getters, setters, and fields do not.
    #[test]
    fn fn_sig_composition_gates_the_declaration_shapes() {
        let fix = Fix::new();
        let mut top = TopDecl::bare(MicroKind::TopLevelFn);
        assert_eq!(FnSig::of_top_decl(&top), None, "no formal table yet");
        top.formals = Some(Box::new(formals_of(&fix.strings, &[], &[])));
        assert!(FnSig::of_top_decl(&top).is_some());
        let variable = TopDecl::bare(MicroKind::TopLevelVar);
        assert_eq!(FnSig::of_top_decl(&variable), None);

        let entry = MemberEntry {
            kind: MemberKind::Method,
            is_static: false,
            is_abstract: false,
            is_external: false,
            is_final: false,
            type_params: 0,
            type_param_names: Box::new([]),
            var_ty: Ty::UNKNOWN,
            return_ty: Ty::UNKNOWN,
            formals: Some(Box::new(formals_of(&fix.strings, &[], &[]))),
            annotations: Box::default(),
            name_range: MemberEntry::MISSING_NAME_RANGE,
        };
        assert!(FnSig::of_member(&entry).is_some());
        for kind in [MemberKind::Setter, MemberKind::Getter, MemberKind::Field] {
            let other = MemberEntry {
                kind,
                ..entry.clone()
            };
            assert_eq!(FnSig::of_member(&other), None, "{kind:?} never composes");
        }
    }

    /// The name-map substrate pin (#1778): rows stay ascending by id no
    /// matter the insertion order (builder and `FromIterator` alike), a
    /// duplicate name keeps its first entry, lookups answer exactly, and
    /// the sealed map is iteration-deterministic.
    #[test]
    fn name_map_rows_stay_sorted_and_keep_first() {
        let fix = Fix::new();
        // Interleave insertion so ids arrive neither ascending nor
        // contiguous: the builder's sorted-insert must place each row.
        let spellings = ["map", "int", "String", "List"];
        let builder_map = {
            let mut map = NameMapBuilder::default();
            for (at, spelling) in spellings.iter().enumerate() {
                match map.entry(fix.strings.intern(spelling)) {
                    Entry::Vacant(slot) => slot.insert(at as u16),
                    Entry::Occupied(_) => unreachable!("distinct names"),
                }
            }
            map.finish()
        };
        // Ascending by raw id, whatever the intern order was.
        let ids: Vec<StringId> = builder_map.keys().copied().collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "rows iterate ascending by id");
        // Every lookup answers, misses stay None.
        for (at, spelling) in spellings.iter().enumerate() {
            assert_eq!(
                builder_map.get(&fix.strings.intern(spelling)),
                Some(&(at as u16)),
                "{spelling} resolves to its inserted value"
            );
        }
        assert_eq!(builder_map.get(&fix.strings.intern("absent")), None);
        assert!(!builder_map.contains_key(&fix.strings.intern("absent")));
        assert!(builder_map.contains_key(&fix.int));
        assert_eq!(builder_map.len(), spellings.len());

        // Keep-first on collision: the occupied entry carries the first
        // value and a vacant insert never lands twice.
        let mut keep_first = NameMapBuilder::default();
        let first = keep_first.entry(fix.list);
        assert!(matches!(first, Entry::Vacant(_)));
        keep_first.or_insert_with(fix.list, || 7u16);
        match keep_first.entry(fix.list) {
            Entry::Occupied(existing) => assert_eq!(*existing, 7, "first entry survives"),
            Entry::Vacant(_) => panic!("duplicate name must read occupied"),
        }
        keep_first.or_insert_with(fix.list, || 9u16);
        assert_eq!(keep_first.finish().get(&fix.list), Some(&7));

        // `FromIterator` normalizes a shuffled restore payload to the same
        // sorted rows, keeping the first of a duplicate pair.
        let mut shuffled: Vec<(StringId, u16)> = spellings
            .iter()
            .zip(0..)
            .map(|(s, v)| (fix.strings.intern(s), v))
            .collect();
        shuffled.reverse();
        shuffled.push((fix.strings.intern("map"), 99)); // duplicate: the earlier `map` row wins
        let collected: NameMap<u16> = shuffled.into_iter().collect();
        assert_eq!(collected, builder_map, "restore normalizes to sorted rows");
        assert_eq!(
            collected.get(&fix.strings.intern("map")),
            Some(&0),
            "duplicate keeps the first"
        );
    }

    /// Record fields display in name-byte order regardless of which table
    /// minted which id first: the shared fan-out string table deals ids in
    /// intern-arrival order, so the stored pairs' order varies between runs
    /// and must never reach the output. Two tables interning the same three
    /// names in opposite orders — the stored pairs land fully reversed —
    /// render one identical string.
    #[test]
    fn record_display_order_is_independent_of_intern_arrival() {
        let reverse = StringTable::new();
        let (c, b, a) = (reverse.intern("c"), reverse.intern("b"), reverse.intern("a"));
        let forward = StringTable::new();
        let (a2, b2, c2) = (forward.intern("a"), forward.intern("b"), forward.intern("c"));
        let (int_r, int_f) = (reverse.intern("int"), forward.intern("int"));

        let mut pool_r = ArgPool::default();
        let mut pool_f = ArgPool::default();
        let ty_r = pool_r.record(
            &[],
            &[(b, Ty::named(int_r)), (a, Ty::named(int_r)), (c, Ty::named(int_r))],
            false,
        );
        let ty_f = pool_f.record(
            &[],
            &[(b2, Ty::named(int_f)), (a2, Ty::named(int_f)), (c2, Ty::named(int_f))],
            false,
        );
        assert_eq!(ty_r.render(&reverse, &pool_r), "({int a, int b, int c})");
        assert_eq!(ty_f.render(&forward, &pool_f), "({int a, int b, int c})");
        assert_eq!(
            ty_r.render(&reverse, &pool_r),
            ty_f.render(&forward, &pool_f),
            "arrival order must not decide the rendering"
        );
    }

    /// The display sorts against the stored order even inside one table:
    /// `b` interned before `a` puts the pairs in id order `(b, a)`, and the
    /// rendering still shows the texts' order. Re-interning the same name
    /// set in the other written order shares the slot — the canonical-intern
    /// contract is a name-set identity, untouched by the display order.
    #[test]
    fn record_display_sorts_names_against_intern_order() {
        let strings = StringTable::new();
        let b = strings.intern("b");
        let a = strings.intern("a");
        let int = strings.intern("int");
        let mut pool = ArgPool::default();
        let ty = pool.record(&[], &[(a, Ty::named(int)), (b, Ty::named(int))], false);
        assert_eq!(ty.render(&strings, &pool), "({int a, int b})");
        let twin = pool.record(&[], &[(b, Ty::named(int)), (a, Ty::named(int))], false);
        assert_eq!(ty, twin, "written order does not change the shape's identity");
        assert_eq!(twin.render(&strings, &pool), "({int a, int b})");
    }

    /// Named formals display in name-byte order under the same
    /// arrival-independence contract as record fields: `T Function({int b,
    /// int a})` renders identically whichever table minted the ids.
    #[test]
    fn function_display_order_is_independent_of_intern_arrival() {
        let reverse = StringTable::new();
        let (b, a) = (reverse.intern("b"), reverse.intern("a"));
        let forward = StringTable::new();
        let (a2, b2) = (forward.intern("a"), forward.intern("b"));
        let (int_r, int_f) = (reverse.intern("int"), forward.intern("int"));

        let mut pool_r = ArgPool::default();
        let mut pool_f = ArgPool::default();
        let ty_r = pool_r.function(
            0,
            Ty::named(int_r),
            &[],
            &[(b, Ty::named(int_r)), (a, Ty::named(int_r))],
            false,
        );
        let ty_f = pool_f.function(
            0,
            Ty::named(int_f),
            &[],
            &[(b2, Ty::named(int_f)), (a2, Ty::named(int_f))],
            false,
        );
        assert_eq!(
            ty_r.render(&reverse, &pool_r),
            "int Function({int a, int b})"
        );
        assert_eq!(
            ty_f.render(&forward, &pool_f),
            "int Function({int a, int b})"
        );
        assert_eq!(
            ty_r.render(&reverse, &pool_r),
            ty_f.render(&forward, &pool_f),
            "arrival order must not decide the rendering"
        );
    }
}
