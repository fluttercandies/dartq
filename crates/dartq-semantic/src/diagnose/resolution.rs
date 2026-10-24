use super::*;

impl<'a> Engine<'a> {
    /// The name after the dot of a qualified type arrived: the prefix slot
    /// is consumed.
    pub(crate) fn consume_type_prefix(&mut self) {
        self.type_prefix = None;
    }

    pub(crate) fn enter_non_type_pos(&mut self) {
        self.non_type_pos += 1;
    }

    pub(crate) fn leave_non_type_pos(&mut self) {
        self.non_type_pos -= 1;
    }

    pub(crate) fn enter_ctor_pos(&mut self) {
        self.ctor_pos += 1;
    }

    pub(crate) fn leave_ctor_pos(&mut self) {
        self.ctor_pos -= 1;
    }

    /// A bare name's declared type, innermost frame first.
    pub(crate) fn typed_local(&self, name: StringId) -> Option<Ty> {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.var_types.get(&name).copied())
    }

    /// The declared-type alias fold: a named type whose head resolves to a
    /// type alias rewrites to the alias's captured right-hand type,
    /// carrying the use-site `?` (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/element.dart — a `TypeAliasElement`
    /// use resolves through its aliased type before any type question reads
    /// it). A parameterized alias's written arguments substitute into the
    /// captured RHS first ([`Self::fold_parameterized_alias`]). The loop
    /// depth bounds chains and cycles; an unresolvable head answers as
    /// written (the under-report direction); a function/record right-hand
    /// side rewrites to that right-hand shape whole
    /// (`typedef V = void Function();` — the use-site `?` joins).
    pub(crate) fn fold_alias_ty(&self, ty: Ty) -> Ty {
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return ty;
        };
        // A use with written arguments whose head resolves to a
        // parameterized alias substitutes them into the captured RHS
        // (`Predicate<String>` reads `bool Function(String)`); every other
        // arg-bearing use keeps the written shape.
        if args != ArgListId::EMPTY {
            return self.fold_parameterized_alias(name, nullable, args).unwrap_or(ty);
        }
        let (mut head, mut head_nullable) = (name, nullable);
        for _ in 0..8 {
            let Some(rhs) = self.alias_rhs(head) else {
                break;
            };
            match rhs {
                Ty::Named {
                    name: next,
                    nullable: rhs_nullable,
                    args: ArgListId::EMPTY,
                } => {
                    head = next;
                    head_nullable |= rhs_nullable;
                }
                // A non-empty argument list stands for the use whole —
                // `typedef BlobColumn = Column<Uint8List>` is `Column<
                // Uint8List>` at the use site, not `BlobColumn` (the same
                // stand-for-the-whole rule the function/record arms apply);
                // the use-site `?` joins the right-hand's own (`typedef B =
                // A<int>?` is nullable on both spellings).
                Ty::Named {
                    name: next,
                    nullable: rhs_nullable,
                    args,
                } => {
                    return Ty::Named {
                        name: next,
                        nullable: rhs_nullable | head_nullable,
                        args,
                    };
                }
                // A non-named right-hand side stands for the use whole
                // (`typedef V = void Function();`): the use-site `?` joins.
                Ty::Dynamic => return Ty::Dynamic,
                Ty::Record {
                    shape,
                    nullable: rhs_nullable,
                } => {
                    return Ty::Record {
                        shape,
                        nullable: rhs_nullable | head_nullable,
                    };
                }
                Ty::Function {
                    shape,
                    nullable: rhs_nullable,
                } => {
                    return Ty::Function {
                        shape,
                        nullable: rhs_nullable | head_nullable,
                    };
                }
                // `Null`/`void`/`Never`/`Unknown` right-hand sides keep the
                // written use (an alias to them is not the modeled face).
                _ => break,
            }
        }
        Ty::Named {
            name: head,
            nullable: head_nullable,
            args: ArgListId::EMPTY,
        }
    }

    /// The parameterized-alias use-site substitution face (#2410): the
    /// captured RHS stands for a written use with the alias's own
    /// parameters substituted (`Predicate<T> = Callback<T, bool>` spelled
    /// `Predicate<String>` reads `Callback<String, bool>`, and a co-alias
    /// chain continues into the terminal function shape). `None` keeps
    /// the use as written — a head that resolves to no parameterized
    /// alias (the element-model RHS rides the declaration only when the
    /// collector armed it; a restored pre-slot payload decodes empty and
    /// under-reports), a cyclic chain, and a right-hand shape the fold
    /// does not rewrite (`void`/`Null`/`Unknown`).
    fn fold_parameterized_alias(
        &self,
        name: StringId,
        nullable: bool,
        args: ArgListId,
    ) -> Option<Ty> {
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        let (mut cur, mut cur_nullable, mut cur_args) = (name, nullable, args);
        for _ in 0..8 {
            if !seen.insert(cur) {
                return None;
            }
            match self.instantiate_alias(cur, cur_args)? {
                Ty::Named {
                    name: next,
                    nullable: next_nullable,
                    args: next_args,
                } if next_args != ArgListId::EMPTY => {
                    cur = next;
                    cur_nullable |= next_nullable;
                    cur_args = next_args;
                }
                Ty::Named {
                    name: next,
                    nullable: next_nullable,
                    args: ArgListId::EMPTY,
                } => {
                    // The chain landed on a bare named type (a container
                    // head or a bare-alias face): the use reads that head
                    // whole — the bare loop's terminal shape.
                    return Some(Ty::Named {
                        name: next,
                        nullable: cur_nullable | next_nullable,
                        args: ArgListId::EMPTY,
                    });
                }
                Ty::Function {
                    shape,
                    nullable: next_nullable,
                } => {
                    return Some(Ty::Function {
                        shape,
                        nullable: cur_nullable | next_nullable,
                    });
                }
                Ty::Record {
                    shape,
                    nullable: next_nullable,
                } => {
                    return Some(Ty::Record {
                        shape,
                        nullable: cur_nullable | next_nullable,
                    });
                }
                Ty::Dynamic => return Some(Ty::Dynamic),
                _ => return None,
            }
        }
        None
    }

    /// One substitution step: `name`'s parameterized alias declaration
    /// with `args` zipped against its parameter spellings, the captured
    /// RHS projected through the boundary and substituted. `None` when
    /// the head resolves to no parameterized alias with a captured RHS.
    fn instantiate_alias(&self, name: StringId, args: ArgListId) -> Option<Ty> {
        let (owner, decl) = match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => (self.file, decl),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                (file, self.index.get(file).and_then(|f| f.decl(name))?)
            }
            _ => return None,
        };
        if decl.kind != MicroKind::TypeAlias
            || decl.type_params == 0
            || decl.var_ty == Ty::UNKNOWN
        {
            return None;
        }
        let written: Vec<Ty> = self.boundary.pool().get(args).to_vec();
        let mut subst: FxHashMap<StringId, Ty> = FxHashMap::default();
        for (spelling, arg) in decl.type_param_names.iter().zip(written) {
            subst.insert(*spelling, arg);
        }
        if subst.is_empty() {
            // No spellings to substitute against (a pre-slot payload) —
            // the RHS leaves would stand unsubstituted for the use.
            return None;
        }
        let rhs = self.boundary.ty(owner, decl.var_ty);
        Some(self.substitute_alias_rhs(rhs, &subst))
    }

    /// Deep-substitutes the alias's parameter spellings through a
    /// captured RHS, re-interning every touched pool slot into the
    /// diagnosing universe's pool. An unlisted bare leaf keeps its
    /// spelling — a non-parameter leaf name is some other declaration's
    /// face, not the alias's.
    fn substitute_alias_rhs(&self, ty: Ty, subst: &FxHashMap<StringId, Ty>) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable,
                args: ArgListId::EMPTY,
            } => match subst.get(&name) {
                Some(replacement) => self.with_use_question(*replacement, nullable),
                None => ty,
            },
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                // Snapshot the argument list out of the pool before
                // recursing: `pool()` holds a `Ref` on `args`, and the
                // recursive substitution re-borrows the same `RefCell`
                // mutably to intern the rewritten list — iterating the
                // slice while it is borrowed panics.
                let arg_list: Vec<Ty> = self.boundary.pool().get(args).to_vec();
                let substituted: Vec<Ty> = arg_list
                    .iter()
                    .map(|&arg| self.substitute_alias_rhs(arg, subst))
                    .collect();
                self.boundary.args.borrow_mut().named(name, nullable, &substituted)
            }
            Ty::Record { shape, nullable } => {
                let (positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.record_shape(shape);
                    (shape.positional.to_vec(), shape.named.to_vec())
                };
                let positional: Vec<Ty> = positional
                    .iter()
                    .map(|&field| self.substitute_alias_rhs(field, subst))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|&(field_name, field)| (field_name, self.substitute_alias_rhs(field, subst)))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, nullable)
            }
            Ty::Function { shape, nullable } => {
                let (type_params, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                let return_ty = self.substitute_alias_rhs(return_ty, subst);
                let positional: Vec<Ty> = positional
                    .iter()
                    .map(|&formal| self.substitute_alias_rhs(formal, subst))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|&(formal_name, formal)| {
                        (formal_name, self.substitute_alias_rhs(formal, subst))
                    })
                    .collect();
                self.boundary.args.borrow_mut().function(
                    type_params,
                    return_ty,
                    &positional,
                    &named,
                    nullable,
                )
            }
            _ => ty,
        }
    }

    /// Unites a substituted leaf's own `?` with the use's (the
    /// `T?=String` read is `String?`): re-interning only where the shape
    /// rides a pool slot; the unit kinds keep their nullability face.
    fn with_use_question(&self, ty: Ty, nullable: bool) -> Ty {
        if !nullable {
            return ty;
        }
        match ty {
            Ty::Named {
                name,
                nullable: own,
                args,
            } if args == ArgListId::EMPTY => Ty::Named {
                name,
                nullable: own || nullable,
                args,
            },
            Ty::Named {
                name,
                nullable: own,
                args,
            } => {
                let list: Vec<Ty> = self.boundary.pool().get(args).to_vec();
                self.boundary.args.borrow_mut().named(name, own || nullable, &list)
            }
            Ty::Record {
                shape,
                nullable: own,
            } => {
                let (positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.record_shape(shape);
                    (shape.positional.to_vec(), shape.named.to_vec())
                };
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, own || nullable)
            }
            Ty::Function {
                shape,
                nullable: own,
            } => {
                let (type_params, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                self.boundary.args.borrow_mut().function(
                    type_params,
                    return_ty,
                    &positional,
                    &named,
                    own || nullable,
                )
            }
            other => other,
        }
    }

    /// A record field getter's static type (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/type.dart `RecordTypeImpl`
    /// `namedField`/`positionalField` — the property-access resolver reads
    /// the field off the receiver's record type, `$1`-based positional
    /// spellings included). `receiver` folds through a type alias first
    /// (`typedef R = ({bool? strict}); r.strict`). A nullable record
    /// receiver answers `None` unless the access is null-aware (a plain
    /// access is the unchecked dereference face, not a field read; `r?.f`
    /// reads the field off the promoted non-null record and the caller
    /// joins the `?`), as does any non-record receiver, a
    /// `$0`/out-of-range/non-numeric spelling, and an absent named field
    /// (the `Object` members stay unmodeled — under-report).
    pub(crate) fn record_field_ty(
        &self,
        receiver: Ty,
        field: StringId,
        null_aware: bool,
    ) -> Option<Ty> {
        let Ty::Record { shape, nullable } = self.fold_alias_ty(receiver) else {
            return None;
        };
        if nullable && !null_aware {
            return None;
        }
        let pool = self.boundary.pool();
        let shape = pool.record_shape(shape);
        let ty = match shape.named.iter().find(|(name, _)| *name == field) {
            Some(&(_, ty)) => ty,
            None => {
                // Positional getters are `$1`, `$2`, … (1-based decimal, no
                // sign and no leading zero — `$01` is not a field getter).
                let index = self
                    .strings
                    .resolve(field)
                    .strip_prefix('$')
                    .filter(|digits| {
                        !digits.starts_with('0') && digits.bytes().all(|b| b.is_ascii_digit())
                    })
                    .and_then(|digits| digits.parse::<usize>().ok())?;
                *shape.positional.get(index.checked_sub(1)?)?
            }
        };
        drop(pool);
        // A field spelled through an alias reads as the aliased type, like
        // every declared type the frames bind.
        Some(self.fold_alias_ty(ty))
    }

    /// The captured right-hand type of the non-parameterized type alias
    /// `name`: this file's declaration table first, then the library
    /// namespace (the same resolution order a container use walks).
    pub(crate) fn alias_rhs(&self, name: StringId) -> Option<Ty> {
        let (owner, decl) = match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => (self.file, decl),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                (file, self.index.get(file).and_then(|f| f.decl(name))?)
            }
            _ => return None,
        };
        match decl.kind {
            // A foreign alias's RHS is a slot of its owner's pool: project it
            // into the diagnosing universe before anything reads it.
            MicroKind::TypeAlias if decl.type_params == 0 && decl.var_ty != Ty::UNKNOWN => {
                Some(self.boundary.ty(owner, decl.var_ty))
            }
            _ => None,
        }
    }

    /// The callable signature a parameterized function-type alias spells
    /// (`typedef Predicate<T> = bool Function(T)`): the element model keeps
    /// no RHS for a parameterized alias (its substitution face is
    /// unmodeled), but the bounds capture stores the written shape — the
    /// use-site callable verdict (`Predicate<T> test` invoked as
    /// `test(item)`), the invocation's return type, and its formal table
    /// all read it. Type parameters stay as leaf names (the written-shape
    /// face; `T`-typed formals/returns answer `T`, matching the oracle's
    /// unsubstituted-name behavior at this layer).
    /// `None` when `name` is no this-file alias or aliases a non-function
    /// type; a cross-file alias is invisible to the capture (under-report).
    /// A named-type RHS chases its co-alias's function shape with the
    /// written arguments substituted (`typedef B = A<int>` over
    /// `A<X> = X Function()` resolves `B` to `int Function()`).
    pub(crate) fn alias_fn_sig(&self, name: StringId) -> Option<Box<FnVarSig>> {
        // The dart:core function-alias face (SDK units are unindexed, so
        // the bounds tables never carry it): `Comparator<T>` spells
        // `int Function(T a, T b)` — the callable verdict and the send's
        // `int` return read it (`isSorted`'s `Comparator<T> compare`
        // invoked as `compare(a, b)`, #2172). The alias's own `T` stays a
        // named leaf — the same face a written `Cmp<T>` use answers when
        // the use-site substitution is not modeled — and a locally
        // declared `Comparator` keeps its own shape.
        if self.strings.resolve(name) == "Comparator"
            && self.bounds.alias_fn_shape(name).is_none()
            && self.bounds.alias_named_shape(name).is_none()
        {
            let t = self.intern("T");
            return Some(Box::new(FnVarSig {
                params: Box::default(),
                positional: vec![Ty::named(t), Ty::named(t)].into(),
                required_positional: 2,
                named: Box::default(),
                return_ty: Ty::named(self.members.primitive_ids[0]),
            }));
        }
        // The co-alias substitution maps a parameter name to its written
        // argument tree; it applies at the terminal `Fn` shape's leaves.
        let mut subst: FxHashMap<StringId, TypeTree> = FxHashMap::default();
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        let mut cur = name;
        // Deferred binding: a `Named` RHS's written arguments bind the NEXT
        // alias's parameters, whose names only surface once that alias's own
        // shape answers — carry the (substituted) argument trees forward and
        // zip them against the arriving param list. `B = A<int, String>`
        // over `A<S, T>` binds `S → int`, `T → String`.
        let mut pending_args: Vec<TypeTree> = Vec::new();
        let (ret, positional, named, required_positional) = loop {
            if !seen.insert(cur) {
                return None;
            }
            let args = std::mem::take(&mut pending_args);
            let subst_args: Vec<TypeTree> = args.iter().map(|a| subst_tree(a, &subst)).collect();
            if let Some((params, ret, positional, named, required_positional)) =
                self.bounds.alias_fn_shape(cur)
            {
                subst.clear();
                for ((param, _, _), arg) in params.iter().zip(subst_args.iter()) {
                    subst.insert(*param, arg.clone());
                }
                break (
                    ret.cloned(),
                    positional.to_vec(),
                    named.to_vec(),
                    required_positional,
                );
            }
            let (_params, tree) = match self.bounds.alias_named_shape(cur) {
                Some((params, tree)) => (params, tree),
                // The capture walks in declaration order, so a typedef
                // trailing the use's own declaration is not registered yet
                // when the streaming check reads it (the corpus html rows,
                // `_CharPredicate` declared after the `_skipUntil` /
                // `isWhitespace` tear-off arguments). The element model is
                // complete and order-independent — a non-parameterized
                // alias's RHS rides the declaration's armed type — so the
                // direct-alias case answers from there. A chain in flight
                // keeps the capture's substitution face (its `None` stays
                // the under-report direction).
                None if pending_args.is_empty() => return self.element_model_alias_sig(cur),
                None => return None,
            };
            pending_args = tree.args.clone();
            cur = tree.head;
        };
        let sig = FnVarSig {
            // The alias's own type parameters are bound by the written use
            // (`Predicate<T> test` already instantiates them) — they are
            // not call-site inference variables, so the signature carries
            // none; their leaves stay named `T` in the formal types.
            params: Box::default(),
            positional: positional
                .iter()
                .map(|tree| self.tree_ty(&subst_tree(tree, &subst)))
                .collect(),
            required_positional: u16::try_from(required_positional.unwrap_or(positional.len()))
                .unwrap_or(u16::MAX),
            named: named
                .iter()
                .map(|(name, tree, required)| {
                    (*name, self.tree_ty(&subst_tree(tree, &subst)), *required)
                })
                .collect(),
            return_ty: ret
                .map(|tree| self.tree_ty(&subst_tree(&tree, &subst)))
                .unwrap_or(Ty::Dynamic),
        };
        Some(Box::new(sig))
    }

    /// The element-model fallback of [`Self::alias_fn_sig`]: a
    /// non-parameterized function-type alias the streaming bounds capture
    /// has not reached yet. The collector stores the alias's RHS as the
    /// declaration's armed type (`Ty::Function`), so the complete element
    /// model answers where the walk-order capture cannot. `None` for every
    /// other shape: a parameterized alias keeps the capture's substitution
    /// face, and a non-function RHS is [`Self::alias_rhs`]'s business. The
    /// shape's optional-ness columns are not modeled (a `FnShape` carries
    /// no required boundary and no named-required flags) — positional
    /// formals claim required and named formals claim optional, the
    /// function-subtype direction the written `bool Function(String)`
    /// corpus shapes never leave.
    fn element_model_alias_sig(&self, name: StringId) -> Option<Box<FnVarSig>> {
        let (owner, decl) = match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => (self.file, decl),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                (file, self.index.get(file).and_then(|f| f.decl(name))?)
            }
            _ => return None,
        };
        let ty = match decl.kind {
            MicroKind::TypeAlias if decl.type_params == 0 && decl.var_ty != Ty::UNKNOWN => {
                self.boundary.ty(owner, decl.var_ty)
            }
            _ => return None,
        };
        let Ty::Function { shape, .. } = ty else {
            return None;
        };
        let pool = self.boundary.pool();
        let shape = pool.function_shape(shape);
        Some(Box::new(FnVarSig {
            params: Box::default(),
            positional: shape.positional.iter().copied().collect(),
            required_positional: u16::try_from(shape.positional.len()).unwrap_or(u16::MAX),
            named: shape
                .named
                .iter()
                .map(|(name, ty)| (*name, *ty, false))
                .collect(),
            return_ty: shape.return_ty,
        }))
    }

    /// A container member's declared field type from a clean walk
    /// (`Unknown` for executables and broken chains). The answer folds
    /// through a type alias first — the member read's static type is the
    /// resolved interface type (`c.f as int` over `IntAlias f` is dead).
    /// The fold is idempotent, so the promotable-field substitution that
    /// reads these rows' types re-folds harmlessly (#1409).
    pub(crate) fn member_declared_ty(&self, container: StringId, member: StringId) -> Ty {
        match self.walk_members(container, member) {
            Walk::Found(entry) if entry.kind == MemberKind::Field => {
                self.fold_alias_ty(entry.var_ty)
            }
            _ => Ty::UNKNOWN,
        }
    }

    /// The write target's `getWriteType` — the assignment resolver's
    /// `writeAcceptedType` face: a field answers its declared type; a
    /// setter answers its single value formal's declared type (the write
    /// resolves against `name=`, never the getter's return). `Unknown` for
    /// executables, accessors with uncaptured formals, and broken chains —
    /// the under-report direction.
    pub(crate) fn member_write_ty(&mut self, container: StringId, member: StringId) -> Ty {
        match self.walk_members(container, member) {
            Walk::Found(entry) if entry.kind == MemberKind::Field => {
                self.fold_alias_ty(entry.var_ty)
            }
            _ => {
                let setter = self.setter_spelling(member);
                match self.walk_members(container, setter) {
                    Walk::Found(entry) if entry.kind == MemberKind::Setter => entry
                        .formals
                        .as_deref()
                        .and_then(|f| f.positional.first())
                        .map_or(Ty::UNKNOWN, |formal| self.fold_alias_ty(formal.ty)),
                    _ => Ty::UNKNOWN,
                }
            }
        }
    }

    /// The promoted-property substitution (#1409): when a flow
    /// [`dartq_flow::FlowFact::PromotedPropertyRead`] row stands at
    /// exactly `range` (a staged check's own span — the merged member
    /// row's) and the member it names resolves to a promotable field, the
    /// parse-time type substitutes with its NON-NULL face — the official
    /// read's static type is the promoted one
    /// (`property_element_resolver.dart:2158-2166`), and the flow layer's
    /// marker cannot spell the promoted type (the field's declared type is
    /// this layer's knowledge). `None` leaves the staged verdict as-is.
    ///
    /// Eligibility is the official `isPropertyPromotable` verdict
    /// (upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/field_promotability.dart:70-76): a
    /// field, private, final, not external — and no conflicting
    /// non-final field or concrete getter of the same name in the unit
    /// ([`Engine::unit_field_name_conflicted`]; the current file's
    /// containers only — a `part`-split library under-reports the
    /// conflict) — under the
    /// unit's field-promotion grant ([`Engine::field_promotion_enabled`],
    /// the settled language version ≥ 3.2).
    pub(crate) fn promotable_field_substitution(
        &self,
        props: &PropPromotionStaging,
        range: SourceRange,
        from: Ty,
    ) -> Option<Ty> {
        if !props.staged(range) {
            return None;
        }
        let (entry, name, _origin) = self.member_read_resolution(range.offset)?;
        Self::promotable_field_entry(self, &entry, name).then_some(self.non_null_face(from))
    }

    /// The member entry behind one promotable-shaped read, with its
    /// spelling and defining file: the merge-time record's receiver walk
    /// (the live stack is gone by the teardown — pass 2 took it), else the
    /// published bare resolution (`RefTarget::Member` names the enclosing
    /// container). The eligibility gate
    /// ([`Engine::promotable_field_substitution`]) and the why-not-promoted
    /// context face (#1877) share the resolution.
    pub(crate) fn member_read_resolution(
        &self,
        read: u32,
    ) -> Option<(Projected<'a, MemberEntry>, StringId, FileId)> {
        if let Ok(index) = self
            .member_reads
            .binary_search_by(|row| row.offset.cmp(&read))
            && self.member_reads[index].offset == read
        {
            let row = self.member_reads[index];
            let walk = match row.recv {
                // An extension body's `this` member read resolves through
                // the extended type's face too — the own-container walk
                // answers first (the extension's members shadow the `on`
                // type's), the `on`-face climb is the fallback.
                Receiver::This { container, kind } if kind == ContainerKind::Extension => {
                    match self.walk_members(container, row.name) {
                        Walk::Missing => self.on_face_walk(container, kind, row.name),
                        walk => walk,
                    }
                }
                Receiver::This { container, .. }
                | Receiver::Typed {
                    type_name: container,
                    ..
                } => self.walk_members(container, row.name),
                Receiver::Super { container, .. } => self.walk_super(container, row.name),
                _ => return None,
            };
            return match walk {
                Walk::Found(entry) => {
                    let origin = self.boundary.member_origin(&entry)?;
                    Some((entry, row.name, origin))
                }
                _ => None,
            };
        }
        let found = self.refs.iter().rev().find(|r| r.range.offset == read)?;
        let RefTarget::Member { container } = found.target else {
            return None;
        };
        match self.walk_members(container, found.name) {
            Walk::Found(entry) => {
                let origin = self.boundary.member_origin(&entry)?;
                Some((entry, found.name, origin))
            }
            _ => None,
        }
    }

    // —— why-not-promoted context rows (#1877) ————————————————————————

    /// Stages one non-promotion fact row for the context face: the flow
    /// fact's entries resolve to [`NonPromotionReason`]s through
    /// `flow.non_promotion_reasons` (the property verdict this engine
    /// derives — [`Self::property_non_promotability`]), and the upstream
    /// nullable-key filter drops every potentially-nullable row (upstream
    /// Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart:783-791 — the
    /// consumer skips nullable keys and takes the FIRST contributing
    /// entry). The filter reads the flow layer's own nullability flag: the
    /// slot's non-null evidence marker (Unknown, non-nullable) keeps its
    /// row, and `void`/`Never` keys drop with it (upstream's filter skips
    /// `void` as potentially nullable; the M1 lattice has no bottom type).
    /// Facts arrive offset-sorted, so the push keeps the table sorted.
    pub(crate) fn stage_non_promotion(&mut self, flow: &Flow<'_>, fact: &FlowFact, span: Token) {
        let property = self.property_non_promotability(flow, fact);
        let Some(entries) = flow.non_promotion_context(fact) else {
            return;
        };
        let reasons = flow.non_promotion_reasons(fact, property);
        let filtered: Vec<NonPromotionReason> = entries
            .iter()
            .zip(reasons)
            .filter(|(entry, _)| match entry.ty.base {
                FlowBase::Void | FlowBase::Never => false,
                _ => !entry.ty.nullable,
            })
            .map(|(_, reason)| reason)
            .collect();
        // The read's spelling: the variable family reads the fact row's own
        // token, the property family the interned member name (the flow
        // layer's `'<prop>'` id).
        let name = match fact {
            FlowFact::NonPromotedPropertyRead { name, .. } => {
                let spelling = flow.name_lexeme(*name);
                self.strings.intern(spelling)
            }
            _ => {
                let text = self
                    .tokens
                    .get(span.id as usize)
                    .map(|token| &self.source[token.byte_range()])
                    .unwrap_or_default();
                self.strings.intern(text)
            }
        };
        self.non_promotions.push((
            span.range.offset,
            StagedNonPromotion {
                read: span.range,
                name,
                reasons: filtered,
            },
        ));
    }

    /// The element-layer verdict for one non-promotion fact row — the
    /// mirror of upstream's `whyPropertyIsNotPromotable` operation (upstream
    /// Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:8769-8772,
    /// the analyzer's element answer at
    /// flow_analysis_visitor.dart:1070-1094): a public member name answers
    /// [`PropertyNonPromotability::NotPrivate`] statically, a private one
    /// resolves through the member walk (`isNotField` → `isExternal` →
    /// `isNotFinal` → promotable), and `None` is the non-inherent conflict.
    /// An unresolvable member answers `None` too: upstream reports nothing
    /// for an unresolved `propertyMember` (the `propertyMember != null`
    /// gate), and the context face degrades the same way.
    pub(crate) fn property_non_promotability(
        &self,
        flow: &Flow<'_>,
        fact: &FlowFact,
    ) -> Option<PropertyNonPromotability> {
        let FlowFact::NonPromotedPropertyRead { name, .. } = fact else {
            return None;
        };
        let spelling = flow.name_lexeme(*name);
        if !spelling.starts_with('_') {
            return Some(PropertyNonPromotability::NotPrivate);
        }
        // The resolution keys on the read's own offset: the fact row's span
        // is the member read token — the same anchor the merge record and
        // the published bare resolution carry.
        let FlowFact::NonPromotedPropertyRead { span, .. } = fact else {
            return None;
        };
        let (entry, _name, _origin) = self.member_read_resolution(span.range.offset)?;
        if entry.kind != MemberKind::Field {
            return Some(PropertyNonPromotability::NotField);
        }
        if entry.is_external {
            return Some(PropertyNonPromotability::External);
        }
        if !entry.is_final {
            return Some(PropertyNonPromotability::NotFinal);
        }
        None
    }

    /// The why-not-promoted context rows for one read offset (#1877) —
    /// the oracle's `computeWhyNotPromotedMessages` face (upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart:772-813):
    /// the first contributing entry renders, per the #1873 variant→payload→
    /// anchor table —
    ///
    /// * `ExplicitWrite`/`Suspension`: "Variable '<name>' could not be
    ///   promoted due to an assignment / an 'await' or 'yield'" at the write
    ///   / suspension node, this file.
    /// * `PropertyInherent`: "'<prop>' refers to a getter/public
    ///   property/external field/non-final field so it couldn't be
    ///   promoted." at the property's declaration anchor, cross-file through
    ///   the defining file. An unresolvable member renders nothing — the
    ///   anchor never guesses.
    /// * `PropertyConflict`: renders nothing. Upstream enumerates the
    ///   conflicting declarations from the library-level
    ///   `fieldNameNonPromotabilityInfo`; this engine's conflict face is
    ///   unit-level only, so the row omits rather than fakes a partial
    ///   enumeration.
    /// * `This`: "'this' can't be promoted" at the read itself (the error
    ///   entity). Unreachable today — the flow layer emits no
    ///   `ThisNotPromoted` rows — kept for taxonomy completeness.
    ///
    /// An empty answer means "no context", never a dropped diagnostic.
    pub(crate) fn non_promotion_context_at(&self, read: u32) -> Vec<ContextMessage> {
        let Ok(index) = self.non_promotions.binary_search_by(|row| row.0.cmp(&read)) else {
            return Vec::new();
        };
        let staged = &self.non_promotions[index].1;
        let Some(reason) = staged.reasons.first() else {
            return Vec::new();
        };
        let name = self.strings.resolve(staged.name);
        match *reason {
            NonPromotionReason::ExplicitWrite { span } => {
                vec![ContextMessage {
                    message: SharedStr::from(format!(
                        "Variable '{name}' could not be promoted due to an assignment"
                    )),
                    file: self.file,
                    range: span,
                }]
            }
            NonPromotionReason::Suspension { span } => {
                vec![ContextMessage {
                    message: SharedStr::from(format!(
                        "Variable '{name}' could not be promoted due to an 'await' or 'yield'"
                    )),
                    file: self.file,
                    range: span,
                }]
            }
            NonPromotionReason::PropertyInherent { why } => {
                let kind = match why {
                    PropertyNonPromotability::NotField => "getter",
                    PropertyNonPromotability::NotPrivate => "public property",
                    PropertyNonPromotability::External => "external field",
                    PropertyNonPromotability::NotFinal => "non-final field",
                };
                let Some((entry, _name, origin)) = self.member_read_resolution(read) else {
                    return Vec::new();
                };
                if entry.name_range.offset == u32::MAX {
                    return Vec::new();
                }
                vec![ContextMessage {
                    message: SharedStr::from(format!(
                        "'{name}' refers to a {kind} so it couldn't be promoted."
                    )),
                    file: origin,
                    range: entry.name_range,
                }]
            }
            NonPromotionReason::PropertyConflict => Vec::new(),
            NonPromotionReason::This => {
                vec![ContextMessage {
                    message: SharedStr::from("'this' can't be promoted"),
                    file: self.file,
                    range: staged.read,
                }]
            }
        }
    }

    /// Drains the parse-time context requests (#1877): the facts are
    /// staged by now, so each deferred diagnostic takes its rows. An empty
    /// answer leaves the diagnostic untouched — the zero-cost no-context
    /// face.
    pub(crate) fn attach_non_promotion_contexts(&mut self) {
        for (index, read) in std::mem::take(&mut self.pending_non_promotion) {
            let context = self.non_promotion_context_at(read);
            if context.is_empty() {
                continue;
            }
            self.report.diagnostics[index].context = Some(context.into_boxed_slice());
        }
    }

    /// The parse-time emission that defers its why-not-promoted rows
    /// (#1877): the diagnostic pushes now, the context attaches at the
    /// teardown (the flow facts only exist after an executable replays).
    /// The report index holds — nothing removes report rows between push
    /// and drain.
    pub(crate) fn emit_non_promotion(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        read: u32,
    ) {
        let index = self.report.diagnostics.len();
        self.emit(code, range, a0, a1);
        self.pending_non_promotion.push((index, read));
    }

    /// The property-access span of a member read staged at `member`: the
    /// receiver's read span through the member's end (B5). `None` when the
    /// span is not a recorded member read or the receiver's span never
    /// materialized (a receiver-less recovery fold).
    pub(crate) fn property_access_span(&self, member: SourceRange) -> Option<SourceRange> {
        let index = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&member.offset))
            .ok()?;
        let read = self.member_reads[index];
        (read.offset == member.offset && read.recv_span.length > 0).then_some(SourceRange {
            offset: read.recv_span.offset,
            length: member.end().saturating_sub(read.recv_span.offset),
        })
    }

    /// The invocation-expression span of a send's callee row staged at
    /// `callee`: the receiver's start (a member invocation's `c` of
    /// `c.m(...)`) through the closing `)`. `None` when the offset recorded
    /// no completed send — a non-call operand keeps its own span.
    pub(crate) fn invocation_span(&self, callee: SourceRange) -> Option<SourceRange> {
        let index = self
            .call_ends
            .binary_search_by_key(&callee.offset, |(offset, _)| *offset)
            .ok()?;
        let (_, end) = self.call_ends[index];
        let start = self
            .property_access_span(callee)
            .map_or(callee.offset, |span| span.offset);
        Some(SourceRange {
            offset: start,
            length: end.saturating_sub(start),
        })
    }

    /// The promotable-field verdict on one resolved member entry (the
    /// flags half of [`Engine::promotable_field_substitution`]'s contract).
    /// The language-version grant ANDs in first (the official
    /// `isPromotable` conjunction, `flow_analysis.dart:7811-7815`):
    /// `fieldPromotionEnabled` comes from the *accessing* library's
    /// feature set (`ast_resolver.dart:139-140`), so a pre-3.2 unit never
    /// promotes — even a field declared in a 3.2 library.
    pub(crate) fn promotable_field_entry(
        &self,
        entry: &Projected<'_, MemberEntry>,
        name: StringId,
    ) -> bool {
        if !self.field_promotion_enabled {
            return false;
        }
        if entry.kind != MemberKind::Field || entry.is_static || !entry.is_final {
            return false;
        }
        if entry.is_external {
            return false;
        }
        if !self.strings.resolve(name).starts_with('_') {
            return false;
        }
        !self.unit_field_name_conflicted(name)
    }

    /// Whether any unit member of the given private name defeats field
    /// promotion: a non-final or external instance field, or a concrete
    /// (non-abstract) instance getter, under the same name — the official
    /// name-poisoning rule (`addField`/`addGetter`,
    /// field_promotability.dart:100-172). `noSuchMethod`-forwarder faces
    /// are not modeled here (under-report). Scans the current file's
    /// class-family containers only: upstream registers classes, enums,
    /// and mixins with the promotability pass and nothing else — named
    /// extensions and extension types are excluded (upstream Dart SDK
    /// source, pkg/analyzer/lib/src/summary2/library_builder.dart:885-905)
    /// — an extension getter is not a field, and a same-name extension
    /// getter is dead at every read site (the instance member wins), so
    /// scanning extension faces could only reject valid promotions
    /// (over-report).
    pub(crate) fn unit_field_name_conflicted(&self, name: StringId) -> bool {
        let Some(elements) = self.index.get(self.file) else {
            return false;
        };
        let conflicts = |entry: &MemberEntry| {
            let instance = !entry.is_static;
            (entry.kind == MemberKind::Field && instance && (entry.is_external || !entry.is_final))
                || (entry.kind == MemberKind::Getter && instance && !entry.is_abstract)
        };
        elements
            .containers()
            .filter(|(_, container)| {
                !matches!(
                    container.kind,
                    ContainerKind::Extension | ContainerKind::ExtensionType
                )
            })
            .any(|(_, container)| container.members.get(&name).is_some_and(conflicts))
    }

    /// The resolved top-level declaration under `name` (`None` = no check).
    /// An imported declaration projects into the diagnosing pool (F5) —
    /// its declared types are local slots from here on.
    pub(crate) fn top_level_decl(&self, name: StringId) -> Option<Projected<'a, TopDecl>> {
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => Some(Projected::Own(decl)),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                let decl = self.index.get(file).and_then(|f| f.decl(name))?;
                Some(self.boundary.top(file, name, decl))
            }
            _ => None,
        }
    }

    /// The class head a zero-parameter alias-to-class spells
    /// (`typedef L = Real;`): the folded right-hand side's non-nullable
    /// named head, when the head resolves to an indexed container — the
    /// invocation face's creation reshape (`L()` is `Real()`'s unnamed
    /// creation through the alias). Everything else (a parameterized
    /// alias, a function/record right-hand side, an unresolvable head)
    /// answers `None` and keeps the alias's own face.
    pub(crate) fn alias_class_head(&self, name: StringId) -> Option<(StringId, ContainerKind)> {
        let decl = self.top_level_decl(name)?;
        if decl.kind != MicroKind::TypeAlias || decl.type_params != 0 {
            return None;
        }
        let Ty::Named {
            name: head,
            nullable: false,
            ..
        } = self.fold_alias_ty(decl.var_ty)
        else {
            return None;
        };
        // The model's prefixed spellings arrive dot-joined (`shared.Real`,
        // the collector's one-head intern form): the class head the
        // creation reshape carries is the LEAF — the container dispatch
        // itself goes through the prefix-aware lookup.
        let spelling = self.strings.resolve(head);
        let (leaf, container) = match spelling.split_once('.') {
            Some((prefix, leaf)) => {
                let leaf = self.intern(leaf);
                let (container, _) = prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.intern(prefix),
                    leaf,
                )?;
                (leaf, container)
            }
            None => {
                let (container, _) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    head,
                )?;
                (head, container)
            }
        };
        Some((leaf, container.kind))
    }

    /// The full formal table of a resolved member callee, walked from `recv`.
    /// A foreign find's table comes back owned ([`std::borrow::Cow::Owned`],
    /// cloned off the boundary's projected copy); a local find borrows the
    /// unit's own (`'a`).
    pub(crate) fn member_formals_full(
        &self,
        recv: &Receiver,
        name: StringId,
    ) -> Option<std::borrow::Cow<'a, Formals>> {
        let (container, super_walk, recv_ty) = match *recv {
            // An extension body's `this` resolves its member through the
            // extended type's face as well (`ExtensionMemberScope` — the
            // member may live on the `on` type rather than the extension's
            // own map). The own-container walk answers first (the
            // extension's members shadow); the `on`-face climb is the
            // fallback — the same union [`Self::on_face_walk`] computes for
            // the bare-name face.
            Receiver::This { container, kind } if kind == ContainerKind::Extension => {
                let walk = match self.walk_members(container, name) {
                    Walk::Missing => self.on_face_walk(container, kind, name),
                    walk => walk,
                };
                return match walk {
                    Walk::Found(entry) if entry.kind == MemberKind::Method => Some(match entry {
                        Projected::Own(e) => match e.formals.as_deref() {
                            Some(formals) => std::borrow::Cow::Borrowed(formals),
                            None => return None,
                        },
                        Projected::Foreign(e) => match e.formals.as_deref() {
                            Some(formals) => std::borrow::Cow::Owned(formals.to_owned()),
                            None => return None,
                        },
                    }),
                    _ => None,
                };
            }
            Receiver::This { container, .. } => (container, false, None),
            Receiver::Super { container, .. } => (container, true, None),
            // A declaration-typed receiver's written type arguments
            // instantiate the member face (`Box<int> b; b.set(v)` checks
            // `v` against `int`, never the `T` the member declares) — the
            // environment composes at the find below.
            Receiver::Typed { type_name, ty } => (type_name, false, Some(ty)),
            // A folded creation (`C().m(...)`): the instance face — the
            // constructor-argument membership check walks the superclass
            // chain like a declaration-typed receiver. The table is not
            // name-level only: `check_arguments` stages and renders formal
            // types against it, and those renders stay single-pool because
            // the walks below route every find through the boundary.
            Receiver::Created { name, .. } => (name, false, None),
            Receiver::Container {
                name: container, ..
            } => {
                // F1: the hit may belong to a foreign unit's pool — project
                // it through the boundary before its formals flow into rows
                // or message text (the arms below reach the projection via
                // `walk_members`/`walk_super`; this arm resolves directly).
                let hit = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    container,
                )
                .and_then(|(c, owner)| c.members.get(&name).map(|entry| (owner, entry)));
                return match hit {
                    Some((owner, entry)) if entry.kind == MemberKind::Method => {
                        match self.boundary.member(owner, container, name, entry) {
                            Projected::Own(e) => {
                                e.formals.as_deref().map(std::borrow::Cow::Borrowed)
                            }
                            Projected::Foreign(e) => e
                                .formals
                                .as_deref()
                                .map(|formals| std::borrow::Cow::Owned(formals.to_owned())),
                        }
                    }
                    _ => None,
                };
            }
            // A prefixed container callee (`io.Directory.new()`): same
            // foreign-unit projection as the bare-container arm, resolved
            // through the prefix table — the name sits behind the prefix,
            // never in the diagnosing library's own scope.
            Receiver::PrefixedContainer {
                prefix,
                name: container,
                ..
            } => {
                let hit = prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    container,
                )
                .and_then(|(c, owner)| c.members.get(&name).map(|entry| (owner, entry)));
                return match hit {
                    Some((owner, entry)) if entry.kind == MemberKind::Method => {
                        match self.boundary.member(owner, container, name, entry) {
                            Projected::Own(e) => {
                                e.formals.as_deref().map(std::borrow::Cow::Borrowed)
                            }
                            Projected::Foreign(e) => e
                                .formals
                                .as_deref()
                                .map(|formals| std::borrow::Cow::Owned(formals.to_owned())),
                        }
                    }
                    _ => None,
                };
            }
            // A prefixed top-level callee (`p.f(...)` through `import as`):
            // the same unmerged-placeholder shape as the plain arm above —
            // the prefix resolves to its target file and the declaration's
            // formals project through the boundary (foreign slots re-intern
            // into the diagnosing pool, single-pool with the argument rows).
            // An unregistered prefix, an ambiguous or absent name, and a
            // non-executable target all keep the under-report.
            Receiver::Prefix { name: prefix } => {
                let dartq_resolution::Lookup::Found(NamespaceEntry::Decl(file)) = self
                    .namespaces
                    .lookup_prefixed(self.graph, self.lib, prefix, name)
                else {
                    return None;
                };
                let decl = self.index.get(file).and_then(|f| f.decl(name))?;
                return match self.boundary.top(file, name, decl) {
                    Projected::Own(decl) => decl.formals.as_deref().map(std::borrow::Cow::Borrowed),
                    Projected::Foreign(decl) => decl
                        .formals
                        .as_deref()
                        .map(|formals| std::borrow::Cow::Owned(formals.to_owned())),
                };
            }
            Receiver::Unknown => return None,
        };
        let walk = if super_walk {
            self.walk_super(container, name)
        } else {
            self.walk_members(container, name)
        };
        // An explicit instance receiver (`x.m`, `C().m`) that resolves a
        // STATIC member is the official `instance_access_to_static_member`
        // world (the access resolves the static with its DEDICATED error,
        // never the arity/argument checks against its formals) — and where
        // the flow later narrows the receiver past the static's
        // declaration (a pattern-narrowed scrutinee:
        // `switch (type) { CustomSqlType() => type.read(x) }` over
        // `BaseSqlType<T>`'s static helper), the oracle resolves the
        // instance member outright. Either way this parse-time close must
        // not check the send against the static's formals. ONLY value
        // receivers: an implicit-`this` UNQUALIFIED call (`_helper()` in a
        // class body) resolves the static through the class scope — a
        // legal call whose arity checks DO run — and `this`/`super`
        // receivers keep the static formals for the same reason.
        match walk {
            Walk::Found(entry)
                if entry.kind == MemberKind::Method
                    && !(entry.is_static
                        && matches!(recv, Receiver::Typed { .. } | Receiver::Created { .. })) =>
            {
                // The receiver-instantiation environment (#2433): a
                // `Receiver::Typed`'s written arguments substitute the
                // container parameters the member's formals spell. `None`
                // keeps the declared table byte-identical.
                let env =
                    recv_ty.and_then(|ty| self.member_recv_env(ty, container, &entry));
                if let Some(env) = env {
                    return match entry.formals.as_deref() {
                        Some(formals) => Some(std::borrow::Cow::Owned(
                            self.substitute_formals(formals, &env),
                        )),
                        None => None,
                    };
                }
                match entry {
                    Projected::Own(e) => match e.formals.as_deref() {
                        Some(formals) => Some(std::borrow::Cow::Borrowed(formals)),
                        None => None,
                    },
                    Projected::Foreign(e) => match e.formals.as_deref() {
                        Some(formals) => Some(std::borrow::Cow::Owned(formals.to_owned())),
                        None => None,
                    },
                }
            }
            _ => None,
        }
    }

    /// The receiver-instantiation environment of a `Receiver::Typed` member
    /// face: the written type arguments of the receiver's own type zipped
    /// onto the declaring container's parameters — `Box<int> b` hands
    /// `b.set`'s formal the `int` face the oracle's instantiated member
    /// reads (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/property_element_resolver.dart —
    /// the member lookup runs on the receiver's resolved type, whose type
    /// arguments instantiate the result's signature). `None` keeps the
    /// declared face: a bare head writes no arguments, an arity-mismatched
    /// spelling is the InvalidType recovery's face to keep raw, and a
    /// foreign container exposes only a parameter count — the name-forced
    /// rule the super-face instantiation keeps (one declared parameter, one
    /// written argument, one free parameter spelling in the member's
    /// signature). The member's own type parameters are bound in its
    /// signature and never substitute — `m<M>(M x)` shadows any same-spelled
    /// container key.
    pub(crate) fn member_recv_env(
        &self,
        recv_ty: Ty,
        container: StringId,
        entry: &MemberEntry,
    ) -> Option<Vec<(StringId, Ty)>> {
        let Ty::Named { name, args, .. } = recv_ty else {
            return None;
        };
        // An inherited member (`List<int>.map` declared on `Iterable<E>`):
        // the receiver viewed as an instance of the declaring container
        // (`asInstanceOf` over the structural hierarchy) spells the
        // environment the clause hops compose — `E := int`.
        if name != container {
            return self
                .ops?
                .instance_env(recv_ty, container, &self.boundary.args);
        }
        if args == ArgListId::EMPTY {
            return None;
        }
        let written: Vec<Ty> = self.boundary.args.borrow().get(args).to_vec();
        let mut map: Vec<(StringId, Ty)> = if let Some(params) =
            self.bounds.container_params(container)
        {
            let names: Vec<StringId> = params.iter().map(|(param, ..)| *param).collect();
            if names.len() != written.len() {
                return None;
            }
            names.into_iter().zip(written).collect()
        } else {
            // A foreign container's parameters spell nowhere in this unit.
            // The owner's element model carries the spellings in
            // declaration order (`Container::type_param_names`, #2502) —
            // freshly collected SDK faces and current persisted payloads
            // zip the receiver's written arguments onto them directly, the
            // exact pairing the oracle's receiver-type instantiation reads
            // (`V putIfAbsent(K key, ...)` substitutes by the declaration
            // `[K, V]`, never by the order the signature spells them).
            let (found, owner) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                container,
            )?;
            if !found.type_param_names.is_empty() {
                if found.type_param_names.len() != written.len() {
                    return None;
                }
                found.type_param_names.iter().copied().zip(written).collect()
            } else {
                // A pre-field persisted payload spells none — the legacy
                // name-forced rule binds when the member's own signature
                // spells exactly the declaring parameter set, in first-seen
                // order (`Map<K,V>`'s `forEach(action(K,V))` spells `K`/`V`
                // once each; `Iterable<E>`'s `fold` spells `E` in
                // `combine`). A count mismatch — the member never names
                // every parameter — keeps the under-report (the written
                // arguments cannot pair).
                let mut free: Vec<StringId> = Vec::new();
                let mut collect = |ty: Ty| {
                    self.member_free_params(ty, entry, owner, &mut free);
                };
                collect(entry.var_ty);
                collect(entry.return_ty);
                if let Some(formals) = entry.formals.as_deref() {
                    for formal in formals.positional.iter().chain(formals.named.iter()) {
                        collect(formal.ty);
                    }
                }
                if usize::from(found.type_params) != free.len() || free.len() != written.len() {
                    return None;
                }
                free.into_iter().zip(written).collect()
            }
        };
        map.retain(|(param, _)| !entry.type_param_names.contains(param));
        (!map.is_empty()).then_some(map)
    }

    /// One type's free container-parameter spellings: a bare `Named` head
    /// records when the member's own type parameters do not bind it and the
    /// declaring file's index tags it `TypeParam` (the same decl-kind probe
    /// the super-face instantiation's free-parameter walk reads); a
    /// parameterized head descends into its argument list. First-seen order,
    /// deduplicated.
    fn member_free_params(
        &self,
        ty: Ty,
        member: &MemberEntry,
        scope_file: FileId,
        free: &mut Vec<StringId>,
    ) {
        match ty {
            Ty::Named { name, args, .. } => {
                let is_type_param = !member.type_param_names.contains(&name)
                    && self
                        .index
                        .get(scope_file)
                        .and_then(|f| f.decl(name))
                        .is_some_and(|decl| decl.kind == crate::model::MicroKind::TypeParam);
                if is_type_param && !free.contains(&name) {
                    free.push(name);
                }
                if args == ArgListId::EMPTY {
                    return;
                }
                let list: Vec<Ty> = self.boundary.args.borrow().get(args).to_vec();
                for arg in list {
                    self.member_free_params(arg, member, scope_file, free);
                }
            }
            Ty::Function { shape, .. } => {
                let (positional, named, return_ty) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.positional.clone(),
                        shape.named.clone(),
                        shape.return_ty,
                    )
                };
                for ty in positional {
                    self.member_free_params(ty, member, scope_file, free);
                }
                for (_, ty) in named {
                    self.member_free_params(ty, member, scope_file, free);
                }
                self.member_free_params(return_ty, member, scope_file, free);
            }
            Ty::Record { shape, .. } => {
                let (positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.record_shape(shape);
                    (shape.positional.clone(), shape.named.clone())
                };
                for ty in positional {
                    self.member_free_params(ty, member, scope_file, free);
                }
                for (_, ty) in named {
                    self.member_free_params(ty, member, scope_file, free);
                }
            }
            _ => {}
        }
    }

    /// The formal table under a receiver-instantiation environment: every
    /// bare container-parameter spelling becomes its written argument
    /// ([`Engine::subst_params`]'s rewrite — records, function types, and
    /// nullable parameter occurrences pass through unrewritten, so the
    /// unrewritten formal keeps its declared face).
    fn substitute_formals(&self, formals: &Formals, env: &[(StringId, Ty)]) -> Formals {
        let subst = |formal: &crate::model::Formal| crate::model::Formal {
            ty: self.subst_params(formal.ty, env).unwrap_or(formal.ty),
            ..*formal
        };
        Formals {
            positional: formals.positional.iter().map(subst).collect(),
            named: formals.named.iter().map(subst).collect(),
        }
    }

    /// The receiver an unmerged member placeholder resolves against: the
    /// classification [`Self::merge_member`] applies at the dot event, plus
    /// the call-result arm — a plain row whose send already completed reads
    /// its callee's declared return type (`g().m(...)`; the result's static
    /// type IS the return type, the [`Self::receiver_static_ty`] invoke
    /// face). The merge itself keeps such receivers untyped (under-report);
    /// only the argument check widens, because it sees the placeholder
    /// before the merge runs.
    pub(crate) fn argument_receiver(&self, row: &Row) -> Receiver {
        match &row.shape {
            RowShape::Bare(info) => {
                let recv = Engine::receiver_of_bare(*info);
                if matches!(recv, Receiver::Unknown)
                    && matches!(row.intent, Intent::Invoke)
                    && let ty @ Ty::Named { name, .. } = self.callee_return_ty(row)
                {
                    Receiver::Typed {
                        type_name: name,
                        ty,
                    }
                } else {
                    recv
                }
            }
            RowShape::Receiver(recv) => *recv,
            _ => Receiver::Unknown,
        }
    }

    // —— resolution ——————————————————————————————————————————————————

    /// Resolves a bare name in value position: locals, type parameters, the
    /// member face (own-file container chain), the extension pool, enum
    /// synthesized members, then the scoped library lookup.
    pub(crate) fn resolve_bare(&self, name: StringId) -> (BareInfo, RefTarget) {
        if self.frames.iter().rev().any(|f| f.locals.contains(&name)) {
            return (
                BareInfo::Plain {
                    ty: self.typed_local(name).unwrap_or(Ty::UNKNOWN),
                },
                RefTarget::Local,
            );
        }
        // The frameless initializer world: a pattern variable of a switch /
        // if-case expression in a top-level variable's initializer binds
        // here (no executable frame exists to join). The clause's reads —
        // the case guard, the case body — resolve to it; the type stays
        // unknown (the binding carried none without a frame), so
        // member/type faces over it under-report.
        if self.initializer_locals.contains(&name) {
            return (BareInfo::Plain { ty: Ty::UNKNOWN }, RefTarget::Local);
        }
        if self.type_params.contains(&name) || self.pending_type_params.contains(&name) {
            // A still-parked parameter belongs to the header being parsed:
            // its own bound and its later siblings' bounds are in scope from
            // its declaration (`class C<T extends A<T>>`).
            return (
                BareInfo::Plain {
                    ty: Ty::named(name),
                },
                RefTarget::TypeParam,
            );
        }
        if let Some((container, kind)) = self.container_context() {
            // A record or function `on` type gives `this` a fully decidable
            // member face: the record's field getters (the function type's
            // `call`), then nothing — no supertype climb can hide a member.
            // A `$N`/named-field hit resolves like a member; a miss is
            // decidable too, so the own-face climb below must not suppress
            // it — the `Container` model has no clause edges for the written
            // `on` type, and the unindexed SDK heads its members walk
            // happens to climb (`int` of `on (int, int)` — latched as the
            // face's `on_type` name) would answer `ChainBroken` and steal
            // every miss the oracle reports (`$9`'s `undefined_identifier`,
            // `thirdMiss()`'s `undefined_method` for '<unknown>').
            let decidable_this = kind == ContainerKind::Extension
                && matches!(
                    self.this_static_ty(container, kind),
                    Ty::Record {
                        nullable: false,
                        ..
                    } | Ty::Function {
                        nullable: false,
                        ..
                    }
                );
            if decidable_this
                && let Some(ty) =
                    self.record_field_ty(self.this_static_ty(container, kind), name, false)
            {
                return (BareInfo::Plain { ty }, RefTarget::Opaque);
            }
            // #1701: an unnamed extension resolves bare names against its
            // application face's member map — upstream treats named and
            // unnamed declarations alike on this face (upstream Dart SDK
            // source, pkg/analyzer/lib/src/dart/resolver/
            // property_element_resolver.dart:2138-2141, the scope lookup
            // then the implicit-this getter on the on-type). A face hit
            // suppresses exactly like a named container's member hit; a
            // miss falls through as a clean `Walk::Missing` would — the
            // placeholder frame name's own walk always answers
            // `ChainBroken` (the collector registers no "" container) and
            // must not suppress every bare name in the body.
            let member_found = match self.unnamed_extension_members() {
                Some(members) => {
                    if let Some(entry) = members.get(&name) {
                        let ty = match entry.kind {
                            MemberKind::Field => entry.var_ty,
                            _ => Ty::UNKNOWN,
                        };
                        return (BareInfo::Plain { ty }, RefTarget::Member { container });
                    }
                    false
                }
                // A named extension still consults its own member map (the
                // `walk_members` climb's first level is the container's
                // declared members — a declared `helper` answers before any
                // `on`-type face). For a record/function `this` the climb
                // past that level cannot hide anything, so the own-face
                // miss reads as `Walk::Missing` rather than the broken
                // chain the unindexed `on`-type head would answer.
                None if decidable_this => {
                    matches!(self.walk_members(container, name), Walk::Found(_))
                }
                // Only a clean `Walk::Found` claims the member face. A
                // broken chain (a supertype hop that resolved to a
                // declaration the index cannot model) leaves the member
                // question undecided in BOTH directions, so the name falls
                // through to the ordinary scope lookup like a clean miss:
                // a top-level/imported hit resolves there (the bare
                // creation send `Thing()` inside a broken-chain class is
                // the class's creation, never an implicit-this member —
                // the drift corpus's 17 false `undefined_method` rows),
                // and a full miss reports the `undefined_*` face against
                // `this`'s static type exactly like the oracle's final
                // member scope (the star corpus's silent o-only mirrors —
                // the oracle's own interface ends at the resolved part of
                // the chain and still answers the miss).
                //
                // The frozen-face refinement (#2420 D1): a broken chain over
                // the SDK's names-only stub is not undecided — the member
                // tables carry `dart:core`'s declared instance surface
                // exactly (the same verdict the dotted-member arm asks
                // below). `extends Iterable<E>` therefore claims
                // `iterator`/`first`/`length`, whether the argument is a
                // real type or an unresolvable one (probed 3.47.1: the
                // oracle keeps the supertype and reads every member
                // silently). Anything else keeps the fall-through.
                None => match self.walk_members(container, name) {
                    Walk::Found(_) => true,
                    Walk::ChainBroken => matches!(
                        self.frozen_member_verdict(container, name),
                        super::row_resolution::FrozenFace::Defined
                    ),
                    Walk::Missing => false,
                },
            };
            if member_found {
                return (
                    BareInfo::Plain {
                        ty: self.member_declared_ty(container, name),
                    },
                    RefTarget::Member { container },
                );
            }
            // The `on`-face member scope (#2186 S2): inside a mixin or an
            // extension body a bare name also resolves through the
            // `on`-constrained / `on`-typed member face — `this`'s
            // interface includes it (upstream's `unpromotedThisType`,
            // type_analyzer.dart). A hit is the same member resolution the
            // own-face arm returns; a broken climb suppresses identically —
            // the member could live on the unindexed hop. Only the clean
            // miss is decidable: it leaves the member scope final and the
            // scoped lookup decides the reference — the emit's
            // `undefined_identifier` the blanket `on_opaque` skip used to
            // steal.
            if matches!(kind, ContainerKind::Mixin | ContainerKind::Extension) {
                match self.on_face_walk(container, kind, name) {
                    // The hit belongs to a constraint's face, not the
                    // enclosing container's own map. A mixin's constraint
                    // owns no concrete declaration — `RefTarget::Opaque`
                    // (the "resolved, cannot name the owner" answer) keeps
                    // the read from being credited to the enclosing
                    // container's member set, which would leave the owning
                    // face's field looking unread downstream. An
                    // extension's `on` type DOES own one: `this` inside the
                    // extension IS the extended type, so the read's owner
                    // is the `on` container (the same face
                    // `note_member_read`'s `This` arm credits) — pass 2's
                    // unused-field join can then spare the declaring field
                    // (#2282). An unresolved `on` name has no honest owner
                    // and stays opaque.
                    Walk::Found(entry) => {
                        let ty = match entry.kind {
                            MemberKind::Field => self.fold_alias_ty(entry.var_ty),
                            _ => Ty::UNKNOWN,
                        };
                        let target = if kind == ContainerKind::Extension {
                            self.enclosing_extension_on_type(container)
                                .map_or(RefTarget::Opaque, |on| RefTarget::Member { container: on })
                        } else {
                            RefTarget::Opaque
                        };
                        return (BareInfo::Plain { ty }, target);
                    }
                    // The broken climb's blanket answer suppresses the
                    // ordinary bare-name face (the member could live on the
                    // unindexed hop), but never the annotation face: an
                    // annotation name resolves through the scope lookup alone
                    // (upstream Dart SDK source,
                    // pkg/analyzer/lib/src/dart/resolver/
                    // annotation_resolver.dart — the annotation expression is
                    // resolved like any other constant-context expression,
                    // and a lookup failure is reported by that resolution),
                    // so the name falls through to the scoped lookup and its
                    // miss reports `undefined_annotation` (the corpus's
                    // `@useResult` inside an `on String` extension whose
                    // climb ends at the unindexed core face). Annotation
                    // arguments resolve in the merge pass after the metadata
                    // window closes, so the window flags the annotation
                    // name's own resolution only.
                    Walk::ChainBroken if self.annotation_at.is_none() => {
                        return (BareInfo::Plain { ty: Ty::UNKNOWN }, RefTarget::Opaque);
                    }
                    Walk::ChainBroken | Walk::Missing => {}
                }
            }
            if kind == ContainerKind::Enum && self.members.enum_synth_members.contains(&name) {
                return (BareInfo::Plain { ty: Ty::UNKNOWN }, RefTarget::Opaque);
            }
            if self.instance_context() && self.members.object_members.contains(&name) {
                return (BareInfo::Plain { ty: Ty::UNKNOWN }, RefTarget::Opaque);
            }
        }
        if self
            .index
            .get(self.file)
            .is_some_and(|f| f.has_extension_member(name))
        {
            return (BareInfo::Plain { ty: Ty::UNKNOWN }, RefTarget::Opaque);
        }
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => (
                self.bare_info_of(name, decl),
                RefTarget::Decl { file: self.file },
            ),
            ScopedHit::Found(entry) => self.bare_info_of_entry(name, entry),
            ScopedHit::Ambiguous(_) => (BareInfo::Plain { ty: Ty::UNKNOWN }, RefTarget::Opaque),
            ScopedHit::NotFound => {
                // The implicit-`this` extension application: a bare name
                // missing every scope still resolves as `this.name` when an
                // in-scope extension face declares `name` on `this`'s
                // static type (`extension_member_resolver.dart`
                // `findExtension` runs after the scope miss — the same
                // verdict `member_defined` reaches for the explicit
                // `x.name` form). An applied face suppresses the miss
                // exactly like an `Opaque` member hit; an ambiguous or
                // inapplicable pool changes nothing — the oracle's
                // `undefined_*` face stands.
                if let Some((container, kind)) = self.container_context()
                    && self.instance_context()
                    && self.this_extension_member_applies(container, kind, name)
                {
                    return (BareInfo::Plain { ty: Ty::UNKNOWN }, RefTarget::Opaque);
                }
                (self.missing_info(name), RefTarget::Missing)
            }
        }
    }

    /// Whether `name` is an applicable extension member for the implicit
    /// `this` inside `container` (`extension_member_resolver.dart`'s
    /// `_getApplicable` over the `accessibleExtensions` scope face): the
    /// unit's own faces plus the named faces every import edge's target
    /// exports under its combinators — the same pool
    /// [`Engine::accessible_extension_faces`] materializes for the
    /// explicit-receiver face, recomputed here because the bare-name path
    /// holds `&self` (a bare miss is a cold path — the per-call fill costs
    /// a small scan, not a state). Applicability is the receiver's subtype
    /// relation to the face's `on` type, argument-gated exactly like
    /// [`Engine::extension_application`]; the receiver is `this`'s static
    /// type — the `on` type inside an extension body, the enclosing
    /// container's head everywhere else.
    fn this_extension_member_applies(
        &self,
        container: StringId,
        kind: ContainerKind,
        name: StringId,
    ) -> bool {
        let receiver_ty = self.this_static_ty(container, kind);
        let Ty::Named {
            name: receiver,
            nullable: receiver_nullable,
            ..
        } = receiver_ty
        else {
            return false;
        };
        // A nullable `this` (`on C?`, an unbounded `on T`) reads a
        // possibly-null receiver — the `unchecked_use_of_nullable_value`
        // family owns that face; a bare miss stays reportable there.
        if receiver_nullable {
            return false;
        }
        // The oracle gates application on the receiver resolving at its
        // head — an `InvalidType` `this` applies no extension. The bare
        // head resolved to the enclosing container by construction, so
        // only the `on`-clause head needs the check.
        if kind == ContainerKind::Extension
            && self.type_resolution(receiver).0 != TypeResolution::Type
        {
            return false;
        }
        let world = self.name_world();
        let object = self.intern("Object");
        let iterable = self.intern("Iterable");
        let strings = self.strings;
        let reaches = |sub: StringId, sup: StringId| {
            sub == sup
                || sup == object
                || world.named_sub(sub, sup)
                || (sup == iterable && {
                    let text = strings.resolve(sub);
                    matches!(text, "List" | "Set")
                        || crate::member_face::COLLECTION_INTERFACE_ALIASES
                            .iter()
                            .any(|(alias, roots)| *alias == text && roots.contains(&"Iterable"))
                })
        };
        let arg_applicable = |face: &ExtensionFace, face_file: FileId| {
            let on_ty = self.boundary.ty(face_file, face.on_ty);
            let (
                Ty::Named {
                    name: receiver_head,
                    args: receiver_args,
                    ..
                },
                Ty::Named {
                    name: on_head,
                    args: on_args,
                    ..
                },
            ) = (receiver_ty, on_ty)
            else {
                return true;
            };
            if receiver_head != on_head {
                return true;
            }
            let receiver_args = self.boundary.pool().get(receiver_args).to_vec();
            let on_args = self.boundary.pool().get(on_args).to_vec();
            let receiver_args = if receiver_args.is_empty() && !on_args.is_empty() {
                vec![Ty::Dynamic; on_args.len()]
            } else {
                receiver_args
            };
            if receiver_args.len() != on_args.len() {
                return true;
            }
            receiver_args
                .iter()
                .zip(on_args.iter())
                .all(|(&receiver_arg, &on_arg)| {
                    if let Ty::Named {
                        name: arg_name,
                        args,
                        ..
                    } = on_arg
                        && args == ArgListId::EMPTY
                        && face.type_param_names.contains(&arg_name)
                    {
                        return true;
                    }
                    let (
                        Ty::Named {
                            name: arg_head,
                            args: arg_args,
                            ..
                        },
                        Ty::Named {
                            name: receiver_head,
                            args: receiver_args,
                            ..
                        },
                    ) = (on_arg, receiver_arg)
                    else {
                        return !matches!(receiver_arg, Ty::Dynamic)
                            || matches!(on_arg, Ty::Dynamic);
                    };
                    if arg_args != ArgListId::EMPTY || receiver_args != ArgListId::EMPTY {
                        return true;
                    }
                    reaches(receiver_head, arg_head)
                })
        };
        // The `accessibleExtensions` enumeration (`scope.dart:473-488`):
        // own unit files first, then the named faces each import edge's
        // target exports — combinator filtered; an unnamed face never
        // reaches the import scope but an own-library unnamed face
        // applies all the same.
        let probe = |face: &ExtensionFace, file: FileId| -> bool {
            let Some(on) = face.on_type else {
                return false;
            };
            if !face.members.contains_key(&name) {
                return false;
            }
            // Receiver nullability already returned above; against a
            // non-nullable `this` both `on` nullability faces apply.
            reaches(receiver, on) && arg_applicable(face, file)
        };
        let unit_has = |file: FileId| -> bool {
            self.index.get(file).is_some_and(|elements| {
                elements
                    .extension_faces()
                    .iter()
                    .any(|face| probe(face, file))
            })
        };
        if unit_has(self.file) {
            return true;
        }
        for &unit in self.graph.unit_files(self.lib) {
            if unit != self.file && unit_has(unit) {
                return true;
            }
        }
        for edge in self.graph.import_edges(self.lib) {
            let Some(target) = edge.target else {
                continue;
            };
            for (exported, entry) in self.graph.export_scope(target) {
                if !edge.combinators.allows(exported) {
                    continue;
                }
                let Some(elements) = self.index.get(entry.file) else {
                    continue;
                };
                if elements
                    .extension_faces()
                    .iter()
                    .any(|face| face.name == Some(exported) && probe(face, entry.file))
                {
                    return true;
                }
            }
        }
        false
    }

    /// The `on`-face member walk for a bare name inside a mixin or an
    /// extension body (#2186 S2): the member scope `this` carries beyond the
    /// container's own map — a mixin's `on` constraints (each constraint's
    /// members plus its supertypes, the same seed [`Engine::walk_super`]
    /// applies for `super.m`) and an extension's `on` type (the extended
    /// type's whole member face, `this`'s static type inside the body —
    /// upstream `ExtensionMemberScope` / `unpromotedThisType`). Answers the
    /// shared face-list climb's verdict: `Missing` only when every
    /// constraint hop was decidable — an undefined `on` name contributes
    /// nothing (the oracle's `InvalidType`, the member scope stays final) —
    /// `ChainBroken` when a hop resolves to a declaration the index cannot
    /// model (its members could carry the name). Every other container
    /// kind answers `Missing`.
    pub(crate) fn on_face_walk(
        &self,
        container: StringId,
        kind: ContainerKind,
        member: StringId,
    ) -> Walk<'a> {
        let mut seen: FxHashSet<StringId> = FxHashSet::default();
        seen.insert(container);
        match kind {
            ContainerKind::Mixin => {
                let Some(decl) = self
                    .index
                    .get(self.file)
                    .and_then(|f| f.container(container))
                else {
                    // The enclosing container the index cannot model — its
                    // constraint face is unknowable, so a member could hide.
                    return Walk::ChainBroken;
                };
                // The mixin body's member scope is `this`'s interface: the
                // `on` constraints AND the `implements` faces — the oracle's
                // `MixinMemberScope` resolves a bare name through both
                // (`toSql` inside `mixin M implements Base` resolves
                // `Base.toSql`, #2473 G2b). `on` faces still climb their own
                // constraints transitively via `climb_on`.
                self.walk_face_list(
                    decl.on_clause
                        .iter()
                        .chain(decl.implements_clause.iter())
                        .map(|face| (*face, self.file, container)),
                    self.lib,
                    member,
                    &mut seen,
                    true,
                )
            }
            ContainerKind::Extension => {
                // The written `on` type's own shape decides first: a record
                // (`on (A, B)`) or function (`on void Function()`) `this`
                // carries no climbable supertype edge — its member face is
                // the field getters/`call` alone (answered in the
                // record-`this` arm before this walk), so the member scope
                // ends in a clean miss and the scoped lookup decides. The
                // latched `on_type` name cannot see that: for a record
                // clause it is the LAST field type's head (`on (int, int)`
                // latches `int`), which resolves to an unindexed SDK
                // declaration — a `ChainBroken` that would suppress every
                // bare miss the oracle reports (`$9`'s
                // `undefined_identifier`, `thirdMiss()`'s
                // `undefined_method` for '<unknown>'). An `Unknown` `on_ty`
                // (missing/recovery clause) contributes nothing either.
                let on_ty = self.this_static_ty(container, ContainerKind::Extension);
                if !matches!(on_ty, Ty::Named { .. }) {
                    return Walk::Missing;
                }
                let Some(on) = self.enclosing_extension_on_type(container) else {
                    // No honest `on` name — the extended type contributes
                    // nothing (the oracle's `InvalidType` extendedType).
                    return Walk::Missing;
                };
                match self.walk_face_list(
                    [(on, self.file, container)].into_iter(),
                    self.lib,
                    member,
                    &mut seen,
                    true,
                ) {
                    Walk::Found(entry) => Walk::Found(entry),
                    walk => {
                        // An `on` head that resolves to nothing (`extension
                        // E on Gone` — the same `Missing` verdict that
                        // reports `undefined_class` on the clause) is the
                        // oracle's `InvalidType` extended type: it
                        // contributes no member face at all, so the
                        // extension's scope is its own member map alone and
                        // a bare miss reports `undefined_identifier` (#2186
                        // S2, probed 3.13.1). A head the scope resolves or
                        // cannot decide (an unindexed SDK interface such as
                        // `MessagePort`), and a frozen SDK face spelling
                        // (`Stream` — the tables stand in for the library
                        // the diagnosing scope may not index), take the
                        // frozen consult below.
                        if matches!(self.type_resolution(on).0, TypeResolution::Missing)
                            && !Self::frozen_face_text(self.strings.resolve(on))
                        {
                            return Walk::Missing;
                        }
                        // The `on` head's frozen-face consult (#2474 G1): an
                        // SDK interface name the scope resolves ambiguously
                        // (`Stream` — `dart:core`'s re-export of `dart:async`'s
                        // decl lands two `Decl` face files, the `Ambiguous`
                        // verdict) walks to a dead-edge `Missing` here even
                        // though the oracle's `this` carries the interface's
                        // full member surface. The frozen member tables model
                        // exactly that surface: `Defined` suppresses (the
                        // member lives on the `on` type — the same
                        // undecidable-owner posture `ChainBroken` carries),
                        // `Unknown` stays undecidable, and `Absent` keeps
                        // the walk's clean miss (a name `Stream` genuinely
                        // lacks still reports `undefined_*` for 'Stream').
                        match self.frozen_member_verdict(on, member) {
                            super::row_resolution::FrozenFace::Defined
                            | super::row_resolution::FrozenFace::Unknown => Walk::ChainBroken,
                            super::row_resolution::FrozenFace::Absent => walk,
                        }
                    }
                }
            }
            _ => Walk::Missing,
        }
    }

    /// The bare-name member union [`Engine::resolve_bare`] computes: the
    /// container's own climb (`walk_members` — declared members plus the
    /// supertype chain), then the `on`-face climb for a mixin or extension
    /// container whose own climb did not find the member. The invocation
    /// face shares this — a bare send inside an extension body resolves
    /// against `this`'s full interface (upstream `ExtensionMemberScope`),
    /// so `dartCast<String>()` inside `extension E on Expression<int>`
    /// finds `Expression.dartCast` here even though the extension's own
    /// member map lacks it (#2473 G2a).
    pub(crate) fn bare_member_walk(
        &self,
        container: StringId,
        kind: ContainerKind,
        member: StringId,
    ) -> Walk<'a> {
        match self.walk_members(container, member) {
            Walk::Missing if matches!(kind, ContainerKind::Mixin | ContainerKind::Extension) => {
                self.on_face_walk(container, kind, member)
            }
            walk => walk,
        }
    }

    /// The `on` clause type name of the innermost extension frame: the
    /// named container's application face by name, or the frame-latched
    /// face for an unnamed extension (the same latch
    /// [`Engine::unnamed_extension_members`] walks). `None` for a face whose
    /// clause never produced an honest name and inside every non-extension
    /// container.
    pub(crate) fn enclosing_extension_on_type(&self, container: StringId) -> Option<StringId> {
        let elements = self.index.get(self.file)?;
        let innermost = self
            .frames
            .iter()
            .rev()
            .find(|f| matches!(f.kind, FrameKind::Container { .. }));
        if let Some(FrameKind::Container {
            extension_face: Some(face),
            ..
        }) = innermost.map(|f| &f.kind)
        {
            return elements
                .extension_faces()
                .iter()
                .filter(|f| f.name.is_none())
                .nth(*face)?
                .on_type;
        }
        elements
            .extension_faces()
            .iter()
            .find(|face| face.name == Some(container))
            .and_then(|face| face.on_type)
    }

    /// The push-time context snapshot of a bare miss (pass 2 runs after the
    /// frames tear down — see [`BareInfo::Missing`]).
    pub(crate) fn missing_info(&self, name: StringId) -> BareInfo {
        let context = self.container_context();
        BareInfo::Missing {
            // The UnqualifiedLookupDomain names the enclosing class-family
            // element whenever the miss sits inside its region — static
            // members, field initializers, and nested locals included
            // (upstream resolves the bare invocation through the
            // class-member namespace first, so `static m() => C()` reports
            // `undefined_method` on the class, never `undefined_function`).
            // `enclosingInstanceElement` upstream is an `InstanceElement` —
            // classes, enums, mixins, AND extension types
            // (`ExtensionTypeElementImpl extends InterfaceElementImpl`; the
            // implicit `this` of an extension-type body carries the
            // extension type itself, so its bare miss names the head like a
            // class body names the class) — but never an extension, whose
            // bare miss keeps the top-level `undefined_function` face (the
            // on-type names only the explicit-`this` member walks). The
            // header's clause text pushes TypeUse rows, never Bare rows, so
            // no header-position guard is needed.
            this_type: context
                .filter(|(_, kind)| {
                    matches!(
                        kind,
                        ContainerKind::Class
                            | ContainerKind::Enum
                            | ContainerKind::Mixin
                            | ContainerKind::ExtensionType
                    )
                })
                .map(|(name, _)| name),
            // Inside an `extension` body a bare invocation miss names the
            // ON type's `this` — `undefined_method` for the extended type
            // (probed: `extension E on C` → 'C', `on List<int>` → 'List',
            // `on void Function()` → 'Function', `on (int, int)` →
            // '<unknown>'; upstream `unpromotedThisType` is the extended
            // type, the receiver the method lookup reports). An
            // unresolvable `on` name reports NOTHING — the `this` is the
            // oracle's `InvalidType` and the lookup dies on it — and a
            // nullable `this` (`on int?`, an unbounded `on T`) takes the
            // `unchecked_use_of_nullable_value` face this family cannot
            // spell, so both keep `None` (the under-report direction).
            ext_invoke: context
                .filter(|(_, kind)| *kind == ContainerKind::Extension)
                .map(|(container, _)| self.extension_invoke_target(container)),
            in_exec: self.in_executable(),
            // The constraint-member conservatism (#2186 S2), now the
            // oracle's own decision procedure instead of the blanket skip:
            // inside a mixin or an extension body a miss is suppressed only
            // when a constraint member could actually hide behind a hop the
            // index cannot model — the `on`-face climb's `ChainBroken`.
            // Every decidable outcome reports exactly like the oracle: an
            // `on` name that resolves to nothing contributes an
            // `InvalidType` face, and a fully walked face carries the
            // member set — the bare-name miss is `undefined_identifier`
            // (lexical_lookup.dart's fall-through).
            on_opaque: context.is_some_and(|(container, kind)| {
                matches!(kind, ContainerKind::Mixin | ContainerKind::Extension)
                    && matches!(self.on_face_walk(container, kind, name), Walk::ChainBroken)
            }),
        }
    }

    /// The `undefined_method` host for a bare invocation miss inside an
    /// `extension` body (the `ext_invoke` slot of [`BareInfo::Missing`]):
    /// `Some` the display name the oracle reports — the resolved `on`
    /// type's head (`on C` → 'C', `on List<int>` → 'List'), `<unknown>`
    /// for a face that never interned a named head (a record `on` clause's
    /// `this` is the record itself — its spelled display is '<unknown>')
    /// or `on void Function()` (whose head displays 'Function'). `None`
    /// when the `this` cannot carry the face: an unresolvable `on` name
    /// (the `InvalidType` `this` — every member question on it is silent),
    /// a nullable `this` (the `unchecked_use_of_nullable_value` family),
    /// or an in-scope type-parameter head (`on T` — a `T` bound the model
    /// cannot decide, same nullable family).
    fn extension_invoke_target(&self, container: StringId) -> Option<StringId> {
        match self.this_static_ty(container, ContainerKind::Extension) {
            Ty::Named {
                name,
                nullable: false,
                ..
            } => {
                if self.type_params.contains(&name) || self.pending_type_params.contains(&name) {
                    None
                } else {
                    match self.type_resolution(name).0 {
                        TypeResolution::Type => Some(name),
                        _ => None,
                    }
                }
            }
            // `on void Function()`: the `this` is the function type, whose
            // display head is 'Function' (the oracle's element display).
            Ty::Function {
                nullable: false, ..
            } => Some(self.intern("Function")),
            // `on (int, int)`: a record `this` displays '<unknown>' in the
            // oracle's message — the extended type has no element name.
            Ty::Record {
                nullable: false, ..
            } => Some(self.intern("<unknown>")),
            // A nullable `this` (`on int?`, `on T`, `on C?`) takes the
            // `unchecked_use_of_nullable_value` face this family cannot
            // spell — the under-report direction. `dynamic`/`Never`/`Null`
            // `on` types carry no decidable member face either.
            _ => None,
        }
    }

    pub(crate) fn bare_info_of_entry(
        &self,
        name: StringId,
        entry: NamespaceEntry,
    ) -> (BareInfo, RefTarget) {
        match entry {
            NamespaceEntry::Prefix => (BareInfo::Prefix { name }, RefTarget::Prefix),
            NamespaceEntry::Decl(file) => match self.index.get(file) {
                Some(elements) => match elements.decl(name) {
                    Some(decl) => {
                        // F5: an imported declaration's declared type
                        // projects before it arms a row.
                        let projected = self.boundary.top(file, name, decl);
                        (
                            self.bare_info_of(name, &projected),
                            RefTarget::Decl { file },
                        )
                    }
                    None => (
                        self.bare_info_of(name, &TopDecl::bare(MicroKind::TopLevelVar)),
                        RefTarget::Decl { file },
                    ),
                },
                // A declaring file outside the element index is the
                // pipeline's synthetic SDK face (`<dart:*>` files carry
                // graph declarations but no model). A primitive spelling
                // there is the definitional core class — the class-static
                // face the member read resolves against (`double.nan`'s
                // `isDoubleNan`, best_practices_verifier.dart:2264-2277) —
                // so the receiver classifies like an indexed class instead
                // of the untyped `Plain` shape, which folds to
                // `Receiver::Unknown` at the dot merge and silences every
                // container face downstream. Every other unindexed name
                // keeps `Plain` (kind unknown, the under-report posture).
                None if self.members.primitive_ids.contains(&name) => (
                    BareInfo::Container {
                        name,
                        kind: ContainerKind::Class,
                    },
                    RefTarget::Decl { file },
                ),
                None => (
                    self.bare_info_of(name, &TopDecl::bare(MicroKind::TopLevelVar)),
                    RefTarget::Decl { file },
                ),
            },
        }
    }

    /// The [`BareInfo`] of a resolved top-level declaration.
    pub(crate) fn bare_info_of(&self, name: StringId, decl: &TopDecl) -> BareInfo {
        match decl.kind {
            // Type parameters are declaration-scoped: the per-unit tag is
            // stale outside its declaration, and in-scope uses were answered
            // by the engine's type-parameter stack. The stale reference is a
            // plain miss whose code name follows the live invocation context
            // (`undefined_method`'s host inside a class instance member).
            MicroKind::TypeParam => self.missing_info(name),
            MicroKind::Class => BareInfo::Container {
                name,
                kind: ContainerKind::Class,
            },
            MicroKind::Enum => BareInfo::Container {
                name,
                kind: ContainerKind::Enum,
            },
            MicroKind::Mixin => BareInfo::Container {
                name,
                kind: ContainerKind::Mixin,
            },
            MicroKind::Extension => BareInfo::Container {
                name,
                kind: ContainerKind::Extension,
            },
            // A `TypeAlias` falls here (the container kinds answer above):
            // reachable only from a value-position alias reference — invalid
            // Dart, recovery territory — it answers the captured right-hand
            // type, so the undefined-name row stays suppressed and the
            // oracle's invalid-use diagnostic stays under-reported. Type
            // positions never walk this face (the type parser arms them).
            _ => BareInfo::Plain { ty: decl.var_ty },
        }
    }

    /// Whether the token immediately before `utf16` is the `new` or `const`
    /// keyword: the creation-head test for the bare-miss row shaping (the
    /// preceding token of a creation head is always the creation keyword —
    /// comments never intervene, the scanner drops them). The table is
    /// offset-ordered, so the predecessor is one partition point.
    fn preceded_by_creation_keyword(&self, utf16: u32) -> bool {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < utf16);
        let Some(token) = at.checked_sub(1).and_then(|i| tokens.get(i)) else {
            return false;
        };
        matches!(
            token.kind,
            dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::New)
                | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Const)
        )
    }

    /// Whether `name` resolves to the SDK projection's names-only stub of a
    /// dart:core collection head — the resolution grade a generic class
    /// type literal takes in a value position (`List` of `List < 1` names
    /// the class through an unindexed `<dart:core>` file, landing in the
    /// untyped `Plain` shape). A user declaration of the same name resolves
    /// first and keeps its own face.
    pub(crate) fn sdk_collection_type_literal(&self, name: StringId) -> bool {
        if !self.collection_heads.contains(&name) {
            return false;
        }
        match self.resolve_bare(name) {
            (BareInfo::Plain { .. }, RefTarget::Decl { file }) => self.index.get(file).is_none(),
            _ => false,
        }
    }

    // —— row stack ———————————————————————————————————————————————————

    pub(crate) fn push_row(&mut self, row: Row) {
        // The conditional-URI window: the configuration is never resolved
        // as an expression, so its rows drop at the resolution pass.
        if self.conditional_uri_window {
            self.conditional_uri_rows.insert(row.range.offset);
        }
        self.rows.push(row);
    }

    /// A bare identifier in value position: resolved immediately (the
    /// verdict is the snapshot pass 2 emits from). A miss under `new`/`const`
    /// is a creation head: it names a class, so the guarded miss is the
    /// oracle's `creation_with_non_type` head face (`new Missing()`),
    /// never an invocation read — the row reclassifies as a type use.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn push_bare(&mut self, name: StringId, range: SourceRange) {
        let (info, target) = self.resolve_bare(name);
        // The query face's publication (#1056): the push-time decision
        // ships as-is — the resolution is final even where a later fold
        // retracts the row (a creation fold's truncated callee resolved
        // where it resolved).
        self.refs.push(UnitRef {
            range,
            name,
            target,
        });
        // The usage cluster's local-use capture: a bare read that resolves
        // into the frame stack counts for the declaration it lands on.
        // (A local function's own name is absent from the tracked stack
        // while its body parses, so in-body self-references never count —
        // the official `_enclosingExec` exclusion.) The final intent may
        // still cancel it (write-only, statement-discarded reads).
        if let Some(hit) = self.resolve_local(name) {
            let hit = hit as usize;
            self.locals[hit].uses += 1;
            self.local_decl_at_read.insert(range.offset, self.locals[hit].range.offset);
            self.local_hits.insert(range.offset, hit as u32);
        }
        // The official self-reference exclusions (`_useIdentifierElement`):
        // a reference to the enclosing container or the enclosing executable
        // itself never marks that declaration used. The row offset joins the
        // skip set so the resolve-time marking obeys the same rule.
        if self.exec_self_name() == Some(name)
            || self.container_context().is_some_and(|(c, _)| c == name)
        {
            self.self_scope_rows.insert(range.offset);
        }
        // A bare mention of a field name inside its own container: the
        // final intent decides read vs write at resolve time. The member
        // scope reaches past the container's own map — an inherited field
        // reads through `this` identically (#2316 recipe-a): the defining
        // container (superclass, mixin application, or interface face —
        // `member_lookup_defining`'s climb) records the read so
        // `member_was_read` spares `unused_field` on the declaring member.
        // Statics mark the same: a static member of the enclosing (or
        // inherited) container is in bare-name scope inside its body, and
        // the official read credit (`_isReadMember`'s readMembers join)
        // precedes its static early-return — that return only gates the
        // override face (#2335). An extension body's bare read of an
        // `on`-type INSTANCE field defines on the extended container the
        // same way (#2282 — `this` inside an extension IS the `on` type;
        // the climb above keys on the extension's own map only): the
        // `on` type's statics stay out — no implicit receiver carries
        // them (bare static access needs qualification, the extension's
        // lexical scope is the library), so a static hit there is not an
        // honest read.
        let mut field_declaring = None;
        if let Some((container, kind)) = self.container_context() {
            if let Some((found, entry)) = member_lookup_defining(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                container,
                name,
                false,
            ) {
                if entry.kind == MemberKind::Field {
                    field_declaring = Some(found);
                }
            } else if kind == ContainerKind::Extension
                && let Some(on) = self.enclosing_extension_on_type(container)
                && let Walk::Found(entry) = self.walk_members(on, name)
                && entry.kind == MemberKind::Field
                && !entry.is_static
            {
                field_declaring = Some(on);
            }
        }
        if let Some(declaring) = field_declaring {
            self.field_read_rows.insert(range.offset, declaring);
        }
        // The implicit-this face's push-time snapshots: the field-group
        // state tears down before pass 2 resolves the row. The second set
        // carries the member-ness verdict (an instance member of the
        // enclosing container only — statics and non-members never ride
        // `this`).
        if self.in_instance_field_initializer() {
            let instance_member = match &target {
                RefTarget::Member { container } => match self.walk_members(*container, name) {
                    Walk::Found(entry) => !entry.is_static,
                    // A chain-broken climb answers no: the name may be an
                    // inherited member (suppress) or a scoped declaration
                    // the broken chain preempted (`HashSet<T>()` in a field
                    // initializer of a class `implements Iterator<T>` — the
                    // unindexed hop claimed the scoped class as an instance
                    // member and over-reported the initializer face,
                    // #2172). The Object-member arm below keeps the
                    // always-known faces reporting.
                    Walk::ChainBroken => false,
                    Walk::Missing => false,
                },
                _ => {
                    self.container_context().is_some_and(|(_, kind)| {
                        kind == ContainerKind::Enum
                            && self.members.enum_synth_members.contains(&name)
                    }) || self.members.object_members.contains(&name)
                }
            };
            if instance_member {
                self.field_init_this_rows.insert(range.offset);
            }
        }
        if self.ctor_pos > 0 && matches!(info, BareInfo::Missing { .. }) {
            // Only a `new`/`const` creation HEAD is a type position: the
            // oracle names a miss at the head `creation_with_non_type`
            // while a miss in the creation's arguments is an ordinary
            // expression (`undefined_identifier` — the
            // `List<String>.from(miss.keys)` argument face). `ctor_pos`
            // arms for whole creations, so the head is discriminated by
            // its preceding token: the creation keyword itself. A nested
            // creation head (`new O(new I(x))`) reads the same way; the
            // implicit-creation form (`Foo<T>.sel(...)`) arms late — its
            // head is pushed before the arm engages — so only keyword
            // heads ever reach here.
            if self.preceded_by_creation_keyword(range.offset) {
                self.push_row(Row {
                    name,
                    range,
                    qualified: false,
                    shape: RowShape::TypeUse(TypeVerdict::Missing, false),
                    intent: Intent::Read,
                    write_op: None,
                    ty: Ty::UNKNOWN,
                });
                return;
            }
        }
        // F-C: a bare name inside the constructor-initializer list joins
        // the initializer world — the write intent is the field-initializer
        // LHS (`x = e`) and resolves through that face at pass 2, never
        // `undefined_identifier`. Reads keep the ordinary resolution.
        if self.initializer_depth > 0
            && let Some((container, _)) = self.container_context()
        {
            self.init_rows.insert(range.offset, container);
        }
        let ty = match info {
            BareInfo::Plain { ty } if ty != Ty::UNKNOWN => ty,
            // #279 type folding: an in-file const top-level variable's
            // declared type folds to its evaluated constant's type (the
            // oracle infers the variable's type from the initializer). A
            // shadowing local keeps its own (already-folded) type.
            BareInfo::Plain { .. } => {
                let from_local = self.frames.iter().any(|f| f.locals.contains(&name));
                let is_top_var = self
                    .top_level_decl(name)
                    .is_some_and(|d| d.kind == MicroKind::TopLevelVar);
                if !from_local && is_top_var {
                    self.const_top_level_ty(name).unwrap_or(Ty::UNKNOWN)
                } else {
                    Ty::UNKNOWN
                }
            }
            _ => Ty::UNKNOWN,
        };
        // The implicit-`this` member read's query row (#1869-2): a bare
        // name the enclosing container's instance face resolved carries the
        // member's declared type — the same static type `this.name` reads.
        // The flow layer cannot type these reads (its rows are local
        // bindings), so the merge at the teardown join never collides.
        if matches!(target, RefTarget::Member { .. }) && ty != Ty::UNKNOWN {
            self.query_type_rows.push((range, ty));
        }
        self.push_row(Row {
            name,
            range,
            qualified: false,
            shape: RowShape::Bare(info),
            intent: Intent::Read,
            write_op: None,
            ty,
        });
    }

    /// An identifier after a dot, awaiting the merge at the dot event.
    pub(crate) fn push_member(&mut self, name: StringId, range: SourceRange) {
        // A record-typed receiver's field access is resolvable without the
        // member face: the record shape the receiver's row carries names
        // the positional `$n` and named slots (`rect.width` on
        // `({int width, int height})`). The row keeps the resolved field
        // type so the binary fold and the initializer faces read the
        // access's own type (record fields carry no member walk — the
        // `Receiver` is `Unknown`, so pass-2 stays silent on it either way).
        let recv_ty = self.rows.last().map(|recv| {
            if recv.ty != Ty::UNKNOWN {
                recv.ty
            } else if let RowShape::Bare(BareInfo::Plain { ty }) = &recv.shape {
                *ty
            } else {
                Ty::UNKNOWN
            }
        });
        let ty = match recv_ty {
            Some(Ty::Record { shape, .. }) => {
                let pool = self.boundary.pool();
                let shape = pool.record_shape(shape);
                let text = self.strings.resolve(name);
                let field_ty = if let Some(index) = text
                    .strip_prefix('$')
                    .and_then(|digits| digits.parse::<usize>().ok())
                    .and_then(|index| index.checked_sub(1))
                {
                    shape.positional.get(index).copied()
                } else {
                    shape
                        .named
                        .iter()
                        .find(|(field, _)| *field == name)
                        .map(|(_, ty)| *ty)
                };
                field_ty.unwrap_or(Ty::UNKNOWN)
            }
            _ => Ty::UNKNOWN,
        };
        self.push_row(Row {
            name,
            range,
            qualified: true,
            shape: RowShape::Bare(BareInfo::Plain { ty }),
            intent: Intent::Read,
            write_op: None,
            ty,
        });
    }

    /// A resolved non-receiver value (literal, parenthesized group, closure,
    /// collection): member merges past it see an unknown receiver. The row
    /// carries the construct's span so the assignment/operator scans can
    /// tell "before the operator" (candidate) from "after" (skip).
    pub(crate) fn push_value_row(&mut self, token: Token) {
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        });
    }

    /// The `null` literal: the only literal whose type M1 knows exactly.
    pub(crate) fn push_null_row(&mut self, token: Token) {
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::Null,
        });
    }

    /// The recovered `is` expression's result row (`<synthetic> is T`, the
    /// operand-splice recovery face): the expression's static type is `bool`
    /// whatever the operand's recovery shape (`visitIsExpression`), so the
    /// enclosing single-row check reads the expression's own type instead of
    /// the recovery residue below it.
    pub(crate) fn push_bool_row(&mut self, token: Token) {
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: Ty::named(self.members.primitive_ids[3]),
        });
    }

    /// A record literal's result row: the field types decided by
    /// [`Engine::record_field_tys`] ride the row — the positional list in
    /// `shape` (the render spells the oracle's record type display,
    /// `(int, {String s})`), the full shape interned into `ty` for the
    /// assignability face.
    pub(crate) fn push_record_row(
        &mut self,
        token: Token,
        positional: Box<[Ty]>,
        named: Box<[(StringId, Ty)]>,
    ) {
        let ty = self
            .boundary
            .args
            .borrow_mut()
            .record(&positional, &named, false);
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Record(positional),
            intent: Intent::Read,
            write_op: None,
            ty,
        });
    }

    /// The record literal's field types, from the field window its begin
    /// event marked: the window's top-level commas split the fields
    /// (`(a, b, c)` — a comma inside a nested group does not split), and
    /// each field's type is the last row strictly inside its token span.
    /// An empty field (`(42, ,)`) carries [`Ty::Unknown`] — the recovery's
    /// `InvalidType` splice, the under-report direction. The named fields
    /// `names` are the trailing [`Self::record_literal_names`] entries of
    /// the split. A field whose window rows disagree on one known type
    /// stays undecided (`None`, the literal keeps its opaque value row).
    /// The stacks pop either way — the literal's own `(` window never
    /// leaks into the enclosing expression.
    pub(crate) fn record_field_tys(
        &mut self,
        count: usize,
        names: Vec<StringId>,
    ) -> Option<(usize, Box<[Ty]>, Box<[(StringId, Ty)]>)> {
        let (mark, open) = self.record_marks.pop()?;
        if count == 0 || mark > self.rows.len() {
            return None;
        }
        let rows = &self.rows[mark..];
        // The field spans: the window's `(` through the literal's `)`
        // (the open token's matching close — never a comma), split on the
        // depth-0 commas between them.
        let close = self.tokens.matching_end(open.id as usize)?;
        let mut boundaries = vec![open.id];
        let mut depth = 0u32;
        for id in open.id + 1..close as u32 {
            let kind = self.tokens.get(id as usize).map(|t| t.kind);
            match kind {
                Some(kind) if kind.is_begin_group() => depth += 1,
                Some(kind) if kind.matching_opens().first().is_some() => {
                    depth = depth.saturating_sub(1);
                }
                Some(dartq_scanner::TokenKind::Comma) if depth == 0 => boundaries.push(id),
                _ => {}
            }
        }
        if boundaries.len() == count + 1 {
            // A trailing comma's empty tail is not a field — drop it.
            boundaries.pop();
        }
        if boundaries.len() != count {
            // The written count and the split disagree — a recovery shape
            // the span walk cannot trust; the opaque row stands.
            return None;
        }
        let mut fields = Vec::with_capacity(count);
        for window in boundaries
            .windows(2)
            .map(|w| (w[0], w[1]))
            .chain(std::iter::once((*boundaries.last()?, close as u32)))
        {
            let (lo, hi) = (
                self.tokens
                    .get(window.0 as usize)
                    .map(|t| t.utf16_offset)
                    .unwrap_or(0),
                self.tokens
                    .get(window.1 as usize)
                    .map(|t| t.utf16_offset)
                    .unwrap_or(u32::MAX),
            );
            let field_rows: Vec<&Row> = rows
                .iter()
                .filter(|row| row.range.offset > lo && row.range.end() <= hi)
                .collect();
            match field_rows.as_slice() {
                [] => fields.push(Ty::Unknown),
                [single] => fields.push(single.ty),
                many => {
                    let first = many[0].ty;
                    if first == Ty::UNKNOWN || !many.iter().all(|row| row.ty == first) {
                        return None;
                    }
                    fields.push(first);
                }
            }
        }
        // The named pairs trail the positional list (the parser's own
        // field order — `(1, s: 'x', b: true)` keeps `s` before `b`).
        let named_start = fields.len().saturating_sub(names.len());
        let named: Vec<(StringId, Ty)> = names
            .into_iter()
            .zip(fields[named_start..].iter().copied())
            .collect();
        Some((mark, fields[..named_start].into(), named.into_boxed_slice()))
    }

    /// The record type's official display (`(int,)`, `(int, String)`): the
    /// positional fields comma-joined, a lone field keeping its trailing
    /// comma (`RecordType`'s `toString`, records.dart's written form).
    pub(crate) fn render_record(&self, fields: &[Ty]) -> String {
        let mut text = String::from("(");
        for (index, ty) in fields.iter().enumerate() {
            if index > 0 {
                text.push_str(", ");
            }
            text.push_str(&ty.render(&self.strings, &self.boundary.pool()));
        }
        if fields.len() == 1 {
            text.push(',');
        }
        text.push(')');
        text
    }

    /// The operator application on a record-typed receiver (`(x-1) + 1`): a
    /// record declares no user operators, so each applied spelling that
    /// `Object` does not define reports `undefined_operator` at the
    /// operator's own span (`pkg/analyzer/lib/src/dart/ast/ast.dart`
    /// `RecordLiteralImpl.staticType`; the type display rides the message).
    /// An extension application on a record shape is an inference face M1
    /// does not model — a program that declares the operator through
    /// `extension on (…)` over-reports here (a documented divergence; the
    /// corpus carries no such declaration).
    pub(crate) fn resolve_record_row(&mut self, row: &Row, fields: &[Ty]) {
        let Intent::Operator(_, op_range) = row.intent else {
            return;
        };
        let (read, write) = self.operator_spellings(row);
        let type_text = self.render_record(fields);
        for spelling in [read, write].into_iter().flatten() {
            if spelling == self.intern("==") {
                continue;
            }
            let op_text = self.strings.resolve(spelling).to_string();
            self.emit(
                SemanticCode::UndefinedOperator,
                op_range,
                &op_text,
                &type_text,
            );
        }
    }

    /// The recovered operand's fold (`return is(x-1)` parses as
    /// `<synthetic> is (x-1)`): the synthetic head fired no identifier
    /// event, so no operand row exists, and the suppressed `TypeUse` rows
    /// the recovered type left would stand in as the expression's row — an
    /// `unknown` that silences the enclosing single-row check. The oracle
    /// still types the expression `bool` and runs the return matrix on it
    /// (`return_of_invalid_type` bool->int,
    /// keyword_named_class_methods_2.dart:184).
    ///
    /// The dropped residue keeps its push-time faces: a suppressed miss
    /// still feeds the names table exactly as its pass-2 resolve would (the
    /// emit is suppression-gated off), so only the row leaves. A window that
    /// holds anything but suppressed misses (an unresolved prefixed head
    /// keeps its own emission) folds nothing — the under-report posture
    /// stands for the exotic recovery shape.
    pub(crate) fn fold_recovered_is(&mut self, at: Token) {
        let Some(mark) = self.is_type_mark.take() else {
            return;
        };
        if mark > self.rows.len() {
            return;
        }
        let residue: Vec<Row> = self.rows.drain(mark..).collect();
        let mut misses = Vec::with_capacity(residue.len());
        let mut retained: Vec<Row> = Vec::new();
        let mut decidable = true;
        for row in &residue {
            match row.shape {
                // The recovery shape's undefined type names: the miss joins
                // the face's own report set.
                RowShape::TypeUse(TypeVerdict::Missing, true) => {
                    misses.push((row.name, row.range));
                }
                // A resolved non-type inside the recovered operand (the
                // golden `x isn't a type` on the recursion parameter): the
                // row stays for the resolve pass, and the recovered
                // expression itself is still statically `bool`.
                RowShape::TypeUse(TypeVerdict::NonType, true) => {
                    retained.push(row.clone());
                }
                _ => {
                    decidable = false;
                    break;
                }
            }
        }
        if decidable {
            for (name, range) in misses {
                self.record_miss(name, range, false, NameUse::Type);
            }
            self.rows.extend(retained);
            self.push_bool_row(at);
        } else {
            self.rows.extend(residue);
        }
    }

    /// The `is`/`as` operand's undefined-type face
    /// (`visitIsExpression`'s `type_test_with_undefined_name`,
    /// `visitAsExpression`'s `cast_to_non_type`): the operand window's
    /// suppressed `Missing` row reports the dedicated code — the resolver's
    /// plain path never fires for the operand (the non-type-pos
    /// suppression holds), and the resolve pass keeps its miss record.
    ///
    /// #2193: the split is by syntactic role — a miss inside the target's
    /// type-argument list (`x is List<Missing>`) reports the name-level
    /// `non_type_as_type_argument` (the oracle walks the target's argument
    /// tree through the plain context chain), while a bare operand head
    /// (`x is Missing`) keeps the operand code. The arg-face rows emit
    /// here because the window's suppression gates them out of the
    /// resolve pass's `Missing` arm; their offsets stay in
    /// `type_arg_miss_rows` — the resolve-pass gate makes the leftovers
    /// unreachable.
    pub(crate) fn emit_operand_type_miss(&mut self, mark: Option<usize>, code: SemanticCode) {
        let Some(start) = mark else {
            return;
        };
        if start >= self.rows.len() {
            return;
        }
        let mut head_hit: Option<(StringId, SourceRange)> = None;
        let mut arg_hits: Vec<(StringId, SourceRange)> = Vec::new();
        for row in &self.rows[start..] {
            if let RowShape::TypeUse(TypeVerdict::Missing, true) = row.shape {
                if self.type_arg_miss_rows.contains(&row.range.offset) {
                    arg_hits.push((row.name, row.range));
                } else if head_hit.is_none() {
                    head_hit = Some((row.name, row.range));
                }
            }
        }
        for (name, range) in arg_hits {
            if !self.guard.ignore(None, name) {
                let name_text = self.strings.resolve(name).to_string();
                self.emit_corrected(SemanticCode::NonTypeAsTypeArgument, range, &name_text);
            }
        }
        if let Some((name, range)) = head_hit {
            if !self.guard.ignore(None, name) {
                let name_text = self.strings.resolve(name).to_string();
                self.emit(code, range, &name_text, "");
            }
        }
    }

    pub(crate) fn emit_is_type_miss(&mut self) {
        let mark = self.is_type_mark.take();
        self.emit_operand_type_miss(mark, SemanticCode::TypeTestWithUndefinedName);
    }

    pub(crate) fn emit_as_type_miss(&mut self) {
        let mark = self.as_type_mark.take();
        self.emit_operand_type_miss(mark, SemanticCode::CastToNonType);
    }

    /// `this`/`super` in expression position: a typed receiver when a
    /// container is in scope, unknown otherwise (top-level recovery). The
    /// declaration-context faces ride the push: an invalid context reports
    /// `invalid_reference_to_this` / `super_in_invalid_context` at the
    /// keyword (type_analyzer.dart's `isThisAccessible` /
    /// element_resolver.dart:360-369), before the row's uses decide.
    /// Exception: at an initializer item's head the keyword is a bare token
    /// of the folded initializer node (`ConstructorFieldInitializer`,
    /// `RedirectingConstructorInvocation`, `SuperConstructorInvocation` —
    /// upstream `buildInitializer`), never an expression reference, so the
    /// context verdict does not apply to it.
    pub(crate) fn push_keyword_receiver(&mut self, token: Token, is_super: bool) {
        let item_head =
            self.initializer_depth > 0 && self.init_item_head == Some(token.range.offset);
        if !item_head {
            match self.keyword_receiver_state(is_super) {
                ReceiverState::Invalid => {
                    let code = if is_super {
                        SemanticCode::SuperInInvalidContext
                    } else {
                        SemanticCode::InvalidReferenceToThis
                    };
                    self.emit(code, token.range, "", "");
                }
                // The extension/extension-type `super` faces live outside the
                // code set; the extension instance `this` is legal.
                ReceiverState::Skip | ReceiverState::Valid => {}
            }
        } else if is_super {
            // A `super` keyword heading an initializer item folds to a
            // `SuperConstructorInvocation` when its tail invokes (`super(...)`
            // or `super.name(...)`) — the explicit-super satisfaction the
            // `implicit_super_initializer_missing_arguments` face reads
            // (upstream `error_verifier.dart`'s
            // `_checkForUndefinedConstructorInInitializerImplicit` early
            // return on `SuperConstructorInvocation`). A `super.name = v`
            // item is a `ConstructorFieldInitializer` instead — no
            // satisfaction — so the invocation tail is read off the token
            // table: `(` decides immediately; a `.` selector decides on the
            // `(` after its name. Comments and whitespace never intervene —
            // the scanner drops them.
            let tokens = self.tokens.as_slice();
            let at = tokens.partition_point(|t| t.utf16_offset < token.range.end());
            let satisfied = match tokens.get(at).map(|t| &t.kind) {
                Some(dartq_scanner::TokenKind::OpenParen) => true,
                Some(dartq_scanner::TokenKind::Period) => matches!(
                    tokens.get(at + 2).map(|t| &t.kind),
                    Some(dartq_scanner::TokenKind::OpenParen)
                ),
                _ => false,
            };
            if satisfied && let Some(site) = self.open_ctor_site {
                self.ctor_super_init.insert(site);
            }
        } else {
            // `this` as the bare initializer TARGET — `this = 0` parses its
            // `this` as the field-initializer's head but no `.field` follows.
            // Upstream `buildInitializer` (ast_builder.dart:634-756) folds
            // `this = expr` as a `ConstructorFieldInitializer` whose target
            // is a bare `ThisExpression` — the verifier reports
            // `invalid_reference_to_this` at the keyword
            // (element_resolver.dart:357-374's `isThisAccessible` is true,
            // so the `invalidReferenceToThis` fallthrough reports), and the
            // empty field name then walks `initializer_for_non_existent_field`
            // at `''`. Peek past the keyword: `= ` ends the target, `.`
            // opens a selector, `(` opens a redirect invocation — only the
            // bare `=` shape is the misfire.
            let next = self.skip_whitespace(token.range.end());
            if self.byte_at(next) == Some(b'=') {
                self.emit(SemanticCode::InvalidReferenceToThis, token.range, "", "");
                // The empty-name field-initializer face — oracle's
                // `_checkForInvalidField` walks `''` against the field set,
                // misses, and reports `initializer_for_non_existent_field`
                // at the same span.
                self.emit(
                    SemanticCode::InitializerForNonExistentField,
                    token.range,
                    "",
                    "",
                );
            }
        }
        let recv = match self.container_context() {
            Some((container, kind)) => {
                if is_super {
                    Receiver::Super { container, kind }
                } else {
                    // The query face's `this` publication (#1869-2): the
                    // reference row binds the keyword to the enclosing
                    // container and the read-type row answers `type_of`
                    // with its type ([`Self::this_static_ty`] — the same
                    // answer the null-aware receiver classification
                    // reads). Both rows join their outlets' offset sorts,
                    // so the publication is order-free.
                    self.refs.push(UnitRef {
                        range: token.range,
                        name: self.intern("this"),
                        target: RefTarget::This,
                    });
                    // The extension face publishes the `on` type (an
                    // `extension E on B?` makes `this : B?`) — the same
                    // answer the null-aware receiver classification reads.
                    let this_ty = self.this_static_ty(container, kind);
                    self.query_type_rows.push((token.range, this_ty));
                    Receiver::This { container, kind }
                }
            }
            None => Receiver::Unknown,
        };
        self.push_row(Row {
            name: self.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Receiver(recv),
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        });
    }

    /// A type-annotation position. Defined types push no row; the names
    /// table carries misses only. The declaration binding arms at
    /// `handle_type` now — this only classifies the reference.
    pub(crate) fn push_type_use(&mut self, name: StringId, range: SourceRange, head: TypeHead) {
        // The query face's publication (#1056): every bare type-reference
        // position ships `(span, name)` — a creation head's container
        // resolution keys on it.
        self.type_uses.push((range, name));
        let suppressed = self.non_type_pos > 0;
        match head {
            TypeHead::Bare => {
                if self.members.builtin_types.contains(&name) {
                    return;
                }
                // #1595/#1596: the plain resolver path's three answers
                // (named_type_resolver.dart:190-330 + reportNullOrNonTypeElement
                // :730-810). A creation head never reaches the plain path —
                // the constructor-reference dispatch answers it (a resolving
                // non-type head is `creation_with_non_type`, a miss
                // `undefined_class` via the context rows below) — and a
                // redirecting-factory target keeps its dedicated silent
                // verdict.
                if self.redirect_target_depth == 0 {
                    match self.type_resolution(name) {
                        (TypeResolution::NonType, anchor) if self.ctor_pos == 0 => {
                            // A resolving name credits its import in every
                            // position — `ImportsTracking` counts the scope
                            // lookup itself (`is`/`as` operand, self-scope
                            // reference), while `used_names` keeps the
                            // `unused_element` face's narrower exclusions.
                            self.import_used.insert(name);
                            // The use-count face rides the same guards as a
                            // defined type (`_useIdentifierElement` — inside
                            // an `is` operand and self-references excepted).
                            if self.is_type_pos == 0 && !self.in_own_named_scope(name) {
                                self.used_names.insert(name);
                            }
                            // A type-argument position replaces the plain
                            // face (`non_type_as_type_argument`, the same
                            // context chain the miss rows ride).
                            if self.type_arg_depth > 0 {
                                self.type_arg_miss_rows.push(range.offset);
                            }
                            // The plain face's declaration anchor rides the
                            // row for the resolve pass's context row.
                            if let Some(anchor) = anchor {
                                self.non_type_anchors.push((range.offset, anchor));
                            }
                            self.push_row(Row {
                                name,
                                range,
                                qualified: false,
                                shape: RowShape::TypeUse(TypeVerdict::NonType, suppressed),
                                intent: Intent::Read,
                                write_op: None,
                                ty: Ty::UNKNOWN,
                            });
                            return;
                        }
                        (TypeResolution::Type, _) => {
                            // The import credit is unconditional (see the
                            // NonType arm): the scope lookup succeeded, so
                            // the providing import is used.
                            self.import_used.insert(name);
                            // A defined type reference uses the type (the
                            // official `visitNamedType` →
                            // `_useIdentifierElement` face) — except inside
                            // an `is` type operand, where the oracle
                            // deliberately does not count it
                            // (`_useIdentifierElement`'s
                            // `_insideIsExpression` early return), and
                            // except a self-reference from inside the
                            // declaration itself (`_enclosingClass` /
                            // `_enclosingExec`). A creation head resolves
                            // through the same lookup — a defined class is
                            // silent here (the constructor-reference
                            // dispatch owns its remaining faces).
                            if self.is_type_pos == 0 && !self.in_own_named_scope(name) {
                                self.used_names.insert(name);
                            }
                            return;
                        }
                        // A creation head that resolved to an import prefix
                        // (`fb.RuleState.stable()`): the constructor-reference
                        // dispatch's importPrefix arm owns it — the head is a
                        // resolved prefix (its use counts, no row reports on
                        // its span), and the class segment's prefixed lookup
                        // decides at `end_constructor_reference`
                        // (named_type_resolver.dart:140-147 + the tail's
                        // `creation_with_non_type` at `node.name`). Every
                        // other resolving non-type keeps the miss machinery
                        // below (`creation_with_non_type` at the head).
                        (TypeResolution::NonType, _)
                            if self.ctor_pos > 0
                                && matches!(
                                    lookup_scoped(
                                        self.graph,
                                        self.namespaces,
                                        self.index,
                                        self.lib,
                                        self.file,
                                        name,
                                    ),
                                    ScopedHit::Found(NamespaceEntry::Prefix)
                                ) =>
                        {
                            self.import_used.insert(name);
                            self.used_names.insert(name);
                            self.ctor_prefix_heads.push(range.offset);
                            return;
                        }
                        _ => {}
                    }
                }
                // #1314 FAM13: the resolver's context chain decides the
                // miss's code — a type-argument reference reports
                // `non_type_as_type_argument`, a creation head
                // `creation_with_non_type`, everything else keeps
                // `undefined_class` (named_type_resolver.dart:720-770; the
                // offset joins the face's row for the resolve pass).
                // A redirecting-factory target rides the dedicated silent
                // verdict ALONE — the oracle's `redirect_to_non_class` face
                // sits outside the frozen gate, and the plain-path `Missing`
                // row would score `undefined_class` beside it (the
                // resolver's plain path never runs: the constructor-reference
                // dispatch answers the target head).
                if self.redirect_target_depth > 0 {
                    self.push_row(Row {
                        name,
                        range,
                        qualified: false,
                        shape: RowShape::TypeUse(TypeVerdict::RedirectTarget, suppressed),
                        intent: Intent::Read,
                        write_op: None,
                        ty: Ty::UNKNOWN,
                    });
                } else {
                    // A type-argument position codes through the context
                    // chain whatever its head resolved to: the oracle walks
                    // an unresolvable NamedType's arguments too (probed:
                    // `W<L>` reports `undefined_class` on `W` AND
                    // `non_type_as_type_argument` on `L`, in declarations,
                    // creations, and typedefs alike).
                    if self.type_arg_depth > 0 {
                        self.type_arg_miss_rows.push(range.offset);
                    } else if self.ctor_pos > 0 && self.type_arg_depth == 0 {
                        let explicit = self.implicit_creation_depth == 0;
                        self.creation_miss_rows.push((range.offset, explicit));
                    }
                    self.push_row(Row {
                        name,
                        range,
                        qualified: false,
                        shape: RowShape::TypeUse(TypeVerdict::Missing, suppressed),
                        intent: Intent::Read,
                        write_op: None,
                        ty: Ty::UNKNOWN,
                    });
                }
            }
            TypeHead::Continuation => {
                let Some(tp) = self.type_prefix.as_ref() else {
                    return;
                };
                if !tp.live {
                    return; // the head already reports; the name is dead
                }
                let prefix = tp.prefix;
                let hit = self.prefixed_hit(prefix, name);
                if !hit {
                    // The local-prefix face snapshots here: a head naming a
                    // local or parameter element is not an import prefix, and
                    // the frame stack answering that question exists only at
                    // push time.
                    let local = self.frames.iter().rev().any(|f| f.locals.contains(&prefix));
                    self.push_row(Row {
                        name,
                        range,
                        qualified: true,
                        shape: RowShape::TypeUse(TypeVerdict::Prefixed { prefix, local }, false),
                        intent: Intent::Read,
                        write_op: None,
                        ty: Ty::UNKNOWN,
                    });
                } else {
                    // The prefixed type resolved: the prefix scope lookup
                    // credits the import (`ImportsTracking` — the `is`
                    // exclusion is the used-elements visitor's, not the
                    // tracker's).
                    self.import_used.insert(name);
                    if self.is_type_pos == 0 && !self.in_own_named_scope(name) {
                        self.used_names.insert(name);
                    }
                }
            }
        }
    }

    /// The type-position lookup's full answer for a bare type head — the
    /// official chain (`named_type_resolver.dart:129-131` `_lookupGetter
    /// (nameScope, …)` over the member's scope), keeping the element kind
    /// (`_lookupGetter` + `_instantiateElement`'s non-interface fall,
    /// named_type_resolver.dart:197-330, and the
    /// `reportNullOrNonTypeElement` tail :780-806): a resolving non-type
    /// element reports `not_a_type` unless it is a local variable or local
    /// function (the `referenced_before_declaration` face, outside this
    /// engine), a multiply-defined element stays silent
    /// (`InvalidType` with no report), and a miss keeps the existing
    /// `undefined_class` machinery. The container step is the face the
    /// expression lookup lacks: `InstanceScope.lookup`
    /// (scope.dart:355-372) — an instance member with the basename yields
    /// NOTHING, so the enclosing scopes (the library and its imports) are
    /// never consulted and the reference reports `undefined_class` even when
    /// a top-level type of the same name exists; a static member returns
    /// itself (the resolver's `notAType` face — defined here, no row).
    /// Unlike [`Engine::resolve_bare`] — the expression face — a container
    /// member is never a satisfying type answer, so a recovery-created
    /// field named `augment` leaves every `augment` annotation undefined
    /// (#1445: the gated-off augmentation fixtures). The answer pairs the
    /// resolution with the `not_a_type` context anchor — the resolved
    /// element's declaration (file, name span) when the face fires and the
    /// span is carried; `None` keeps the plain row.
    pub(crate) fn type_resolution(
        &self,
        name: StringId,
    ) -> (TypeResolution, Option<(FileId, SourceRange)>) {
        if self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
            || self
                .fn_type_capture
                .iter()
                .any(|capture| capture.scope.contains(&name))
        {
            return (TypeResolution::Type, None);
        }
        // A formal parameter in type position reports (`FormalParameterElement`
        // is not a local variable — the golden face `x isn't a type` on the
        // method's own parameter); a local variable or catch parameter
        // routes to the referenced-before face and stays silent.
        if let Some(index) = self.resolve_local(name) {
            let decl = &self.locals[index as usize];
            return match decl.kind {
                LocalKind::Parameter { .. } => {
                    (TypeResolution::NonType, Some((self.file, decl.range)))
                }
                _ => (TypeResolution::Type, None),
            };
        }
        if let Some((container, _)) = self.container_context()
            && let Some(entry) = self
                .index
                .get(self.file)
                .and_then(|f| f.container(container))
                .and_then(|c| c.members.get(&name))
        {
            return if entry.is_static {
                // The static member's own site: the declaration capture
                // keeps the member's name span (the context row's anchor).
                (
                    TypeResolution::NonType,
                    self.decl_sites
                        .iter()
                        .find(|site| {
                            site.name == name
                                && match &site.kind {
                                    DeclKind::Member {
                                        container: owner, ..
                                    } => *owner == container,
                                    _ => false,
                                }
                        })
                        .map(|site| (self.file, site.range)),
                )
            } else {
                (TypeResolution::Missing, None)
            };
        }
        // The getter namespace answers before the type table
        // (`_lookupGetter(nameScope, …)` runs first): an extension's name
        // lives there as a sole entry — class/mixin/typedef declarations
        // sharing the name sit in the type namespace only (the corpus's
        // `class Function {}` + `extension Function on List {}` shape) — so
        // a type position resolving to an extension's name finds a
        // non-type element and reports `not_a_type`. Every closer scope
        // (type parameters, locals, the container's own members) already
        // answered above; the built-in type names sit behind it (they are
        // the no-declaration fallback).
        if self
            .index
            .get(self.file)
            .is_some_and(|f| f.extension_faces.iter().any(|face| face.name == Some(name)))
        {
            return (TypeResolution::NonType, None);
        }
        if self.members.builtin_types.contains(&name) {
            return (TypeResolution::Type, None);
        }
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::NotFound => (TypeResolution::Missing, None),
            ScopedHit::Local(decl) => self.scoped_resolution(name, self.file, decl.kind),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                match self.index.get(file).and_then(|f| f.decl(name)) {
                    Some(decl) => self.scoped_resolution(name, file, decl.kind),
                    None => (TypeResolution::Type, None),
                }
            }
            // A bare import prefix in type position resolves to the prefix
            // element — a non-type (`PrefixElement` falls through the
            // resolver's interface/alias checks to `not_a_type`). The
            // prefix's declaration span is not captured (the import site
            // keeps the URI span) — the plain row.
            ScopedHit::Found(NamespaceEntry::Prefix) => (TypeResolution::NonType, None),
            // MultiplyDefinedElement → `InvalidType` with no report.
            ScopedHit::Ambiguous(_) => (TypeResolution::Type, None),
        }
    }

    /// The scoped top-level answer with its `not_a_type` anchor: the
    /// declaration capture supplies the kept declaration's name span only
    /// for this file's own top-level functions and variables — a
    /// cross-file element's span is not carried, and that face degrades to
    /// the plain row (the under-attach direction).
    pub(crate) fn scoped_resolution(
        &self,
        name: StringId,
        file: FileId,
        kind: MicroKind,
    ) -> (TypeResolution, Option<(FileId, SourceRange)>) {
        let resolution = type_resolution_of(kind);
        let anchor = (resolution == TypeResolution::NonType && file == self.file)
            .then(|| {
                self.decl_sites
                    .iter()
                    .find(|site| {
                        site.name == name
                            && matches!(site.kind, DeclKind::TopLevelFn | DeclKind::TopLevelVar)
                    })
                    .map(|site| (self.file, site.range))
            })
            .flatten();
        (resolution, anchor)
    }

    /// Arms the type-prefix slot for a `PrefixedTypeReference` head; a miss
    /// also pushes the head's own report row. A live prefix head counts as
    /// a prefix use (the `unused_import` prefix face).
    pub(crate) fn arm_type_prefix(&mut self, name: StringId, range: SourceRange) {
        let (info, target) = self.resolve_bare(name);
        // The importPrefix gate's fall-through: a head naming a local scope
        // element (local, parameter, in-scope type parameter) is not an
        // import prefix — the oracle reports `prefixShadowedByLocalDeclaration
        // ` at the prefix token and marks the NamedType invalid (upstream
        // Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
        // named_type_resolver.dart:86-120 — the scope lookup's non-prefix,
        // non-class answer). Such a use never counts as a use of the
        // shadowing element (the oracle's `var p = 1; p.T q;` still
        // reports `p` unused).
        let shadowed = matches!(target, RefTarget::Local | RefTarget::TypeParam);
        let live = !matches!(info, BareInfo::Missing { .. }) && !shadowed;
        if live {
            self.used_names.insert(name);
        }
        self.type_prefix = Some(TypePrefix { prefix: name, live });
        if !live {
            if shadowed {
                let name_text = self.strings.resolve(name).to_string();
                self.emit(
                    SemanticCode::PrefixShadowedByLocalDeclaration,
                    range,
                    &name_text,
                    "",
                );
            } else {
                self.push_row(Row {
                    name,
                    range,
                    qualified: false,
                    shape: RowShape::TypeUse(TypeVerdict::PrefixHead, false),
                    intent: Intent::Read,
                    write_op: None,
                    ty: Ty::UNKNOWN,
                });
            }
        }
    }

    /// Rewrites a qualified creation head's miss (`prefix.C` of `new
    /// prefix.C.new()`): a three-segment reference resolves its head as a
    /// prefixed NamedType — the official importPrefix face reports
    /// `undefined_identifier` over the written `prefix.C` span, never the
    /// creation-head type-miss a bare head carries (`new Missing()` keeps
    /// `creation_with_non_type`). The head row converts in place and the
    /// creation-head context entry it armed is consumed (a dead prefix
    /// never asks the type-miss question).
    pub(crate) fn requalify_creation_head(&mut self, start: Token, period: Token) {
        let end = period.range.offset;
        self.requalify_creation_head_to(start, end);
    }

    /// [`Engine::requalify_creation_head`] with a written span end — the
    /// two-segment qualified reference (`Missing.new`) spans through the
    /// selector's name, its `.new` having ridden the qualified rest.
    pub(crate) fn requalify_creation_head_to(&mut self, start: Token, end: u32) {
        let offset = start.range.offset;
        let Some(index) = self.rows.iter().rposition(|row| {
            row.range.offset == offset && matches!(row.shape, RowShape::TypeUse(..))
        }) else {
            return;
        };
        let row = &mut self.rows[index];
        if let RowShape::TypeUse(TypeVerdict::Missing, suppressed) = row.shape {
            row.range.length = end.saturating_sub(offset);
            row.shape = RowShape::TypeUse(TypeVerdict::PrefixHead, suppressed);
            if let Ok(entry) = self
                .creation_miss_rows
                .binary_search_by_key(&offset, |entry| entry.0)
            {
                self.creation_miss_rows.remove(entry);
            }
        }
    }

    /// The class segment's verdict for a prefix-headed constructor
    /// reference (`fb.RuleState.stable()` — the head latched in
    /// [`Engine::ctor_prefix_heads`], resolved here at
    /// `end_constructor_reference`): `_lookupGetter(prefixElement.scope,
    /// node.name)` upstream (named_type_resolver.dart:140-147). A type hit
    /// stays silent — the constructor-selector face owns any remaining
    /// question. A miss or a non-type element reports
    /// `creation_with_non_type` at the class-name token (the tail's
    /// `diagnostic.at(node.name)`, :305-330); an ambiguous composition is
    /// the oracle's `MultiplyDefinedElement` → `InvalidType`, also silent.
    /// An unindexed declaring file keeps the under-report direction (the
    /// SDK face's kind is unknowable here). A selector-less `fb.RuleState`
    /// still latches its class segment through the qualified rest, so
    /// `class_name`/`class_range` always name the written second segment —
    /// except a bare `fb()` where the segment IS the head's span (the same
    /// token upstream's `node.name` reports at).
    pub(crate) fn resolve_ctor_prefix_class(
        &mut self,
        prefix: StringId,
        class_name: StringId,
        class_range: SourceRange,
    ) {
        let silent = match self
            .namespaces
            .lookup_prefixed(self.graph, self.lib, prefix, class_name)
        {
            dartq_resolution::Lookup::Found(NamespaceEntry::Decl(file)) => self
                .index
                .get(file)
                .and_then(|f| f.decl(class_name))
                .map_or(true, |decl| {
                    type_resolution_of(decl.kind) == TypeResolution::Type
                }),
            // Multiply-defined → `InvalidType` with no report; a prefix
            // entry cannot nest inside a prefix scope.
            dartq_resolution::Lookup::Ambiguous(_)
            | dartq_resolution::Lookup::Found(NamespaceEntry::Prefix) => true,
            dartq_resolution::Lookup::NotFound => false,
        };
        if !silent {
            let name_text = self.strings.resolve(class_name).to_string();
            self.emit(
                SemanticCode::CreationWithNonType,
                class_range,
                &name_text,
                "",
            );
        }
    }
}
