use super::*;
use super::scopes_bodies::PATTERN_ARM_SCOPE_BASE;
use dartq_scanner::{Keyword, TokenKind};

/// One `case`-head contribution to a shared-body member group (#3519):
/// the pattern variables it declared (`name`, declared type, is-final)
/// and whether a member label or `default` header rode along — the
/// `_SharedCaseScope` reason split's inputs.
struct SharedHeadVars {
    vars: Vec<(StringId, Option<Ty>, bool)>,
    label_or_default: bool,
    /// The head's `case` keyword offset (the member-label ordering key);
    /// `None` for the synthetic `default:` member row.
    offset: Option<u32>,
}

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
        self.ctor_pos_levels.push(self.arg_levels.len());
    }

    pub(crate) fn leave_ctor_pos(&mut self) {
        self.ctor_pos -= 1;
        self.ctor_pos_levels.pop();
    }

    /// Whether an argument group opening now belongs to the innermost
    /// creation expression itself — no argument list opened since the
    /// creation began.
    pub(crate) fn at_creation_level(&self) -> bool {
        self.ctor_pos_levels
            .last()
            .is_some_and(|&level| level == self.arg_levels.len())
    }

    /// A bare name's declared type, innermost frame first.
    pub(crate) fn typed_local(&self, name: StringId) -> Option<Ty> {
        self.frames
            .iter()
            .rev()
            .find_map(|f| f.var_types.get(&name).copied())
    }

    /// Whether the frame at `depth` still holds a LIVE declaration of
    /// `name` — a row of its `locals` range that no case scope has retired.
    ///
    /// `Frame::locals` is a name set, so a pattern binder's membership
    /// outlives the case scope that bound it and a shadowing binder shares
    /// its spelling's slot with the declaration it hid. The row flag is the
    /// only per-declaration state: retired everywhere in the frame means
    /// the spelling is out of scope here, and a read walks on to the outer
    /// frame (the at-close `restore_var_types` already put the shadowed
    /// type back, which is why the typed case answers in `var_types` first).
    fn frame_binds_live(&self, depth: usize, name: StringId) -> bool {
        let Some(&lo) = self.frame_local_marks.get(depth) else {
            return false;
        };
        let hi = self
            .frame_local_marks
            .get(depth + 1)
            .copied()
            .unwrap_or(self.locals.len());
        self.locals
            .get(lo..hi)
            .is_some_and(|rows| rows.iter().any(|decl| decl.name == name && !decl.retired))
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
            return self
                .fold_parameterized_alias(name, nullable, args)
                .unwrap_or(ty);
        }
        let (mut head, mut head_nullable) = (name, nullable);
        for _ in 0..8 {
            // A self-referencing alias never unfolds: it instantiates to
            // `dynamic` (`TypeAliasElementImpl.instantiateImpl`,
            // element.dart:11277-11288 — the `hasSelfReference` arm), so
            // `typedef F = F Function();` reads `dynamic` at every use
            // instead of the right-hand side that mentions itself.
            if self.bounds.alias_self_reference(head) {
                return Ty::Dynamic;
            }
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
                // A<int>?` is nullable on both spellings). A parameterized
                // chain hop continues the substitution face (a bare co-alias
                // landing on a generic function-RHS alias folds whole); an
                // unfoldable head keeps the written shape.
                Ty::Named {
                    name: next,
                    nullable: rhs_nullable,
                    args,
                } => {
                    return self
                        .fold_parameterized_alias(next, rhs_nullable | head_nullable, args)
                        .unwrap_or(Ty::Named {
                            name: next,
                            nullable: rhs_nullable | head_nullable,
                            args,
                        });
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
        // A bare alias head surviving the fold carries the same resolved
        // `staticType` a written one does — the oracle's
        // `instantiateTypeFormalsToBounds` face (`G` reads `G<A<dynamic>>`,
        // never the raw head; #2926 rider, the fourth fill point of the
        // #2925 adoption — the `Named { _, EMPTY }` shape decided here).
        self.instantiated_head_ty(Ty::Named {
            name: head,
            nullable: head_nullable,
            args: ArgListId::EMPTY,
        })
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
            // The parameterized twin of the bare fold's collapse: a
            // self-referencing alias instantiates to `dynamic` instead of
            // substituting into a right-hand side that mentions itself.
            if self.bounds.alias_self_reference(cur) {
                return Some(Ty::Dynamic);
            }
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
        if decl.kind != MicroKind::TypeAlias || decl.type_params == 0 || decl.var_ty == Ty::UNKNOWN
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
                self.boundary
                    .args
                    .borrow_mut()
                    .named(name, nullable, &substituted)
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
                    .map(|&(field_name, field)| {
                        (field_name, self.substitute_alias_rhs(field, subst))
                    })
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, nullable)
            }
            Ty::Function { shape, nullable } => {
                let (type_params, type_param_names, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.type_params_named.clone(),
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
                // The written bounds substitute with the rest (a bound
                // naming the aliased parameter reads the rewritten face).
                let names: Vec<(StringId, Option<Ty>)> = type_param_names
                    .iter()
                    .map(|&(name, bound)| {
                        (name, bound.map(|b| self.substitute_alias_rhs(b, subst)))
                    })
                    .collect();
                // A count-only source (names never recorded) keeps its
                // count-only identity — the rebuild never demotes it.
                let mut args = self.boundary.args.borrow_mut();
                let ty = if names.is_empty() && type_params > 0 {
                    args.function(type_params, return_ty, &positional, &named, nullable)
                } else {
                    args.function_generic(&names, return_ty, &positional, &named, nullable)
                };
                drop(args);
                ty
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
                self.boundary
                    .args
                    .borrow_mut()
                    .named(name, own || nullable, &list)
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
                let (type_params, type_param_names, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.type_params_named.clone(),
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                // A count-only source keeps its count-only identity.
                let mut args = self.boundary.args.borrow_mut();
                let ty = if type_param_names.is_empty() && type_params > 0 {
                    args.function(type_params, return_ty, &positional, &named, own || nullable)
                } else {
                    args.function_generic(
                        &type_param_names,
                        return_ty,
                        &positional,
                        &named,
                        own || nullable,
                    )
                };
                drop(args);
                ty
            }
            other => other,
        }
    }

    /// A receiver STATIC TYPE's declared read face for `field` — the member
    /// the type carries, resolved without a receiver row.
    ///
    /// A record answers the shape's field getter (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/type.dart `RecordTypeImpl`
    /// `namedField`/`positionalField` — the property-access resolver reads
    /// the field off the receiver's record type, `$1`-based positional
    /// spellings included). A NAMED class answers its declared field/getter
    /// instantiated with the type's own written arguments (#3420): the
    /// member merge's non-`this`/`super`/`typed` arm holds the receiver's
    /// static type only (a folded creation's published instantiation
    /// carries no `Receiver::Typed` row of its own), so this face restores
    /// the declared type the read publishes — the official
    /// `PropertyAccess.staticType` resolves the accessor on the receiver's
    /// static type whatever expression produced it.
    ///
    /// `receiver` folds through a type alias first
    /// (`typedef R = ({bool? strict}); r.strict`). A nullable receiver
    /// answers `None` unless the access is null-aware (a plain access is
    /// the unchecked dereference face, not a field read; `r?.f` reads the
    /// field off the promoted non-null receiver and the caller joins the
    /// `?`), as does a `$0`/out-of-range/non-numeric spelling, an absent
    /// named field, an executable member (a method read is a tear-off the
    /// name-level model does not render), a receiver whose class the
    /// element model cannot instantiate, and every other receiver shape
    /// (the `Object` members stay unmodeled — under-report).
    pub(crate) fn record_field_ty(
        &self,
        receiver: Ty,
        field: StringId,
        null_aware: bool,
    ) -> Option<Ty> {
        let folded = self.fold_alias_ty(receiver);
        match folded {
            Ty::Record { shape, nullable } => {
                if nullable && !null_aware {
                    return None;
                }
                let pool = self.boundary.pool();
                let shape = pool.record_shape(shape);
                let ty = match shape.named.iter().find(|(name, _)| *name == field) {
                    Some(&(_, ty)) => ty,
                    None => {
                        // Positional getters are `$1`, `$2`, … (1-based
                        // decimal, no sign and no leading zero — `$01` is not
                        // a field getter).
                        let index = self
                            .strings
                            .resolve(field)
                            .strip_prefix('$')
                            .filter(|digits| {
                                !digits.starts_with('0')
                                    && digits.bytes().all(|b| b.is_ascii_digit())
                            })
                            .and_then(|digits| digits.parse::<usize>().ok())?;
                        *shape.positional.get(index.checked_sub(1)?)?
                    }
                };
                drop(pool);
                // A field spelled through an alias reads as the aliased
                // type, like every declared type the frames bind.
                Some(self.fold_alias_ty(ty))
            }
            // The declared face, then the receiver's substitution: the
            // model's `var_ty`/`return_ty` spells the container's own
            // parameter (`T?`), and the read's static type is that face
            // instantiated with the written arguments (`G<int>.slot` reads
            // `int?`, never the raw `T?`). A receiver the instantiation
            // cannot decide (no written arguments, an arity the declaration
            // rejects) keeps the under-report — the RAW schema must never
            // leak as the read's static type.
            Ty::Named { name, nullable, .. } => {
                if nullable && !null_aware {
                    return None;
                }
                let declared = self.member_declared_ty(name, field);
                if declared == Ty::UNKNOWN {
                    return None;
                }
                let recv = Receiver::Typed {
                    type_name: name,
                    ty: folded,
                };
                let ty = self.instantiate_accessor_static(name, &recv, declared);
                (ty != Ty::UNKNOWN).then_some(ty)
            }
            _ => None,
        }
    }

    /// The captured right-hand type of the non-parameterized type alias
    /// `name`: this file's declaration table first, then the library
    /// namespace (the same resolution order a container use walks).
    pub(crate) fn alias_rhs(&self, name: StringId) -> Option<Ty> {
        // A prefixed annotation captured whole (`plural_rules.PluralRule` —
        // the interned one-head joined form): the tail resolves through the
        // import prefix's own library, the same face `prefixed_container`
        // gives containers.
        if let Some((prefix, tail)) = self.split_prefixed_spelling(name) {
            return match self
                .namespaces
                .lookup_prefixed(self.graph, self.lib, prefix, tail)
            {
                Lookup::Found(NamespaceEntry::Decl(owner)) => {
                    let decl = self.index.get(owner)?.decl(tail)?;
                    self.alias_decl_rhs(owner, decl)
                }
                _ => None,
            };
        }
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
            // A prefixed import's alias never enters the bare namespace
            // (`plural_rules.PluralRule`), but a FOREIGN signature's return
            // rides it: the send's callee declared its return under the
            // alias, and the chase must resolve the spelling in its OWN
            // library. The graph-wide unique declaration is that owner —
            // two same-named aliases in mutually invisible libraries would
            // answer Ambiguous and stay unchased (the under-report
            // direction).
            _ => {
                let libs = self.graph.declaring_libraries(name);
                if libs.len() != 1 {
                    return None;
                }
                let file = self.graph.unit_files(libs[0]).iter().copied().find(|&f| {
                    self.index
                        .get(f)
                        .is_some_and(|unit| unit.decl(name).is_some())
                })?;
                let decl = self.index.get(file)?.decl(name)?;
                (file, decl)
            }
        };
        self.alias_decl_rhs(owner, decl)
    }

    /// The non-parameterized alias gate shared by the scoped and prefixed
    /// resolution paths: a foreign alias's RHS is a slot of its owner's
    /// pool, projected into the diagnosing universe before anything reads
    /// it.
    fn alias_decl_rhs(&self, owner: FileId, decl: &crate::model::TopDecl) -> Option<Ty> {
        match decl.kind {
            MicroKind::TypeAlias if decl.type_params == 0 && decl.var_ty != Ty::UNKNOWN => {
                Some(self.boundary.ty(owner, decl.var_ty))
            }
            _ => None,
        }
    }

    /// Splits an interned joined spelling (`prefix.Name`) at its last dot
    /// when the prefix resolves as an import prefix of the diagnosing
    /// library; a dotted tail whose head is not an import prefix (a
    /// library-URI qualified name, recovery residue) stays whole.
    fn split_prefixed_spelling(&self, name: StringId) -> Option<(StringId, StringId)> {
        let spelled = self.strings.resolve(name);
        let dot = spelled.rfind('.')?;
        let (head, tail) = (&spelled[..dot], &spelled[dot + 1..]);
        if head.is_empty() || tail.is_empty() || head.contains('.') {
            return None;
        }
        let prefix = self.strings.intern(head);
        self.graph
            .import_edges(self.lib)
            .iter()
            .any(|edge| edge.prefix == Some(prefix))
            .then(|| (prefix, self.strings.intern(tail)))
    }

    /// The clause-face hop with the alias fold (the name-level twin of
    /// [`crate::diagnose::Engine::resolve_clause_face`]'s fallback): a
    /// clause face spelling a TYPE ALIAS binds the alias's right-hand
    /// container. Answers the resolved (owner, head) pair the
    /// [`crate::diagnose::types::FoldHierarchy`] walk crosses.
    pub(crate) fn alias_clause_hop(
        &self,
        name: StringId,
        file: FileId,
        lib: LibraryId,
    ) -> Option<(FileId, StringId)> {
        let mut head = name;
        for _ in 0..8 {
            let Some(Ty::Named { name: next, .. }) = self.alias_rhs_any(head) else {
                return None;
            };
            if next == head {
                return None;
            }
            head = next;
            if let Some((_, owner)) =
                resolve_container(self.graph, self.namespaces, self.index, lib, file, head)
            {
                return Some((owner, head));
            }
        }
        None
    }

    /// [`Self::alias_rhs`] without the non-parameterized gate: the
    /// clause-face fold (a `with ListMixin<E>` application over SDK 3.47's
    /// `typedef ListMixin<E> = ListBase<E>`) needs the right-hand HEAD of a
    /// parameterized class alias too — the parameter leaves ride bare and
    /// the application's own clause arguments substitute at the member
    /// sites, so only the head name matters there.
    pub(crate) fn alias_rhs_any(&self, name: StringId) -> Option<Ty> {
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
            MicroKind::TypeAlias if decl.var_ty != Ty::UNKNOWN => {
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
            if let Some((params, _rhs_type_params, ret, positional, named, required_positional)) =
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

    /// A container member's declared read type from a clean walk (`Unknown`
    /// for executables and broken chains): a field answers its declared (or
    /// inferred) type, a getter its declared return — the static type of the
    /// property read upstream, where `c.bad` over `void get bad` is `void`.
    /// The answer folds through a type alias first — the member read's static
    /// type is the resolved interface type (`c.f as int` over `IntAlias f` is
    /// dead). The fold is idempotent, so the promotable-field substitution
    /// that reads these rows' types re-folds harmlessly (#1409).
    pub(crate) fn member_declared_ty(&self, container: StringId, member: StringId) -> Ty {
        match self.walk_members(container, member) {
            Walk::Found(entry) if entry.kind == MemberKind::Field => {
                self.fold_alias_ty(self.field_model_ty(container, member, entry.var_ty))
            }
            // A getter read's declared type is its return; staging only
            // fields made every getter-read argument read as `Unknown`
            // (`E(c.bad)` over `void get bad` never reached the void-argument
            // face).
            Walk::Found(entry) if entry.kind == MemberKind::Getter => {
                self.fold_alias_ty(entry.return_ty)
            }
            _ => Ty::UNKNOWN,
        }
    }

    /// A field's model `var_ty` with the inferred overlay applied: a
    /// `var`/`final`-inferred declaration carries `Unknown` in the element
    /// model (the collector runs before any inference), so the diagnosing
    /// pass's own initializer inference — [`Self::bind_inferred_ty`]'s model
    /// slots — answers instead. `container` is the walk's start; the overlay
    /// keys on the DECLARING container, the origin the walk latched, so an
    /// inherited field answers too. A written annotation or a field no walk
    /// resolved keeps the model face.
    pub(crate) fn field_model_ty(&self, container: StringId, field: StringId, model: Ty) -> Ty {
        if model != Ty::UNKNOWN {
            return model;
        }
        let declaring = self.member_faces.last_origin().unwrap_or(container);
        self.field_inferred_tys
            .get(&(declaring, field))
            .copied()
            .unwrap_or(model)
    }

    /// A top-level variable's model `var_ty` with the same overlay: its
    /// `TopDecl::var_ty` carries `Unknown` for an inferred declaration.
    pub(crate) fn top_var_model_ty(&self, name: StringId, model: Ty) -> Ty {
        if model != Ty::UNKNOWN {
            return model;
        }
        self.top_var_inferred_tys
            .get(&name)
            .copied()
            .unwrap_or(model)
    }

    /// The write target's `getWriteType` — the assignment resolver's
    /// `writeAcceptedType` face: a field answers its declared type; a
    /// setter answers its single value formal's declared type (the write
    /// resolves against `name=`, never the getter's return). `Unknown` for
    /// executables, accessors with uncaptured formals, and broken chains —
    /// the under-report direction.
    ///
    /// The answer is the DECLARED face and must stay uninstantiated: a
    /// member whose declared type is a bounded type parameter
    /// (`class Box<T extends A> { T slot; }`) answers `T`, NEVER its bound.
    /// The oracle checks the write against the parameter itself (upstream
    /// Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
    /// assignment_expression_resolver.dart:361 —
    /// `_checkForInvalidAssignment(node.writeType!, ...)`), and a bare type
    /// variable admits no interface source at all — not even a subtype of
    /// its own bound, since the "Left Type Variable Bound" leg needs the
    /// SOURCE to be a type variable (pkg/analyzer/lib/src/dart/element/
    /// subtype.dart:175-274). A slot collapsed to `A` would therefore
    /// silence every `<bound subtype> <: T` rejection, so instantiation
    /// belongs to the RECEIVER face its callers hold: a written `C<B>`
    /// substitutes `B`, a raw `C` is `C<bounds>` (`instantiateToBounds`),
    /// and an implicit-this slot keeps the bare parameter.
    pub(crate) fn member_write_ty(&mut self, container: StringId, member: StringId) -> Ty {
        match self.walk_members(container, member) {
            Walk::Found(entry) if entry.kind == MemberKind::Field => {
                self.fold_alias_ty(self.field_model_ty(container, member, entry.var_ty))
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
        // The join key is the read's token OFFSET, not the full span: the
        // head latch (#1465) carries the receiver read's own span, but a
        // chained receiver (`c._p.isEven` under `if (c._p != null)`) rides
        // the #2283 chained latch's zero-length marker, whose offset is the
        // receiver read's member token without its length — an exact-span
        // compare drops every chained receiver's promotion, and the member
        // row over-reports `unchecked_use_of_nullable_value` where the
        // official resolves it on the promoted static type
        // (property_element_resolver.dart:2158-2166). Read offsets are
        // unique (one read event folds one row, one fact per read event),
        // so an offset hit is the same read; eligibility below still gates
        // the substitution on the promotable-field verdict.
        if !props.staged(range)
            && props
                .rows
                .binary_search_by(|row| row.offset.cmp(&range.offset))
                .is_err()
        {
            return None;
        }
        let (entry, name, _origin) = self.member_read_resolution(range.offset)?;
        // A promoted field's face drops its `?` whatever the head — a
        // nullable function or record field promotes like a named one.
        let promoted = match from {
            Ty::Function { .. } | Ty::Record { .. } => Self::non_null_form(from),
            _ => self.non_null_face(from),
        };
        if !Self::promotable_field_entry(self, &entry, name) {
            return None;
        }
        // The promoted TYPE the flow published on the receiver face
        // (#3430): a type test's gated candidate rides the member read's
        // own `read_types` row — `x._f is B` judges the read at `B`, the
        // promoted static type the official resolves members on
        // (`property_element_resolver.dart:2158-2166`). The candidates at
        // the read's offset filter on `!= from` — a promotion narrows, so
        // the declared face itself never qualifies: the semantic query
        // row an implicit-`this` read stages at the same offset (the
        // #1869-2 `query_type_rows` merge — the `read_type_at` poison
        // rule would answer nothing over two differing rows) spells
        // `from` byte for byte, and the marker promotions (`!= null`,
        // `!`) emit no row at all. A row the declared face provably does
        // not subsume is a foreign face, never this read's promotion; a
        // divergent pair (a loop body walked under two states) keeps no
        // single answer — the non-null face stands, the same
        // under-report direction `PromotionStaging`'s own poison takes.
        let start = self
            .read_types
            .partition_point(|(row, _)| row.offset < range.offset);
        let end = start
            + self.read_types[start..].partition_point(|(row, _)| row.offset == range.offset);
        let mut candidates = self.read_types[start..end].iter().filter_map(|(_, ty)| {
            (*ty != from
                && !matches!(ty, Ty::Unknown | Ty::Void | Ty::Never)
                && self.declared_subtype_negative(ty, &from) != Some(false))
            .then_some(*ty)
        });
        if let Some(ty) = candidates.next()
            && candidates.next().is_none()
        {
            return Some(ty);
        }
        Some(promoted)
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
        // The resolution keys on the read's own offset: the fact row's span
        // is the member read token — the same anchor the merge record and
        // the published bare resolution carry.
        let FlowFact::NonPromotedPropertyRead { span, .. } = fact else {
            return None;
        };
        let (entry, _name, _origin) = self.member_read_resolution(span.range.offset)?;
        // The flow fact carries the flow layer's own name id; the verdict
        // keys the semantic table, so the spelling re-interns here (the
        // same re-intern [`Engine::stage_non_promotion`] performs for the
        // context row's `'<prop>'`).
        let spelling = self.strings.intern(flow.name_lexeme(*name));
        // The shared eligibility verdict ([`Engine::property_promotion_verdict`]):
        // the same conjunction [`Engine::promotable_field_entry`] gates
        // every promotion path with, so the context row can never name a
        // face the promotion gate disagrees with. A name conflict is the
        // non-inherent arm — the context producer renders nothing for it
        // (it enumerates the conflicting declarations, which this engine
        // does not model).
        match self.property_promotion_verdict(&entry, spelling) {
            PropertyPromotionVerdict::Inherent(reason) => Some(reason),
            PropertyPromotionVerdict::Promotable | PropertyPromotionVerdict::NameConflict => None,
        }
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

    /// The ONE eligibility verdict every promotion path shares — the
    /// official `isPropertyPromotable` conjunction (upstream Dart SDK
    /// source, pkg/_fe_analyzer_shared/lib/src/field_promotability.dart
    /// `addField`/`addGetter`; `flow_analysis.dart`'s `isPropertyPromotable`
    /// delegate). [`Engine::promotable_field_entry`] gates every promotion
    /// substitution on it, and [`Engine::property_non_promotability`]
    /// derives the why-not-promoted context row from the SAME answer — the
    /// verdict and the flow can no longer disagree by construction.
    pub(crate) fn property_promotion_verdict(
        &self,
        entry: &Projected<'_, MemberEntry>,
        name: StringId,
    ) -> PropertyPromotionVerdict {
        // A non-private name can never be promotable (the official
        // `addField` ignores public names entirely: a subclass in another
        // library may alias the read to a getter or a mutable field).
        if !self.strings.resolve(name).starts_with('_') {
            return PropertyPromotionVerdict::Inherent(PropertyNonPromotability::NotPrivate);
        }
        // Only a field (never a getter/method), only an instance member,
        // and only a `final` one — the official `addField` conjunction
        // (`isExternal || !isFinal` is the bail; `isStatic` members never
        // enter the promotability pass at all, `_handleMembers` skips
        // them). An `abstract final` field stays promotable — the oracle
        // promotes it (`abstract final int? _x` under `if (_x != null)`
        // reports nothing), and the official `addField` never consults
        // `isAbstract` for the finality arm.
        if entry.kind != MemberKind::Field || entry.is_static || !entry.is_final {
            return PropertyPromotionVerdict::Inherent(if entry.kind == MemberKind::Field {
                PropertyNonPromotability::NotFinal
            } else {
                PropertyNonPromotability::NotField
            });
        }
        // An external field's read may run arbitrary foreign code.
        if entry.is_external {
            return PropertyPromotionVerdict::Inherent(PropertyNonPromotability::External);
        }
        // The library-wide name poisoning (`addField`/`addGetter`,
        // field_promotability.dart:100-172): any same-named non-final
        // field or concrete getter anywhere in the library defeats every
        // field of that name. The field itself is fine — the official
        // models this as a NON-inherent reason, and the context producer
        // renders no row for it (it enumerates the conflicting
        // declarations, which this engine does not model).
        if self.unit_field_name_conflicted(name) {
            return PropertyPromotionVerdict::NameConflict;
        }
        PropertyPromotionVerdict::Promotable
    }

    /// The promotable-field verdict on one resolved member entry (the
    /// flags half of [`Engine::promotable_field_substitution`]'s contract):
    /// the shared [`Engine::property_promotion_verdict`] under the unit's
    /// field-promotion grant (`fieldPromotionEnabled` — the *accessing*
    /// library's settled feature set, `ast_resolver.dart:139-140`, so a
    /// pre-3.2 unit never promotes even a field declared in a 3.2
    /// library).
    pub(crate) fn promotable_field_entry(
        &self,
        entry: &Projected<'_, MemberEntry>,
        name: StringId,
    ) -> bool {
        self.field_promotion_enabled
            && matches!(
                self.property_promotion_verdict(entry, name),
                PropertyPromotionVerdict::Promotable
            )
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
            Receiver::Created { name, ty, .. } => (name, false, ty),
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
                let env = recv_ty.and_then(|ty| self.member_recv_env(ty, container, &entry));
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
        let mut map: Vec<(StringId, Ty)> =
            if let Some(params) = self.bounds.container_params(container) {
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
                    found
                        .type_param_names
                        .iter()
                        .copied()
                        .zip(written)
                        .collect()
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
            // A prefixed top-level value (`diag.deadCode.atOffset(...)`,
            // `p.f().m(...)`): the read's declared type (or the callee's
            // return) is the receiver — the merge's own prefix arm, seen
            // before the merge runs. A prefixed container hit answers no
            // type here and keeps the unknown face.
            RowShape::Member {
                recv: Receiver::Prefix { .. },
            } => match self.receiver_static_ty(row) {
                ty @ Ty::Named { name, .. } => Receiver::Typed {
                    type_name: name,
                    ty,
                },
                _ => Receiver::Unknown,
            },
            _ => Receiver::Unknown,
        }
    }

    // —— resolution ——————————————————————————————————————————————————

    /// Resolves a bare name in value position: locals, type parameters, the
    /// member face (own-file container chain), the extension pool, enum
    /// synthesized members, then the scoped library lookup.
    pub(crate) fn resolve_bare(&self, name: StringId) -> (BareInfo, RefTarget) {
        // A primary constructor's formal is in scope from the constructor's
        // own planes alone ([`Self::primary_formal_hidden`]): a static or
        // `late` field initializer is outside both, and the read falls
        // through to the member/library faces exactly as an undeclared name
        // does (`undefined_identifier`).
        if !self.primary_formal_hidden(name) {
            // Innermost frame first: a pattern binder's var_types slot and a
            // tracked local resolve per-frame, so a binder shadowing an
            // outer SAME-NAMED local (`if (x case var x?)` rebinds `x` to the
            // non-null matched type) wins over the outer's declared type —
            // a graph-wide locals-first scan would read the outer's nullable
            // face and lose the binder's promotion.
            //
            // A frame whose every same-named row is RETIRED holds no
            // binding for the spelling: a case binder leaves its
            // `Frame::locals` membership behind when its scope closes
            // ([`super::pattern_scope::retire_binders`] stamps the row, the
            // set is name-only and its slot is shared with whatever the
            // binder shadowed), so the read walks on to the outer frame —
            // the declaration the binder hid — or to the member/library
            // faces as `undefined_identifier`.
            for (depth, frame) in self.frames.iter().enumerate().rev() {
                if let Some(ty) = frame.var_types.get(&name) {
                    return (BareInfo::Plain { ty: *ty }, RefTarget::Local);
                }
                if frame.locals.contains(&name) && self.frame_binds_live(depth, name) {
                    return (
                        BareInfo::Plain {
                            ty: self.typed_local(name).unwrap_or(Ty::UNKNOWN),
                        },
                        RefTarget::Local,
                    );
                }
            }
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
            // its declaration (`class C<T extends A<T>>`). The read itself is
            // a TypeLiteral — the reified `Type` object, never the parameter's
            // own type (upstream Dart SDK source, pkg/analyzer/lib/src/dart/
            // resolver/simple_identifier_resolver.dart:268-269, the
            // `TypeParameterElement` arm answering `_typeProvider.typeType`).
            // `return T` therefore asks `Type` against the declared return
            // and reports `return_of_invalid_type` wherever the parameter's
            // face is not `Type`-compatible — the #3056 family.
            return (
                BareInfo::Plain {
                    ty: Ty::named(self.intern("Type")),
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
            // The `this`-lookup accessibility gate (upstream
            // `element.isAccessibleIn(_definingLibrary)` —
            // property_element_resolver.dart:463-465): a `_`-prefixed
            // member the climb found on ANOTHER library's file is invisible
            // to this unit's `this` — the oracle leaves the reference
            // unresolved and reports `undefined_identifier`, never the
            // member. `walk_members`'s `found_origin` latched the declaring
            // file; a miss or a same-library hit keeps the member face.
            if member_found {
                let inaccessible = self.is_private_name(name)
                    && self
                        .member_faces
                        .last_found_file()
                        .is_some_and(|owner| self.graph.library_of_file(owner) != Some(self.lib));
                if !inaccessible {
                    return (
                        BareInfo::Plain {
                            ty: self.member_declared_ty(container, name),
                        },
                        RefTarget::Member { container },
                    );
                }
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
            ScopedHit::Local(decl) => {
                // This unit's own declaration: the model row answers, with
                // the inferred overlay for a `var`-inferred top-level
                // variable (`TopDecl::var_ty` is `Unknown` there — the
                // collector runs before any inference).
                let shape = match self.bare_info_of(name, decl) {
                    BareInfo::Plain { ty } => BareInfo::Plain {
                        ty: self.top_var_model_ty(name, ty),
                    },
                    other => other,
                };
                (shape, RefTarget::Decl { file: self.file })
            }
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
        // A nullable `this` (`on C?`, an unbounded `on T`) resolves an
        // extension member through the nullable type exactly like a
        // non-null one: the official member lookup consults the `Object`
        // members and then the extension pool against the potentially
        // nullable receiver type itself, so a face whose `on` clause
        // accepts the `?` spelling answers there (upstream Dart SDK
        // source, pkg/analyzer/lib/src/dart/resolver/
        // type_property_resolver.dart:96-112). A non-nullable `on C` face
        // never applies to `C?` (`C?` is not a subtype of `C`), which the
        // `probe` gate below enforces — such a miss keeps the
        // `unchecked_use_of_nullable_value` face.
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
            // An `on` clause that names one of the face's own type
            // parameters (`extension E<T> on T`) instantiates that parameter
            // with the receiver's type, so the applicability question is the
            // parameter's declared bound (`_getApplicable`'s
            // `tryChooseFinalTypes` inference): an absent bound
            // ([`Ty::Dynamic`], the unbounded marker — the implicit `Object?`
            // the model reads) admits every receiver, nullable included; a
            // written bound admits the receivers reaching its head, and under
            // a nullable `this` only a bound that is itself nullable (`T
            // extends C?`) keeps the face. A bound over the parameter's own
            // face (`T extends Comparable<T>`) is beyond the head-level gate
            // and keeps the miss.
            if let Some(index) = face.type_param_names.iter().position(|&param| param == on) {
                let Some(&bound) = face.type_param_bounds.get(index) else {
                    return false;
                };
                return match bound {
                    Ty::Dynamic => arg_applicable(face, file),
                    Ty::Named {
                        name: bound_head,
                        nullable: bound_nullable,
                        args,
                    } => {
                        args == ArgListId::EMPTY
                            && (!receiver_nullable || bound_nullable)
                            && reaches(receiver, bound_head)
                            && arg_applicable(face, file)
                    }
                    _ => false,
                };
            }
            // The clause's own nullability: a written `on C?` applies to both
            // receiver spellings, a non-nullable `on C` only to the non-null
            // one (`C?` is not a subtype of `C`).
            if receiver_nullable && !face.on_type_nullable {
                return false;
            }
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
            // The implicit `this` of an extension body over a resolvable
            // nullable `on` type (`on DartType?`): `ThisLookup` resolves the
            // name against that potentially nullable receiver, and
            // `type_property_resolver`'s `isNullable` arm reports the
            // unchecked access before the lexical fall-through — static
            // members included (probed: the oracle reports there too). An
            // invalid `on` head is the `InvalidType` `this` — silent.
            ext_nullable_this: context.is_some_and(|(container, kind)| {
                kind == ContainerKind::Extension
                    && match self.this_static_ty(container, ContainerKind::Extension) {
                        Ty::Named {
                            name,
                            nullable: true,
                            ..
                        } => {
                            self.type_params.contains(&name)
                                || self.pending_type_params.contains(&name)
                                || self.type_resolution(name).0 == TypeResolution::Type
                        }
                        _ => false,
                    }
            }),
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
        // A case header's type head that resolves to `InvalidType` gates the
        // open switch's exhaustiveness pass off (`hasInvalidType`). Only a
        // head counts: `List<Missing>` is an interface type, not Invalid.
        if self.switch.case_sink_open
            && self.type_arg_depth == 0
            && matches!(
                row.shape,
                RowShape::TypeUse(TypeVerdict::Missing | TypeVerdict::NonType, _)
            )
            && let Some(ctx) = self.switch.switches.last_mut()
        {
            ctx.has_invalid_type = true;
        }
        self.rows.push(row);
    }

    /// The guarded-pattern scope faces (#3519) evaluated at bare-identifier
    /// resolution time, when the token window still shows the enclosing
    /// `case`/`when`/`default` structure:
    ///
    /// * **Relational operand** (`case > x:`): the operand resolves in the
    ///   scope enclosing the pattern — a hit on the pattern's own variable
    ///   reports `referenced_before_declaration` (the official
    ///   `_hiddenElements` hides every guarded-pattern variable while the
    ///   pattern visits), and a provably non-constant operand reports
    ///   `non_constant_relational_pattern_expression`
    ///   (`_evaluateAndReportError`, resolver_visitor.dart
    ///   `visitRelationalPattern` — `final a` fires, `const a` and
    ///   literals stay silent).
    /// * **Guard write** (`when a = 1`): an unqualified write to the
    ///   guarded pattern's own variable inside the `when` clause reports
    ///   `pattern_variable_assignment_inside_guard`
    ///   (`isVisitingWhenClause` + `UnqualifiedNameAssignmentTarget`,
    ///   error_verifier.dart `_checkForPatternVariableAssignmentInsideGuard`
    ///   — nested closures keep the flag, member targets and plain reads
    ///   stay silent).
    /// * **Shared case scope** (`case int a: case num a: a;`): a body read
    ///   of a variable the heads joined inconsistently reports
    ///   `invalid_pattern_variable_in_shared_case_scope` with the official
    ///   reason split — absent in some heads (`not_all_cases`), a label or
    ///   `default` member in the group (`has_label`), mismatched type or
    ///   finality (`differentFinalityOrType`), `_SharedCaseScope.finish` in
    ///   variable_bindings.dart reporting at each `variable.references`
    ///   read.
    fn check_pattern_scope_faces(
        &mut self,
        name: StringId,
        range: SourceRange,
        target: &mut RefTarget,
    ) {
        if self.switch.pattern_depth > 0
            && (self.relational_operand_at(range) || self.relational_operand_contains(range))
        {
            self.check_relational_operand(name, range);
        }
        if self.in_when_guard(range) {
            self.check_guard_write(name, range);
        }
        self.check_shared_case_scope(name, range, target);
    }

    /// Whether the token at `range` begins a relational pattern's operand:
    /// scanning back over the unary prefix spellings (`-`, `~`, `!`) and
    /// the operand's own grouping `(` lands on `>`, `>=`, `<`, `<=`, `==`,
    /// or `!=`. Only consulted while a pattern is open.
    fn relational_operand_at(&self, range: SourceRange) -> bool {
        let toks = self.tokens.as_slice();
        let mut i = self.token_at_offset(range.offset);
        while i > 0 {
            i -= 1;
            match toks[i].kind {
                TokenKind::Minus
                | TokenKind::Tilde
                | TokenKind::Bang
                | TokenKind::OpenParen => {}
                TokenKind::Gt
                | TokenKind::GtEq
                | TokenKind::Lt
                | TokenKind::LtEq
                | TokenKind::EqEq
                | TokenKind::BangEq => return true,
                _ => return false,
            }
        }
        false
    }

    /// Whether `range`'s identifier sits ANYWHERE inside a relational
    /// pattern's operand, not only at its head. `visitRelationalPattern`
    /// evaluates the whole operand expression
    /// (`_evaluateAndReportError(node.operand, …)`), and the oracle's
    /// failing entity is the deepest non-constant construct — for the
    /// compound operands the corpus writes (`== 0 + i`, `> a + 1`,
    /// `>= j && < 20`) that construct is a name the head-only probe never
    /// reaches. The scan walks back over the operand's expression tokens
    /// at bracket depth 0 and stops at:
    ///
    /// * a relational operator — the identifier is inside that operand;
    /// * a call/index opener whose callee is an expression (`f(` / `a[`) —
    ///   the call's own face covers its arguments (the operand's evaluation
    ///   reports the invocation, never its argument);
    /// * any pattern-level boundary (`&&`, `||`, `:`, `,`, an opener, an
    ///   arrow, `case`/`when`/`if`, the clause keyword) — not our operand.
    fn relational_operand_contains(&self, range: SourceRange) -> bool {
        let toks = self.tokens.as_slice();
        let mut i = self.token_at_offset(range.offset);
        let mut depth: i32 = 0;
        while i > 0 {
            i -= 1;
            let kind = toks[i].kind;
            match kind {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth += 1;
                }
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    if depth > 0 {
                        depth -= 1;
                        continue;
                    }
                    // Only a grouping paren continues the operand: `[`, `{`
                    // at depth 0 open a nested pattern (a list/map element
                    // whose own faces own the identifier), and a call's
                    // paren means the invocation's face owns its arguments.
                    if kind != TokenKind::OpenParen {
                        return false;
                    }
                    let caller = i.checked_sub(1).and_then(|j| toks.get(j)).is_some_and(|t| {
                        matches!(
                            t.kind,
                            TokenKind::Identifier
                                | TokenKind::CloseParen
                                | TokenKind::CloseSquare
                                | TokenKind::CloseCurly
                        )
                    });
                    if caller {
                        return false;
                    }
                }
                TokenKind::Gt
                | TokenKind::GtEq
                | TokenKind::Lt
                | TokenKind::LtEq
                | TokenKind::EqEq
                | TokenKind::BangEq => {
                    return depth == 0;
                }
                TokenKind::AmpersandAmpersand
                | TokenKind::BarBar
                | TokenKind::Colon
                | TokenKind::Comma
                | TokenKind::Semicolon
                | TokenKind::Function
                | TokenKind::Keyword(_) => return false,
                // The operand's own expression tokens.
                TokenKind::Identifier
                | TokenKind::Int
                | TokenKind::Double
                | TokenKind::Hexadecimal
                | TokenKind::String
                | TokenKind::Minus
                | TokenKind::Plus
                | TokenKind::Star
                | TokenKind::Slash
                | TokenKind::Tilde
                | TokenKind::Bang
                | TokenKind::Period
                | TokenKind::QuestionPeriod
                | TokenKind::Question
                | TokenKind::PlusPlus
                | TokenKind::MinusMinus => {}
                _ => return false,
            }
        }
        false
    }

    /// The relational operand's two verdicts: a same-head pattern variable
    /// resolves in the enclosing scope (hidden upstream), and the operand
    /// must be a constant. The self/same-head read answers BOTH rows —
    /// upstream resolves the operand to the pattern variable itself (the
    /// `referenced_before_declaration` report), which the constant visitor
    /// then judges non-constant (`_evaluateAndReportError`'s own row) —
    /// an outer `const` never feeds the ask.
    ///
    /// The `>`-prefix shapes the oracle's entity reads:
    ///
    /// * a bare name — the identifier itself;
    /// * a postfix `++`/`--` on the name — the WHOLE `i++` (`visitPostfixExpression`
    ///   evaluates the operand, fails, and reports its own node);
    /// * a call — the resolver rewrote it to a `FunctionExpressionInvocation`
    ///   (see [`CallClass::ValueCall`]), whose `visitNode` fallback reports
    ///   the invocation, not the callee; but a `MethodInvocation` on a
    ///   top-level FUNCTION reports `const_eval_method_invocation` from the
    ///   allow list, which wins over this site's default (the row then
    ///   carries that code, not this one).
    fn check_relational_operand(&mut self, name: StringId, range: SourceRange) {
        let self_read = self
            .resolve_local(name)
            .is_some_and(|hit| self.pattern_decl_in_head(hit, range.offset));
        // `name(...)`: a call — never a constant operand, and its own row
        // (if any) owns the anchor. A top-level function's invocation is a
        // real `MethodInvocation` and reports `const_eval_method_invocation`
        // (allow-listed, so the site default never fires); every other
        // callee shape falls through to the enclosing default.
        let next = self.token_at_offset(range.offset) + 1;
        let toks = self.tokens.as_slice();
        let call_shape = toks
            .get(next)
            .is_some_and(|t| t.kind == TokenKind::OpenParen);
        if call_shape {
            // A top-level FUNCTION callee is a real `MethodInvocation`:
            // `ConstantVisitor.visitMethodInvocation` answers the
            // allow-listed `const_eval_method_invocation`, which overrides
            // this site's default — the const-eval close owns the row. Every
            // other callee shape (a local/formal/field read) was rewritten
            // to a `FunctionExpressionInvocation`, whose `genericError` is
            // NOT allow-listed, so the enclosing default reports here.
            let allow_listed = matches!(
                self.top_level_decl(name).map(|d| d.kind),
                Some(MicroKind::TopLevelFn)
            );
            if !allow_listed {
                self.emit(
                    SemanticCode::NonConstantRelationalPatternExpression,
                    range,
                    "",
                    "",
                );
            }
            return;
        }
        // A postfix `++`/`--` widens the anchor over the operator token.
        let postfix = toks.get(next).filter(|t| {
            matches!(t.kind, TokenKind::PlusPlus | TokenKind::MinusMinus)
        });
        let anchor = postfix.map_or(range, |t| SourceRange {
            offset: range.offset,
            length: t.utf16_offset + t.byte_len as u32 - range.offset,
        });
        let nonconst = self_read
            || matches!(
                ConstLookup::ident(self, name),
                IdentVerdict::NotConstant | IdentVerdict::Unresolved
            );
        if nonconst {
            self.emit(
                SemanticCode::NonConstantRelationalPatternExpression,
                anchor,
                "",
                "",
            );
        }
        if self_read {
            let text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::ReferencedBeforeDeclaration, range, &text, "");
        }
    }

    /// Whether the `locals` row `hit` is a pattern variable declared inside
    /// the still-open pattern window: its sentinel band names the case
    /// head, and an ungrouped/site binder in the open if-case or shared
    /// head belongs to it whenever the declaration offset follows the
    /// clause anchor's.
    fn pattern_decl_in_head(&self, hit: u32, at: u32) -> bool {
        let decl = &self.locals[hit as usize];
        if !matches!(decl.kind, LocalKind::PatternVariable) || decl.range.offset >= at {
            return false;
        }
        match decl.group {
            Some(group) if group >= PATTERN_ARM_SCOPE_BASE => true,
            Some(_) => false,
            None => {
                let anchor = self
                    .clause_anchor
                    .or_else(|| self.switch.switches.last().map(|ctx| ctx.switch_kw));
                anchor.is_some_and(|a| decl.range.offset >= a.offset)
            }
        }
    }

    /// Whether `range`'s token sits inside an open `when` clause: the
    /// backward scan finds `when` at bracket depth 0 before any
    /// guard-ending boundary — `case`/`default`/`;`, a non-ternary `:`, or
    /// (in a switch expression) the arm's `=>`. Nested closures inside the
    /// guard keep the flag upstream, so `(`/`{` openers step through to
    /// the enclosing context; an arrow function inside a STATEMENT guard
    /// still belongs to the clause, so `=>` only bounds a switch
    /// expression's arm.
    fn in_when_guard(&self, range: SourceRange) -> bool {
        let toks = self.tokens.as_slice();
        let is_expression = self
            .switch
            .switches
            .last()
            .is_some_and(|ctx| ctx.is_expression);
        let read = self.token_at_offset(range.offset);
        let mut i = read;
        let mut depth = 0i32;
        let mut ternaries = 0i32;
        while i > 0 {
            i -= 1;
            match toks[i].kind {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth += 1;
                }
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    // A bracket at depth 0 that closed BEFORE the read
                    // means the scan left the clause's own group — the
                    // `if (`/`case ...:` head preceding a then body.
                    // Grouping that still wraps the read (`(a = 1)` inside
                    // the guard) steps through to `when`.
                    if depth == 0 {
                        let Some(end) = self.tokens.matching_end(i) else {
                            return false;
                        };
                        if end < read {
                            return false;
                        }
                        // A `{` still wrapping the read: a statement block
                        // (`if (...) {`, `case X: {`, `=> {`) closes the
                        // clause — the read is the member body, never the
                        // guard. An expression `{` (a `() {` closure or a
                        // map literal) keeps `isVisitingWhenClause` set
                        // upstream, so the scan continues.
                        if matches!(toks[i].kind, TokenKind::OpenCurly)
                            && self.curly_is_statement_block(i)
                        {
                            return false;
                        }
                    }
                    depth -= 1;
                }
                TokenKind::Keyword(Keyword::When) if depth <= 0 => return true,
                TokenKind::Keyword(Keyword::Case | Keyword::Default)
                | TokenKind::Semicolon
                    if depth <= 0 =>
                {
                    return false;
                }
                TokenKind::Function if depth <= 0 => {
                    if is_expression {
                        return false;
                    }
                }
                TokenKind::Colon if depth <= 0 => {
                    if ternaries == 0 {
                        return false;
                    }
                    ternaries -= 1;
                }
                TokenKind::Question if depth <= 0 => ternaries += 1,
                _ => {}
            }
        }
        false
    }

    /// Whether a `{` opens a statement block rather than an expression:
    /// the previous token answers it — `:` (a case/default head's colon
    /// or a `label:`), a control-flow `)` whose `(` rides a keyword
    /// (`if (`/`for (`/`while (`/`switch (`/`catch (`), `=>`, `else`,
    /// `try`, `finally`, `do` — everything else (a call's `)`, `(`,
    /// `,`, `=`, an operator, a map-literal `[`/`{`) is a guard
    /// expression's own group.
    fn curly_is_statement_block(&self, open: usize) -> bool {
        let toks = self.tokens.as_slice();
        match toks.get(open.wrapping_sub(1)).map(|t| t.kind) {
            Some(TokenKind::Colon)
            | Some(TokenKind::Function)
            | Some(TokenKind::Keyword(
                Keyword::Else | Keyword::Try | Keyword::Finally | Keyword::Do,
            )) => true,
            Some(TokenKind::CloseParen) => {
                // `) {` — walk the `)`'s `(` backward and read the token
                // in front of it: a control keyword makes the block a
                // statement; a name/`(`/operator makes `()` a call or a
                // closure signature — the guard's expression continues.
                let mut inner = 0i32;
                let mut j = open - 1;
                while j > 0 {
                    j -= 1;
                    match toks[j].kind {
                        TokenKind::CloseParen | TokenKind::CloseSquare
                        | TokenKind::CloseCurly => inner += 1,
                        TokenKind::OpenParen => {
                            if inner == 0 {
                                return matches!(
                                    toks.get(j.wrapping_sub(1)).map(|t| t.kind),
                                    Some(TokenKind::Keyword(
                                        Keyword::If
                                            | Keyword::For
                                            | Keyword::While
                                            | Keyword::Switch
                                            | Keyword::Catch
                                    ))
                                );
                            }
                            inner -= 1;
                        }
                        TokenKind::OpenSquare | TokenKind::OpenCurly => {
                            if inner == 0 {
                                return false;
                            }
                            inner -= 1;
                        }
                        _ => {}
                    }
                }
                true
            }
            _ => false,
        }
    }

    /// An unqualified write inside the guard: `a = v`, any compound `op=`,
    /// or `a++`/`a--`/`++a`/`--a` hitting a guarded pattern variable (its
    /// arm-sentinel group or ungrouped/site binder — a declaration-window
    /// even group belongs to a statement destructure, never a guard).
    fn check_guard_write(&mut self, name: StringId, range: SourceRange) {
        let toks = self.tokens.as_slice();
        let idx = self.token_at_offset(range.offset);
        let write = toks.get(idx + 1).is_some_and(|t| {
            matches!(
                t.kind,
                TokenKind::Eq
                    | TokenKind::PlusEq
                    | TokenKind::MinusEq
                    | TokenKind::StarEq
                    | TokenKind::SlashEq
                    | TokenKind::TildeSlashEq
                    | TokenKind::PercentEq
                    | TokenKind::AmpersandEq
                    | TokenKind::BarEq
                    | TokenKind::CaretEq
                    | TokenKind::LtLtEq
                    | TokenKind::GtGtEq
                    | TokenKind::GtGtGtEq
                    | TokenKind::QuestionQuestionEq
                    | TokenKind::IndexEq
                    | TokenKind::PlusPlus
                    | TokenKind::MinusMinus
            )
        }) || idx > 0
            && matches!(
                toks[idx - 1].kind,
                TokenKind::PlusPlus | TokenKind::MinusMinus
            );
        if !write {
            return;
        }
        let Some(hit) = self.resolve_local(name) else {
            return;
        };
        let decl = &self.locals[hit as usize];
        let guarded = matches!(decl.kind, LocalKind::PatternVariable)
            && decl.group.is_none_or(|group| group >= PATTERN_ARM_SCOPE_BASE || group % 2 == 1);
        if guarded {
            self.emit(
                SemanticCode::PatternVariableAssignmentInsideGuard,
                range,
                "",
                "",
            );
        }
    }

    /// The `_SharedCaseScope` join: a body read of a pattern variable the
    /// member's heads declared inconsistently — or the `default`-member
    /// continuation of a labeled group. `target` upgrades `Missing` to
    /// `Local`: the official resolves the read to the joined variable (it
    /// is in scope, merely flagged), so no `undefined_identifier` may ride
    /// beside this row.
    fn check_shared_case_scope(
        &mut self,
        name: StringId,
        range: SourceRange,
        target: &mut RefTarget,
    ) {
        let Some(heads) = self.shared_case_heads(range.offset) else {
            return;
        };
        // The read hits a name the group declares: present in at least one
        // head's variable set.
        let mut present = 0usize;
        let mut declared = false;
        for head in &heads {
            if head.vars.iter().any(|(n, ..)| *n == name) {
                present += 1;
            }
        }
        declared = present > 0;
        if !declared {
            return;
        }
        // The body-position gate: the read follows the member's last head —
        // the clause anchor's offset must precede it, and a read still
        // inside a `when` clause keeps the hidden-elements rules instead
        // (mid-head the last scope's `head_vars` is still empty, so a
        // guard read would mis-join as "absent in this head").
        if self
            .clause_anchor
            .is_none_or(|anchor| anchor.end() > range.offset)
            || self.in_when_guard(range)
            || self.switch.pattern_depth > 0
            || self.switch.case_sink_open
        {
            return;
        }
        let has_label = heads.iter().any(|head| head.label_or_default);
        // The official precedence (variable_bindings.dart:183-194):
        // `allCases` joins cleanly or by type/finality; otherwise the
        // label/default flag outranks the absent-in-a-head reason.
        let code = if present == heads.len() {
            // `_finishJoinedPatternVariables`: the joined variable demands
            // the same declared type and finality in every head.
            let consistent = {
                let mut it = heads.iter().filter_map(|head| {
                    head.vars.iter().find(|(n, ..)| *n == name)
                });
                match it.next() {
                    Some((_, ty, fin)) => it.all(|(_, ty2, fin2)| ty2 == ty && fin2 == fin),
                    None => true,
                }
            };
            if consistent {
                return;
            }
            SemanticCode::PatternVariableSharedCaseScopeDifferentFinalityOrType
        } else if has_label {
            SemanticCode::PatternVariableSharedCaseScopeHasLabel
        } else {
            SemanticCode::PatternVariableSharedCaseScopeNotAllCases
        };
        let text = self.strings.resolve(name).to_string();
        self.emit(code, range, &text, "");
        if matches!(target, RefTarget::Missing | RefTarget::Opaque) {
            *target = RefTarget::Local;
        }
    }

    /// The member's joined head list at a body position, `None` outside a
    /// shared-body group: the trailing run of this switch depth's
    /// `switch_arm_scopes` — closed members stay on the stack as
    /// bookkeeping rows and `member_body` marks where the previous
    /// statement-carrying member ended the run (the consecutive
    /// `case`/`default` headers feeding one statement list upstream).
    /// A run needs at least one head carrying variables (else nothing
    /// joins).
    fn shared_case_heads(&self, read: u32) -> Option<Vec<SharedHeadVars>> {
        let depth = self.switch.switches.len();
        if depth == 0 {
            return None;
        }
        // The trailing run at this depth: walk back past empty-head
        // members (`member_body == false`), stopping before the first
        // body-carrying member — that member's statements ended the
        // previous group.
        // A `head_offset` at-or-before the open switch's keyword marks a
        // FINISHED same-depth switch's stale scopes (its last member may
        // have closed on a `default:` — leaving `member_body` false on
        // the trailing row); they must not leak into this group.
        let switch_offset = self.switch.switches.last().map_or(0, |ctx| ctx.switch_kw.offset);
        let run: Vec<&SwitchArmScope> = self
            .switch_arm_scopes
            .iter()
            .rev()
            .take_while(|scope| scope.depth == depth)
            .take_while(|scope| !scope.member_body)
            .filter(|scope| scope.head_offset > switch_offset)
            .collect();
        let mut heads: Vec<SharedHeadVars> = run
            .into_iter()
            .rev()
            .map(|scope| SharedHeadVars {
                vars: scope.head_vars.clone(),
                // A `default:` member's synthetic head carries the
                // label/default reason with an empty variable set (the
                // official `switchStatementSharedCaseScopeEmpty` join).
                label_or_default: scope.is_default,
                offset: (scope.head_offset != u32::MAX).then_some(scope.head_offset),
            })
            .collect();
        // A `default:` member's head does not exist yet while its body
        // reads — its synthetic scope lands at `switch_member_end`, after
        // the statements ran. The token window answers it at read time:
        // scanning back at bracket depth ≤ 0, a `default` keyword before
        // the enclosing `{`/boundary means this group has a default head
        // (upstream's `switchStatementSharedCaseScopeEmpty` join).
        if !heads.iter().any(|h| h.label_or_default) {
            let toks = self.tokens.as_slice();
            let mut i = self.token_at_offset(read);
            let mut depth = 0i32;
            let mut has_default = false;
            while i > 0 {
                i -= 1;
                match toks[i].kind {
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                        depth += 1;
                    }
                    TokenKind::OpenCurly if depth == 0 => break,
                    TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                        depth -= 1;
                    }
                    TokenKind::Keyword(Keyword::Default) if depth <= 0 => {
                        has_default = true;
                        break;
                    }
                    TokenKind::Keyword(Keyword::Case)
                    | TokenKind::Semicolon
                    | TokenKind::Function
                        if depth <= 0 =>
                    {
                        break;
                    }
                    _ => {}
                }
            }
            if has_default {
                heads.push(SharedHeadVars {
                    vars: Vec::new(),
                    label_or_default: true,
                    offset: None,
                });
            }
        }
        // Member-level labels still open at the read count toward the
        // label reason when they precede the group's last head.
        if let Some(last) = heads.iter().filter_map(|h| h.offset).max() {
            for scope in &self.label_scopes {
                for (_, lrange, _) in &scope.labels {
                    if lrange.offset < last {
                        for head in heads.iter_mut() {
                            head.label_or_default = true;
                        }
                    }
                }
            }
        }
        (!heads.is_empty()).then_some(heads)
    }

    /// A bare identifier in value position: resolved immediately (the
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn push_bare(&mut self, name: StringId, range: SourceRange) {
        let (info, mut target) = self.resolve_bare(name);
        // The guarded-pattern scope faces (#3519): while a case/if-case
        // pattern is open the reads inside it resolve against the official
        // hidden-element and shared-scope rules — a relational operand's
        // reference to the pattern's own variables, a write to a pattern
        // variable inside `when`, and a body read of an inconsistently
        // joined shared-scope variable each carry their own row upstream.
        self.check_pattern_scope_faces(name, range, &mut target);
        // The nullable implicit-`this` member face (#2926 F2): a bare send
        // inside `extension E on A?` resolving through the `on` face (the
        // `RefTarget::Member` naming the `on` head — `walk_members`'s
        // climb reached `A`'s member map) rides the nullable `this`
        // receiver upstream's `type_property_resolver` guards before the
        // member lookup (its `isNullable` tail: the Object and extension
        // consults run first — `toString` and E's own members never reach
        // it, and only the resolvable-`on` `this` qualifies, the
        // `InvalidType` arm's silence).
        //
        // The `Missing` twin latches the WRITE side independently of
        // nullability: a bare `name = v` resolves the `name=` spelling —
        // the extension's own member map first (the lexical
        // `ExtensionMemberScope` — `set bar` inside `extension E on A?`
        // resolves without ever consulting `this`), then the `on` face's
        // member scope (`set foo` on `A`, resolved against `this`). Pass 2
        // cannot re-walk either (the frames are gone) and
        // `bare_write_target_exists` answers through the scope lookup
        // alone, so both verdicts snapshot here — the `Missing` write arm
        // reads the union for `undefined_identifier`, and the nullable-`on`
        // arm reads the `on`-face disjunction for the unchecked variant
        // (a lexical `E` hit means the `this` lookup never ran — silent).
        if let Some((container, kind)) = self.container_context()
            && kind == ContainerKind::Extension
        {
            let nullable_this = self.nullable_this_receiver(&Receiver::This { container, kind });
            let on_member = match target {
                RefTarget::Member { container: on } => {
                    nullable_this
                        && self.enclosing_extension_on_type(container) == Some(on)
                        && !self.members.object_members.contains(&name)
                }
                RefTarget::Missing => {
                    let setter = self.setter_spelling(name);
                    let own = matches!(self.walk_members(container, setter), Walk::Found(_));
                    let on_face =
                        matches!(self.on_face_walk(container, kind, setter), Walk::Found(_));
                    if own || on_face {
                        self.ext_write_target_rows.insert(range.offset);
                    }
                    nullable_this && on_face && !own
                }
                _ => false,
            };
            if on_member {
                self.ext_nullable_member_rows.insert(range.offset);
            }
        }
        if matches!(
            target,
            RefTarget::Local | RefTarget::TypeParam | RefTarget::Member { .. }
        ) {
            self.shadowed_bare_rows.insert(range.offset);
        }
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
            self.local_decl_at_read
                .insert(range.offset, self.locals[hit].range.offset);
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
            // A bare type-ALIAS name read in expression position is a
            // `TypeLiteral` — its static type is `dart:core Type`, never
            // the alias's right-hand face (upstream Dart SDK source,
            // `pkg/analyzer/lib/src/dart/resolver/simple_identifier_resolver.dart`
            // `_resolve2`: a `TypeAliasElement` identifier answers
            // `_typeProvider.typeType` whenever `_isExpressionIdentifier`
            // holds or its `aliasedType` is not an `InterfaceType`). The
            // row's `shape.ty` keeps the alias's declared right-hand side —
            // the `L()`/`L.named` invoke and member faces still read it —
            // while the read operand (`row.ty`) the assignability and
            // postfix checks consume is `Type`, so `f(F)` against a `Type`
            // formal stays silent instead of reporting the RHS function
            // type (census-12 shape 2.1).
            //
            // The rewrite is gated on the head NOT being a member-receiver
            // (`Cb.foo`, `Cb.new`): a selector head's operand type is the
            // literal-receiver face the member-miss checks read through
            // `shape.ty`/`receiver_static_ty` — rewriting `row.ty` there
            // would collapse the `Type`-display miss the alias literal
            // family pins (the `followed_by_period` lookahead is the same
            // `p.C` head probe the type-use arm runs).
            BareInfo::Plain { ty }
                if ty != Ty::UNKNOWN
                    && !self.followed_by_period(range.end())
                    && self
                        .top_level_decl(name)
                        .is_some_and(|d| d.kind == MicroKind::TypeAlias) =>
            {
                Ty::named(self.intern("Type"))
            }
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
        // #2859: the assignment-side literal/closure face. An inferred
        // local's initializer decides the local's static type at the
        // declaration (the oracle binds `var x = {1, 2, 3}` as `Set<int>`
        // — `VariableElementImpl.type` from the initializer's
        // `staticType`), and the read row carries it: the every-check
        // verdict (`x is Set<int>`), the operator faces, and the
        // `var`-inference window all read this slot. An initializer the
        // face cannot spell (a bare read, a send, a typed literal — its
        // own row already bound the exact face) keeps the unknown
        // sentinel, the under-report direction every untyped slot holds.
        let ty = if ty == Ty::UNKNOWN && matches!(info, BareInfo::Plain { .. }) {
            self.initializer_literal_ty(name)
        } else {
            ty
        };
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

    /// The inferred local's initializer static type at a bare read: a
    /// `var` declarator whose initializer is an untyped collection
    /// literal or a block-bodied function literal carries the
    /// initializer's own static type (`typed_literal_resolver.dart`'s
    /// no-context resolution — the same conventions #2854 pinned for the
    /// direct is-check operand, now feeding the declaration's slot). The
    /// window is the declaration's own: the tracked local's name token
    /// ([`Self::resolve_local`], the same frame-correct walk the usage
    /// credit runs) followed by `=` and the expression's first group. A
    /// group's later declarators (`var x, y = {}`) skip the `=`-less
    /// names the same gate. [`Ty::UNKNOWN`] — the suppress-everything
    /// sentinel — for every other shape.
    fn initializer_literal_ty(&self, name: StringId) -> Ty {
        let Some(hit) = self.resolve_local(name) else {
            return Ty::UNKNOWN;
        };
        let bytes = self.source.as_bytes();
        let mut at = self.byte_of(self.locals[hit as usize].range.end());
        while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
            at += 1;
        }
        // A plain declarator init only: `var x = <expr>`.
        if bytes.get(at) != Some(&b'=') || bytes.get(at + 1) == Some(&b'=') {
            return Ty::UNKNOWN;
        }
        at += 1;
        while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
            at += 1;
        }
        // An explicit `const` rides the literal (`var x = const {1};`).
        if bytes
            .get(at..)
            .is_some_and(|rest| rest.starts_with(b"const"))
        {
            at += 5;
            while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
                at += 1;
            }
        }
        match bytes.get(at) {
            Some(b'[' | b'{') => self.collection_init_ty(bytes, at),
            Some(b'(') => self.closure_init_ty(bytes, at),
            _ => Ty::UNKNOWN,
        }
    }

    /// The untyped collection initializer's inferred type: the close
    /// bracket's `untyped_literals` kind decides the head, the element
    /// leaves' least row type the arguments — the same conventions
    /// `collection_operand_ty` runs at a direct is-check operand. A
    /// missing kind stays unknown: a typed literal's own row already
    /// bound the exact face, and a read that textually precedes its
    /// same-named declaration finds no key (the close event only
    /// latches at parse time).
    fn collection_init_ty(&self, bytes: &[u8], open: usize) -> Ty {
        let Some(close) = matching_bracket_fwd(bytes, open) else {
            return Ty::UNKNOWN;
        };
        let Some(&kind) = self.untyped_literals.get(&self.utf16_of(close)) else {
            return Ty::UNKNOWN;
        };
        let dynamic = Ty::Dynamic;
        let (head, args) = match kind {
            // The empty `{}` literal's map default (`literal_return_ty`'s
            // no-context arm).
            UntypedLiteral::EmptyBraces => (
                self.collection_heads[2],
                self.boundary.intern_list(&[dynamic, dynamic]),
            ),
            UntypedLiteral::List => (
                self.collection_heads[0],
                self.boundary
                    .intern_list(&[self.init_leaf_least_ty(bytes, open + 1, close)]),
            ),
            UntypedLiteral::SetElements => (
                self.collection_heads[1],
                self.boundary
                    .intern_list(&[self.init_leaf_least_ty(bytes, open + 1, close)]),
            ),
            UntypedLiteral::MapEntries => {
                let (key, value) = self.init_entry_leaf_tys(bytes, open + 1, close);
                (
                    self.collection_heads[2],
                    self.boundary.intern_list(&[key, value]),
                )
            }
        };
        Ty::Named {
            name: head,
            nullable: false,
            args,
        }
    }

    /// The element leaves' least row type over the literal's inner span:
    /// each top-level comma segment's own completion row (a primitive
    /// literal's declared primitive, a bare local's declared type, a
    /// typed nested literal's face), folded to the least type the
    /// lattice can stand. A segment without an exact leaf row forces
    /// `dynamic` — the under-report that decides every provable ask and
    /// refuses every unprovable one.
    fn init_leaf_least_ty(&self, bytes: &[u8], start: usize, end: usize) -> Ty {
        let leaves: Vec<Option<Ty>> = value_comma_segments(bytes, start, end)
            .into_iter()
            .map(|(s, e)| self.leaf_row_ty_in(bytes, s, e))
            .collect();
        if leaves.iter().any(|leaf| leaf.is_none()) {
            return Ty::Dynamic;
        }
        self.init_leaf_least_from(leaves.into_iter().flatten().collect())
    }

    /// The map entry leaves' key/value least types: each segment's
    /// top-level colon splits the sides, a colonless leaf (a spread, a
    /// control-flow element) keeps both sides `dynamic`.
    fn init_entry_leaf_tys(&self, bytes: &[u8], start: usize, end: usize) -> (Ty, Ty) {
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for (s, e) in value_comma_segments(bytes, start, end) {
            match value_top_level_colon(bytes, s, e) {
                Some(colon) => {
                    keys.push(self.leaf_row_ty_in(bytes, s, colon));
                    values.push(self.leaf_row_ty_in(bytes, colon + 1, e));
                }
                None => {
                    keys.push(None);
                    values.push(None);
                }
            }
        }
        let fold = |tys: Vec<Option<Ty>>| -> Ty {
            if tys.iter().any(|ty| ty.is_none()) {
                return Ty::Dynamic;
            }
            self.init_leaf_least_from(tys.into_iter().flatten().collect())
        };
        (fold(keys), fold(values))
    }

    /// The leaf segment's completion row type: the last non-type-use row
    /// inside the segment's span, its push-time type only. The row stack
    /// answers first, the fold-drained queue second — a list literal's
    /// element rows drain into `deferred_miss_rows` at its close event
    /// (the set/map leaves stay on the stack), and a drained sub-literal's
    /// rows always predate every in-span stack row, so the stack hit IS
    /// the later push. A row without an exact type — a send, an untyped
    /// read — is no exact face and answers `None`.
    fn leaf_row_ty_in(&self, bytes: &[u8], start: usize, end: usize) -> Option<Ty> {
        let start = trim_value_ws(bytes, start, end.min(bytes.len()));
        let end = end.min(bytes.len());
        if start >= end {
            return None;
        }
        let (s16, e16) = (self.utf16_of(start), self.utf16_of(end));
        let in_span = |row: &Row| {
            row.range.offset >= s16
                && row.range.end() <= e16
                && !matches!(row.shape, RowShape::TypeUse(..))
        };
        let ty = self
            .rows
            .iter()
            .rev()
            .find(|row| in_span(row))
            .or_else(|| {
                self.deferred_miss_rows
                    .iter()
                    .rev()
                    .find(|row| in_span(row))
            })
            .map(|row| row.ty)?;
        (ty != Ty::UNKNOWN && ty != Ty::Dynamic).then_some(ty)
    }

    /// The collected leaf types' least face (`least_leaf_ty_from`'s
    /// convention): one exact type keeps itself, `int` mixed with
    /// `double` lifts to `num`, everything else — `Null` leaves,
    /// differing faces — keeps `dynamic` (the oracle's `Object?`/`Object`
    /// lifts are provability-equal: every target those accept, `dynamic`
    /// accepts, and every target those refuse, `dynamic` refuses).
    fn init_leaf_least_from(&self, tys: Vec<Ty>) -> Ty {
        let int = Ty::named(self.members.primitive_ids[0]);
        let double = Ty::named(self.members.primitive_ids[1]);
        let mut first: Option<Ty> = None;
        let mut diverse = false;
        let mut other_face = false;
        for ty in tys {
            if ty == Ty::Dynamic {
                return Ty::Dynamic;
            }
            if ty != int && ty != double {
                other_face = true;
            }
            match first {
                None => first = Some(ty),
                Some(prev) if prev != ty => diverse = true,
                Some(_) => {}
            }
        }
        match first {
            None => Ty::Dynamic,
            Some(ty) if !diverse => ty,
            // The int/double pair is the one diverse fold the lattice
            // can name (`num`); every other mix stays `dynamic`.
            Some(_) if !other_face => Ty::named(self.members.primitive_ids[2]),
            Some(_) => Ty::Dynamic,
        }
    }

    /// The block-bodied function literal initializer's written-signature
    /// face — the `closure_operand_ty` conventions: the plain positional
    /// shapes only, the written formal annotations as the parameter
    /// columns (an unannotated formal keeps the official element's
    /// `dynamic` column, an unresolved annotation head refuses the whole
    /// face), the return `dynamic` — the block body's inferred return is
    /// a flow face the row model does not carry, and `dynamic` in the
    /// operand's own column decides every return-top ask and refuses
    /// every mismatching one. An arrow body (its return column is the
    /// body expression's type — no `dynamic` stand-in answers for it), a
    /// generic literal, a named-formal section, or a brace inside the
    /// formal group refuses.
    pub(crate) fn closure_init_ty(&self, bytes: &[u8], paren_open: usize) -> Ty {
        let Some(paren_close) = matching_bracket_fwd(bytes, paren_open) else {
            return Ty::UNKNOWN;
        };
        let mut after = paren_close + 1;
        while bytes.get(after).is_some_and(|b| b.is_ascii_whitespace()) {
            after += 1;
        }
        // The body's kind: a `{` block keeps the `dynamic` return column
        // (the flow face the row model does not carry); an `=>` arrow
        // types its return as the body expression's own static type —
        // the literal's interior rows drained into `deferred_miss_rows`
        // at the close, so the body's row is the newest one the literal's
        // span covers (`var g = () => 0` binds `int Function()`, the
        // oracle's `FunctionExpressionImpl.staticType`). Anything else is
        // no literal this face reads.
        let arrow = bytes.get(after) == Some(&b'=') && bytes.get(after + 1) == Some(&b'>');
        if !arrow && bytes.get(after) != Some(&b'{') {
            return Ty::UNKNOWN;
        }
        // A `<...>` group head before the `(` is a generic literal: its
        // own parameters ride no declared face here.
        let mut head = paren_open;
        while head > 0 && bytes[head - 1].is_ascii_whitespace() {
            head -= 1;
        }
        if head > 0 && bytes[head - 1] == b'>' {
            return Ty::UNKNOWN;
        }
        // A named-formal section (`{...}`) or any brace inside the group
        // is no plain positional shape.
        if (paren_open + 1..paren_close).any(|i| bytes[i] == b'{') {
            return Ty::UNKNOWN;
        }
        // The group's written formals: each top-level comma segment's
        // trailing identifier is the formal's name, the head before it
        // the written annotation (its type-use row starts the segment and
        // only `?`/whitespace separates it from the name). A trailing
        // comma's empty tail segment is legal and skipped.
        let mut segments = value_comma_segments(bytes, paren_open + 1, paren_close);
        let tail_empty = match segments.last() {
            Some((s, e)) => trim_value_ws(bytes, *s, *e) >= *e,
            None => false,
        };
        if tail_empty {
            segments.pop();
        }
        let mut positional = Vec::new();
        for (s, e) in segments {
            let seg_end = value_default_cut(bytes, s, e);
            let seg_start = trim_value_ws(bytes, s, seg_end);
            if seg_start >= seg_end {
                return Ty::UNKNOWN;
            }
            let mut name_end = seg_end;
            while name_end > seg_start {
                let b = bytes[name_end - 1];
                if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                    name_end -= 1;
                } else {
                    break;
                }
            }
            if name_end == seg_end {
                // No identifier tail (`covariant`-only residue, a
                // malformed group): refuse.
                return Ty::UNKNOWN;
            }
            let head_start = trim_value_ws(bytes, seg_start, name_end);
            if head_start >= name_end {
                // No written annotation: the official element's
                // `dynamic` column.
                positional.push(Ty::Dynamic);
                continue;
            }
            let (s16, e16) = (self.utf16_of(head_start), self.utf16_of(name_end));
            let Some(&(range, name)) = self
                .type_uses
                .iter()
                .find(|(r, _)| r.offset == s16 && r.end() <= e16)
            else {
                return Ty::UNKNOWN;
            };
            // Only `?` and whitespace may ride between the head token and
            // the formal's name.
            let mut cursor = self.byte_of(range.end());
            let mut nullable = false;
            while cursor < name_end {
                match bytes[cursor] {
                    b if b.is_ascii_whitespace() => cursor += 1,
                    b'?' if !nullable => {
                        nullable = true;
                        cursor += 1;
                    }
                    _ => return Ty::UNKNOWN,
                }
            }
            // An unresolved annotation head is the oracle's InvalidType
            // formal — the closure's own static type goes invalid with it
            // and every verdict stays informative.
            if !self.named_type_is_type(name) {
                return Ty::UNKNOWN;
            }
            positional.push(Ty::Named {
                name,
                nullable,
                args: crate::model::ArgListId::EMPTY,
            });
        }
        let positional: Vec<Ty> = positional
            .into_iter()
            .map(|ty| self.fold_alias_ty(ty))
            .collect();
        let return_ty = if arrow {
            // The arrow body's static type: the literal's interior rows
            // drained into `deferred_miss_rows` at the close and the body's
            // own expression row is the newest one inside the literal's
            // span (`f(g())` drains `f`, `g`, `g()`, `f(...)` — the last is
            // the body's). `operand_static_ty` answers the send's result /
            // the read's face; a body the row model cannot type keeps the
            // `dynamic` column (the under-report direction — the verdict
            // stays informative exactly like the block body's).
            let lit_start = self.utf16_of(paren_open);
            self.deferred_miss_rows
                .iter()
                .rfind(|row| row.range.offset >= lit_start)
                .map(|row| self.operand_static_ty(row))
                .filter(|ty| *ty != Ty::UNKNOWN)
                .unwrap_or(Ty::Dynamic)
        } else {
            Ty::Dynamic
        };
        self.boundary
            .intern_function_ty(0, return_ty, &positional, &[], false)
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
    /// `code` reports the operand's unresolved head; `non_type_code` reports
    /// a resolved non-type head (`as` collapses both to `cast_to_non_type`,
    /// `is` splits `type_test_with_undefined_name`/`type_test_with_non_type`
    /// — named_type_resolver.dart's `_isTypeInAsExpression` /
    /// `_isTypeInIsExpression` arms run before the `element != null`
    /// `not_a_type` tail, :621-647).
    pub(crate) fn emit_operand_type_miss(
        &mut self,
        mark: Option<usize>,
        code: SemanticCode,
        non_type_code: Option<SemanticCode>,
    ) {
        let Some(start) = mark else {
            return;
        };
        if start >= self.rows.len() {
            return;
        }
        let mut head_hit: Option<(StringId, SourceRange)> = None;
        let mut non_type_head: Option<(StringId, SourceRange)> = None;
        let mut arg_hits: Vec<(StringId, SourceRange)> = Vec::new();
        let mut silence: Vec<usize> = Vec::new();
        for (index, row) in self.rows.iter().enumerate().skip(start) {
            match row.shape {
                RowShape::TypeUse(TypeVerdict::Missing, true) => {
                    if self.type_arg_miss_rows.contains(&row.range.offset) {
                        arg_hits.push((row.name, row.range));
                    } else if head_hit.is_none() {
                        head_hit = Some((row.name, row.range));
                    }
                }
                // A resolved non-type element inside the operand reports the
                // operand's code, never `not_a_type` — the pass-2 `NonType`
                // arm does not honor the suppression flag, so the row is
                // consumed here (a qualified `p.C` head keeps `not_a_type`:
                // upstream's `_rewriteToConstructorName` outranks the
                // operand context, named_type_resolver.dart:105-113/:373-457).
                RowShape::TypeUse(TypeVerdict::NonType, _) if !row.qualified => {
                    if self.type_arg_miss_rows.contains(&row.range.offset) {
                        arg_hits.push((row.name, row.range));
                    } else if non_type_head.is_none() {
                        non_type_head = Some((row.name, row.range));
                    }
                    silence.push(index);
                }
                _ => {}
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
        if let Some((name, range)) = non_type_head
            && let Some(non_type_code) = non_type_code
            && !self.guard.ignore(None, name)
        {
            let name_text = self.strings.resolve(name).to_string();
            self.emit(non_type_code, range, &name_text, "");
        }
        for index in silence {
            self.rows[index].shape = RowShape::Value;
        }
    }

    pub(crate) fn emit_is_type_miss(&mut self) {
        let mark = self.is_type_mark.take();
        // `type_test_with_non_type` is outside this engine's code set — a
        // resolved non-type element in an `is` operand stays silent rather
        // than mis-coding as `type_test_with_undefined_name`.
        self.emit_operand_type_miss(mark, SemanticCode::TypeTestWithUndefinedName, None);
    }

    pub(crate) fn emit_as_type_miss(&mut self) {
        let mark = self.as_type_mark.take();
        self.emit_operand_type_miss(
            mark,
            SemanticCode::CastToNonType,
            Some(SemanticCode::CastToNonType),
        );
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
            // the scanner drops them. This is the explicit-call set's ONLY
            // writer: a `this`/`super` formal never joins, its supplies feed
            // the flush's granular super-formal subtraction instead.

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
                // at the same span: the whole `ConstructorFieldInitializer`
                // node (`this = 0`), the keyword through the value's end.
                let range = self.initializer_node_span(token.range);
                self.emit(SemanticCode::InitializerForNonExistentField, range, "", "");
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
                // #2832: a type-position identifier that names a local is a
                // read of that local — the oracle's `visitNamedType` runs
                // `_useIdentifierElement` on the resolved element in every
                // type position (unused_local_elements_verifier.dart
                // :243-246/:402-423; an `is` operand is the one exclusion),
                // so `f(a<b, c>(d))` with `b`/`c` locals spares both
                // declarations the `unused_local_variable` row beside the
                // argument face's own reports. No `local_hits` entry rides:
                // the write-discard conversions key on value rows, and a
                // type-position mention is an unconditional use upstream.
                if self.is_type_pos == 0
                    && let Some(hit) = self.resolve_local(name)
                {
                    self.locals[hit as usize].uses += 1;
                }
                // An `is`/`as` operand admits a local element as a non-type
                // element: upstream's `_isTypeInAsExpression` /
                // `_isTypeInIsExpression` arms run before the
                // `LocalVariableElement`/`LocalFunctionElement` fall-through
                // (named_type_resolver.dart:621-647/:682-691), so `1 as
                // localVar` reports `cast_to_non_type`, never the silent
                // `Type` answer `type_resolution` gives the same local in a
                // declaration. The `NonType` row joins the operand window —
                // `emit_operand_type_miss` owns its report.
                if self.non_type_pos > 0 && self.resolve_local(name).is_some() {
                    if self.type_arg_depth > 0 {
                        self.type_arg_miss_rows.push(range.offset);
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
                // A keyworded creation head followed by `.` is a prefixed
                // NamedType's importPrefix — upstream `resolve()`'s
                // importPrefix arm (named_type_resolver.dart:85-128) reports
                // `prefix_shadowed_by_local_declaration` at the head token
                // for a resolved non-type non-prefix element and resolves
                // the whole `X.Y` to `InvalidType`: the `.Class`
                // continuation and constructor-selector faces never run.
                // Implicit creations take the method-invocation parse
                // instead (`nonPrefix.Class.named()`'s `undefined_getter`
                // face), so only explicit `new`/`const` heads ask the
                // question. A real prefix keeps its `ctor_prefix_heads`
                // dispatch; a class head rewrites to `C.named`; a miss
                // keeps the `undefined_identifier`-over-`p.C` requalify.
                if self.ctor_pos > 0
                    && self.type_arg_depth == 0
                    && self.redirect_target_depth == 0
                    && self.implicit_creation_depth == 0
                    && self.followed_by_period(range.end())
                {
                    let (_head_info, head_target) = self.resolve_bare(name);
                    if self.is_shadowed_prefix(name, head_target) {
                        let name_text = self.strings.resolve(name).to_string();
                        self.emit(
                            SemanticCode::PrefixShadowedByLocalDeclaration,
                            range,
                            &name_text,
                            "",
                        );
                        self.push_row(Row {
                            name,
                            range,
                            qualified: true,
                            shape: RowShape::Value,
                            intent: Intent::Read,
                            write_op: None,
                            ty: Ty::UNKNOWN,
                        });
                        return;
                    }
                    // An INHERITED member head (`new field.Class()` in a
                    // subclass — `field` declared on the supertype): the
                    // upstream member fallback is declared-only, so the
                    // importPrefix arm's element answers `null` — the same
                    // answer a plain miss gives — and
                    // `reportNewWithNonType` reports `undefined_identifier`
                    // over the whole `field.Class` span
                    // (named_type_resolver.dart:566-590). The plain-miss
                    // requalification in `end_constructor_reference` gates
                    // on `resolve_bare(head)` answering `Missing`, which a
                    // member never does — so the `PrefixHead` verdict over
                    // the written `p.C` span is latched directly (pass-2
                    // reports `undefined_identifier` naming the head).
                    if matches!(head_target, RefTarget::Member { .. }) {
                        let (_, leaf_range) = self.prefixed_leaf(range);
                        self.push_row(Row {
                            name,
                            range: SourceRange {
                                offset: range.offset,
                                length: leaf_range.end() - range.offset,
                            },
                            qualified: false,
                            shape: RowShape::TypeUse(TypeVerdict::PrefixHead, false),
                            intent: Intent::Read,
                            write_op: None,
                            ty: Ty::UNKNOWN,
                        });
                        return;
                    }
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
                    // A `p.C` whose head resolved to a type-bearing element
                    // (a class, enum, mixin, or alias spelled `C` —
                    // `type_resolution` answers `Type` for them and `NonType`
                    // for a real `PrefixElement`) is upstream's
                    // `_rewriteToConstructorName` non-creation arm: the
                    // named-type face reports `not_a_type` over the whole
                    // written `p.C` naming the pair (named_type_resolver.dart
                    // :105-113 + :373-457 — the `element is InstanceElement`
                    // member consult and the notAType tail). A REAL import
                    // prefix keeps the `Prefixed` miss's `undefined_class`.
                    if !local && self.type_resolution(prefix).0 == TypeResolution::Type {
                        let prefix_text = self.strings.resolve(prefix);
                        let name_text = self.strings.resolve(name);
                        let joined = format!("{prefix_text}.{name_text}");
                        let joined = self.strings.intern(&joined);
                        let head_range =
                            self.pending_prefixed.map(|(head, _)| head).unwrap_or(range);
                        self.push_row(Row {
                            name: joined,
                            range: SourceRange {
                                offset: head_range.offset,
                                length: range.end() - head_range.offset,
                            },
                            qualified: true,
                            shape: RowShape::TypeUse(TypeVerdict::NonType, suppressed),
                            intent: Intent::Read,
                            write_op: None,
                            ty: Ty::UNKNOWN,
                        });
                    } else {
                        // A real import prefix's member miss routes through
                        // the resolver's context chain wherever a bare head
                        // would: the dedicated codes
                        // (`type_test_with_undefined_name`,
                        // `cast_to_non_type`,
                        // `non_type_as_type_argument`,
                        // `non_type_in_catch_clause`) are the oracle's
                        // `reportNullOrNonTypeElement` answer for `p.C`
                        // exactly as for `C` (named_type_resolver.dart
                        // :720-770 — the `is`/`as`/type-argument/`on`
                        // context arms never ask the prefix question), so
                        // the row reports `undefined_class` only in the
                        // plain positions the `Prefixed` verdict owns (the
                        // local-head silence and the broken-edge guard ride
                        // its own fields). `non_type_pos` marks the `is`/`as`
                        // operand window (`emit_operand_type_miss` consumes
                        // it); `type_arg_miss_rows` keys the argument face;
                        // the catch `on` row rides `catch_on_miss_rows` —
                        // each keyed by the qualified `p.C` span the oracle
                        // anchors (`_getErrorRange`'s prefix-through-name
                        // default). A LOCAL prefix head keeps `Prefixed`:
                        // its miss stays silent in every position.
                        let catch_on =
                            self.catch_on_miss_rows.last().copied() == Some(range.offset);
                        if local
                            || (!suppressed && self.type_arg_depth == 0 && !catch_on)
                        {
                            self.push_row(Row {
                                name,
                                range,
                                qualified: true,
                                shape: RowShape::TypeUse(
                                    TypeVerdict::Prefixed { prefix, local },
                                    false,
                                ),
                                intent: Intent::Read,
                                write_op: None,
                                ty: Ty::UNKNOWN,
                            });
                        } else {
                            let head_range =
                                self.pending_prefixed.map(|(head, _)| head).unwrap_or(range);
                            let qualified = SourceRange {
                                offset: head_range.offset,
                                length: range.end() - head_range.offset,
                            };
                            if self.type_arg_depth > 0 {
                                self.type_arg_miss_rows.push(qualified.offset);
                            }
                            if catch_on
                                && let Some(key) = self.catch_on_miss_rows.last_mut()
                            {
                                // The listener armed the leaf offset; the
                                // context-miss lookup keys on the row's own
                                // offset — the qualified span's open.
                                *key = qualified.offset;
                            }
                            self.push_row(Row {
                                name,
                                range: qualified,
                                qualified: true,
                                shape: RowShape::TypeUse(TypeVerdict::Missing, suppressed),
                                intent: Intent::Read,
                                write_op: None,
                                ty: Ty::UNKNOWN,
                            });
                        }
                    }
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
    /// engine — EXCEPT inside a type-argument group, where the argument
    /// face wins the decision order, #2832), a multiply-defined element stays silent
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
        // routes to the referenced-before face and stays silent — except
        // inside a type-argument group, where the resolver's decision order
        // puts the argument face first (`_isTypeInTypeArgumentList` guards
        // `nonTypeAsTypeArgument` before the LocalVariableElement /
        // LocalFunctionElement arm, named_type_resolver.dart:659-691): a
        // method-instantiation or annotation argument naming any local
        // spelling reports `non_type_as_type_argument` (co19
        // ambiguities_A01_t01, `f(a<b, c>(d))` with `b`/`c` locals).
        if let Some(index) = self.resolve_local(name) {
            let decl = &self.locals[index as usize];
            return match decl.kind {
                LocalKind::Parameter { .. } => {
                    (TypeResolution::NonType, Some((self.file, decl.range)))
                }
                _ if self.type_arg_depth > 0 => {
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

    /// The `p` of `p.C`: whether the resolved element shadows the prefix
    /// slot — the oracle's `resolve()` importPrefix arm reports
    /// `prefixShadowedByLocalDeclaration` for ANY element that is neither a
    /// class-ish (`InterfaceElement`/`TypeAliasElement`, the
    /// `_rewriteToConstructorName` face), nor a `PrefixElement`, nor `null`
    /// (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart:85-128):
    /// locals and parameters, in-scope type parameters, members DECLARED on
    /// the enclosing container (the `enclosingClass.getMethod/getGetter/
    /// getSetter` fallback — declared only, no supertype climb), top-level
    /// functions and variables, `dynamic`, and extension names. An
    /// inherited member answers `null` there (the fallback never climbs)
    /// and reads as a miss instead.
    fn is_shadowed_prefix(&self, name: StringId, target: RefTarget) -> bool {
        // `dynamic` resolves through the built-in table — a
        // `DynamicElementImpl` upstream, never a prefix.
        if self.members.builtin_types.contains(&name) {
            return true;
        }
        match target {
            RefTarget::Local | RefTarget::TypeParam => true,
            RefTarget::Member { container } => {
                // Own-declared members only (statics included): the
                // container's declared member table answers the upstream
                // `enclosingClass` member fallback, whose getters stop at
                // the declaring container.
                self.container_context().is_some_and(|(enclosing, _)| {
                    enclosing == container
                        && self
                            .index
                            .get(self.file)
                            .and_then(|f| f.container(container))
                            .is_some_and(|c| c.members.get(&name).is_some())
                })
            }
            RefTarget::Prefix | RefTarget::Missing | RefTarget::This => false,
            _ => self.type_resolution(name).0 == TypeResolution::NonType,
        }
    }

    /// The continuation token of a `p.C` prefixed reference: the head's own
    /// token table successor is the period, and the token after it is `C`.
    /// `(name, span)` of `C`; `name` is `None` for a parser-inserted
    /// synthetic (an empty lexeme upstream's `isSynthetic` guard drops).
    fn prefixed_leaf(&self, head: SourceRange) -> (Option<StringId>, SourceRange) {
        let toks = self.tokens.as_slice();
        let at = toks.partition_point(|t| t.utf16_offset < head.end());
        let Some(cont) = toks.get(at + 1) else {
            return (None, head);
        };
        let text = &self.source[cont.byte_range()];
        let name = (!text.is_empty()).then(|| self.strings.intern(text));
        let range = SourceRange {
            offset: cont.utf16_offset,
            length: self.utf16_of(cont.byte_range().end) - cont.utf16_offset,
        };
        (name, range)
    }

    /// Whether the next real token after `end` is a period — the `p.` of a
    /// prefixed type or constructor reference (comments never occupy token
    /// slots, so the successor is direct).
    fn followed_by_period(&self, end: u32) -> bool {
        let toks = self.tokens.as_slice();
        let at = toks.partition_point(|t| t.utf16_offset < end);
        matches!(
            toks.get(at).map(|t| &t.kind),
            Some(dartq_scanner::TokenKind::Period)
        )
    }

    /// Arms the type-prefix slot for a `PrefixedTypeReference` head; a
    /// shadowed head reports `prefix_shadowed_by_local_declaration` at the
    /// head itself, and a miss resolves the whole `p.C` to `InvalidType` —
    /// the context-chain report lands over the written `p.C` span naming
    /// `C` (`reportNullOrNonTypeElement`, named_type_resolver.dart:596-728:
    /// `undefined_class`, `type_test_with_undefined_name`,
    /// `cast_to_non_type`, `non_type_as_type_argument`, or
    /// `non_type_in_catch_clause` — all routed by the pass-2 `Missing`
    /// arm's context rows). A live prefix head counts as a prefix use (the
    /// `unused_import` prefix face).
    pub(crate) fn arm_type_prefix(&mut self, name: StringId, range: SourceRange) {
        let (info, target) = self.resolve_bare(name);
        // A shadowed head never counts as a use of the shadowing element
        // (the oracle's `var p = 1; p.T q;` still reports `p` unused).
        let shadowed = self.is_shadowed_prefix(name, target);
        let missing = matches!(info, BareInfo::Missing { .. });
        let live = !missing && !shadowed;
        if live {
            self.used_names.insert(name);
        }
        self.type_prefix = Some(TypePrefix { prefix: name, live });
        if live {
            return;
        }
        if shadowed {
            let name_text = self.strings.resolve(name).to_string();
            self.emit(
                SemanticCode::PrefixShadowedByLocalDeclaration,
                range,
                &name_text,
                "",
            );
            return;
        }
        // Missing head: the whole `p.C` resolves to `InvalidType` upstream —
        // one context-chain report lands over `p.C` naming `C` (the element
        // is `null`, so `reportNullOrNonTypeElement`'s argument is the
        // node's own name — the continuation — while `_getErrorRange`
        // spans prefix-token through name-end). The `Missing` verdict rides
        // the same suppressed/context marks a bare head does, so `is`/`as`
        // operands, type-argument groups, and `catch` `on` types code
        // through the pass-2 context chain unchanged. A synthetic
        // continuation is the recovery face — no report (`isSynthetic`,
        // named_type_resolver.dart:597).
        let (leaf_name, leaf_range) = self.prefixed_leaf(range);
        let Some(leaf_name) = leaf_name else {
            return;
        };
        let suppressed = self.non_type_pos > 0;
        if self.type_arg_depth > 0 {
            self.type_arg_miss_rows.push(range.offset);
        }
        if self.record_type_depth == 0
            && self.catch_clause_start.is_some()
            && self.catch_params == 0
            && self.catch_on_type.is_none()
        {
            self.catch_on_type = Some(leaf_name);
            self.catch_on_miss_rows.push(range.offset);
        }
        self.push_row(Row {
            name: leaf_name,
            range: SourceRange {
                offset: range.offset,
                length: leaf_range.end() - range.offset,
            },
            qualified: true,
            shape: RowShape::TypeUse(TypeVerdict::Missing, suppressed),
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        });
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
        // The three-segment `unresolved.Class<int>.new` spans the head's
        // miss through the class segment's own end (the `?` generic tail is
        // not part of the resolved type name — the oracle reports
        // `unresolved.Class` at |16, never through `<int>` at |21). The
        // latched `creation_ref` selector IS that class token for the
        // three-segment shape; a two-segment `Missing.new` latched `new`
        // instead, and its `.new` rode the qualified rest so the period
        // remains the end. The importPrefix arm upstream reports
        // `prefix.C` through the class-name token (named_type_resolver.dart's
        // `diagnostic.at(node.name)` — the `name` is the class segment).
        let end = match self.creation_ref {
            Some((_, Some((_, class_range)))) => class_range.end(),
            _ => period.range.offset,
        };
        self.requalify_creation_head_to(start, end);
    }

    /// [`Engine::requalify_creation_head`] with a written span end — the
    /// two-segment qualified reference (`Missing.new`) spans through the
    /// selector's name, its `.new` having ridden the qualified rest.
    ///
    /// Only a head whose own element lookup answers `null` takes the
    /// `undefined_identifier`-over-the-span verdict: the upstream
    /// `resolveConstructorTypeReference` importPrefix arm resolves the
    /// head's first segment in the name scope and reports the
    /// `undefined_identifier` face only when it is missing outright
    /// (named_type_resolver.dart:566-590). A head that resolves to a
    /// NON-TYPE element (`const NonType<int>.named()`'s function head) keeps
    /// its `Missing` verdict — the creation context then reports
    /// `creation_with_non_type` at the head token, exactly the oracle's
    /// `reportNullOrNonTypeElement` tail. The `resolve_bare` probe asks the
    /// same scope question the official element lookup does.
    pub(crate) fn requalify_creation_head_to(&mut self, start: Token, end: u32) {
        let offset = start.range.offset;
        let Some(index) = self.rows.iter().rposition(|row| {
            row.range.offset == offset && matches!(row.shape, RowShape::TypeUse(..))
        }) else {
            return;
        };
        if let Some(head) = self.text_of(start) {
            let head = self.intern(head);
            if !matches!(self.resolve_bare(head).0, BareInfo::Missing { .. }) {
                return;
            }
        }
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

/// The byte offset of the bracket matching the opener at `open` — a
/// depth walk over `()[]{}` with string contents skipped (the forward
/// twin of the literal faces' back walk). `None` when unbalanced.
fn matching_bracket_fwd(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'\'' | b'"' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Top-level comma segments of `[start, end)`: a depth walk over
/// `()[]{}` with string contents skipped — the same split the literal
/// element faces run at a direct operand.
fn value_comma_segments(bytes: &[u8], start: usize, end: usize) -> Vec<(usize, usize)> {
    let end = end.min(bytes.len());
    let mut segments = Vec::new();
    let mut seg_start = start;
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                segments.push((seg_start, i));
                seg_start = i + 1;
            }
            b'\'' | b'"' => {
                let quote = bytes[i];
                i += 1;
                while i < end && bytes[i] != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    segments.push((seg_start, end));
    segments
}

/// The first top-level `:` in `[start, end)`, string contents skipped —
/// the map-entry leaf's key/value split.
fn value_top_level_colon(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let end = end.min(bytes.len());
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => return Some(i),
            b'\'' | b'"' => {
                let quote = bytes[i];
                i += 1;
                while i < end && bytes[i] != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The end of a formal segment's signature part: the first top-level `=`
/// (the default value's head), or `end`.
fn value_default_cut(bytes: &[u8], start: usize, end: usize) -> usize {
    let end = end.min(bytes.len());
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'=' if depth == 0 => return i,
            b'\'' | b'"' => {
                let quote = bytes[i];
                i += 1;
                while i < end && bytes[i] != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    end
}

/// The first significant byte at or after `start`.
fn trim_value_ws(bytes: &[u8], start: usize, end: usize) -> usize {
    let end = end.min(bytes.len());
    let mut start = start.min(end);
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    start
}

/// The implicit-`this` extension-application verdicts of
/// [`Engine::this_extension_member_applies`], with every face declared in an
/// **imported** library: the bare name must then resolve through the
/// extension pool (`resolve_bare`'s same-file `has_extension_member` guard
/// stays out of the way), so the oracle's application verdict is the only
/// decision under test.
///
/// Oracle provenance: `dart analyze` 3.13.1 over the two-file shapes below —
/// `extension OnNullable on C? { int get extra => 42; }` read from inside
/// another `on C?` body is silent, the same read through a non-nullable
/// `on C` face reports `unchecked_use_of_nullable_value` +
/// `undefined_identifier`, `extension Unbounded<T> on T` applies to a
/// nullable `this` and to an ordinary class body alike, and
/// `extension Bounded<T extends C> on T` rejects both the nullable `C?`
/// spelling and a class outside `C`.
#[cfg(test)]
mod implicit_this_extension_tests {
    use super::test_rig::{RigImport, code_names, report_ops_imports};

    const FACES: &str = "class C {}\n\
         extension OnNullable on C? {\n\
         \x20 int get extra => 42;\n\
         \x20 void poke() {}\n\
         }\n\
         extension OnPlain on C {\n\
         \x20 int get plain => 7;\n\
         }\n\
         extension Unbounded<T> on T {\n\
         \x20 int get mem => 0;\n\
         }\n\
         extension Bounded<T extends C> on T {\n\
         \x20 int get bmem => 0;\n\
         }\n\
         extension NullableBound<T extends C?> on T {\n\
         \x20 int get nmem => 0;\n\
         }\n";

    /// Diagnoses `source` (the main unit) against the shared face library.
    fn report(source: &str) -> super::FileReport {
        report_ops_imports(
            source,
            &[RigImport {
                uri: "package:a/faces.dart",
                prefix: None,
                source: FACES,
            }],
        )
    }

    fn has(report: &super::FileReport, code: &str) -> bool {
        code_names(report).contains(&code)
    }

    fn assert_silent_on(report: &super::FileReport, codes: &[&str]) {
        let names = code_names(report);
        for code in codes {
            assert!(!names.contains(code), "unexpected {code}: {names:?}");
        }
    }

    fn assert_reports(report: &super::FileReport, codes: &[&str]) {
        let names = code_names(report);
        for code in codes {
            assert!(names.contains(code), "missing {code}: {names:?}");
        }
    }

    /// A written `on C?` face applies to a nullable `this`: the property
    /// read resolves, so neither the lexical miss nor the nullable
    /// dereference reports.
    #[test]
    fn nullable_on_face_applies_to_nullable_this() {
        let report = report("import 'package:a/faces.dart';\n\
             extension E on C? {\n\
             \x20 void g() {\n\
             \x20   extra;\n\
             \x20 }\n\
             }\n");
        assert_silent_on(
            &report,
            &["undefined_identifier", "unchecked_use_of_nullable_value"],
        );
    }

    /// The same law for an invocation member.
    #[test]
    fn nullable_on_face_method_applies_to_nullable_this() {
        let report = report("import 'package:a/faces.dart';\n\
             extension E on C? {\n\
             \x20 void g() {\n\
             \x20   poke();\n\
             \x20 }\n\
             }\n");
        assert_silent_on(
            &report,
            &["undefined_identifier", "undefined_method", "unchecked_use_of_nullable_value"],
        );
    }

    /// An unbounded `on T` face instantiates to the nullable receiver (the
    /// parameter's implicit `Object?` bound admits `C?`).
    #[test]
    fn unbounded_on_param_face_applies_to_nullable_this() {
        let report = report("import 'package:a/faces.dart';\n\
             extension E on C? {\n\
             \x20 void g() {\n\
             \x20   mem;\n\
             \x20 }\n\
             }\n");
        assert_silent_on(
            &report,
            &["undefined_identifier", "unchecked_use_of_nullable_value"],
        );
    }

    /// A nullable written bound (`T extends C?`) admits the nullable
    /// receiver — the bound its own `?` spelling satisfies.
    #[test]
    fn nullable_bound_on_param_face_applies_to_nullable_this() {
        let report = report("import 'package:a/faces.dart';\n\
             extension E on C? {\n\
             \x20 void g() {\n\
             \x20   nmem;\n\
             \x20 }\n\
             }\n");
        assert_silent_on(
            &report,
            &["undefined_identifier", "unchecked_use_of_nullable_value"],
        );
    }

    /// A non-nullable `on C` face never applies to a nullable `this` (`C?`
    /// is not a subtype of `C`): the dereference reports unchecked and the
    /// name stays undefined.
    #[test]
    fn non_nullable_on_face_rejects_nullable_this() {
        let report = report("import 'package:a/faces.dart';\n\
             extension E on C? {\n\
             \x20 void g() {\n\
             \x20   plain;\n\
             \x20 }\n\
             }\n");
        assert_reports(
            &report,
            &["undefined_identifier", "unchecked_use_of_nullable_value"],
        );
    }

    /// A non-nullable written bound (`T extends C`) rejects the nullable
    /// receiver the same way.
    #[test]
    fn non_nullable_bound_on_param_face_rejects_nullable_this() {
        let report = report("import 'package:a/faces.dart';\n\
             extension E on C? {\n\
             \x20 void g() {\n\
             \x20   bmem;\n\
             \x20 }\n\
             }\n");
        assert_reports(
            &report,
            &["undefined_identifier", "unchecked_use_of_nullable_value"],
        );
    }

    /// An unbounded `on T` face applies to an ordinary non-nullable `this`
    /// too: the parameter instantiates to the receiver's type.
    #[test]
    fn unbounded_on_param_face_applies_to_class_this() {
        let report = report("import 'package:a/faces.dart';\n\
             class D {\n\
             \x20 void m() {\n\
             \x20   mem;\n\
             \x20 }\n\
             }\n");
        assert_silent_on(&report, &["undefined_identifier"]);
    }

    /// The parameter's written bound still orders real receivers: a class
    /// outside `C` applies no face, so the bare name stays undefined.
    #[test]
    fn bounded_on_param_face_rejects_out_of_bound_class_this() {
        let report = report("import 'package:a/faces.dart';\n\
             class D {\n\
             \x20 void n() {\n\
             \x20   bmem;\n\
             \x20 }\n\
             }\n");
        assert_reports(&report, &["undefined_identifier"]);
    }

    /// A bound subtype reaches the bounded face: the reader class is
    /// irrelevant — the *member* the face declares resolves the bare name
    /// through the receiver's bound relation.
    #[test]
    fn unbounded_on_param_face_applies_to_bound_subtype_class_this() {
        let report = report("import 'package:a/faces.dart';\n\
             class Decl extends C {\n\
             \x20 void m() {\n\
             \x20   mem;\n\
             \x20 }\n\
             }\n");
        assert_silent_on(&report, &["undefined_identifier"]);
    }
}
