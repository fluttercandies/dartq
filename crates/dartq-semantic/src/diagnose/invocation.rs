use super::*;

/// Which associated parameter one parked `super.name` sentinel reads: the
/// forward's own index among the constructor's super formals, or the named
/// entry's spelling.
#[derive(Clone, Copy)]
enum SuperQuery {
    Positional(usize),
    Named(StringId),
}

impl<'a> Engine<'a> {
    /// Merges `receiver.member` at the dot/cascade event: pops the member row,
    /// classifies the receiver below it, and pushes the merged row. A
    /// null-aware access on a strictly non-nullable receiver emits
    /// `invalid_null_aware_operator` (a `STATIC_WARNING` — the receiver can
    /// never be null, `type_system.dart:1921-`). A null-aware access on a
    /// nullable local's read latches the dead-chain candidate (the
    /// `_verifyUnassignedVariable` face — resolved at teardown against the
    /// definite-unassignment facts).
    pub(crate) fn merge_member(
        &mut self,
        is_null_aware: bool,
        op_range: SourceRange,
        chain_end: u32,
    ) {
        // F-H arg-window displacement: a completed send's callee placeholder
        // is the member row this merge must fold, but recovery shapes can
        // leave the argument window's rows above it — every row above the
        // callee then lies inside the completed send, so the top row ends
        // before the chain end AND before this merge's operator (a row
        // ending after the operator is the current chain's own member-name
        // row, never a displaced argument). Drain the window above the
        // latched callee so the normal merge folds [callee placeholder,
        // receiver below]: its unresolved rows (`Bare(Missing)`/`TypeUse`/
        // intermediate `Member`) defer to pass 2 through the landed
        // `deferred_miss_rows` re-entry, resolved rows stay (the known
        // under-report boundary). Geometric matches take the original path
        // untouched; the latch is single-shot.
        
        if let Some((index, send_end)) = self.last_send_callee.take() {
            let displaced = self.rows.get(index).is_some_and(|row| {
                matches!(row.shape, RowShape::Member { .. })
                    && row.range.end() <= op_range.offset
                    && index + 1 < self.rows.len()
            }) && self
                .rows
                .last()
                .is_some_and(|top| top.range.end() <= send_end);
            if displaced {
                for row in self.rows.drain(index + 1..) {
                    if matches!(
                        row.shape,
                        RowShape::Bare(BareInfo::Missing { .. })
                            | RowShape::TypeUse(..)
                            | RowShape::Member { .. }
                    ) {
                        // Stranded-row tripwire: an honest argument-window
                        // row lies inside the completed send, so it ends at
                        // or before the send's own closing token. A drained
                        // row that outlives that bound is a later
                        // expression's own row — the d1b3378f theft class —
                        // and must surface here, not vanish into a wrong
                        // fold.
                        debug_assert!(row.range.end() <= send_end);
                        self.deferred_miss_rows.push(row);
                    }
                }
                // Argument-window entries pointing into the drained span
                // lose their rows; a stale index could only misattribute
                // the arity check, so the entries drop (under-report).
                self.deferred_args.retain(|(row, _, _)| *row <= index);
            }
        }
        let Some(mut member) = self.rows.pop() else {
            return;
        };
        // A member send's explicit type arguments (`c.m<T>(...)`) leave their
        // `TypeUse` rows stacked above the member-name row: the send folds
        // nothing (`open_arguments` links no callee behind the group's
        // trailing rows), so without this drain the merge below would fold
        // the type argument AS the member and the name row AS its receiver.
        // The oracle resolves every written type argument's name in place
        // whatever the callee's resolution (`named_type_resolver.dart`'s
        // `_isTypeInTypeArgumentList` — the argument's parent is a
        // `TypeArgumentList` either way), so each row defers to pass 2
        // (its `type_arg_miss_rows` entry rides) and the real member row
        // merges against its receiver.
        let mut type_arg_rows: Vec<Row> = Vec::new();
        while matches!(member.shape, RowShape::TypeUse(..))
            && let Some(next) = self.rows.pop()
        {
            type_arg_rows.push(member);
            member = next;
        }
        if !type_arg_rows.is_empty() {
            // The invoked spelling (`c.m<T>(...)`) keeps the invocation
            // intent on the member row: the send's own fold skipped it (no
            // callee link behind the group), and pass 2's member-miss face
            // picks the code from the intent (`undefined_method`, never the
            // property read's `undefined_getter` — probed
            // `b.noSuch<Missing>()` vs oracle). A tear-off application
            // (`c.m<T>;`) keeps the read intent.
            let after = type_arg_rows.last().expect("checked non-empty").range.end();
            let mut next = self.skip_whitespace(after);
            while self.byte_at(next) == Some(b'>') {
                next = self.skip_whitespace(next + 1);
            }
            if self.byte_at(next) == Some(b'(') {
                member.intent = Intent::Invoke;
            }
            // The merge removes every row at or above the member row's slot
            // (the member and receiver pops below replace the receiver's
            // slot with the merged row), so entries anchored there lose
            // their rows — the F-H drain's same under-report discipline. A
            // stale entry would otherwise re-ground on a LATER send's
            // callee and check the dead group's count against an unrelated
            // method (the `_isValidContext` misattribution).
            let first_removed = self.rows.len();
            let boundary = self
                .rows
                .get(first_removed)
                .map_or(u32::MAX, |row| row.range.offset);
            self.deferred_args
                .retain(|(row, _, _)| *row < first_removed);
            self.call_type_args.retain(|(offset, _)| *offset < boundary);
            self.deferred_miss_rows.append(&mut type_arg_rows);
        }
        // The `.{`/`?.{` operand recovery (`AstBuilder.doDotExpression`):
        // the operand parses as a set-literal Value row anchored at `{` —
        // the oracle instead resolves a member named `{` at that token
        // (the recovered `self.{` surfaces the nullable-dereference face,
        // `self?.{` an `undefined_getter` `'{'`). Rename and re-anchor the
        // folded row so pass 2 resolves `{` against the receiver. A
        // cascade section owns its own verdict; a receiver row (the
        // recovery left no operand row) never renames.
        if !self.in_cascade
            && matches!(member.shape, RowShape::Value)
            && member.range.offset > op_range.offset
            && self.byte_at(member.range.offset) == Some(b'{')
        {
            member.name = self.intern("{");
            member.range = SourceRange {
                offset: member.range.offset,
                length: 1,
            };
        }
        // The `?.[`/`?.(` operand recovery (`AstBuilder.doDotExpression`):
        // the operand parses as a complete expression (`[0]` lands its own
        // value row plus its inner element rows), so the receiver is the
        // last row ending before the operator — not the row the operand's
        // contents left on top.
        let member_is_value = matches!(member.shape, RowShape::Value);
        let bracket_operand = is_null_aware
            && !self.in_cascade
            && member_is_value
            && self.bracket_after(op_range.end()).is_some();
        // `x?.m(args)` / `x?..m(args)`, and their plain twins `x.m(args)` /
        // `a..b(args)`: the member's send completes before the access merge
        // (`parse_send_from_identifier` parses the argument list inside
        // `parse_primary`, so the dot event fires after the send), and its
        // argument/result rows sit above the member-name row
        // (`C.named('s')` stacks `[C, named, 's']`). The name row below the
        // value row is the member; the receiver is the last row ending
        // before the operator (#1758 for the null-aware spellings —
        // otherwise the verdict reads the member's own send and never
        // fires; the plain spellings misfold the same way, re-anchoring
        // the send's argument rows onto the corrupted member row).
        let invoked_member = member_is_value
            && !bracket_operand
            && self
                .rows
                .last()
                .is_some_and(|row| row.range.offset > op_range.offset);
        let popped_receiver = if bracket_operand {
            self.rows
                .iter()
                .rposition(|row| row.range.end() <= op_range.offset)
                .map(|index| self.rows.remove(index))
        } else if invoked_member {
            // Remove the receiver first, then the member-name row that
            // followed it — any argument rows between them stay (the send's
            // own machinery owns them).
            self.rows
                .iter()
                .rposition(|row| row.range.end() <= op_range.offset)
                .and_then(|index| {
                    let receiver = self.rows.remove(index);
                    // The member's name row sat immediately above the
                    // receiver. Rows that end before the operator yet start
                    // at or before it belong to no chain node — a head
                    // group's stranded type-argument rows (#1765's d1b3378f
                    // class) — and they break the adjacency read below.
                    // Drain them into the fold-deferred set (their own
                    // resolution keeps its emission through the offset
                    // interleave) before retrying the name row; a healthy
                    // geometry finds the name row on the first read and
                    // drains nothing.
                    if self
                        .rows
                        .get(index)
                        .is_none_or(|row| row.range.offset <= op_range.offset)
                    {
                        let mut stranded = 0;
                        while self
                            .rows
                            .get(index + stranded)
                            .is_some_and(|row| row.range.end() <= op_range.offset)
                        {
                            stranded += 1;
                        }
                        if stranded > 0 {
                            for row in self.rows.drain(index..index + stranded) {
                                if matches!(
                                    row.shape,
                                    RowShape::Bare(BareInfo::Missing { .. })
                                        | RowShape::TypeUse(..)
                                        | RowShape::Member { .. }
                                ) {
                                    self.deferred_miss_rows.push(row);
                                }
                            }
                            // Rows above the drained span shift down; any
                            // argument entry pointing past the receiver
                            // loses its row (the F-H drain's same
                            // under-report discipline).
                            self.deferred_args.retain(|(row, _, _)| *row <= index);
                        }
                    }
                    if let Some(name_row) = self.rows.get(index).cloned() {
                        if name_row.range.offset > op_range.offset {
                            self.rows.remove(index);
                            member.name = name_row.name;
                            member.range = name_row.range;
                            member.intent = name_row.intent;
                        }
                    }
                    Some(receiver)
                })
        } else {
            self.rows.pop()
        };
        // S4-B cascade-target collapse: after a `..x = rhs` section the RHS's
        // leftover row sits on the stack top, so the next section's
        // `popped_receiver` is that RHS (`C()..p = 1..q = true` reads `q`'s
        // receiver as `int`). Upstream resolves EVERY section against
        // `cascade.target2`'s static type (`cascadeReceiver`,
        // type_analyzer.dart:611-618). `member_reads`'s last entry — when it
        // belongs to this cascade (its selector offset was already pushed
        // into `cascade_sections` by `handle_cascade_access`) — carries the
        // latched target `Receiver`, substituted in below.
        // The latch fires only when the last read belongs to THIS cascade
        // (same `cascadeExpression` node): two sections of one cascade are
        // chained by `..` with no `;` between them — a statement separator
        // can never appear inside a single expression, so any `;` in the
        // span proves a fresh cascade (`C()..p = 1; D()..q = true;` keeps
        // its own real receiver `D`), while a bare-identifier RHS
        // (`..version = version`, a `Bare` leftover) collapses exactly like
        // a literal `Value` one.
        // Only a section selector (`..m` / `?..m`) resolves against the
        // latched target: a plain `.` access inside a section's argument
        // list (`..writeln(argParser.usage)`) merges with its own bare
        // receiver row, which is no leftover — latching it read `usage`
        // against the cascade target (`StringBuffer`) and reported every
        // such argument undefined.
        let is_section = {
            let at = self.byte_of(op_range.offset);
            let rest = &self.source.as_bytes()[at..];
            rest.starts_with(b"..") || rest.starts_with(b"?..")
        };
        let cascade_recv = (self.in_cascade && is_section)
            .then(|| {
                // The latched target read is the LAST CASCADE-SECTION read, not
                // `member_reads`' plain tail: a member-read RHS (`..p =
                // original.p`) records its own read AFTER the section's, so the
                // tail answers the RHS's face and the next section resolves
                // against the RHS type (`..followRedirects =
                // original.followRedirects` then reports `..maxRedirects`
                // against `bool`). Scanning back to the newest read whose
                // selector offset is in `cascade_sections` answers the section's
                // own record; the walk is short in practice (the entries past a
                // section's selector are just its RHS's own reads).
                self.member_reads
                    .iter()
                    .rev()
                    .find(|read| self.cascade_sections.contains(&read.offset))
            })
            .flatten()
            .filter(|_| {
                // A chained access ON a section (`..headers.addAll(x)`) pops the
                // section's own selector row — that send must resolve against
                // the section's member type, never the latched target. An RHS
                // leftover never sits at a section offset, so the popped row's
                // own offset answers which shape this merge is.
                popped_receiver
                    .as_ref()
                    .is_none_or(|row| !self.cascade_sections.contains(&row.range.offset))
            })
            .filter(|last| {
                // A `;` inside the popped RHS's own span (a `"a;b"` literal) is
                // part of the expression, not a boundary — the separator probe
                // ends where the leftover row begins.
                let start = self.byte_of(last.offset);
                let end = popped_receiver
                    .as_ref()
                    .map(|row| self.byte_of(row.range.offset))
                    .unwrap_or_else(|| self.byte_of(member.range.offset));
                !self.source.as_bytes()[start.min(end)..end].contains(&b';')
            })
            .map(|last| last.recv);
        let receiver_offset = popped_receiver
            .as_ref()
            .map(|r| r.range.offset)
            .or_else(|| self.rows.last().map(|r| r.range.offset));
        // The merged head's own identity — a redirecting factory's `= C.name`
        // reference needs the container spelling even when the receiver
        // classifies as Unknown (the `ConstructorReference` head pushes a
        // TypeUse row, whose member-merge receiver is Unknown).
        let popped_head = popped_receiver.as_ref().map(|r| (r.name, r.range));
        // The receiver's read span and local verdict, captured for the
        // staged null-aware faces before the hit moves onto the member row.
        let mut recv_span = SourceRange {
            offset: 0,
            length: 0,
        };
        let mut was_local = false;
        // The receiver's expression static type (`receiver_static_ty`
        // reads the row's postfix-upgraded face — `x!?.m` reads non-null):
        // the null-aware check below decides against it, not the shape's
        // push-time payload.
        let mut merged_receiver_ty = Ty::UNKNOWN;
        let recv = match popped_receiver {
            Some(mut popped) => {
                // The parenthesized group's bool collapse: `(x as dynamic)`'s
                // `as` fold leaves its `Value` result row at the operand's
                // exact span (a `dynamic`/`bool` head pushes no `TypeUse`
                // row to extend it), which the cast's own boolish-target
                // latch then covers — the group row reads `bool` where
                // `ParenthesizedExpression.staticType` is the operand's own
                // type. The folded `Value` row still sits below the group
                // row (nested group rows included) with the operand's real
                // static type — `dynamic` among them — so the receiver
                // classification and the never-junction read it instead of
                // the collapsed `bool` (the oracle's
                // `type_property_resolver` routes `dynamic` to the
                // no-lookup dispatch).
                if !popped.qualified
                    && self.strings.resolve(popped.name).is_empty()
                    && matches!(
                        popped.shape,
                        RowShape::Bare(BareInfo::Plain {
                            ty: Ty::Named { name, .. }
                        }) if name == self.members.primitive_ids[3]
                    )
                {
                    let mut index = self.rows.len();
                    while index > 0 {
                        let row = &self.rows[index - 1];
                        let group_row = !row.qualified
                            && matches!(row.shape, RowShape::Bare(BareInfo::Plain { .. }))
                            && self.strings.resolve(row.name).is_empty();
                        if !group_row {
                            break;
                        }
                        index -= 1;
                    }
                    if let Some(inner) = index.checked_sub(1).and_then(|i| self.rows.get(i))
                        && matches!(inner.shape, RowShape::Value)
                        && inner.ty != Ty::UNKNOWN
                    {
                        popped.ty = inner.ty;
                        popped.shape = RowShape::Bare(BareInfo::Plain { ty: inner.ty });
                    }
                }
                // The never-receiver junction (`receiverOfTypeNever`): the
                // receiver row is in hand exactly once, here — every
                // member face (getter read, setter target, method
                // invocation, cascade section) folds through this pop.
                let receiver_ty = self.receiver_static_ty(&popped);
                merged_receiver_ty = receiver_ty;
                self.stage_receiver_of_type_never(popped.range, receiver_ty);
                recv_span = popped.range;
                was_local = self.local_hits.contains_key(&recv_span.offset);
                // The dead-chain candidate: a null-aware operator over a
                // local's nullable read (`c1?.g()`, `c1?..g()`).
                if is_null_aware && was_local && Self::is_nullable_ty(receiver_ty) {
                    self.dead_null_chains
                        .push((op_range.offset, chain_end, recv_span));
                }
                // The receiver's read counts for the local it resolved to
                // (`v.m` reads `v`): the member row inherits the hit. The
                // decl-keyed twin rides the move while the frame is still
                // open (the pass-2 re-bind lookups key on the
                // declaration's name-token offset — the scoped index
                // aliases once the frames close).
                if let Some(hit) = self.local_hits.remove(&popped.range.offset) {
                    self.local_hits.insert(member.range.offset, hit);
                    if let Some(decl) = self.locals.get(hit as usize) {
                        self.local_decl_at_member
                            .insert(member.range.offset, decl.range.offset);
                    }
                }
                // The receiver's own reference marks its name used (the
                // official `members` face, and `unused_import`'s
                // hasPrefixUsedWithoutName face for a prefix receiver):
                // the row folds into the member row, so pass 2 never
                // resolves it. A bare field name folded here is always a
                // field READ (`_subscription.cancel()`, `_subscription!.x`
                // — reaching the member requires the field's value), so it
                // joins `read_names` for the `unused_field` faces: the
                // folded row's own pass-2 read never runs.
                if matches!(popped.shape, RowShape::Bare(_))
                    && !self.self_scope_rows.contains(&popped.range.offset)
                {
                    self.used_names.insert(popped.name);
                    if let Some(&pushed) = self.field_read_rows.get(&popped.range.offset) {
                        self.read_names.insert(popped.name);
                        // The element-scoped twin: a bare field read is a
                        // this-scope read of its owning container's member.
                        if let Some(container) = self
                            .read_owner_container(popped.range.offset)
                            .or(Some(pushed))
                        {
                            self.member_read_owners
                                .entry(popped.name)
                                .or_default()
                                .insert((self.file, container));
                        }
                    }
                }
                // The unresolved-receiver face: a missing bare head and an
                // intermediate member row still owe their own resolution
                // after the fold — the official resolves every chain node
                // (`visitPrefixedIdentifier`'s prefix miss, each
                // PropertyAccess's own lookup), so the row joins the
                // deferred set instead of dying here. A primitive-spelled
                // receiver (`double.nan`) resolves as a container instead.
                let popped_shape = popped.shape;
                // The promoted-receiver candidate (#1465): a bare
                // declared-type read's member re-decides in pass 2 against
                // the flow read type at this span — the merged row keeps the
                // push-time classification until then. A cascade SECTION
                // selector takes no latch (the chained-read branch below
                // documents the same rule): its "receiver" row is the
                // previous section's RHS leftover, and pass 2's promoted
                // re-decision would re-type the member against the RHS's
                // declared face (`..version = version` → `String`) where
                // upstream resolves the latched cascade target
                // (type_analyzer.dart:611-618). A plain `.` merge inside a
                // section (`..markOpened(file.path, ...)` — the argument
                // list is ordinary expression context) is NOT a section
                // merge: its receiver is a genuine bare read, and skipping
                // its latch froze the push-time nullable face (the promoted
                // local read inside cascade arguments reported
                // `unchecked_use_of_nullable_value` where the oracle stays
                // silent — the promotion fact exists at the read; only the
                // re-decision latch was gated on `in_cascade`).
                if matches!(popped_shape, RowShape::Bare(BareInfo::Plain { .. }))
                    && !(self.in_cascade && is_section)
                    // The invoked callee's span describes the callee VALUE's
                    // declared type in the flow table, never the send result
                    // — a promotion re-decision there would un-type the
                    // implicit-`call` chain back to the callee's own class
                    // (`body(ctx).m` re-read from `body`'s field type,
                    // census A4).
                    && popped.intent != Intent::Invoke
                {
                    let member_offset = member.range.offset;
                    if let Err(index) = self
                        .promoted_recv_reads
                        .binary_search_by(|(member, _)| member.cmp(&member_offset))
                    {
                        self.promoted_recv_reads
                            .insert(index, (member_offset, recv_span));
                    }
                }
                // The FIRST cascade section's selector re-reads the cascade
                // target (`builder..a = x`'s `..a` reads `builder` exactly
                // as a plain access does): it latches on the target's own
                // read span, so the flow's promotion of the target read
                // answers the receiver verdict (`var builder = _b;
                // if (builder != null) builder..a = x` stays clean,
                // ast_builder.dart:1349). Later sections keep the latched
                // cascade-target classification (S4-B below) — keying them
                // here would race the leftover receiver's own re-decision
                // (the previous section's RHS read), which types the
                // section against the RHS's face.
                if self.in_cascade
                    && is_section
                    && cascade_recv.is_none()
                    && let Some(target_span) = self.cascade_target_spans.last().copied()
                {
                    let member_offset = member.range.offset;
                    if let Err(index) = self
                        .promoted_recv_reads
                        .binary_search_by(|(member, _)| member.cmp(&member_offset))
                    {
                        self.promoted_recv_reads
                            .insert(index, (member_offset, target_span));
                    }
                }
                // The chained-read candidate (#2283): a member folded over a
                // promotion-latched member receiver freezes its receiver
                // classification from the UNPROMOTED result face (`a.value`
                // over an `a is Sub` promotion types `.isNaN` on the
                // declared getter's `Object`, never the promoted field's
                // `double` — pass 1 cannot see the flow promotion), so this
                // row's pass-2 verdict owes the same re-derivation its
                // receiver got. A cascade SECTION selector takes no latch:
                // its receiver re-classifies to the latched cascade target
                // (S4-B), a face no chain link describes — a plain `.`
                // merge inside a section's arguments takes the latch like
                // any other chain link (the same section-scoped rule as the
                // bare-read latch above). The zero-length
                // span marks the chained form — read-type lookups compare
                // the full range and never hit it, so pass 2 keeps the
                // frozen row byte for byte — while its offset keys the
                // receiver read's own entry for the teardown settlement
                // (`finalize_redirects`' tail).
                if matches!(popped_shape, RowShape::Member { .. })
                    && !(self.in_cascade && is_section)
                    && self
                        .promoted_recv_reads
                        .binary_search_by(|(row, _)| row.cmp(&popped.range.offset))
                        .is_ok()
                {
                    let member_offset = member.range.offset;
                    if let Err(index) = self
                        .promoted_recv_reads
                        .binary_search_by(|(row, _)| row.cmp(&member_offset))
                    {
                        self.promoted_recv_reads.insert(
                            index,
                            (
                                member_offset,
                                SourceRange {
                                    offset: popped.range.offset,
                                    length: 0,
                                },
                            ),
                        );
                    }
                }
                let deferred_receiver = match &popped_shape {
                    RowShape::Bare(BareInfo::Missing { .. }) => {
                        !self.members.primitive_ids.contains(&popped.name)
                    }
                    RowShape::Member { .. } => true,
                    // A type-head receiver that merged into a member shape
                    // (`case A.`'s synthetic continuation) still owes its
                    // own miss row — pass 2 resolves it through
                    // `resolve_type_row`.
                    RowShape::TypeUse(..) => true,
                    _ => false,
                };
                let recv = match &popped_shape {
                    // A dart:core primitive spelled as the receiver
                    // (`double.nan`, `int.parse`): no indexed container
                    // answers the bare name, but it IS the class-static face
                    // the member resolves against — the official reads the
                    // resolved getter's enclosing element
                    // (`isDoubleNan`, best_practices_verifier.dart:2264-2277).
                    // The unresolvable-container dispatch stays silent, so
                    // the row only feeds type-shape faces.
                    RowShape::Bare(BareInfo::Missing { .. })
                        if self.members.primitive_ids.contains(&popped.name) =>
                    {
                        Receiver::Container {
                            name: popped.name,
                            kind: ContainerKind::Class,
                        }
                    }
                    // The constructor-reference continuation (`C<int>.new`,
                    // the const-pattern selector): upstream lets `new` name
                    // the unnamed constructor in a continuation context
                    // (`allowsNewAsIdentifier`, upstream Dart SDK source,
                    // pkg/_fe_analyzer_shared/lib/src/parser/
                    // identifier_context_impl.dart:391) — the selector
                    // resolves against the type head's container, not as a
                    // property on an invalid receiver. An unresolvable head
                    // keeps the Unknown dispatch (its type-miss row already
                    // reports).
                    RowShape::TypeUse(..) if member.name == self.intern("new") => {
                        Receiver::Container {
                            name: popped.name,
                            kind: ContainerKind::Class,
                        }
                    }
                    // A postfix-upgraded receiver carries its static type
                    // on the row (`x!.m`'s `!` rewrite in
                    // [`Self::check_null_assert`]): the `Plain` payload is
                    // the declared face, so the row's own type wins for the
                    // `Typed` receiver when it names the same container —
                    // `command!.subcommands` resolves on `C`, not `C?`
                    // (#2161).
                    //
                    // The invoked callee (`body(ctx).m`) is the completed
                    // send's anchor, so its receiver face is the SEND's
                    // static type — `receiver_static_ty`'s row.ty-first
                    // answer, where the implicit-`call` retype wrote the
                    // call method's return — never the bare payload, which
                    // spells the callee VALUE's own class and would resolve
                    // `.m` there (census A4). A result-less send keeps the
                    // value-face dispatch.
                    RowShape::Bare(info) if popped.intent != Intent::Invoke => {
                        match Engine::receiver_of_bare(*info) {
                            Receiver::Typed { type_name, .. }
                                if matches!(
                                    receiver_ty,
                                    Ty::Named { name, .. } if name == type_name
                                ) =>
                            {
                                Receiver::Typed {
                                    type_name,
                                    ty: receiver_ty,
                                }
                            }
                            // An information-less bare head whose spelling
                            // resolves to an SDK declaration is a class-static
                            // face on an unindexed (SDK synthetic) container
                            // (`RangeError.checkValidRange(...)`): arm the
                            // container so the member's send types against the
                            // declared static table instead of the Unknown
                            // dispatch — whose positional recovery cannot serve
                            // the merged row (the receiver row is folded away
                            // here, and the send tiles the merged row's range
                            // past it). SDK-only: a user-declared spelling
                            // (a top-level function head, a same-spelled class)
                            // keeps the Unknown dispatch whose resolve arm the
                            // cascade/unused-result faces consult, and a local
                            // read never arms (the local shadows any
                            // same-spelled declaration). An unresolved spelling
                            // keeps Unknown too — its own miss row already
                            // reports.
                            Receiver::Unknown
                                if !was_local
                                    && matches!(
                                        self.namespaces.lookup(self.graph, self.lib, popped.name),
                                        Lookup::Found(NamespaceEntry::Decl(owner))
                                        if self.graph.library_of_file(owner).is_some_and(
                                            |lib| self.graph.uri(lib).starts_with("dart:"),
                                        )
                                    ) =>
                            {
                                Receiver::Container {
                                    name: popped.name,
                                    kind: ContainerKind::Class,
                                }
                            }
                            // The accessor-read fall-through (`sub.key`): a bare
                            // getter/field read carries no push-time type
                            // (`member_declared_ty` answers fields only), but
                            // the read's static type IS the accessor's declared
                            // return — `receiver_static_ty`'s accessor face
                            // answers it above, and an information-less head
                            // whose read type names a container resolves the
                            // member on that container exactly like the chained
                            // face below (`_androidParams.key`, webview
                            // android_webview_controller.dart:1131). Every
                            // other answer keeps the prior dispatch.
                            other => match (other, &receiver_ty) {
                                (Receiver::Unknown, Ty::Named { name, .. }) => Receiver::Typed {
                                    type_name: *name,
                                    ty: receiver_ty,
                                },
                                // The record face has no receiver spelling —
                                // the parse-time type keys the side table the
                                // pass-2 record-extension application reads.
                                (Receiver::Unknown, Ty::Record { .. }) => {
                                    self.record_recv_tys
                                        .push((member.range.offset, receiver_ty));
                                    other
                                }
                                _ => other,
                            },
                        }
                    }
                    // The invoked bare callee's chain face: the send result
                    // types the next receiver (`body(ctx).m` resolves `.m`
                    // on `Expr.call`'s return, census A4) — a non-named
                    // verdict keeps the Unknown dispatch.
                    RowShape::Bare(_) => match receiver_ty {
                        Ty::Named { name, .. } => Receiver::Typed {
                            type_name: name,
                            ty: receiver_ty,
                        },
                        Ty::Record { .. } => {
                            self.record_recv_tys
                                .push((member.range.offset, receiver_ty));
                            Receiver::Unknown
                        }
                        _ => Receiver::Unknown,
                    },
                    RowShape::Receiver(recv) => *recv,
                    // The resolved-prefix container member (`io.Directory
                    // .new`, `io.Directory.gone`): the middle member row
                    // resolved a name through the prefix's table; when that
                    // name is a container the next segment reads its static
                    // face — `.new` resolves the unnamed constructor
                    // (silent), a missing member reports the container
                    // family (`undefined_getter` "for the type 'Directory'",
                    // not the prefix-miss `undefined_prefixed_name`; the
                    // oracle's ast_rewrite keeps the qualified type as the
                    // receiver's static type). A non-container prefix hit
                    // (a top-level function/variable) resolves to `None`
                    // and keeps the Unknown dispatch — its own resolution
                    // already reported.
                    // An INVOKED prefixed head (`io.File(p).existsSync()`)
                    // is an instance-creation result: the next selector
                    // reads the instance face, never the static one.
                    RowShape::Member {
                        recv: Receiver::Prefix { name: prefix },
                    } if popped.intent == Intent::Invoke
                        && let Some((resolved, _)) = prefixed_container(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            *prefix,
                            popped.name,
                        )
                        && matches!(resolved.kind, ContainerKind::Class)
                        // The bare spelling must name the SAME declaration
                        // (or nothing) in this scope: the typed receiver
                        // carries no owner, so a same-named local class
                        // (`Link` of the file-system API beside `io.Link`)
                        // would answer the walk instead.
                        && resolve_container(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            popped.name,
                        )
                        .is_none_or(|(local, _)| std::ptr::eq(local, resolved)) =>
                    {
                        
                        Receiver::Typed {
                            type_name: popped.name,
                            ty: Ty::named(popped.name),
                        }
                    }
                    RowShape::Member {
                        recv: Receiver::Prefix { name: prefix },
                    } => match prefixed_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        *prefix,
                        popped.name,
                    ) {
                        Some((resolved, _)) => Receiver::PrefixedContainer {
                            prefix: *prefix,
                            name: popped.name,
                            kind: resolved.kind,
                        },
                        None => Receiver::Unknown,
                    },
                    // The chained-receiver face (`a.b().c`, `x.m.y`): a
                    // value/member receiver whose expression static type
                    // names a container resolves its member on that
                    // container — the official `staticType` of the preceding
                    // chain node is the member face's receiver type, so the
                    // folded `a.b()` result feeds `.c` exactly like a
                    // declared-type read. `receiver_ty` (computed above
                    // from `receiver_static_ty`: the row's own type wins,
                    // else the invoked callee's declared return type)
                    // carries it; a non-named verdict (dynamic, unknown,
                    // function-type) keeps the Unknown dispatch — no member
                    // face exists to walk there.
                    _ => match receiver_ty {
                        Ty::Named { name, .. } => Receiver::Typed {
                            type_name: name,
                            ty: receiver_ty,
                        },
                        Ty::Record { .. } => {
                            self.record_recv_tys
                                .push((member.range.offset, receiver_ty));
                            Receiver::Unknown
                        }
                        _ => Receiver::Unknown,
                    },
                };
                if deferred_receiver {
                    self.deferred_miss_rows.push(Row {
                        shape: popped_shape,
                        ..popped
                    });
                }
                recv
            }
            None => Receiver::Unknown,
        };
        // S4-B: a cascade section's receiver is the latched cascade target —
        // the RHS-value leftover popped above is never the real receiver
        // (`C()..p = 1..q` resolves `q` on `C`, not the `1` row). The normal
        // merge already recorded `member_reads` for this section; only the
        // classification feeds the override.
        let recv = cascade_recv.unwrap_or(recv);
        // A later section's popped row is the previous section's RHS
        // leftover: the receiver span the pass-2 flow re-derivation reads
        // is the cascade target's own read, never that leftover's.
        if cascade_recv.is_some()
            && let Some(target_span) = self.cascade_target_spans.last().copied()
        {
            
            recv_span = target_span;
        }
        if is_null_aware {
            // The receiver's expression static type decides the face
            // (`_checkForUnnecessaryNullAware` reads the flow-aware static
            // type): the merged row's own ty carries the postfix upgrades
            // (`x!?.m`'s `!` rewrite) the receiver classification keeps in
            // its declared payload, so it wins when set. The classifier
            // covers the receivers whose static type lives off the row —
            // `this`'s container, a creation's result (`C()?..new()`'s
            // `C`), `super`'s parse-owned ban.
            let receiver_ty = if merged_receiver_ty != Ty::UNKNOWN {
                merged_receiver_ty
            } else {
                match recv {
                    Receiver::This { container, kind } => self.this_static_ty(container, kind),
                    Receiver::Super { .. } => Ty::UNKNOWN,
                    Receiver::Created { name, kind } if kind != ContainerKind::Extension => {
                        Ty::named(name)
                    }
                    Receiver::Typed { ty, .. } => ty,
                    _ => Ty::UNKNOWN,
                }
            };
            // The operator is the anchor (`_checkForUnnecessaryNullAware`'s
            // `.at(operator)`), not the member row the merge folded into.
            let kind = if self.in_cascade {
                NullAwareKind::Cascade
            } else {
                NullAwareKind::Access
            };
            // The bound-aware strictly face: an in-scope parameter
            // receiver decides through its written bound (`T extends
            // AstNodeImpl?` keeps `?.` legal, `T extends C` reports).
            if self.strictly_non_nullable(receiver_ty) {
                let (op, replacement) = kind.lexemes();
                self.emit3(
                    SemanticCode::InvalidNullAwareOperator,
                    op_range,
                    op,
                    replacement,
                    "",
                );
            } else if was_local {
                // The walk-time type decides nothing (the declared nullable
                // /dynamic shape): re-decide at teardown against the flow's
                // promoted receiver type.
                self.null_aware_staged.push((recv_span, op_range, kind));
            }
            // The `?.[` recovery shape (`AstBuilder.doDotExpression`): the
            // parser folds the `[...]` operand into the dot access, so the
            // merged member row is the operand's value row — the oracle
            // instead resolves a member named `[` at the bracket. Rename
            // and re-anchor the folded row so pass 2 resolves `[` against
            // the receiver (`undefined_getter`/`undefined_setter` `'['`).
            if !self.in_cascade
                && matches!(member.shape, RowShape::Value)
                && let Some(bracket) = self.bracket_after(op_range.end())
            {
                member.name = self.intern("[");
                member.range = SourceRange {
                    offset: bracket,
                    length: 1,
                };
            }
        }
        // `super.named()` in a constructor-initializer list is a super
        // *constructor* reference, never a member access — but the intent
        // only exists after the merge (`(` vs `=` vs `;`), so the folded
        // candidate waits in `pending_init_super` for its shape: an
        // argument group decides `undefined_constructor_in_initializer` at
        // the list's close, while `super.name = v` (the field-initializer
        // face) and the `super().foo` recovery residue drop. The row folds
        // either way — pass 2's `super.member` dispatch must never see it.
        // The invocation verdict reads the source at the merge — the next
        // significant byte after the selector is the group's `(` — and the
        // group-open link below arms the same flag for the
        // scanner-synthesized parens (the `super.x` recovery inserts them
        // as tokens, invisible to the source bytes).
        if self.initializer_depth > 0
            && let Receiver::Super { container, .. } = recv
        {
            let span = SourceRange {
                offset: receiver_offset.unwrap_or(member.range.offset),
                length: member.range.end() - receiver_offset.unwrap_or(member.range.offset),
            };
            // The invocation verdict reads the next significant byte after
            // the selector: `(` for the `super.name(...)` call shape,
            // anything else for the field-initializer `super.name = v` /
            // recovery forms (`open_arguments`'s scanner-synthesized rider
            // covers `super.x;` parens inserted as tokens, invisible to the
            // source bytes).
            // `member.range.end()` is a UTF-16 offset; the source byte slice
            // reads through `byte_of` so a wide char before the selector
            // cannot desync the `(` probe (#2161).
            let invoked = self
                .source
                .as_bytes()
                .get(self.byte_of(member.range.end())..)
                .unwrap_or_default()
                .iter()
                .find(|b| !b.is_ascii_whitespace())
                .is_some_and(|b| *b == b'(');
            self.pending_init_super.push(PendingInitSuper {
                selector_offset: member.range.offset,
                container,
                name: member.name,
                span,
                invoked,
                head: receiver_offset.is_some_and(|offset| self.init_item_head == Some(offset)),
            });
            self.push_row(Row {
                name: member.name,
                range: member.range,
                qualified: true,
                shape: RowShape::Value,
                intent: Intent::Read,
                write_op: None,
                ty: Ty::UNKNOWN,
            });
            return;
        }
        // The extraction itself is a candidate: a UseResult-carrying
        // getter/field read discards here (`_check`'s property-extraction
        // face, the accessor unwrap included).
        self.ur_register(member.range.offset);
        // The merged read's declared type (#611 consumption): an instance
        // field read through a classified receiver carries the field's
        // written type — the same member face `resolve_bare` types the
        // implicit-`this` bare name with, boundary-projected so the slot
        // stays single-pool (`super.` skips the own member map). Getters,
        // statics, and unresolvable chains stay `Unknown` (under-report).
        // An invoked member's row already carries the send's own result
        // type — the instantiated generic face the argument-group write
        // landed (`r.readEnum(...)`'s solved `Tag`) — which the declared
        // re-read must not discard: the folded row IS the invocation's
        // expression node, so its type is the call's solved return (the
        // `pending_send_ty` creation fold's same hole, member-side).
        let ty = if member.ty != Ty::UNKNOWN && member.intent == Intent::Invoke {
            member.ty
        } else if member.name == self.intern("values")
            && let Some(container) = match &recv {
                Receiver::Container {
                    name,
                    kind: ContainerKind::Enum,
                } => Some(*name),
                Receiver::This {
                    container,
                    kind: ContainerKind::Enum,
                } => Some(*container),
                _ => None,
            }
        {
            // The enum's synthetic `values` static getter (`Tag.values`):
            // the constant list's fixed face `List<Tag>` — the member walk
            // cannot serve it (the synthetic member is no declared field),
            // and the untyped under-report starves the call-site inference
            // (`readEnum(Tag.values)` binds `T := Tag` from exactly this
            // element face). A non-enum container (an SDK `Map.values`
            // instance read) never takes this arm.
            self.boundary
                .args
                .borrow_mut()
                .named(self.intern("List"), false, &[Ty::named(container)])
        } else {
            match &recv {
                Receiver::This { container, .. } => {
                    self.member_declared_ty(*container, member.name)
                }
                Receiver::Super { container, .. } => {
                    match self.walk_super(*container, member.name) {
                        Walk::Found(entry) if entry.kind == MemberKind::Field => {
                            self.fold_alias_ty(entry.var_ty)
                        }
                        _ => Ty::UNKNOWN,
                    }
                }
                Receiver::Typed { type_name, .. } => {
                    let declared = self.member_declared_ty(*type_name, member.name);
                    // The field's written schema must never ride raw: a
                    // generic container's field (`MapEntry.key` writes `K`)
                    // carries the RECEIVER's instantiation (the official
                    // `_executeGetter`/field-read face), so the bare `K`
                    // would leak into every downstream member solve — an
                    // uninstantiable receiver keeps `Unknown` (under-report).
                    if declared == Ty::UNKNOWN {
                        declared
                    } else {
                        self.instantiate_accessor_static(*type_name, &recv, declared)
                    }
                }
                // A record-typed receiver (`config.strict`, `pair.$1`): the
                // field getter's type is the shape's own field type (records
                // carry no interface members besides `Object`'s, which stay
                // `Unknown`); a `?.` read joins the null-aware `?` — and the
                // field's own face rides the shorted-face map so a chain
                // continuation (`result?.read.icc`) reads the member the way
                // the class design reads the row's non-nullable type (the
                // baked `?` is the SHORT's null, not the field's — the
                // continuation latches off it, never dereferences it).
                _ => match self.record_field_ty(merged_receiver_ty, member.name, is_null_aware) {
                    Some(field) if is_null_aware => {
                        if !Self::is_nullable_ty(field) {
                            self.record_short_fields.insert(member.range.offset, field);
                        }
                        Self::nullable_form(field)
                    }
                    Some(field) => field,
                    None => Ty::UNKNOWN,
                },
            }
        };
        // A redirecting factory's `= C.name` target: the constructor
        // reference parses as a `head.selector` member merge under
        // `redirect_target_depth` — latch it for
        // `end_redirecting_factory_body`, which writes the open factory's
        // `recursive_constructor_redirect` edge (the head's spelling keys
        // the target container; an unnamed `= C` head never reaches this
        // merge — the reference's own close covers it).
        if self.redirect_target_depth > 0
            && let Some((head_name, head_range)) = popped_head
        {
            self.factory_redirect_target = Some((
                head_name,
                member.name,
                SourceRange {
                    offset: head_range.offset,
                    length: member.range.end() - head_range.offset,
                },
            ));
        }
        // F-C: a `this.` selector pushed inside the constructor-initializer
        // list is the field-initializer (`this.x = v`) or redirect
        // (`this.other()`) shape — the oracle resolves it through the
        // dedicated initializer world (`element_resolver.dart`'s
        // `ConstructorFieldInitializer` / `RedirectingConstructorInvocation`
        // visits; a missing redirect target reports nothing) and the
        // instance-member trio never fires there. The stamp folds the row
        // at the pass-2 dispatch. The argument-group spelling is the
        // redirect edge (`recursive_constructor_redirect`'s cycle walk).
        if self.initializer_depth > 0
            && let Receiver::This { container, .. } = recv
        {
            self.init_rows.insert(member.range.offset, container);
            // `member.range.end()` is a UTF-16 offset: a wide char anywhere
            // upstream (a doc comment's `—`, a string literal) desyncs a raw
            // byte read, so the `(` probe goes through `byte_at` (#2161).
            if self.byte_at(member.range.end()) == Some(b'(') {
                let begin = receiver_offset.unwrap_or(member.range.offset);
                self.note_redirect_edge(
                    container,
                    member.name,
                    SourceRange {
                        offset: begin,
                        length: member.range.end() - begin,
                    },
                    member.range,
                );
            }
        }
        // The Object-face receiver record (#2521): a local receiver whose
        // static type names no container (a function value, a record, a
        // `dynamic`) merges as the Unknown dispatch and the receiver row
        // folds away with it — the merge's own static-type answer is the
        // only surviving witness of what `Object`'s member face resolves
        // against downstream (`callback.toString()` types `String` through
        // the function value's chain exactly as the official resolver
        // does), so it rides this side table keyed by the member row.
        if was_local
            && matches!(recv, Receiver::Unknown)
            && !matches!(merged_receiver_ty, Ty::Unknown | Ty::Void | Ty::Never)
        {
            self.member_recv_tys
                .insert(member.range.offset, merged_receiver_ty);
        }
        // The staged-join record (B5): the merged row's facts outlive pass
        // 2's stack take — one read event folds once, so the repeat insert
        // is a no-op.
        if let Err(index) = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&member.range.offset))
        {
            self.member_reads.insert(
                index,
                MemberRead {
                    offset: member.range.offset,
                    name: member.name,
                    recv,
                    recv_span,
                },
            );
        }
        if is_null_aware {
            self.null_aware_members.insert(member.range.offset);
        } else if let Some(receiver_offset) = receiver_offset {
            // The `?.` short-circuit's chain continuation (`a?.b.c`'s `.c`,
            // `a?.b.c.d`'s `.d`): the guarded `?.` makes the WHOLE chain
            // node nullable — upstream's `ShortCircuiting` resolution types
            // `a?.b.c` as `C?` even though `.c` itself resolves on `b`'s
            // written type. The member's result-nullability latch joins the
            // chain here so the enclosing-expression reads (assignment,
            // return, argument) see the `?` — but ONLY while the folded
            // receiver reads non-null: a nullable member result (`a?.b.c`
            // on `B?`) keeps the continuation's own
            // `unchecked_use_of_nullable_value` verdict, which the
            // receiver-side `!null_aware_members.contains` exclusion would
            // otherwise silence. A null-aware RECORD-field receiver is the
            // one folded face that reads nullable while its field reads
            // non-null — the record arm bakes the SHORT's `?` into the
            // stored face, so the shorted-field map answers the same
            // question the row type answers for classes. Cascade sections
            // stay out — their own `?..` merge latches the first section,
            // and a section's member row is never the cascade's value (the
            // target is).
            let record_short = self
                .record_short_fields
                .get(&receiver_offset)
                .is_some_and(|field| !Self::is_nullable_ty(*field));
            if !self.in_cascade
                && self.null_aware_members.contains(&receiver_offset)
                && (!Self::is_nullable_ty(merged_receiver_ty) || record_short)
            {
                self.null_aware_members.insert(member.range.offset);
            }
        }
        // The implicit-`call` retype at the fold: the send's own
        // `consume_send` retype runs BEFORE this merge folds the row (the
        // parser completes the argument group ahead of the dot-expression
        // reduction), so the callee row it read was still the
        // information-less selector row — the merged row is where the
        // member's read type first exists, and an invoked member whose type
        // names a callable class answers the `call` method's return (the
        // same face `consume_send` applies, `XPathExpression`'s `call`
        // making `entry.key(context)` read `XPathSequence`, never the
        // field's own class). A `Never`-typed member keeps its own face.
        let ty = if member.intent == Intent::Invoke && !self.is_never_ty(ty) {
            match self.implicit_call_face(ty) {
                Some((_, call_ret, _)) => call_ret,
                None => ty,
            }
        } else {
            ty
        };
        self.push_row(Row {
            name: member.name,
            range: member.range,
            qualified: true,
            shape: RowShape::Member { recv },
            intent: member.intent,
            write_op: member.write_op,
            ty,
        });
    }

    /// F-C: records a redirecting-constructor invocation edge
    /// (`this(...)` in an initializer list). The enclosing constructor's
    /// spelling comes from its open declaration site; the site-less shapes
    /// (a factory never carries `this`) drop. `member_span` is the target
    /// selector's own token — the generative faces' anchor.
    pub(crate) fn note_redirect_edge(
        &mut self,
        container: StringId,
        target: StringId,
        span: SourceRange,
        member_span: SourceRange,
    ) {
        let Some((_, Some(site))) = self.ctor_pending else {
            return;
        };
        let Some(ctor) = self.decl_sites.get(site).map(|s| s.name) else {
            return;
        };
        // The redirecting generative ctor's own site — its finals route
        // through the target's `this.` formals, so the
        // `final_not_initialized_constructor` pass skips it
        // (`ConstructorFieldsVerifier` walks only non-redirecting ctors).
        self.ctor_redirects.insert(site);
        self.redirect_edges.push(RedirectEdge {
            container,
            ctor,
            target_container: container,
            target,
            span,
            target_span: member_span,
            factory: false,
            is_const: self.in_const_ctor,
        });
    }

    /// The declared constructor site for `(container, selector)` — the
    /// last registration wins (a file declares a spelling once; recovery
    /// residue keeps the settled face). Foreign containers hold no sites
    /// here (`None` — the under-report posture).
    pub(crate) fn ctor_decl_site(
        &self,
        container: StringId,
        selector: StringId,
    ) -> Option<&DeclSite> {
        // The unnamed face has three spellings: the class name (`C()`), the
        // `.new` selector, and the bare `new` head (`const new();`), whose
        // head token is the `(` — its site carries that spelling.
        let new = self.intern("new");
        let paren = self.intern("(");
        let unnamed = selector == container || selector == new;
        self.decl_sites.iter().rev().find(|s| {
            matches!(s.kind, DeclKind::Constructor { container: c } if c == container)
                && (s.name == selector
                    || (unnamed && (s.name == container || s.name == new || s.name == paren)))
        })
    }

    /// Whether `(container, selector)` denotes a declared constructor face
    /// of `decl`: a named selector joins through the container's selector
    /// set, the unnamed spellings (the head itself, `new`) through the
    /// implicit/explicit unnamed face.
    pub(crate) fn ctor_face_exists(
        &self,
        decl: &Container,
        container: StringId,
        selector: StringId,
    ) -> bool {
        decl.constructors.contains(&selector)
            || ((selector == container || selector == self.intern("new"))
                && decl.has_unnamed_constructor)
    }

    /// The offset just past the argument group whose `(` sits at `open`:
    /// a depth-balanced scan with string-literal awareness — the
    /// redirecting invocation's node extent, the generative faces' anchor
    /// (`.at(invocation)`).
    pub(crate) fn argument_group_end(&self, open: u32) -> u32 {
        let bytes = self.source.as_bytes();
        let mut i = open as usize;
        let mut depth = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return (i + 1) as u32;
                    }
                }
                b'\'' | b'"' => {
                    let quote = bytes[i];
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == b'\\' {
                            i += 1;
                        } else if bytes[i] == quote {
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        open + 1 // unterminated: keep the open paren covered
    }

    /// The chained-read settlement (#2283): re-derives each latched member
    /// read's result face from its receiver's promoted face and re-decides
    /// the row's pass-2 member trio on it. A chain head's face is the flow
    /// read type at the receiver's span (the #1465 promoted receiver); a
    /// chained link's face is the link below it's settled result — the
    /// oracle's member re-binding (`a is Sub` promotes `a`, so `a.value`
    /// resolves on `Sub` and its RESULT type is `Sub`'s own `value` member,
    /// whose face every later chain node reads). Reads walk in source
    /// order, so each link consumes only earlier entries' faces and
    /// arbitrary depth settles in one pass.
    fn settle_promoted_chains(&mut self) {
        if self.promoted_recv_reads.is_empty() {
            return;
        }
        let latches = self.promoted_recv_reads.clone();
        let reads = self.member_reads.clone();
        let mut derived: FxHashMap<u32, Ty> = FxHashMap::default();
        for read in &reads {
            let Ok(index) = latches.binary_search_by(|(member, _)| member.cmp(&read.offset)) else {
                continue;
            };
            let span = latches[index].1;
            // The head-linked latch (#1465) carries the receiver read's own
            // span; the chained latch's zero-length span keys the receiver
            // read's entry instead (its face is the chain's settled
            // result). A face the walk cannot name (`dynamic`, unknown)
            // keeps the frozen row — the under-report posture. The head
            // arm reads the re-bound local face first (#2539): the flow
            // read still spells the parse-time bind, so a settled chain
            // through a re-bound local (`var a = f.element; a.m`) would
            // otherwise re-type every link on the stale base declaration.
            let face = if span.length == 0 {
                derived.get(&span.offset).copied()
            } else {
                self.rebound_read_face(span).or_else(|| self.read_type_at(span))
            };
            let Some(face @ Ty::Named { name, .. }) = face else {
                continue;
            };
            // The chained link re-decides whether or not the settled face
            // declares the member — a miss is exactly the re-render face
            // (the row keeps reporting, now naming the settled face). Only
            // the result-type derivation below needs the `Found` arm.
            if span.length == 0 {
                self.settle_chain_row(read, name, face);
            }
            let Walk::Found(entry) = self.walk_members(name, read.name) else {
                continue;
            };
            let result = match entry.kind {
                MemberKind::Field => self.fold_alias_ty(entry.var_ty),
                _ => self.fold_alias_ty(entry.return_ty),
            };
            derived.insert(read.offset, result);
        }
    }

    /// One chained row's verdict re-decision: pass 2 judged the row on the
    /// frozen unpromoted result face, so its member-trio emission re-decides
    /// against the settled face — a member the face declares retracts (the
    /// oracle resolves it there and stays silent), a still-missing member
    /// reports against the settled face's display (the oracle's re-bound
    /// receiver names `double`, never the frozen `Object`). The verdict
    /// mirrors the pass-2 member trio's own shape: the indexed walk decides,
    /// Object's universal face spares, and a root chain-break (the
    /// primitive/external head — `double`, `List`) falls to the frozen
    /// member tables ([`Engine::frozen_member_verdict`], the trio's
    /// `ChainBroken` consult) — `double.isNaN` retracts through the frozen
    /// `double` row, `double.missing` re-renders through its `Absent`.
    fn settle_chain_row(&mut self, read: &MemberRead, face_head: StringId, face: Ty) {
        let plain_walk = self.walk_members(face_head, read.name);
        let setter_name = self.setter_spelling(read.name);
        let setter_walk = self.walk_members(face_head, setter_name);
        // The frozen-table verdict behind a root chain-break (`Walk::
        // ChainBroken` — an unindexed head the element model cannot carry):
        // `Defined` retracts, `Absent`/`Unknown` keep the row. An indexed
        // head's walk (`Found`/`Missing`) decided already.
        let frozen_defined = |engine: &Self, name: StringId| {
            matches!(engine.walk_members(face_head, name), Walk::ChainBroken)
                && matches!(
                    engine.frozen_member_verdict(face_head, name),
                    super::row_resolution::FrozenFace::Defined
                )
        };
        let object_face =
            read.name == self.intern("==") || self.members.object_members.contains(&read.name);
        let mut index = 0;
        while index < self.report.diagnostics.len() {
            let diagnostic = &self.report.diagnostics[index];
            if diagnostic.range.offset != read.offset {
                index += 1;
                continue;
            }
            let (code, exists) = match diagnostic.code.name {
                "undefined_getter" => (
                    SemanticCode::UndefinedGetter,
                    matches!(&plain_walk, Walk::Found(entry) if matches!(entry.kind, MemberKind::Field | MemberKind::Getter))
                        || object_face
                        || frozen_defined(self, read.name),
                ),
                "undefined_method" => (
                    SemanticCode::UndefinedMethod,
                    matches!(&plain_walk, Walk::Found(entry) if entry.kind == MemberKind::Method)
                        || object_face
                        || frozen_defined(self, read.name),
                ),
                "undefined_setter" => (
                    SemanticCode::UndefinedSetter,
                    matches!(&setter_walk, Walk::Found(entry) if entry.kind == MemberKind::Setter)
                        || matches!(&plain_walk, Walk::Found(entry) if entry.kind == MemberKind::Field && !entry.is_final)
                        || object_face
                        || frozen_defined(self, read.name)
                        || frozen_defined(self, setter_name),
                ),
                _ => {
                    index += 1;
                    continue;
                }
            };
            if exists {
                self.report.diagnostics.remove(index);
            } else {
                let name_text = self.strings.resolve(read.name).to_string();
                let face_text = self.render_ty(face);
                self.report.diagnostics[index] = Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    code.message(&name_text, &face_text),
                    self.file,
                    diagnostic.range,
                    None,
                );
                index += 1;
            }
        }
    }

    /// F-C: the `recursive_constructor_redirect` walk — a redirecting
    /// constructor whose invocation chain returns to itself reports at the
    /// invocation (`_checkForRecursiveConstructorRedirect`'s
    /// `isInRedirectingConstructorCycle`; the per-file edge list stands in
    /// for the element property). Generative `this(...)` edges target their
    /// own container; a redirecting factory's `= C.name` may point at any
    /// class — the walk keys on `(target_container, target)`. The visited
    /// set bounds the walk: every step either terminates or interns a fresh
    /// key.
    ///
    /// The redirect-target faces ride the same drain: the whole file's
    /// declaration sites are complete here, so a forward-declared target
    /// (`factory W.n()` declared after its `W() : this.n()` user) judges
    /// exactly (`_checkForConflictingInitializerErrorCodes` /
    /// `_checkForRedirectingConstructorErrorCodes` run on settled elements).
    ///
    /// This teardown hook also settles the promotion-refined chained reads
    /// (#2283) — the module's only post-resolve seam: pass 2 has already
    /// emitted the chained rows against their frozen unpromoted receiver
    /// faces, and the settlement re-decides them below.
    pub(crate) fn finalize_redirects(&mut self) {
        self.settle_promoted_chains();
        let edges = std::mem::take(&mut self.redirect_edges);
        for edge in &edges {
            // The generative target's face
            // (`_checkForConflictingInitializerErrorCodes`'s
            // `RedirectingConstructorInvocation` arm,
            // error_verifier.dart:4300-4306): a resolved target that is a
            // factory reports `redirect_generative_to_non_generative_
            // constructor` at the invocation node; a missing target is the
            // missing-constructor face, outside the burn set — silence.
            if !edge.factory
                && self
                    .ctor_decl_site(edge.target_container, edge.target)
                    .is_some_and(|site| site.is_factory)
            {
                let length = if self.byte_at(edge.span.end()) == Some(b'(') {
                    self.argument_group_end(edge.span.end()) - edge.span.offset
                } else {
                    edge.span.length
                };
                self.emit(
                    SemanticCode::RedirectGenerativeToNonGenerativeConstructor,
                    SourceRange {
                        offset: edge.span.offset,
                        length,
                    },
                    "",
                    "",
                );
            }
            // The const redirect's target constness
            // (`_checkForRedirectToNonConstConstructor`,
            // error_verifier.dart:7335-7347): a `const` redirecting
            // constructor or factory whose target constructor is not const
            // reports at the target — the selector for a `this.b()` shape
            // (`constructorSelector?.name2 ?? thisKeyword`), the `= C.name`
            // extent for a factory. The target's element must resolve (the
            // null redirectedElement arm skips): a same-file declaration
            // reads its own const bit, a same-file implicit unnamed default
            // is non-const, anything foreign or missing stays silent.
            if edge.is_const {
                let target_const = self
                    .ctor_decl_site(edge.target_container, edge.target)
                    .map(|site| site.const_ctor)
                    .or_else(|| {
                        let (decl, owner) = resolve_container(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            edge.target_container,
                        )?;
                        if owner != self.file
                            || !self.ctor_face_exists(decl, edge.target_container, edge.target)
                        {
                            return None;
                        }
                        Some(false) // the implicit unnamed default: never const
                    });
                if target_const == Some(false) {
                    let span = if edge.factory {
                        edge.span
                    } else {
                        edge.target_span
                    };
                    self.emit(SemanticCode::RedirectToNonConstConstructor, span, "", "");
                }
            }
            let mut current_container = edge.target_container;
            let mut current = edge.target;
            let mut visited: FxHashSet<(StringId, StringId)> =
                FxHashSet::from_iter([(edge.container, edge.ctor)]);
            let in_cycle = loop {
                if current_container == edge.container && current == edge.ctor {
                    break true;
                }
                if !visited.insert((current_container, current)) {
                    break false;
                }
                let Some(next) = edges
                    .iter()
                    .find(|e| e.container == current_container && e.ctor == current)
                    .map(|e| (e.target_container, e.target))
                else {
                    break false;
                };
                current_container = next.0;
                current = next.1;
            };
            if in_cycle {
                self.emit(
                    SemanticCode::RecursiveConstructorRedirect,
                    edge.span,
                    "",
                    "",
                );
            }
        }
    }

    /// The super-constructor selector in an initializer list
    /// (`error_verifier.dart`, the `SuperConstructorInvocation` resolution):
    /// the direct superclass's named constructor set decides. An
    /// unresolvable superclass (external, `on`-constraint chain) suppresses.
    pub(crate) fn check_super_constructor(
        &mut self,
        container: StringId,
        name: StringId,
        span: SourceRange,
    ) {
        let Some(superclass) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass)
        else {
            return;
        };
        let resolved = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            superclass,
        )
        .or_else(|| {
            // The container's superclass edge keeps the bare continuation
            // name only (the collector's qualified-extends capture: `p.S`
            // stores `S`), so a prefix-only import misses the bare lookup.
            // The oracle resolves the same spelling qualified, so retry the
            // library's registered prefixes — first hit wins — and run the
            // ordinary membership face on it.
            self.graph.import_edges(self.lib).iter().find_map(|edge| {
                let prefix = edge.prefix?;
                prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    superclass,
                )
            })
        });
        let Some((super_container, _)) = resolved else {
            // The dead-edge Object fallback (upstream Dart SDK source,
            // `error_verifier.dart`'s unresolved-superclass walk): a truly
            // undefined superclass resolves no element, so the check walks
            // the implicit Object supertype — the unnamed `super()` face is
            // satisfied there, and a written named selector misses Object
            // and reports. A merely unindexed declaration (the synthetic
            // SDK face the oracle resolves for real) keeps the under-report
            // posture.
            if name != self.intern("new")
                && self.supertype_hop_dead(superclass, self.lib, self.file)
            {
                let class_text = self.strings.resolve(superclass).to_string();
                let name_text = self.strings.resolve(name).to_string();
                self.emit(
                    SemanticCode::UndefinedConstructorInInitializer,
                    span,
                    &class_text,
                    &name_text,
                );
            }
            return;
        };
        if super_container.constructors.contains(&name) {
            // The super-ctor selector counts as the constructor's
            // reference: `super._(...)` spares the named super constructor
            // from `unused_element` (the official reference set includes
            // superclass-constructor invocations — #2172's
            // `QueueList._`/`BoolList._`, whose only uses are subclass
            // `super._` initializers).
            self.used_names.insert(name);
            return;
        }
        let class_text = self.strings.resolve(superclass).to_string();
        let name_text = self.strings.resolve(name).to_string();
        self.emit(
            SemanticCode::UndefinedConstructorInInitializer,
            span,
            &class_text,
            &name_text,
        );
    }

    /// The `this.name` formal's field-existence face (upstream Dart SDK
    /// source, `error_verifier.dart` `_checkForValidField`): the name must
    /// resolve to a declared instance field of the immediately enclosing
    /// class — an inherited field does not count, a getter/setter-only name
    /// counts as missing, and an unresolvable container suppresses. The
    /// report spans the whole parameter node (`.at(parameter)` upstream:
    /// `head` is the `var`/`final` keyword when present, else the `this`
    /// keyword, through the name). A static field carries its own code
    /// upstream, outside the current burn set — silence beats a wrong-code
    /// row.
    pub(crate) fn check_field_formal(&mut self, container: StringId, head: Token, name: Token) {
        let Some(name_id) = self.name_of(name) else {
            return;
        };
        let entry = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.members.get(&name_id));
        if let Some(entry) = entry {
            if entry.kind == MemberKind::Field && !entry.is_static {
                return;
            }
            // A static field reports `initializer_for_static_field` on the
            // `this` keyword upstream — a code outside the current burn set;
            // the non-existent-field code would be a wrong-code row, so a
            // static member falls through to silence here.
            if entry.is_static {
                return;
            }
        }
        let range = SourceRange {
            offset: head.range.offset,
            length: name.range.end().saturating_sub(head.range.offset),
        };
        let display = self.strings.resolve(name_id).to_string();
        self.emit_corrected(
            SemanticCode::InitializingFormalForNonExistentField,
            range,
            &display,
        );
    }

    /// The `super.name` formal's association face (upstream Dart SDK source,
    /// `error_verifier.dart` `visitSuperFormalParameter`:2238-2251): the
    /// associated parameter comes from the constructor's *resolved* super
    /// constructor — `superConstructor` is the initializer list's actual
    /// redirect target (`super.named(...)` picks that table; the unnamed
    /// face otherwise). A positional super formal needs the target's
    /// parameter at its own index *among the constructor's super formals*
    /// (`indexIn`, element.dart:10779-10783), a named one the same-spelling
    /// named entry. `.at(node.name)` anchors the name token. A class without
    /// an `extends` clause still reports — its implicit `Object`
    /// super-constructor takes no parameters, so every super formal is
    /// unassociated; an unresolvable superclass face is the same shape (the
    /// live 3.13.1 oracle reports every super formal unassociated when the
    /// super name is undefined — `AndroidSslAuthError extends
    /// PlatformSslAuthError` on a broken import, all rows fire).
    pub(crate) fn check_super_formal(
        &mut self,
        container: StringId,
        super_index: u32,
        named: bool,
        name_id: StringId,
        range: SourceRange,
        target: Option<StringId>,
    ) {
        let formals = match self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass)
        {
            Some(superclass) => {
                match resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    superclass,
                ) {
                    Some((super_container, _)) => super_container
                        .ctor_formals
                        .get(&target.unwrap_or(superclass)),
                    // A dead-edge super face takes no parameters (the
                    // undefined-name shape above); a declaration the index
                    // cannot model (the synthetic SDK face) keeps the
                    // under-report early return — the oracle sees formals we
                    // cannot walk.
                    None if self.supertype_hop_dead(superclass, self.lib, self.file) => None,
                    None => return,
                }
            }
            // No `extends`: the implicit `Object` face declares no formals.
            None => None,
        };
        let associated = match formals {
            Some(formals) if named => {
                // The super constructor's `this._x` named formal carries the
                // public element name `x` upstream (`correspondingPublicName`)
                // — a `super._x` forwarding it is associated (the parameter's
                // own name stays private, the *associated* lookup spells the
                // public face).
                let public = self.formal_public_name(name_id);
                formals
                    .named
                    .iter()
                    .any(|f| f.name == name_id || f.name == public)
            }
            Some(formals) => formals
                .positional
                .get(usize::try_from(super_index).unwrap_or(usize::MAX))
                .is_some(),
            None => false,
        };
        if associated {
            return;
        }
        let code = if named {
            SemanticCode::SuperFormalParameterWithoutAssociatedNamed
        } else {
            SemanticCode::SuperFormalParameterWithoutAssociatedPositional
        };
        self.emit_corrected(code, range, "");
    }

    /// Drains `owner`'s buffered `super.` formals against the
    /// super-invocation target (`Some(selector)` for `super.named(...)`,
    /// `None` for the unnamed face — `super(...)`, `super.new(...)`, or no
    /// explicit invocation at all). `owner` is the enclosing constructor's
    /// decl site; `None` selects the primary constructor's entries (a
    /// secondary constructor's drain must not resolve them early — their
    /// target lives in the primary-constructor body's `this : super.x()`
    /// initializer, parsed after the class header).
    pub(crate) fn drain_super_formals(&mut self, owner: Option<usize>, target: Option<StringId>) {
        if self
            .pending_super_formals
            .iter()
            .all(|(entry_owner, ..)| *entry_owner != owner)
        {
            return;
        }
        let mut rest = Vec::new();
        for entry @ (entry_owner, index, named, name, range, container) in
            std::mem::take(&mut self.pending_super_formals)
        {
            if entry_owner == owner {
                let _ = entry;
                self.check_super_formal(container, index, named, name, range, target);
            } else {
                rest.push(entry);
            }
        }
        self.pending_super_formals = rest;
    }

    /// Opens an argument group: the top row is its callee — unless the group
    /// belongs to a `new`/`const`/implicit-creation reference, whose class
    /// name resolves as a type use, never an invocation.
    /// The named selector a dot-preceded creation head spells (`broadcast`
    /// in `Ctr<int>.broadcast(`): the identifier immediately before the
    /// open paren, required to sit after a `.` — the mirror of
    /// [`Engine::bare_creation_head_before`] for the selector posture its
    /// left-guard rejects. ASCII-only walk, the same identifier shape.
    fn named_selector_head_before(&self, open: u32) -> Option<(StringId, SourceRange)> {
        let bytes = self.source.as_bytes();
        let mut end = self.byte_of(open);
        while end > 0 && (bytes[end - 1].is_ascii_alphanumeric() || bytes[end - 1] == b'_' || bytes[end - 1] == b'$') {
            end -= 1;
        }
        let start = end;
        if start == self.byte_of(open)
            || start == 0
            || bytes[start - 1] != b'.'
            || !(bytes[start].is_ascii_alphabetic() || bytes[start] == b'_' || bytes[start] == b'$')
        {
            return None;
        }
        // The walked span `[start, open)` is one contiguous identifier.
        Some((
            self.intern(&self.source[start..self.byte_of(open)]),
            SourceRange {
                offset: self.utf16_of(start),
                length: open - self.utf16_of(start),
            },
        ))
    }

    /// The class segment a named-selector creation head spells (`C.named(`,
    /// `C<int>.named(`): the identifier before the selector's `.` — across a
    /// balanced `<…>` type-argument group when one sits between. `None` when
    /// no dot precedes the selector (an unnamed `C(` head folds through its
    /// own bare container row) or no identifier answers. ASCII-only walk,
    /// the same identifier shape [`Engine::bare_creation_head_name`]
    /// accepts.
    fn creation_class_before(&self, selector: SourceRange) -> Option<(StringId, u32)> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(selector.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor == 0 || bytes[cursor - 1] != b'.' {
            return None;
        }
        cursor -= 1;
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor > 0 && bytes[cursor - 1] == b'>' {
            let mut depth = 0usize;
            while cursor > 0 {
                let byte = bytes[cursor - 1];
                cursor -= 1;
                if byte == b'>' {
                    depth += 1;
                } else if byte == b'<' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            if cursor == 0 {
                return None;
            }
            while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
                cursor -= 1;
            }
        }
        let end = cursor;
        while cursor > 0 && (bytes[cursor - 1].is_ascii_alphanumeric() || bytes[cursor - 1] == b'_' || bytes[cursor - 1] == b'$')
        {
            cursor -= 1;
        }
        if cursor == end {
            return None;
        }
        Some((
            self.intern(&self.source[cursor..end]),
            self.utf16_of(cursor),
        ))
    }

    /// Whether the token ending just before `offset` (trivia skipped)
    /// spells the `const`/`new` creation keyword: the explicit-keyword
    /// posture the unnamed creation fold arms on (the keyword's
    /// constructor-reference event resolves the head as a type use, the
    /// callee transport drops it, and the value row is the fold's to push).
    fn keyword_creation_head(&self, offset: u32) -> bool {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        for keyword in ["const", "new"] {
            let Some(start) = cursor.checked_sub(keyword.len()) else {
                continue;
            };
            if bytes[start..cursor] != *keyword.as_bytes() {
                continue;
            }
            // Whole word: no identifier character directly before.
            if start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_' || bytes[start - 1] == b'$')
            {
                return true;
            }
        }
        false
    }

    /// The creation argument group's open-paren offset: the first
    /// non-trivia byte at or after the selector's end (whitespace only —
    /// nothing else parses between a selector and its argument list). Every
    /// row the group pushed starts at or after this offset.
    fn creation_open_offset(&self, from: u32) -> u32 {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(from);
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        self.utf16_of(cursor)
    }

    /// The creation head a written type-argument group separates from its
    /// argument list (`const C<T>(...)`, `new C<T>(...)`): the identifier
    /// before the balanced `<…>`, under the same guards
    /// [`Engine::bare_creation_head_name`] applies (ASCII identifier shape,
    /// no selector `.` left of it — the named-selector spellings
    /// (`C<T>.named(`) answer through [`Engine::creation_class_before`]
    /// instead). `None` when no balanced group sits between the head and
    /// the paren — the strict-adjacency fallback
    /// ([`Engine::bare_creation_head_before`]) owns the no-group shape.
    fn type_arg_creation_head_before(&self, open: u32) -> Option<(StringId, SourceRange)> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(open);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor == 0 || bytes[cursor - 1] != b'>' {
            return None;
        }
        let mut depth = 0usize;
        loop {
            if cursor == 0 {
                return None;
            }
            cursor -= 1;
            match bytes[cursor] {
                b'>' => depth += 1,
                b'<' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        let end = cursor;
        while cursor > 0 && (bytes[cursor - 1].is_ascii_alphanumeric() || bytes[cursor - 1] == b'_' || bytes[cursor - 1] == b'$')
        {
            cursor -= 1;
        }
        if cursor == end {
            return None;
        }
        // The head is ASCII, so its UTF-16 length is its byte length.
        let range = SourceRange {
            offset: self.utf16_of(cursor),
            length: self.utf16_of(end) - self.utf16_of(cursor),
        };
        // A dot-preceded head (`p.C<T>(`) is the prefixed unnamed
        // creation: the no-selector guard [`Engine::bare_creation_head_name`]
        // rejects it, but the close's arms own the face — arm 1's
        // dot-segment must resolve as a type, else arm 3's prefix table
        // does (`const fb.ListReader<idl.X>(…)` folds through `fb`). Every
        // other left-guard (`?.`, `..`) and a bad identifier shape keep
        // `None` — the walk's span is already a plain identifier run, so
        // only the head character needs re-checking.
        let bytes = self.source.as_bytes();
        let name = if cursor > 0
            && bytes[cursor - 1] == b'.'
            && !(cursor >= 2 && (bytes[cursor - 2] == b'?' || bytes[cursor - 2] == b'.'))
            && (bytes[cursor].is_ascii_alphabetic() || bytes[cursor] == b'_' || bytes[cursor] == b'$')
        {
            self.intern(&self.source[cursor..end])
        } else {
            self.bare_creation_head_name(range)?
        };
        Some((name, range))
    }

    /// The UTF-16 offset just after the balanced `<…>` group a creation
    /// head writes before its argument list — the anchor the
    /// written-argument parse reads the group's segments against (the
    /// group's `>` is the byte before it). `None` when the first
    /// non-trivia bytes after `from` are not a balanced group closed
    /// directly by the argument list's `(`.
    fn type_arg_group_end(&self, from: u32) -> Option<u32> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(from);
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'<' {
            return None;
        }
        let mut depth = 0usize;
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            cursor += 1;
        }
        if depth != 0 {
            return None;
        }
        cursor += 1;
        let mut probe = cursor;
        while probe < bytes.len() && bytes[probe].is_ascii_whitespace() {
            probe += 1;
        }
        (probe < bytes.len() && bytes[probe] == b'(').then_some(self.utf16_of(cursor))
    }

    pub(crate) fn open_arguments(&mut self, open_offset: u32) {
        // F-C: an argument group right after a folded `super.` selector arms
        // the same invocation flag the merge's source peek set — this rider
        // covers the scanner-synthesized parens (`: super.x;` recovery
        // inserts them as tokens, so no source byte announces them).
        if self.initializer_depth > 0
            && let Some(offset) = self.rows.last().map(|row| row.range.offset)
            && let Some(candidate) = self
                .pending_init_super
                .iter_mut()
                .rev()
                .find(|c| c.selector_offset == offset)
        {
            candidate.invoked = true;
        }
        // F-C: a bare `this(` inside a constructor-initializer list is the
        // unnamed-face redirect edge — the keyword receiver still tops the
        // row stack at the group's open (a selector spelling tops the
        // merged member row instead, captured at the merge).
        if self.initializer_depth > 0
            && let Some(row) = self.rows.last().cloned()
            && let RowShape::Receiver(Receiver::This { container, .. }) = row.shape
            && row.name == self.intern("")
        {
            // The `this` keyword row is both the invocation anchor and the
            // unnamed face's target spelling (`constructorSelector?.
            // name2 ?? thisKeyword` — no selector here).
            let keyword_range = row.range;
            self.note_redirect_edge(container, container, keyword_range, keyword_range);
        }
        // The callee is the head row only when it ABUTS the open paren: a
        // recovery shape can open a fresh group while an unrelated row sits
        // on top (the recovered enum element's
        // `this(int x) : assert(1 > 2)` — `x` tops the stack at assert's
        // `(`) and the stale link would upgrade that row's intent to
        // `Invoke`, coding its own miss `undefined_function` where the
        // oracle keeps `undefined_identifier`
        // (enum_primary_constructor_body_error 6:12). A type-argument group
        // between the head and the paren still counts as abutting (the
        // `g<int>(...)` shape — the group defers its arity check against
        // this very link); only trivia otherwise may separate — comments
        // included, which the oracle attaches as trivia to the following
        // token (`f /*c*/ (x)` still checks the argument). A value row tops
        // the group the same way — the
        // literal-invocation recovery shapes (`null(x)`, `true(x)`,
        // `1(x)`) arm the needs-getter face on the literal's own static
        // type.
        let top = self
            .callee_index()
            .or_else(|| {
                // The type-argumented send's head (`f<Missing>(...)`): the
                // group's UNRESOLVED arguments are the `TypeUse` rows stacked
                // between the head and the paren (a resolved argument pushes no
                // row), so the plain top-row read misses the Bare head and the
                // send links nothing — the callee keeps its read intent and
                // pass 2 codes the bare-read face where the oracle's invocation
                // rewrite reports the method/function face (probed:
                // `Undefined<X>(...)` in an instance body is
                // `undefined_method` of the enclosing type, at top level
                // `undefined_function`). Only a live group registration arms
                // the scan's candidate — `type_argumented` re-checks
                // `deferred_args` below, so a stray type row above an
                // unrelated head upgrades nothing.
                if !matches!(self.rows.last()?.shape, RowShape::TypeUse(..)) {
                    return None;
                }
                let mut index = self.rows.len();
                while index > 0 && matches!(self.rows[index - 1].shape, RowShape::TypeUse(..)) {
                    index -= 1;
                }
                let candidate = index.checked_sub(1)?;
                matches!(
                    self.rows[candidate].shape,
                    RowShape::Bare(_) | RowShape::Member { .. }
                )
                .then_some(candidate)
            })
            .or_else(|| {
                // Value and keyword-receiver heads arm the same invocation face:
                // the literal-invocation recovery shapes (`null(x)`, `true(x)`,
                // `1(x)`) read the literal's static type, and a bare
                // `this(...)`/`super(...)` in an instance body reads the
                // enclosing container's. A bare `this(` inside a
                // constructor-initializer list is the redirecting-constructor
                // edge, not an invocation — the F-C redirect face below owns
                // it.
                let index = self.rows.len().checked_sub(1)?;
                let row = &self.rows[index];
                let arms = match &row.shape {
                    RowShape::Value => true,
                    RowShape::Receiver(Receiver::This { .. } | Receiver::Super { .. }) => {
                        !(self.initializer_depth > 0 && row.name == self.intern(""))
                    }
                    _ => false,
                };
                arms.then_some(index)
            });
        let type_argumented =
            top.is_some_and(|i| self.deferred_args.iter().any(|(row, _, _)| *row == i));
        let bang_at = |offset: u32| self.source.as_bytes().get(self.byte_of(offset)) == Some(&b'!');
        let abuts = top.is_some_and(|i| {
            self.rows.get(i).is_some_and(|row| {
                let end = row.range.end();
                if self.trivia_between(end, open_offset) {
                    return true;
                }
                // A postfix `!` between the callee and the group keeps the
                // head: the null-assert belongs to the invoked expression
                // (`key!(ctx)` — the oracle's FunctionExpressionInvocation
                // over the `!` rewrite), so the adjacency read spans one
                // `!` (multi-bang sends keep the under-report posture)
                // still surrounded by trivia on both sides.
                end < open_offset
                    && (end..open_offset).any(|bang| {
                        bang_at(bang)
                            && self.trivia_between(end, bang)
                            && self.trivia_between(bang + 1, open_offset)
                    })
            })
        });
        let callee = if self.ctor_pos > 0 {
            // The creation head's callee row must abut the `(`. A member
            // selector of a parameterized named-ctor reference
            // (`List<T>.unmodifiable(...)`) pushes no row — the resolved
            // head carries the call — so without the adjacency gate the
            // window's `callee_index` reaches back past the creation's
            // head and reads a stale member row from an earlier statement
            // (a `super.put(k, v)` two lines up), reporting the arg count
            // against the wrong formals.
            self.callee_index().filter(|&i| {
                matches!(
                    self.rows.get(i).map(|r| &r.shape),
                    Some(RowShape::Member { .. })
                ) && self
                    .rows
                    .get(i)
                    .is_some_and(|row| self.trivia_between(row.range.end(), open_offset))
            })
        } else if abuts || type_argumented {
            top
        } else {
            None
        };
        self.invokes.push(callee);
        // A zero-parameter alias-to-class callee (`typedef L = Real;
        // L()`): the send is the aliased class's unnamed creation (the
        // oracle resolves the constructor through the alias, the
        // named-type resolver's typedef chase), so the callee row takes
        // the folded class's container shape here — every
        // container-gated face downstream (the creation fold, the close
        // solver, the arity check) reads the class. The reshape rides
        // only the invoked callee: a bare alias read (`L` as a value) is
        // its own face, and a parameterized alias keeps the
        // under-report (its substitution is unmodeled).
        if let Some(index) = callee
            && let Some(row) = self.rows.get(index)
            && matches!(row.shape, RowShape::Bare(BareInfo::Plain { .. }))
            && let Some((class, kind)) = self.alias_class_head(row.name)
        {
            self.used_names.insert(row.name);
            self.rows[index].shape = RowShape::Bare(BareInfo::Container {
                name: class,
                kind,
            });
        }
        // The source-spelled fallback head: only when the row transport
        // dropped the callee outright. Strict adjacency (no trivia skip) —
        // the commented-head shape keeps the under-report posture. A
        // dot-preceded word (`Ctr<int>.broadcast(` — the type-argument
        // group demotes the head row to a `TypeUse` shape the callee
        // filter skips) arms the same latch as the named-selector head:
        // the unnamed face re-derives the dot left-guard and skips it,
        // the named-selector creation fold consumes it.
        let bare_head = if callee.is_none() {
            self.bare_creation_head_before(open_offset)
                .or_else(|| self.named_selector_head_before(open_offset))
                .or_else(|| self.type_arg_creation_head_before(open_offset))
        } else {
            None
        };
        self.pending_bare_creation = bare_head;
        let mark = self.rows.len();
        // An enum constant's own argument list (`low(1)`): the enclosing
        // element window is open and no outer argument list owns this `(`
        // — the callee is the enum's selected constructor, never a row.
        let enum_const = self.in_enum_element.is_some() && self.arg_levels.is_empty();
        // The head row at group open: `top` when the invocation dispatch
        // found a callee, else the last row when it is a keyword receiver
        // (the bare `super(`/`this(` initializer-invocation head `invokes`
        // excludes). Any other shape — a stale unrelated row — stays
        // `None`; the per-arg faces read `callee_row` only under
        // `initializer_depth`.
        let callee_row = top.or_else(|| {
            let index = mark.checked_sub(1)?;
            self.rows
                .get(index)
                .filter(|row| {
                    matches!(
                        row.shape,
                        RowShape::Receiver(Receiver::This { .. } | Receiver::Super { .. })
                    )
                })
                .map(|_| index)
        });
        self.arg_levels.push(ArgLevel {
            args: Vec::new(),
            arg_open: false,
            pending_name: None,
            first_label: None,
            callee_row,
            mark,
            closures: Vec::new(),
            enum_const,
            named_labels: Vec::new(),
            unnamed_head: bare_head.filter(|&(_, range)| self.unnamed_creation_head(range)),
        });
    }

    /// Closes an argument group; its callee awaits the send event. The
    /// collected argument types check against the callee's formals first
    /// (`argument_type_not_assignable`, decided on declared types only).
    /// `positional` is the parser's total argument count feeding the
    /// given-at-call index; `close` is the argument list's closing-paren
    /// span (the count rows' fallback anchor).
    pub(crate) fn close_arguments(&mut self, positional: u16, close: SourceRange) {
        self.flush_unclaimed();
        self.flush_argument();
        let levels = self.arg_levels.pop();
        let enum_const = levels.as_ref().is_some_and(|level| level.enum_const);
        // The send-close upward inference: the argument types close with
        // this level, so the callee's inference parameters the call solves
        // from them unify exactly here — every later return read (the
        // assignment staging, the `var` window) adopts the stashed
        // solution through [`Engine::instantiate_call_return`].
        if let Some(level) = levels.as_ref()
            && !enum_const
            && let Some(Some(callee)) = self.invokes.last().copied()
            && let Some(row) = self.rows.get(callee).cloned()
        {
            self.solve_call_params(callee, &row, &level.args);
            self.refine_numeric_call_return(callee, &row, &level.args);
        }
        // Re-arm this level's own creation head: the engine slot is single,
        // so a nested argument group's open overwrote it with the inner
        // head (and the inner close's `report_bare_creation_miss` path may
        // have consumed it) — the fold below must see the creation it
        // belongs to (`const C<T>(inner())` folds the outer creation, never
        // the inner head the interleaving left in the slot).
        let bare_head = self
            .pending_bare_creation
            .or_else(|| levels.as_ref().and_then(|level| level.unnamed_head));
        self.check_arguments(levels, positional, close);
        self.pending_send = self
            .invokes
            .pop()
            .flatten()
            .map(|index| (index, close.end()));
        // The named-selector implicit creation's fold (`C.named(...)`,
        // `C<T>.named(...)`): the constructor-reference posture pushes no
        // callee row — a resolving head is a type use whose `(Type,
        // _)` arm returns without a row (the constructor-reference dispatch
        // owns its faces), and the selector pushes none — so no send ever
        // consumes the group and the argument rows stand as the enclosing
        // initializer's window: a `var` declarator binds the LAST
        // ARGUMENT's static type and the assignment faces read it as the
        // creation's (probed `final c = Ctr.broadcast(sync: true)` binds
        // `bool`; the oracle answers `Ctr`). The oracle's
        // InstanceCreationExpression folds here: drain the group's rows to
        // their pass-2 obligations (consume_send's same transport) and
        // stack the creation's value row over the whole written span — the
        // var window binds the class, the member faces resolve through the
        // frozen SDK tables for the unindexed heads (`StreamController`).
        // An unresolved head keeps its own pushed miss row's posture (the
        // oracle's error-recovery creation type is invalid — the window
        // stays wide, every face under-reports), and a nested named
        // creation keeps the prior shape (its inner close consumed the
        // head latch). The gate is the group's own evidence — an
        // unconsumed send link, a non-enum level, and a bare head whose
        // dot-preceded class segment resolves as a type: an ordinary send
        // carries its callee link here, and the reduced parser's event
        // order (the implicit-creation close can precede the argument
        // close) makes a `ctor_pos` read unreliable at this point.
        //
        // The explicit-keyword UNNAMED creation (`const C(...)`,
        // `new C(...)`) shares the no-callee-row posture — the keyword's
        // constructor-reference event resolves the head as a type use —
        // but no selector precedes the paren, so the named arm's class
        // walk never fires and no value row stacks. A following member
        // merge then pops the stale row an earlier body left and resolves
        // against it (`const HtmlEscape().convert(text)` reads the
        // receiver as `String` — probed 3.47.1). The keyword face is the
        // unnamed arm's discriminator: the head itself names the class,
        // and the fold mirrors the named arm's drain + value row exactly.
        if self.pending_send.is_none()
            && !enum_const
            && let Some((head, head_range)) = bare_head
            {
                if let Some((class, class_offset)) = self.creation_class_before(head_range)
                    && matches!(self.type_resolution(class).0, TypeResolution::Type)
                {
                    let open = self.creation_open_offset(head_range.end());
                    let kept = self.rows.partition_point(|row| row.range.offset < open);
                    for row in self.rows.drain(kept..) {
                        self.deferred_miss_rows.push(row);
                    }
                let range = SourceRange {
                    offset: class_offset,
                    length: close.end().saturating_sub(class_offset),
                };
                self.ur_register_creation(class_offset, range, class);
                self.rows.push(Row {
                    name: class,
                    range,
                    qualified: false,
                    shape: RowShape::Receiver(Receiver::Created {
                        name: class,
                        kind: ContainerKind::Class,
                    }),
                    intent: Intent::Read,
                    write_op: None,
                    ty: Ty::named(class),
                });
            } else if self.keyword_creation_head(head_range.offset)
                && matches!(self.type_resolution(head).0, TypeResolution::Type)
            {
                self.fold_unnamed_creation_row(head_range.offset, head_range, close.end(), head, ContainerKind::Class);
            }
            // The prefix-resolved creation (`const fb.Reader().vTableGet`,
            // the unprefixed-spelled `fb.Reader(...)`): the constructor
            // reference `p.C` resolves the head as a type use behind the
            // prefix, so no callee row transports and the same stale-pop
            // the keyword arm folds. The resolved container's own face is
            // the created type; the member verdicts on the folded row
            // under-report through the bare scope the same way the
            // non-keyword prefixed send already does (the class name sits
            // behind the prefix table).
            else if let Some((prefix, prefix_offset)) = self.written_prefix_before(head_range.offset)
                && let Some((resolved, _)) =
                    prefixed_container(self.graph, self.namespaces, self.index, self.lib, prefix, head)
            {
                self.fold_unnamed_creation_row(prefix_offset, head_range, close.end(), head, resolved.kind);
            }
        }
    }

    /// Whether a latched creation head names the unnamed creation
    /// posture: the `const`/`new` keyword before the head (or before its
    /// `p.` prefix segment), or a written type-argument group between the
    /// head and the argument list (`C<T>(`). The named-selector spellings
    /// (`C.named(`, `C<T>.named(`) answer false — their group's arguments
    /// can span whole closures and their fold stays with the callee
    /// transport.
    fn unnamed_creation_head(&self, range: SourceRange) -> bool {
        if self.keyword_creation_head(range.offset) {
            return true;
        }
        if self.type_arg_group_end(range.end()).is_some() {
            return true;
        }
        self.written_prefix_before(range.offset)
            .is_some_and(|(_, prefix_offset)| self.keyword_creation_head(prefix_offset))
    }

    /// The unnamed creation fold's shared body ([`Engine::close_arguments`]):
    /// drains the group's argument rows to their pass-2 obligations and
    /// stacks the creation's value row over the whole written span, from the
    /// `start` the creation's written spelling begins at (the keyword's head
    /// token, or the prefix identifier of `p.C`) through the group's close.
    fn fold_unnamed_creation_row(
        &mut self,
        start: u32,
        head_range: SourceRange,
        close_end: u32,
        head: StringId,
        kind: ContainerKind,
    ) {
        let open = self.creation_open_offset(head_range.end());
        let kept = self.rows.partition_point(|row| row.range.offset < open);
        for row in self.rows.drain(kept..) {
            self.deferred_miss_rows.push(row);
        }
        let range = SourceRange {
            offset: start,
            length: close_end.saturating_sub(start),
        };
        self.ur_register_creation(start, range, head);
        // The written type arguments instantiate the created type (upstream
        // Dart SDK source, pkg/analyzer/lib/src/generated/resolver.dart,
        // visitInstanceCreationExpression — the static type is the
        // constructor's return with the resolved type arguments):
        // `const ListEquality<Attr>().hash(...)` resolves `hash` on
        // `ListEquality<Attr>`, never on the enclosing context's face. A
        // group the segment parser cannot spell (a function-typed argument)
        // keeps the raw head — the under-report posture the no-group
        // spelling shares.
        let ty = self
            .type_arg_group_end(head_range.end())
            .and_then(|group_end| {
                let token = Token {
                    id: 0,
                    range: SourceRange {
                        offset: group_end,
                        length: 1,
                    },
                };
                self.literal_group_args(token)
            })
            .map_or_else(
                || Ty::named(head),
                |args| self.boundary.args.borrow_mut().named(head, false, &args),
            );
        self.rows.push(Row {
            name: head,
            range,
            qualified: false,
            shape: RowShape::Receiver(Receiver::Created { name: head, kind }),
            intent: Intent::Read,
            write_op: None,
            ty,
        });
    }

    /// Starts a new argument in the open argument list (the row window the
    /// argument owns closes at the next [`Engine::flush_argument`]).
    pub(crate) fn open_argument(&mut self, name: Option<StringId>, label: SourceRange) {
        self.flush_unclaimed();
        self.flush_argument();
        if let Some(level) = self.arg_levels.last_mut() {
            level.arg_open = true;
            level.pending_name = name;
            if let Some(n) = name {
                level.first_label.get_or_insert(label);
                level.named_labels.push((n, label));
            }
        }
    }

    /// The multi-row window's composite face. A statically-`bool` composite
    /// (`==`/`!=`, the relational and lazy-boolean operators — the spans
    /// `end_binary_expression` latches through `latch_bool_span`) whose
    /// extent covers the whole window answers `bool` at the composite's own
    /// extent: the oracle reads `BinaryExpression.staticType` there, while
    /// this model folds no single row for these spellings (they carry no
    /// `mark_operator` mark, so `fold_binary` keeps the two operand rows).
    /// Any other multi-row shape stays `None` — the caller's unknown,
    /// zero-span face (compound operands must not lend their type).
    fn composite_bool_arg(&self, mark: usize, hi: usize) -> Option<(Ty, SourceRange)> {
        if hi <= mark + 1 {
            return None;
        }
        let start = self.rows.get(mark)?.range.offset;
        let end = self.rows.get(hi - 1)?.range.end();
        // The closure-argument shape (`f((e) => e.type)`) folds the body's
        // member row BELOW the arrow's Value row — a later stack row ending
        // before an earlier one starts. The window is geometric garbage; no
        // composite answer is honest there (the caller's unknown zero-span
        // face takes over). The release build's wrap-around length hid this
        // as a nonsense span; debug builds panic.
        if end < start {
            return None;
        }
        let covered = self
            .bool_decided
            .iter()
            .any(|span| span.offset <= start && end <= span.end());
        covered.then(|| {
            (
                Ty::named(self.members.primitive_ids[3]),
                SourceRange {
                    offset: start,
                    length: end - start,
                },
            )
        })
    }

    /// Recovers the unclaimed window `[mark, rows.len())` as a positional
    /// argument. The normal expression path fires no per-argument events
    /// (only the const path emits `positional_argument`), so a group whose
    /// window holds exactly one row since `mark` reads that row as the
    /// single positional argument. A wider window never vanishes: the
    /// [`Self::composite_bool_arg`] face answers the statically-`bool`
    /// composites (the `==`/`!=`/relational/lazy-boolean family — dropping
    /// the window shifted every later argument onto the wrong formal, the
    /// #2245 misalignment), anything else records the unknown zero-span
    /// face so the positional pairing stays aligned (under-report).
    pub(crate) fn flush_unclaimed(&mut self) {
        let Some(level) = self.arg_levels.last() else {
            return;
        };
        if level.arg_open {
            return;
        }
        let mark = level.mark;
        let hi = self.rows.len();
        if hi == mark {
            return;
        }
        let arg = self.rows.get(mark).filter(|_| hi == mark + 1).map(|row| {
            (
                // A `?.`-merged argument reads the short-circuit's nullable
                // result (`g(a?.b)` offers `B?`).
                self.expression_row_ty(row),
                Self::primitive_literal(row, &self.members.primitive_ids),
                row.range,
                self.tear_off_sig(row),
            )
        });
        let composite = self.composite_bool_arg(mark, hi);
        let level = self.arg_levels.last_mut().expect("level just read");
        level.mark = hi;
        if let Some((ty, literal, range, sig)) = arg {
            if let Some(sig) = sig {
                self.arg_sigs.insert(range.offset, sig);
            }
            level.args.push((None, ty, literal, range));
        } else if let Some((ty, range)) = composite {
            level.args.push((None, ty, false, range));
        } else if hi > mark {
            level.args.push((
                None,
                Ty::UNKNOWN,
                false,
                SourceRange {
                    offset: 0,
                    length: 0,
                },
            ));
        }
    }

    /// Closes the pending argument: exactly one pushed row since the last
    /// argument boundary is a terminal expression whose type stands for the
    /// whole argument; anything else stays `Unknown` (compound operands must
    /// not lend their type), except the statically-`bool` composite face
    /// ([`Self::composite_bool_arg`]) which answers at the composite's own
    /// extent. An empty window pushes nothing.
    pub(crate) fn flush_argument(&mut self) {
        let Some(level) = self.arg_levels.last() else {
            return;
        };
        if !level.arg_open {
            return;
        }
        let mark = level.mark;
        let hi = self.rows.len();
        let name = level.pending_name;
        let arg = if hi == mark {
            None
        } else {
            match self.rows.get(mark).filter(|_| hi == mark + 1) {
                Some(row) => {
                    // A const creation's inner literal is the creation's
                    // own argument, never the enclosing call's value: the
                    // oracle types `f(const Constant(null))`'s argument
                    // with the creation's downward-instantiated container
                    // (its own faces own the `null`), so the fallback
                    // literal type must not leak into the outer check
                    // (`Expression<D>`'s `null` vs `Expression<D>` row the
                    // oracle never writes). The unknown posture stays
                    // silent exactly where the oracle's creation-typed
                    // argument is.
                    let covered =
                        row.ty == Ty::Null && self.const_capture.creation_covers(row.range.offset);
                    // A `?.`-merged argument reads the short-circuit's
                    // nullable result (`g(a?.b)` offers `B?`).
                    let ty = if covered {
                        Ty::UNKNOWN
                    } else {
                        self.expression_row_ty(row)
                    };
                    Some((
                        ty,
                        Self::primitive_literal(row, &self.members.primitive_ids),
                        row.range,
                        self.tear_off_sig(row),
                    ))
                }
                None => match self.composite_bool_arg(mark, hi) {
                    Some((ty, range)) => Some((ty, false, range, None)),
                    None => {
                        // The unmerged window's last row anchors the
                        // argument for the schema recovery (the type stays
                        // `Unknown` so the assignability face reads the same
                        // posture as before; the closure-schema face re-types
                        // the row on demand through the recorded range).
                        let anchor = hi
                            .checked_sub(1)
                            .and_then(|i| self.rows.get(i))
                            .map_or(
                                SourceRange {
                                    offset: 0,
                                    length: 0,
                                },
                                |row| row.range,
                            );
                        Some((Ty::UNKNOWN, false, anchor, None))
                    }
                },
            }
        };
        let level = self.arg_levels.last_mut().expect("level just read");
        level.arg_open = false;
        level.pending_name = None;
        level.mark = hi;
        if let Some((ty, literal, range, sig)) = arg {
            if let Some(sig) = sig {
                self.arg_sigs.insert(range.offset, sig);
            }
            level.args.push((name, ty, literal, range));
        }
    }

    /// The implicit-`call` invocation face: a value invocation `v(args)`
    /// whose static type names a container declaring a `call` method
    /// resolves to `v.call` — the interface member answers even when
    /// abstract (the xml corpus's `XPathExpression.call`). The member must
    /// be an instance method; a generic container instantiation and every
    /// function-typed alias keep their own faces (`None`). Returns the
    /// call method's formals, its declared return type, and its own
    /// type-parameter names (the inference parameters a generic
    /// `R call<R>(R x)` solves at the send).
    pub(crate) fn implicit_call_face(&self, ty: Ty) -> Option<(Formals, Ty, Box<[StringId]>)> {
        let Ty::Named {
            name: container,
            args: ArgListId::EMPTY,
            ..
        } = self.fold_alias_ty(ty)
        else {
            return None;
        };
        let call = self.intern("call");
        match self.walk_members(container, call) {
            Walk::Found(entry) if entry.kind == MemberKind::Method && !entry.is_static => {
                let (formals, return_ty, params) = match &entry {
                    Projected::Own(e) => (
                        e.formals.as_deref()?.clone(),
                        e.return_ty,
                        e.type_param_names.clone(),
                    ),
                    Projected::Foreign(e) => (
                        e.formals.as_deref()?.to_owned(),
                        e.return_ty,
                        e.type_param_names.clone(),
                    ),
                };
                Some((formals, return_ty, params))
            }
            _ => None,
        }
    }

    /// The tear-off signature of an argument row: a bare unqualified read
    /// of a same-file top-level function carries the function's own
    /// signature as its type-as-a-value (the row's `Ty` stays `Unknown` —
    /// the model has no function variant; the signature is the structural
    /// face). Getters carry no formals and synthesize nothing; member and
    /// prefixed tear-offs stay untyped (under-report). Only a value READ
    /// qualifies: a completed send's callee row keeps its bare shape (the
    /// send flips `intent` to [`Intent::Invoke`] alone), so an ungated
    /// shape check reads `use(f())`'s call result as a tear-off of `f`
    /// and the non-function-formal face co-reports a spurious
    /// `argument_type_not_assignable` (oracle 3.13.1 is silent there).
    pub(crate) fn tear_off_sig(&self, row: &Row) -> Option<Box<FnVarSig>> {
        if row.qualified || row.intent != Intent::Read {
            return None;
        }
        // A frame local shadows any same-spelled top-level declaration (the
        // scope precedence `resolve_bare` applies): the read is the local's
        // own value, never the function's tear-off — the row carries no
        // resolved-target signal (both faces push a `Plain` shape), so the
        // shadow check is the gate.
        if self.frames.iter().any(|f| f.locals.contains(&row.name))
            || self.shadowed_bare_rows.contains(&row.range.offset)
        {
            return None;
        }
        // A missing bare row (no in-file declaration at all) still reads
        // the facade below — the dart:core names are missing by
        // construction in the declaration index.
        let RowShape::Bare(BareInfo::Plain { .. } | BareInfo::Missing { .. }) = row.shape else {
            return None;
        };
        if let Some(sig) = self.top_level_decl(row.name).and_then(|decl| {
            if !matches!(decl.kind, MicroKind::TopLevelFn) {
                return None;
            }
            // A zero-formal declaration (`int top() => 0;`) carries no
            // captured formals: the empty shape is the signature.
            let formals = decl.formals.as_deref();
            let required = formals
                .map(|formals| {
                    formals
                        .positional
                        .iter()
                        .filter(|formal| formal.required)
                        .count()
                })
                .unwrap_or(0);
            Some(Box::new(FnVarSig {
                params: Box::default(),
                positional: formals
                    .map(|formals| formals.positional.iter().map(|f| f.ty).collect())
                    .unwrap_or_default(),
                required_positional: u16::try_from(required).unwrap_or(u16::MAX),
                named: formals
                    .map(|formals| {
                        formals
                            .named
                            .iter()
                            .map(|f| (f.name, f.ty, f.required))
                            .collect()
                    })
                    .unwrap_or_default(),
                return_ty: decl.return_ty,
            }))
        }) {
            return Some(sig);
        }
        // The dart:core facade: an unshadowed bare `print` read that no
        // in-file declaration answers carries the core function's own
        // signature (`void print(Object? object)`) — the same precedence
        // the unqualified invocation face applies (member, top-level, then
        // the core namespace).
        if super::types::CORE_TOP_LEVEL_FNS.contains(&self.strings.resolve(row.name))
            && !self.frames.iter().any(|f| f.locals.contains(&row.name))
            && !self
                .container_context()
                .filter(|(container, _)| {
                    matches!(self.walk_members(*container, row.name), Walk::Found(_))
                })
                .is_some()
        {
            return Some(Box::new(FnVarSig {
                params: Box::default(),
                positional: Box::from([Ty::Named {
                    name: self.intern("Object"),
                    nullable: true,
                    args: ArgListId::EMPTY,
                }]),
                required_positional: 1,
                named: Box::default(),
                return_ty: Ty::Void,
            }));
        }
        None
    }

    /// `ty` with every bare occurrence of a mapped type parameter replaced
    /// (`List<T>` under `T := Object?` → `List<Object?>`). `None` for the
    /// shapes this face does not rewrite (records, function types, a
    /// nullable type-parameter occurrence) — the caller suppresses.
    pub(crate) fn subst_params(&self, ty: Ty, map: &[(StringId, Ty)]) -> Option<Ty> {
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if args == ArgListId::EMPTY {
                    return match map.iter().find(|(param, _)| *param == name) {
                        Some(_) if nullable => None,
                        Some((_, to)) => Some(*to),
                        None => Some(ty),
                    };
                }
                let list: Vec<Ty> = self.boundary.args.borrow().get(args).to_vec();
                let mut out = Vec::with_capacity(list.len());
                for arg in list {
                    out.push(self.subst_params(arg, map)?);
                }
                Some(Ty::Named {
                    name,
                    nullable,
                    args: self.boundary.intern_list(&out),
                })
            }
            Ty::Record { shape, nullable } => {
                // A record-typed member face (`typedef R<T> = (T, {S? s})`
                // declared on a generic container) substitutes through its
                // fields exactly as the function face does: each positional
                // and named field carries the declaring container's
                // parameters, and the substituted fields re-intern
                // canonically. Any field the map cannot spell keeps the
                // whole face unsubstituted (the `?` chain) — the previous
                // wholesale `None` starved every record-returning accessor
                // on a generic receiver into `Unknown` (`b.rec?.copyWith()`
                // lost the receiver face and the extension-use credit).
                let pool_shape = {
                    let pool = self.boundary.pool();
                    pool.record_shape(shape).clone()
                };
                let positional = pool_shape
                    .positional
                    .iter()
                    .map(|ty| self.subst_params(*ty, map))
                    .collect::<Option<Vec<_>>>()?;
                let named = pool_shape
                    .named
                    .iter()
                    .map(|(name, ty)| Some((*name, self.subst_params(*ty, map)?)))
                    .collect::<Option<Vec<_>>>()?;
                Some(self.boundary.args.borrow_mut().record(&positional, &named, nullable))
            }
            Ty::Function { shape, nullable } => {
                // A function-typed member face (`FutureOr<T> Function(T)`
                // on a generic container) substitutes through its parts:
                // the return and each formal carry the declaring
                // container's parameters, and the substituted parts
                // re-intern canonically. Any part the map cannot spell
                // keeps the whole face unsubstituted (the `?` chain).
                let pool_shape = {
                    let pool = self.boundary.pool();
                    pool.function_shape(shape).clone()
                };
                let return_ty = self.subst_params(pool_shape.return_ty, map)?;
                let positional = pool_shape
                    .positional
                    .iter()
                    .map(|ty| self.subst_params(*ty, map))
                    .collect::<Option<Vec<_>>>()?;
                let named = pool_shape
                    .named
                    .iter()
                    .map(|(name, ty)| Some((*name, self.subst_params(*ty, map)?)))
                    .collect::<Option<Vec<_>>>()?;
                Some(self.boundary.intern_function_ty(
                    pool_shape.type_params,
                    return_ty,
                    &positional,
                    &named,
                    nullable,
                ))
            }
            other => Some(other),
        }
    }

    /// The upward-constraint collector (`generic_inferrer.dart`'s
    /// `TypeConstraintFromArgument` origins, upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/generic_inferrer.dart — each
    /// argument-expression type contributes a constraint where the
    /// corresponding formal's declared type mentions an inference
    /// parameter). One written formal walks against the argument's
    /// computed `Ty` and pushes every `(parameter name → matched portion)`
    /// pair the structure forces:
    ///
    /// * a bare `T` formal binds the whole argument type;
    /// * a `C<…>` formal against a same-head `C<…>` argument binds
    ///   pairwise through the argument lists;
    /// * the `List<E>`/`Set<E>` → `Iterable<E>` climb (the builtins
    ///   supertype table's only single-slot interface edge) crosses the
    ///   one head hop `Iterable<T>` formals need;
    /// * a `Function`/`Record` formal against the same shape binds
    ///   positionally (named slots never unify — the oracle's named-slot
    ///   pairing is outside this face).
    ///
    /// Everything else — a `T?` leaf (the substitution's own `None`
    /// shape), a mismatched head, an arity disagreement — binds nothing,
    /// the under-report direction the caller's disagreement rule keeps.
    fn unify_upward(
        &self,
        formal: Ty,
        arg: Ty,
        params: &[(StringId, Option<Ty>)],
        out: &mut Vec<(StringId, Ty)>,
    ) {
        match (formal, arg) {
            (
                Ty::Named {
                    name,
                    nullable: false,
                    args,
                },
                _,
            ) if args == ArgListId::EMPTY => {
                if params.iter().any(|(param, _)| *param == name) {
                    out.push((name, arg));
                }
            }
            (
                Ty::Named {
                    name,
                    nullable: false,
                    args: formal_args,
                },
                Ty::Named {
                    name: arg_head,
                    args: arg_args,
                    ..
                },
            ) if formal_args != ArgListId::EMPTY => {
                let pool = self.boundary.args.borrow();
                let (formal_args, arg_args) = (pool.get(formal_args), pool.get(arg_args));
                if name == arg_head && formal_args.len() == arg_args.len() {
                    for (f, a) in formal_args.iter().zip(arg_args.iter()) {
                        self.unify_upward(*f, *a, params, out);
                    }
                    return;
                }
                // The builtins edge: `List<E>`/`Set<E>` implement
                // `Iterable<E>` — the single-slot interface climb the
                // `Iterable<T>` formal needs to see its `T`.
                let iterable = self.intern("Iterable");
                if name == iterable && formal_args.len() == 1 && arg_args.len() == 1 {
                    let head = self.strings.resolve(arg_head);
                    if head == "List" || head == "Set" {
                        self.unify_upward(formal_args[0], arg_args[0], params, out);
                    }
                }
            }
            (
                Ty::Function {
                    shape: formal_shape,
                    nullable: false,
                },
                Ty::Function {
                    shape: arg_shape, ..
                },
            ) => {
                let pool = self.boundary.pool();
                let (formal_shape, arg_shape) = (
                    pool.function_shape(formal_shape),
                    pool.function_shape(arg_shape),
                );
                for (f, a) in formal_shape
                    .positional
                    .iter()
                    .zip(arg_shape.positional.iter())
                {
                    self.unify_upward(*f, *a, params, out);
                }
                self.unify_upward(formal_shape.return_ty, arg_shape.return_ty, params, out);
            }
            (
                Ty::Record {
                    shape: formal_shape,
                    nullable: false,
                },
                Ty::Record {
                    shape: arg_shape, ..
                },
            ) => {
                let pool = self.boundary.pool();
                let (formal_shape, arg_shape) = (
                    pool.record_shape(formal_shape),
                    pool.record_shape(arg_shape),
                );
                for (f, a) in formal_shape
                    .positional
                    .iter()
                    .zip(arg_shape.positional.iter())
                {
                    self.unify_upward(*f, *a, params, out);
                }
            }
            _ => {}
        }
    }

    /// A member send's inference parameters and receiver-side seeds —
    /// `reader.readEnum(...)`, `box.get()`, `this.adopt(...)`: the found
    /// method's own type-parameter names ([`MemberEntry::type_param_names`])
    /// join the declaring container's captured parameters
    /// ([`Bounds::container_params`], in-file; a uniquely-declared foreign
    /// container answers through [`Self::foreign_container_params`]). The
    /// receiver's written instantiation (`box: Box<int>`) seeds the
    /// container parameters only when the member lives on the receiver's
    /// own head — an inherited member declares its signatures in the
    /// supertype's parameter spellings, whose instantiation this
    /// name-level model cannot map (`C<T> extends B<String>` must not
    /// seed `B`'s `T` from `C<int>`).

    /// Whether a formal face mentions the callee's parameter spelling —
    /// the compound-substitution gate (only faces naming a solvable slot
    /// need the map; everything else keeps its written face byte-identical).
    fn formal_mentions_param(&self, ty: Ty, param: StringId) -> bool {
        match ty {
            Ty::Named { name, args, .. } => {
                if name == param {
                    return true;
                }
                let list = self.boundary.pool().get(args).to_vec();
                list.iter().any(|&arg| self.formal_mentions_param(arg, param))
            }
            Ty::Function { shape, .. } => {
                let pool_shape = {
                    let pool = self.boundary.pool();
                    pool.function_shape(shape).clone()
                };
                self.formal_mentions_param(pool_shape.return_ty, param)
                    || pool_shape
                        .positional
                        .iter()
                        .any(|&arg| self.formal_mentions_param(arg, param))
                    || pool_shape
                        .named
                        .iter()
                        .any(|&(_, arg)| self.formal_mentions_param(arg, param))
            }
            _ => false,
        }
    }
    /// A cross-file container's parameter names (the SDK's `Stream<T>` in
    /// a user unit): the unique declaring library's unit carries the
    /// declaration, and its `type_param_names` are the send-inference
    /// domain when this-file bounds hold no capture.
    fn foreign_container_params(&self, name: StringId) -> Option<Vec<StringId>> {
        let libs = self.graph.declaring_libraries(name);
        if libs.len() != 1 {
            return None;
        }
        let file = self.graph.unit_files(libs[0]).iter().copied().find(|&f| {
            self.index
                .get(f)
                .is_some_and(|unit| unit.container(name).is_some())
        })?;
        let names = self.index.get(file)?.container(name)?.type_param_names.to_vec();
        (!names.is_empty()).then_some(names)
    }

    fn member_send_params(
        &self,
        recv: &Receiver,
        member: StringId,
    ) -> Option<(Box<[(StringId, Option<Ty>)]>, Vec<(StringId, Ty)>)> {
        let (container, kind, recv_ty) = match *recv {
            Receiver::Typed { type_name, ty } => (type_name, ContainerKind::Class, Some(ty)),
            Receiver::This { container, kind } => (container, kind, None),
            _ => return None,
        };
        let Walk::Found(entry) = self.bare_member_walk(container, kind, member) else {
            return None;
        };
        if entry.kind != MemberKind::Method {
            return None;
        }
        let own_member = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .is_some_and(|(c, _)| c.members.get(&member).is_some());
        let mut params: Vec<(StringId, Option<Ty>)> = entry
            .type_param_names
            .iter()
            .map(|name| (*name, None))
            .collect();
        // The declaring container's parameters join only for the
        // receiver's own member: an inherited member's formals spell the
        // SUPERTYPE's parameter names, which happen to collide with the
        // receiver's own spelling (`C<T> extends B<String>` calling
        // `B`'s `m(T)` — binding `C`'s `T` from the argument would invent
        // a solution the oracle never derives). With no seed the foreign
        // member's bare-parameter formal binds nothing (the formal spells
        // a name outside this param list, so `unify_upward` declines it).
        let container_params: Vec<(StringId, Option<crate::bounds::TypeTree>, bool)> = if own_member {
            match self.bounds.container_params(container) {
                Some(params) if !params.is_empty() => params,
                // The SDK's own containers (`Stream<T>` from a user unit)
                // hold no this-file capture; their names still seed the
                // receiver's written instantiation.
                _ => self
                    .foreign_container_params(container)
                    .map(|names| names.into_iter().map(|name| (name, None, false)).collect())
                    .unwrap_or_default(),
            }
        } else {
            Vec::new()
        };
        let mut seeds: Vec<(StringId, Ty)> = Vec::new();
        if own_member && let Some(Ty::Named { args, .. }) = recv_ty {
            let pool = self.boundary.args.borrow();
            let args = pool.get(args);
            if args.len() == container_params.len() {
                for ((param, _, _), ty) in container_params.iter().zip(args.iter()) {
                    seeds.push((*param, *ty));
                }
            }
        }
        params.extend(
            container_params
                .iter()
                .map(|(param, bound, _)| (*param, bound.as_ref().map(|tree| self.tree_ty(tree)))),
        );
        if params.is_empty() {
            return None;
        }
        Some((params.into_boxed_slice(), seeds))
    }

    /// The generic inference-failure face (`couldNotInfer`, upstream Dart
    /// SDK source, pkg/analyzer/lib/src/dart/element/generic_inferrer.dart
    /// `tryChooseFinalTypes` :298-380, message assembly `_formatError`
    /// :538-585 / `_formatConstraints` :789-818; the origin lines are
    /// `TypeConstraintFromArgument` / `TypeConstraintFromReturnType`,
    /// pkg/_fe_analyzer_shared/lib/src/type_inference/type_constraint.dart).
    /// The solver checks each inferred type against every recorded
    /// constraint and reports, at the invocation's constructor name, the
    /// first type parameter whose solution violates one.
    ///
    /// Implemented for one shape class: an unnamed generic creation
    /// `C(<literal>)` without explicit type arguments, passed straight as
    /// a positional argument of an uninstantiated same-library generic
    /// function call that is the whole initializer of an unannotated
    /// declaration (`var x = check(C((List<int> x) {}));`). There the
    /// creation's downward schema is the outer formal (`C<List<T>>`) with
    /// the outer, still-unknown parameters as `_` (`C<List<_>>`); a class
    /// parameter whose schema slot holds `_` is solved by the return
    /// constraint alone to the slot with `_` := `Object?` (oracle-pinned,
    /// `List<Object?>` / `Map<String, Object?>`), and the function-literal
    /// argument's constraint is then checked against that solution. Slots
    /// without `_` are fixed in the downward phase and never re-checked
    /// (`_typesInferredSoFar`). Any context on the outer call (an
    /// annotated declaration, an enclosing argument, a return) fixes its
    /// parameters, the schema loses its `_`, and nothing reports — so
    /// every other placement suppresses (under-report). The general face
    /// — constraint gathering over arbitrary argument/context types,
    /// constraint merging, and the solver's choice — needs a constraint
    /// solver this engine does not have; this verdict is its seam.
    pub(crate) fn check_could_not_infer(
        &mut self,
        callee: usize,
        container: StringId,
        params: &[(StringId, Option<Ty>)],
        formals: &Formals,
        level: &ArgLevel,
        close: SourceRange,
    ) {
        // The outer generic call: its argument list is the innermost open
        // one, and it is the whole initializer of an unannotated
        // declaration (no downward context for its own parameters).
        let Some(outer_level) = self.arg_levels.last() else {
            return;
        };
        if self.arg_levels.len() != 1
            || outer_level.arg_open && outer_level.pending_name.is_some()
            || !self.in_initializer
            || self.pending_decl_ty.is_some()
            || self.init_frame_depth != self.frames.len()
        {
            return;
        }
        let Some(&Some(outer)) = self
            .invokes
            .len()
            .checked_sub(2)
            .and_then(|index| self.invokes.get(index))
        else {
            return;
        };
        // The creation is the outer call's first argument (an earlier
        // compound argument would leave the positional index unattributed).
        let outer_offset = self.rows.get(outer).map(|row| row.range.offset);
        if self.init_mark != Some(outer)
            || callee != outer + 1
            || !outer_level.args.is_empty()
            || self
                .call_type_args
                .iter()
                .any(|(at, _)| Some(*at) == outer_offset)
        {
            return;
        }
        let Some(outer_row) = self.rows.get(outer) else {
            return;
        };
        if outer_row.qualified || !matches!(outer_row.shape, RowShape::Bare(BareInfo::Plain { .. }))
        {
            return;
        }
        let outer_name = outer_row.name;
        if self.frames.iter().any(|f| f.locals.contains(&outer_name)) {
            return;
        }
        let Some(outer_params) = self.exec_bounds.get(&outer_name).filter(|p| !p.is_empty()) else {
            return;
        };
        let Some(outer_formal) = self.top_level_decl(outer_name).and_then(|decl| {
            decl.formals
                .as_deref()
                .and_then(|f| f.positional.first())
                .map(|f| f.ty)
        }) else {
            return;
        };
        let Ty::Named {
            name: head,
            nullable: false,
            args: schema_args,
        } = outer_formal
        else {
            return;
        };
        let schema_args: Vec<Ty> = self.boundary.args.borrow().get(schema_args).to_vec();
        if head != container || schema_args.len() != params.len() {
            return;
        }
        // The schema (`_` for the outer parameters) and the return-only
        // solution (`Object?` for them), slot by slot.
        let hole = Ty::named(self.intern("_"));
        let top = Ty::Named {
            name: self.intern("Object"),
            nullable: true,
            args: ArgListId::EMPTY,
        };
        let to_hole: Vec<(StringId, Ty)> = outer_params.iter().map(|(p, _)| (*p, hole)).collect();
        let to_top: Vec<(StringId, Ty)> = outer_params.iter().map(|(p, _)| (*p, top)).collect();
        let mut schema = Vec::with_capacity(params.len());
        let mut solved = Vec::with_capacity(params.len());
        let mut open = Vec::with_capacity(params.len());
        for arg in &schema_args {
            let (Some(with_hole), Some(with_top)) = (
                self.subst_params(*arg, &to_hole),
                self.subst_params(*arg, &to_top),
            ) else {
                return;
            };
            open.push(with_hole != *arg);
            schema.push(with_hole);
            solved.push(with_top);
        }
        // A declared bound joins the constraint set (the `extends` check);
        // this face does not model it.
        if params.iter().any(|(_, bound)| bound.is_some()) {
            return;
        }
        // The constructor's written function-typed formals pair with its
        // positional formals one to one (the argument check's slot face).
        let Some(table) = self.fn_formal_sigs.get(&container) else {
            return;
        };
        // The argument list is exactly one function literal: its window
        // spans the body's rows, so the row recovery leaves it
        // unattributed — the literal pairs by source adjacency instead
        // (`C(` then the literal then `)`, trivia only between).
        let callee_end = self.rows[callee].range.end();
        let [(start, end, arg_sig)] = level.closures.as_slice() else {
            return;
        };
        let opens = self
            .source
            .get(callee_end as usize..*start as usize)
            .is_some_and(|text| text.trim() == "(");
        if !opens
            || !self.trivia_between(*end, close.offset)
            || !formals.named.is_empty()
            || formals.positional.len() != 1
            || table.positional.len() != 1
        {
            return;
        }
        let mut reports = Vec::new();
        let class_map: Vec<(StringId, Ty)> = params
            .iter()
            .zip(&solved)
            .map(|((param, _), ty)| (*param, *ty))
            .collect();
        for (index, (param, _)) in params.iter().enumerate() {
            if !open[index] {
                continue;
            }
            let probe = [(*param, hole)];
            // The argument constraints on this parameter: exactly one formal
            // signature mentions it (more origins need the merge order).
            let mut mentioning = table.positional.iter().enumerate().filter(|(_, sig)| {
                sig.positional
                    .iter()
                    .any(|ty| self.subst_params(*ty, &probe) != Some(*ty))
            });
            let Some((slot, formal_sig)) = mentioning.next() else {
                continue;
            };
            if mentioning.next().is_some()
                || !formal_sig.params.is_empty()
                || !formal_sig.named.is_empty()
                || self.subst_params(formal_sig.return_ty, &probe) != Some(formal_sig.return_ty)
            {
                return;
            }
            let mut positional = Vec::with_capacity(formal_sig.positional.len());
            for ty in &formal_sig.positional {
                let Some(ty) = self.subst_params(*ty, &class_map) else {
                    return;
                };
                positional.push(ty);
            }
            let instantiated = FnVarSig {
                params: Box::default(),
                positional: positional.into_boxed_slice(),
                required_positional: formal_sig.required_positional,
                named: Box::default(),
                return_ty: formal_sig.return_ty,
            };
            if self.fn_sig_subtype(arg_sig, &instantiated) != Some(false) {
                continue;
            }
            let param_text = self.strings.resolve(*param).to_string();
            let formal_name = self
                .strings
                .resolve(formals.positional[slot].name)
                .to_string();
            let inferred = self.ty_display(solved[index]);
            let declared_args: Vec<Ty> = params.iter().map(|(p, _)| Ty::named(*p)).collect();
            let declared = Ty::Named {
                name: container,
                nullable: false,
                args: self.boundary.intern_list(&declared_args),
            };
            let context = Ty::Named {
                name: container,
                nullable: false,
                args: self.boundary.intern_list(&schema),
            };
            // `_formatConstraints`: `  {prefix}{pad} {middle}` then
            // `\n  {pad to the group's widest prefix} {end}` — one origin
            // per group here, so the pad is the prefix's own width.
            let arg_prefix = format!("Parameter '{formal_name}'");
            let unsatisfied = format!(
                "  {arg_prefix} declared as     '{}'\n  {} but argument is '{}'.",
                self.fn_sig_display(formal_sig),
                " ".repeat(arg_prefix.len()),
                self.fn_sig_display(arg_sig),
            );
            let ret_prefix = "Return type";
            let satisfied = format!(
                "\nThe type '{inferred}' was inferred from:\n  {ret_prefix} declared as '{}'\n  {} used where  '{}' is required.",
                self.ty_display(declared),
                " ".repeat(ret_prefix.len()),
                self.ty_display(context),
            );
            let detail = format!(
                "\n\nTried to infer '{inferred}' for '{param_text}' which doesn't work:\n\
                 {unsatisfied}{satisfied}\n\n\
                 Consider passing explicit type argument(s) to the generic.\n\n"
            );
            reports.push(SemanticCode::CouldNotInfer.message(&param_text, &detail));
        }
        // One row per failing parameter, in declaration order, all at the
        // constructor name (the solver keeps the erroneous type and walks
        // on — `_diagnosticEntity(errorEntity!)`).
        let range = self.rows[callee].range;
        for message in reports {
            self.emit_message_with_correction(SemanticCode::CouldNotInfer, range, message);
        }
    }

    /// The argument face's verdict — the single relation every argument walk
    /// and the staged teardown share (`arg: T` checked against a formal `T`).
    ///
    /// The composition, in order:
    ///
    /// 1. A source head spelling an import prefix is a **broken projection**,
    ///    never a type: the parser hands `handle_type` the prefix token of a
    ///    prefixed annotation (`ast.AstNode?`), so the continuation name is
    ///    lost and the armed `Ty` carries the prefix as its head (the type's
    ///    own decliners live in the parser/collector seam, not here — this
    ///    gate only stops the verdicts the broken head would fabricate).
    ///    Every verdict over such a head is fiction: the nullability rule
    ///    answers `Some(false)` for the `?` alone, so a narrowed read (`if
    ///    (node is ClassDeclaration) take(node)` over an `ast.AstNode?`
    ///    parameter) reports `argument_type_not_assignable` with `ast?` in
    ///    the message. The oracle resolves the annotation and stays silent,
    ///    so the relation must stay undecided here (the under-report
    ///    direction).
    /// 2. The assignability matrix ([`Engine::assignable`]) decides
    ///    everything it can resolve — literals, nullability, generics,
    ///    function shapes, and the declared supertype graph
    ///    (`implements_interface`), which is where the three-hop
    ///    `implements` chain of a narrowed argument is already proven.
    /// 3. On the matrix's `None` cut between two bare named heads, the
    ///    declared-edge name lattice ([`NameWorld::decide_subtype`]) decides
    ///    the plain in-file pair over the same edges the switch tracker
    ///    walks: the superclass/`with`/`implements`/`on` chain plus the core
    ///    edges. This is the consistency leg — one relation for every
    ///    argument site, never a second hierarchy.
    ///
    /// `None` means undecided: neither a rejection to report nor a proof of
    /// compatibility. Callers reject only on `Some(false)`.
    pub(crate) fn argument_verdict(&self, from: Ty, to: Ty, literal: bool) -> Option<bool> {
        if self.prefix_head_ty(from) {
            return None;
        }
        match self.assignable(from, to, literal) {
            Some(verdict) => Some(verdict),
            None => match (from, to) {
                (
                    Ty::Named {
                        name: sub,
                        args: sub_args,
                        ..
                    },
                    Ty::Named {
                        name: sup,
                        args: sup_args,
                        ..
                    },
                ) if sub_args == ArgListId::EMPTY && sup_args == ArgListId::EMPTY => {
                    self.name_world().decide_subtype(sub, sup)
                }
                _ => None,
            },
        }
    }

    /// Whether `ty`'s head is an import prefix rather than a type — the
    /// signature of a mis-projected prefixed annotation
    /// ([`Engine::argument_verdict`]).
    ///
    /// Two lookup faces cover the two ways the broken head reaches an
    /// argument check: the diagnosing library's own `as` spelling (the
    /// annotation is written in this file), and — for a foreign-origin type
    /// the boundary projected (`getLinkingNode` returns `ast.AstNode?` over
    /// in `link.dart`; the local `node` it feeds carries the prefix head
    /// into `simply_bounded.dart`, which registers no `ast` prefix at all)
    /// — a graph-wide prefix registration whose name resolves to nothing
    /// the ordinary face could name. The second face requires the bare
    /// name to be unresolvable here: a declaration, core spelling, or
    /// import the diagnosing file actually sees still takes its ordinary
    /// verdict (a user-typed `ast` local, a `dart:core` name, an imported
    /// declaration) — only a head that is simultaneously a registered
    /// prefix somewhere and no nameable type here is the projection scar.
    pub(crate) fn prefix_head_ty(&self, ty: Ty) -> bool {
        let Ty::Named { name, .. } = ty else {
            return false;
        };
        if let Some(verdict) = self.prefix_head_memo.borrow().get(&name) {
            return *verdict;
        }
        let verdict = self.prefix_head_walk(name);
        self.prefix_head_memo.borrow_mut().insert(name, verdict);
        verdict
    }

    /// The unmemoized [`Engine::prefix_head_ty`] walk: the namespaces
    /// probe, the `name_resolvable` probe, then the prefix-name set —
    /// assembled once from every library's import edges (the O(L+E) scan
    /// the per-call walk used to pay).
    fn prefix_head_walk(&self, name: StringId) -> bool {
        if matches!(
            self.namespaces.lookup(self.graph, self.lib, name),
            dartq_resolution::Lookup::Found(NamespaceEntry::Prefix)
        ) {
            return true;
        }
        if self.name_resolvable(name) {
            return false;
        }
        self.prefix_names
            .get_or_init(|| {
                self.graph
                    .libraries()
                    .flat_map(|lib| self.graph.import_edges(lib).iter())
                    .filter_map(|edge| edge.prefix)
                    .collect()
            })
            .contains(&name)
    }

    /// Whether `name` denotes any type the diagnosing file can spell: a
    /// container in this file or an import its namespace reaches (the
    /// ordinary bare-name resolution [`resolve_container`] runs), a core
    /// primitive, or the implicit `Object`/`Enum` heads the name lattice
    /// grants every unit. The [`Engine::prefix_head_ty`] graph-wide face
    /// skips these — a foreign projection never claims a head the file
    /// itself could name.
    fn name_resolvable(&self, name: StringId) -> bool {
        name == self.intern("Object")
            || name == self.intern("Enum")
            || self.members.primitive_ids.contains(&name)
            || resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            )
            .is_some()
    }

    /// The enum constant's implicit constructor invocation (`low(1)` over
    /// `const Level5(this.describe)`): the constant's argument list checks
    /// against the enclosing enum's selected constructor's formals — the
    /// `.named` selector (when written) keys `ctor_formals`, else the enum's
    /// own name keys the unnamed constructor. The oracle resolves this
    /// invocation as a `const` constructor call, so a type mismatch reports
    /// the pair `const_constructor_param_type_mismatch` +
    /// `argument_type_not_assignable` at the offending argument's span —
    /// gated on the enum's declared const constructor (the spec's enum rule;
    /// a const-carrying enum is the only shape this arm sees). Arity and the
    /// named faces anchor as in [`Engine::check_arguments`]: the deficit at
    /// the after-the-last-positional face (the enum name in the message),
    /// the surplus at the first extra positional, `undefined_named_parameter`
    /// at the label.
    pub(crate) fn check_enum_const_arguments(
        &mut self,
        level: &ArgLevel,
        positional: u16,
        close: SourceRange,
    ) {
        let Some((_, selector)) = self.enum_const_call else {
            return;
        };
        let Some((enum_name, _)) = self.container_context() else {
            return;
        };
        let key = selector.map(|(name, _)| name).unwrap_or(enum_name);
        // The constant's named arguments are call-site givens of the
        // selected constructor (`a(test: 0)` gives `test`): the
        // `unused_element_parameter` rule counts them like any other
        // invocation's (probed: dart analyze 3.13.1 — `a(test: 0)` over
        // `const E({this.test = 0})` reports nothing).
        for (given, ..) in &level.args {
            if let Some(given) = given {
                self.named_given.insert((key, *given));
            }
        }
        let Some((formals, const_ctor)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            enum_name,
        )
        .and_then(|(container, owner)| {
            let mut formals = (**container.ctor_formals.get(&key)?).clone();
            // The resolved table lives in `owner`'s pool — re-intern every
            // `formal.ty` into the diagnosing pool before the per-argument
            // assignability walk derefs a slot (a same-file owner is a
            // no-op).
            self.project_formals(owner, &mut formals);
            // An untyped field formal (`this.describe`) takes its field's
            // declared type (`FieldFormalParameterElement.type`); the field
            // may be declared after the constructor, so the whole-file
            // member table answers it here.
            for formal in formals
                .positional
                .iter_mut()
                .chain(formals.named.iter_mut())
            {
                if formal.ty == Ty::UNKNOWN
                    && let Some(entry) = container.members.get(&formal.name)
                    && entry.kind == MemberKind::Field
                    && !entry.is_static
                {
                    // The field's declared type shares the constructor
                    // table's foreign pool — project it too.
                    formal.ty = self.boundary.ty(owner, entry.var_ty);
                }
            }
            Some((formals, container.has_const_constructor))
        }) else {
            return;
        };
        // The count faces precede the per-argument walk (the oracle's
        // `resolveArgumentsToParameters` runs the arity checks before any
        // assignability verdict — `not_enough_positional_arguments` before
        // `undefined_named_parameter` on a named-deficit shape). A deficit
        // anchors inside the parens (the oracle's `leftParenthesis.next ??
        // rightParenthesis` — the first named label, else the closer); a
        // surplus reports at the first extra positional. `given` is the
        // positional supply — the parser's total less the named entries.
        let named = level
            .args
            .iter()
            .filter(|(name, ..)| name.is_some())
            .count() as u16;
        let given = positional.saturating_sub(named);
        let required = formals.positional.iter().filter(|f| f.required).count() as u16;
        if given < required {
            let anchor = level
                .args
                .iter()
                .filter(|(name, .., range)| name.is_none() && range.length > 0)
                .next_back()
                .map(|(.., range)| SourceRange {
                    offset: range.offset + range.length,
                    length: 1,
                })
                .or(level.first_label)
                .unwrap_or(close);
            let enum_text = self.strings.resolve(enum_name).to_string();
            let (shape, a0, a1) = if required == 1 {
                (
                    ArgCountShape::NameSingular,
                    enum_text.clone(),
                    String::new(),
                )
            } else {
                (
                    ArgCountShape::NamePlural,
                    required.to_string(),
                    given.to_string(),
                )
            };
            let code = SemanticCode::NotEnoughPositionalArguments(shape);
            let message = match shape {
                ArgCountShape::NamePlural => code.message3(&a0, &a1, &enum_text),
                _ => code.message(&a0, &a1),
            };
            self.emit_message_with_correction(code, anchor, message);
        } else if given > formals.positional.len() as u16 {
            let capacity = formals.positional.len() as u16;
            let mut seen = 0usize;
            let mut surplus = None;
            for (name, _, _, range) in &level.args {
                if name.is_some() {
                    continue;
                }
                seen += 1;
                if seen == capacity as usize + 1 {
                    surplus = Some(*range);
                }
            }
            let anchor = (seen == given as usize)
                .then_some(surplus)
                .flatten()
                .unwrap_or(close);
            let code = SemanticCode::ExtraPositionalArguments;
            let message = code.message(&capacity.to_string(), &given.to_string());
            self.emit_message_with_correction(code, anchor, message);
        }
        // The argument walk pairs positionals by index, named by spelling,
        // reporting each type mismatch's code pair at the argument's span.
        let mut positional_index = 0usize;
        for (name, ty, literal, range) in &level.args {
            let formal = if let Some(n) = name {
                match self.named_formal_for(&formals, *n) {
                    Some(formal) => formal,
                    // The unknown named parameter: `undefined_named_parameter`
                    // anchors at the label (the constant's own name anchors
                    // `not_enough` — the label is the miss's span upstream).
                    None => {
                        let text = self.strings.resolve(*n).to_string();
                        let label = level
                            .named_labels
                            .iter()
                            .find(|(name, _)| name == n)
                            .map(|(_, range)| *range)
                            .unwrap_or(*range);
                        self.emit(SemanticCode::UndefinedNamedParameter, label, &text, "");
                        continue;
                    }
                }
            } else {
                let formal = formals.positional.get(positional_index);
                positional_index += 1;
                match formal {
                    Some(formal) => formal,
                    None => continue,
                }
            };
            // [`Engine::argument_verdict`]: the matrix, the prefix-projection
            // gate, and the declared name lattice on the matrix's `None` cut
            // (unrelated in-file heads with no edge reject).
            // The flow read's re-bound or promoted face outranks the
            // parse-time capture — the oracle checks the argument's static
            // type at the send (`uriSource` past `if (uriSource is
            // InSummarySource)` reads `InSummarySource`, never the declared
            // `Source?`); a divergent or absent read keeps the captured
            // face.
            let rebound = self.rebound_read_face(*range);
            let read_at = if rebound.is_none() {
                self.read_type_at(*range)
            } else {
                None
            };
            let flow_ty = rebound.or(read_at).unwrap_or(*ty);
            let rejects = self.argument_verdict(flow_ty, formal.ty, *literal) == Some(false);
            if rejects {
                let from_text = self.ty_display(flow_ty);
                let to_text = self.ty_display(formal.ty);
                dq_probe!(
                    "[arg-type] offset={} from={} to={} (rebound={} read_at={} captured={})",
                    range.offset,
                    from_text,
                    to_text,
                    rebound.is_some(),
                    self.ty_display(*ty) != from_text,
                    self.ty_display(*ty)
                );
                // The const-constructor face precedes the assignability row —
                // the oracle's constant evaluator runs the invocation's
                // argument check before the general assignability verifier
                // reports the same span.
                if const_ctor {
                    let message = SemanticCode::ConstConstructorParamTypeMismatch
                        .message(&from_text, &to_text);
                    self.emit_message_with_correction(
                        SemanticCode::ConstConstructorParamTypeMismatch,
                        *range,
                        message,
                    );
                }
                let message =
                    SemanticCode::ArgumentTypeNotAssignable.message3(&from_text, &to_text, "");
                self.emit_message_with_correction(
                    SemanticCode::ArgumentTypeNotAssignable,
                    *range,
                    message,
                );
            }
        }
    }

    /// The public spelling a private named formal resolves to at call sites
    /// (upstream `correspondingPublicName`,
    /// pkg/_fe_analyzer_shared/lib/src/scanner/token_impl.dart:364): a
    /// `this._x`/`super._x` named formal's element stores the public `x`,
    /// so callers spell `x:` — never `_x:` — and `missing_required_argument`
    /// renders `x`. `None` keeps the written name: a non-private spelling,
    /// the wildcard `_`, a leading digit, a second `_`, or a reserved word
    /// (`_if` has no public face and reports
    /// `private_named_parameter_without_public_name` upstream instead).
    pub(crate) fn formal_public_name(&self, name: StringId) -> StringId {
        let text = self.strings.resolve(name);
        let Some(rest) = text.strip_prefix('_') else {
            return name;
        };
        let valid = rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '$')
            && !matches!(
                dartq_scanner::keyword_from_str(rest),
                Some(k) if k.style() == dartq_scanner::KeywordStyle::Reserved
            );
        if valid { self.intern(rest) } else { name }
    }

    /// The named formal a written `name:` argument resolves to: exact
    /// spelling first, then the private-formal public face (upstream keys
    /// `NamedExpression.name` against `ParameterElement.name`, which is the
    /// public name — `AsyncData._(…, error: e)` binds `required this._error`).
    /// A given `_x:` resolves nothing when `_x`'s public face exists — the
    /// element only answers `x:`.
    pub(crate) fn named_formal_for<'f>(
        &self,
        formals: &'f Formals,
        name: StringId,
    ) -> Option<&'f Formal> {
        let public = self.formal_public_name(name);
        if public != name {
            // The written argument is already private (`_x:`): it resolves a
            // formal whose element name is exactly `_x` — a formal carrying
            // no public face (else its element name is `x`).
            return formals
                .named_formal(name)
                .filter(|f| self.formal_public_name(f.name) == f.name);
        }
        formals.named_formal(name).or_else(|| {
            formals
                .named
                .iter()
                .find(|f| self.formal_public_name(f.name) == name)
        })
    }

    /// The closed group's explicit super-constructor invocation, when it is
    /// one: `Some((enclosing container, selector, bare))` — the selector
    /// shape (`: super.named(...)`) answers through the pending candidate
    /// the member merge latched (the folded value row tops the group and
    /// its offset keys the candidate), the bare shape (`: super(...)`)
    /// through the keyword receiver row the group open latched as
    /// `callee_row`. `bare` marks that third geometry: at label time the
    /// named-membership face (`Walk::check_named_argument`) already reads
    /// the same callee row, so this walk leaves the label miss to it
    /// whenever the table resolved. `super.new(...)` normalizes to the
    /// unnamed face (`None` selector). An assignment shape (`super.name =
    /// v`) opens no group, so it never reaches here.
    fn super_invocation_callee(
        &self,
        level: &ArgLevel,
    ) -> Option<(StringId, Option<StringId>, bool)> {
        // The selector shape at the group's close: the fast parser opens the
        // argument group before the dot-access event merges `super.n`, so
        // the pending candidate (and the merged value row) do not exist yet
        // — the unmerged member placeholder tops the group and the bare
        // `super` keyword receiver directly below it names the container.
        let callee = self.invokes.last().and_then(|slot| *slot);
        if let Some(index) = callee
            && let Some(row) = self.rows.get(index)
            && row.qualified
            && let RowShape::Bare(BareInfo::Plain { .. }) = row.shape
            && let Some(below) = index.checked_sub(1).and_then(|i| self.rows.get(i))
            && let RowShape::Receiver(Receiver::Super { container, .. }) = below.shape
            && below.name == self.intern("")
        {
            let selector = (row.name != self.intern("new")).then_some(row.name);
            return Some((container, selector, false));
        }
        // The merged shape (the dot-access fold ran early — the
        // scanner-synthesized rider): the pending candidate the merge
        // latched answers through the folded value row's offset.
        let selector_row = self
            .invokes
            .last()
            .and_then(|slot| *slot)
            .and_then(|index| self.rows.get(index))
            .map(|row| row.range.offset);
        if let Some(offset) = selector_row
            && let Some(candidate) = self
                .pending_init_super
                .iter()
                .rev()
                .find(|candidate| candidate.invoked && candidate.selector_offset == offset)
        {
            let selector = (candidate.name != self.intern("new")).then_some(candidate.name);
            return Some((candidate.container, selector, false));
        }
        let index = level.callee_row?;
        let row = self.rows.get(index)?;
        match &row.shape {
            RowShape::Receiver(Receiver::Super { container, .. })
                if row.name == self.intern("") =>
            {
                Some((*container, None, true))
            }
            _ => None,
        }
    }

    /// The explicit super-constructor invocation's argument faces
    /// (`constructor_invocation_resolver.dart` resolves the
    /// `SuperConstructorInvocation`, then `resolveArgumentsToParameters`
    /// pairs the arguments): the resolved super constructor's formals
    /// decide the count faces (`extra_positional_arguments[_could_be_named]`
    /// / `not_enough_positional_arguments`) and the per-argument
    /// assignability face at the argument's own span. The named-coverage
    /// face splits by geometry: the bare callee-row shape's label misses
    /// belong to `Walk::check_named_argument` (it reads the same row at
    /// label time — `bare && super_container.is_some()` suppresses the
    /// emit here), every selector shape and every empty-table face
    /// (unresolved superclass, no `extends`) reports at this walk. An
    /// unresolvable superclass or selector keeps the under-report posture
    /// (the selector existence face, `undefined_constructor_in_initializer`,
    /// reports separately at the list's close). `super.new(...)` selects
    /// the unnamed face, the same filter the super-formal drain applies.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn check_super_invocation_arguments(
        &mut self,
        level: &ArgLevel,
        positional: u16,
        close: SourceRange,
        container: StringId,
        selector: Option<StringId>,
        bare: bool,
    ) {
        let superclass = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass);
        let (mut formals, super_container, resolved_super) = match superclass {
            Some(superclass) => {
                match resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    superclass,
                ) {
                    Some((super_container, super_owner)) => {
                        let key = selector
                            .filter(|name| *name != self.intern("new"))
                            .unwrap_or(superclass);
                        let Some(formals) = super_container
                            .ctor_formals
                            .get(&key)
                            .map(|f| (**f).clone())
                        else {
                            // A resolved class without the selector: the
                            // `undefined_constructor_in_initializer` face
                            // reports at the list's close, and the oracle
                            // pairs no arguments against it.
                            return;
                        };
                        // A chained forward's sentinel resolves against the
                        // hop above it (the super constructor's own
                        // `super.name` formal reads the grandparent's
                        // associated parameter). The resolved table lives in
                        // `super_owner`'s pool, so the clone re-interns into
                        // the diagnosing pool before the argument walk
                        // derefs a `formal.ty` slot (a same-file owner is a
                        // no-op).
                        let mut formals = formals;
                        self.project_formals(super_owner, &mut formals);
                        self.patch_super_formals(superclass, &mut formals);
                        // The subclass's written instantiation substitutes
                        // the superclass's own parameters in the formals
                        // (`PathMap<V> extends MapView<String?, V>` pairs
                        // `MapView(Map<K, V>?)`'s `K := String?`) — the
                        // element model's clause face carries names only,
                        // so the written argument group is the only source.
                        if let Some(args) = self.clause_instantiation(container, superclass)
                            && !super_container.type_param_names.is_empty()
                        {
                            // The clause scan records one `Ty` per clause
                            // entry — a single instantiated head carries its
                            // own written arguments, so the positional
                            // pairing reads them out of the pool first.
                            let written: Vec<Ty> = match args.as_slice() {
                                [Ty::Named {
                                    args: arg_list, ..
                                }] if *arg_list != crate::model::ArgListId::EMPTY => {
                                    self.boundary.args.borrow().get(*arg_list).to_vec()
                                }
                                many => many.to_vec(),
                            };
                            if written.len() == super_container.type_param_names.len() {
                                let map: Vec<(StringId, Ty)> = super_container
                                    .type_param_names
                                    .iter()
                                    .copied()
                                    .zip(written.iter().copied())
                                    .collect();
                                for formal in formals
                                    .positional
                                    .iter_mut()
                                    .chain(formals.named.iter_mut())
                                {
                                    if let Some(ty) = self.subst_params(formal.ty, &map) {
                                        formal.ty = ty;
                                    }
                                }
                            }
                        }
                        (
                            formals,
                            Some((super_container, super_owner)),
                            Some(superclass),
                        )
                    }
                    // The dead-edge super face (`resolver.dart`'s null
                    // element arm): an unresolvable superclass — the
                    // `extends Missing` shape, `EXTENDS_NON_CLASS` both
                    // sides — resolves no super constructor, and the
                    // oracle's `resolveArgumentList` pairs the arguments
                    // against an EMPTY formal table (every named argument
                    // undefined, every surplus positional extra; probed).
                    // Only the unnamed face: a written selector over the
                    // dead edge resolves against the implicit `Object`
                    // and misses it — the existence face
                    // (`undefined_constructor_in_initializer`) reports
                    // alone, with NO argument faces (probed:
                    // `super.delegate(p, x: 1)` over `extends Missing`).
                    None if selector.is_none()
                        && self.supertype_hop_dead(superclass, self.lib, self.file) =>
                    {
                        (
                            Formals {
                                positional: Box::default(),
                                named: Box::default(),
                            },
                            None,
                            None,
                        )
                    }
                    // A hop the index cannot model (the oracle's full
                    // world could relate it) keeps the under-report
                    // posture; a written selector over any dead hop
                    // defers to the existence face the same way.
                    None => return,
                }
            }
            // No `extends`: the implicit `Object` face declares no formals
            // for the unnamed face (probed: `class D { D() : super(1, x: 2);
            // }` reports the surplus and the undefined named against the
            // empty table). A written selector misses on `Object` and
            // reports only the existence face, like every dead hop.
            None if selector.is_none() => (
                Formals {
                    positional: Box::default(),
                    named: Box::default(),
                },
                None,
                None,
            ),
            None => return,
        };
        // An untyped field formal (`this.name`) takes its field's declared
        // type — the field may be declared after the constructor, so the
        // whole-file member table answers it (the enum-constant walk
        // applies the same face).
        if let Some((super_container, super_owner)) = super_container {
            for formal in formals
                .positional
                .iter_mut()
                .chain(formals.named.iter_mut())
            {
                if formal.ty == Ty::UNKNOWN
                    && let Some(entry) = super_container.members.get(&formal.name)
                    && entry.kind == MemberKind::Field
                    && !entry.is_static
                {
                    // The field's declared type is owned by the same foreign
                    // pool as the constructor table — project it too.
                    formal.ty = self.boundary.ty(super_owner, entry.var_ty);
                }
            }
        }
        // The count decisions and the per-argument walk mirror
        // [`Engine::check_arguments`]: the deficit anchors after the last
        // positional argument (the closing paren at an empty or named-only
        // tail), the surplus at the first extra positional, the unknown
        // named parameter at its label, each type mismatch at the
        // argument's own span.
        let named = level
            .args
            .iter()
            .filter(|(name, ..)| name.is_some())
            .count() as u16;
        let Some(given) = positional.checked_sub(named) else {
            return;
        };
        let mut given = usize::from(given);
        let required = formals.positional.iter().filter(|f| f.required).count();
        let capacity = formals.positional.len();
        if required == 0 && capacity == 0 {
            // The empty-table face: the oracle counts the arguments its
            // parse attributes — a compound operand (a ternary value)
            // leaves an unattributed zero-length window here that is no
            // argument of the oracle's list, so only attributed positional
            // windows count toward the surplus.
            given = level
                .args
                .iter()
                .filter(|(name, .., range)| name.is_none() && range.length > 0)
                .count();
        }
        // The enclosing constructor's `super.` formals forward into the
        // invocation: the surplus count adds the forwarded positionals
        // (probed: `G(super.x) : super(1, 2, 3)` against `B(int x, int y)`
        // reports "2 expected, but 4 found"), and the deficit face stays
        // silent whenever any super formal exists — the coverage question
        // belongs to the `super_formal_parameter_without_associated_*`
        // faces (probed: `D(super.x) : super(1)` against `B(int x, int y)`
        // reports that face alone, and `D({required super.z}) :
        // super.named()` against `B.named(int x)` no deficit).
        let open_site = self.open_ctor_site;
        let super_formals_here: Vec<bool> = self
            .pending_super_formals
            .iter()
            .filter(|(owner, ..)| open_site.is_some() && *owner == open_site)
            .map(|(_, _, named, ..)| *named)
            .collect();
        let has_super_formals = !super_formals_here.is_empty();
        if has_super_formals {
            given += super_formals_here.iter().filter(|named| !**named).count();
        }
        if given < required && !has_super_formals {
            let anchor = level
                .args
                .iter()
                .filter(|(name, .., range)| name.is_none() && range.length > 0)
                .next_back()
                .map(|(.., range)| SourceRange {
                    offset: range.offset + range.length,
                    length: 1,
                })
                .or(level.first_label)
                .unwrap_or(close);
            // The name face carries the super constructor's display
            // (`'Base.new'` for the unnamed face, `'Base.named'` for the
            // selector — the oracle's probed message).
            let mut ctor_text = self
                .strings
                .resolve(resolved_super.unwrap_or(self.intern("Object")))
                .to_string();
            if let Some(selector) = selector {
                ctor_text.push('.');
                ctor_text.push_str(self.strings.resolve(selector));
            } else {
                ctor_text.push_str(".new");
            }
            let (shape, a0, a1) = if required == 1 {
                (
                    ArgCountShape::NameSingular,
                    ctor_text.clone(),
                    String::new(),
                )
            } else {
                (
                    ArgCountShape::NamePlural,
                    required.to_string(),
                    given.to_string(),
                )
            };
            let code = SemanticCode::NotEnoughPositionalArguments(shape);
            let message = match shape {
                ArgCountShape::NamePlural => code.message3(&a0, &a1, &ctor_text),
                _ => code.message(&a0, &a1),
            };
            self.emit_message_with_correction(code, anchor, message);
        } else if given > capacity {
            // The could-be-named verdict: the super constructor declares
            // more named parameters than the call's distinct named
            // arguments.
            let distinct = level
                .args
                .iter()
                .filter_map(|(name, ..)| *name)
                .collect::<rustc_hash::FxHashSet<_>>()
                .len();
            let code = if formals.named.len() > distinct {
                SemanticCode::ExtraPositionalArgumentsCouldBeNamed
            } else {
                SemanticCode::ExtraPositionalArguments
            };
            let mut seen = 0usize;
            let mut surplus = None;
            for (name, .., range) in &level.args {
                if name.is_some() {
                    continue;
                }
                seen += 1;
                // A compound operand's window stays unattributed (a
                // zero-length range) — the anchor degrades to the closing
                // paren, the documented span divergence, never to a
                // synthetic zero span.
                if seen == capacity + 1 && range.length > 0 {
                    surplus = Some(*range);
                }
            }
            let anchor = (seen == given)
                .then_some(surplus)
                .flatten()
                .unwrap_or(close);
            let message = code.message(&capacity.to_string(), &given.to_string());
            self.emit_message_with_correction(code, anchor, message);
        }
        let mut positional_index = 0usize;
        for (name, ty, literal, range) in &level.args {
            let formal = if let Some(n) = name {
                match self.named_formal_for(&formals, *n) {
                    Some(formal) => formal,
                    None => {
                        // The bare callee-row geometry's label misses are
                        // `Walk::check_named_argument`'s (it resolves the
                        // same table at label time) — this walk reports the
                        // miss only where that face cannot see the callee.
                        if !(bare && super_container.is_some()) {
                            let text = self.strings.resolve(*n).to_string();
                            let label = level
                                .named_labels
                                .iter()
                                .find(|(label, _)| label == n)
                                .map(|(_, range)| *range)
                                .unwrap_or(*range);
                            self.emit(SemanticCode::UndefinedNamedParameter, label, &text, "");
                        }
                        continue;
                    }
                }
            } else {
                let formal = formals.positional.get(positional_index);
                positional_index += 1;
                match formal {
                    Some(formal) => formal,
                    None => continue,
                }
            };
            // [`Engine::argument_verdict`] — the same relation the enum
            // constant's walk and the staged teardown consult.
            // The flow read's re-bound or promoted face outranks the
            // parse-time capture — the oracle checks the argument's static
            // type at the send (`uriSource` past `if (uriSource is
            // InSummarySource)` reads `InSummarySource`, never the declared
            // `Source?`); a divergent or absent read keeps the captured
            // face.
            let flow_ty = self
                .rebound_read_face(*range)
                .or_else(|| self.read_type_at(*range))
                .unwrap_or(*ty);
            let rejects = self.argument_verdict(flow_ty, formal.ty, *literal) == Some(false);
            if rejects {
                let from_text = self.ty_display(flow_ty);
                let to_text = self.ty_display(formal.ty);
                let message =
                    SemanticCode::ArgumentTypeNotAssignable.message3(&from_text, &to_text, "");
                self.emit_message_with_correction(
                    SemanticCode::ArgumentTypeNotAssignable,
                    *range,
                    message,
                );
            }
        }
    }

    /// Whether `name` spells a visible type alias and whether its
    /// right-hand side grounds (a constructible class face): the this-file
    /// TopDecl and clause capture first, then the foreign alias through
    /// the scoped lookup (its TopDecl's `var_ty` carries the right-hand
    /// side's head as written). `None` — no visible alias — leaves the
    /// existing faces untouched.
    fn alias_head_grounded(&self, name: StringId) -> Option<bool> {
        if matches!(
            self.top_level_decl(name).as_deref(),
            Some(decl) if decl.kind == MicroKind::TypeAlias
        ) {
            // The this-file clause capture decides when it has the alias;
            // a foreign alias (or one whose capture never armed) reads the
            // declaration's own `var_ty` — the right-hand side's head as
            // written, projected through the boundary — and the graph-wide
            // container face answers the groundedness instead.
            if self.bounds.alias_named_shape(name).is_some()
                || self.bounds.alias_fn_shape(name).is_some()
            {
                return Some(self.alias_creation_grounded(name, 0));
            }
            let var_ty = self.top_level_decl(name).map(|decl| decl.var_ty)?;
            return Some(self.foreign_alias_rhs_grounded(var_ty));
        }
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                self.index
                    .get(file)
                    .and_then(|els| els.decl(name))
                    .and_then(|decl| {
                        (decl.kind == MicroKind::TypeAlias)
                            .then(|| self.foreign_alias_rhs_grounded(decl.var_ty))
                    })
            }
            _ => None,
        }
    }

    /// A foreign alias's right-hand side grounds when its written head
    /// names a constructible type some unit of the graph carries
    /// (`typedef A = Real;` — `A()` is the `Real` creation): the leaf
    /// head resolves as a container (the diagnosing scope or any unit's
    /// declaration face). A head nothing declares — `shared.TCG` behind
    /// an import no config can resolve, its leaf spelling a class no
    /// unit carries — never grounds and the invocation reports. An
    /// uncaptured right-hand side (`Ty::Unknown`, the old-style
    /// function-typed spelling) stays undecided — the under-report
    /// direction.
    fn foreign_alias_rhs_grounded(&self, rhs: Ty) -> bool {
        match rhs {
            // An uncaptured right-hand side — the old-style function-typed
            // spelling, or a modern one whose head sits behind a prefix no
            // config resolves (the armed annotation never completes) —
            // names no constructible class: the oracle's element model
            // holds an unresolvable aliased type and the invocation
            // reports.
            Ty::Unknown => false,
            Ty::Named { name, .. } => {
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    name,
                )
                .is_some()
                    // The container-level world face, never the alias's
                    // own top-level declaration (the corpus spellings
                    // keep the aliased class's leaf identical to the
                    // alias name).
                    || self.name_world().is_known_head(name)
            }
            _ => false,
        }
    }

    /// Re-interns a constructor's formal table into the diagnosing pool.
    /// `resolve_container`'s `ctor_formals` hit is owned by `owner`'s unit —
    /// its `formal.ty` slots index that pool, and the member-walk
    /// projections ([`Boundary::member`]) cover member tables only, never a
    /// constructor's own table. Reading a foreign slot through the
    /// diagnosing pool indexes the wrong universe (an out-of-bounds read —
    /// the `model.rs` `ArgListId`/`FnShapeId` panic — or silently a
    /// different shape), so every `formal.ty` re-interns here before the
    /// argument walk derefs it. A same-file `owner` short-circuits.
    pub(crate) fn project_formals(&self, owner: FileId, formals: &mut Formals) {
        for formal in formals
            .positional
            .iter_mut()
            .chain(formals.named.iter_mut())
        {
            formal.ty = self.boundary.ty(owner, formal.ty);
        }
    }

    /// An untyped constructor formal's declared type is its field's own
    /// (`this.name` initializes the field — `FieldFormalParameterElement
    /// .type` is the field's type): the captured formal stays `Unknown`
    /// when no annotation precedes it, so the whole-file member table
    /// answers it (the field may be declared after the constructor). The
    /// enum-constant walk applies the same face to the enum's selected
    /// constructor.
    pub(crate) fn patch_field_formals(&self, container: StringId, formals: &mut Formals) {
        let Some(entry) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
        else {
            return;
        };
        for formal in formals
            .positional
            .iter_mut()
            .chain(formals.named.iter_mut())
        {
            if formal.ty == Ty::UNKNOWN
                && let Some(member) = entry.members.get(&formal.name)
                && member.kind == MemberKind::Field
                && !member.is_static
            {
                formal.ty = member.var_ty;
            }
        }
        self.patch_super_formals(container, formals);
    }

    /// A `super.name` formal's declared type is the super constructor's
    /// associated parameter's own (`SuperFormalParameterElement.type` —
    /// the association face [`Engine::check_super_formal`] implements: a
    /// positional `super.locale` reads the target constructor's parameter
    /// at the formal's own index among the super formals, a named one the
    /// same-spelling entry under the public spelling). The capture parks
    /// the untyped forward in a [`Ty::Never`] sentinel (a written
    /// annotation wins, as upstream); this pass resolves it against the
    /// superclass's constructor table — the unnamed face keyed by the
    /// superclass's own name, the creation-site posture of the
    /// superConstructor walk. The associated parameter answers through its
    /// declared type; a field-formal associated parameter takes its
    /// field's declared type (the super constructor's `this._locale` reads
    /// `String` from the field table), and a chained `super.name` forward
    /// climbs with the same rule, bounded by the ≤8-hop walk every
    /// recovery loop keeps. Every unresolvable step settles on `Unknown` —
    /// the unassociated formal's `InvalidType` suppresses the argument
    /// check, the same silence [`Engine::check_super_formal`] reports its
    /// own faces over.
    fn patch_super_formals(&self, container: StringId, formals: &mut Formals) {
        let pending = formals
            .positional
            .iter()
            .chain(formals.named.iter())
            .any(|f| f.ty == Ty::Never);
        if !pending {
            return;
        }
        // The positional association indexes the target's table by the
        // forward's own index AMONG the constructor's super formals
        // (`element.dart` `indexIn`) — the forwards consume the target's
        // parameters in order, whatever non-forward parameters share the
        // declaring list. Every positional forward carries the capture's
        // marker (an annotated forward rides its written type instead of
        // the sentinel yet still displaces the count).
        let marker = self.strings.intern("super.forward");
        let mut forward_index = 0usize;
        for formal in formals.positional.iter_mut() {
            let is_forward = formal.default_value == Some(marker);
            if formal.ty == Ty::Never {
                formal.ty = self.associated_super_formal_ty(
                    container,
                    SuperQuery::Positional(forward_index),
                    0,
                );
            }
            if is_forward {
                forward_index += 1;
            }
        }
        for formal in formals.named.iter_mut() {
            if formal.ty == Ty::Never {
                formal.ty =
                    self.associated_super_formal_ty(container, SuperQuery::Named(formal.name), 0);
            }
        }
    }

    /// The associated parameter's declared type for one `super.name`
    /// sentinel, one hop up per chained forward. A chained hop re-derives
    /// the positional query: the associated forward's own among-super-formals
    /// index counts the sentinels its own table lists before it, which a
    /// leading non-forward parameter displaces.
    fn associated_super_formal_ty(&self, container: StringId, query: SuperQuery, depth: u8) -> Ty {
        let mut container = container;
        let mut query = query;
        let mut depth = depth;
        loop {
            if depth >= 8 {
                return Ty::UNKNOWN;
            }
            depth += 1;
            let Some((super_container, super_owner, superclass, formals)) =
                self.associated_super_formals(container)
            else {
                return Ty::UNKNOWN;
            };
            let associated = match query {
                SuperQuery::Positional(index) => formals.positional.get(index),
                SuperQuery::Named(name) => {
                    let public = self.formal_public_name(name);
                    formals
                        .named
                        .iter()
                        .find(|f| f.name == name || f.name == public)
                }
            };
            let Some(associated) = associated else {
                // The unassociated forward's parameter reads `InvalidType`
                // upstream — every question it touches suppresses.
                return Ty::UNKNOWN;
            };
            match associated.ty {
                // The chained forward climbs to the next hop's association:
                // the associated forward's own among-marker count is the
                // table index its hop's target answers.
                Ty::Never => {
                    if let SuperQuery::Positional(index) = query {
                        let marker = self.strings.intern("super.forward");
                        let forward_index = formals
                            .positional
                            .iter()
                            .take(index)
                            .filter(|f| f.default_value == Some(marker))
                            .count();
                        query = SuperQuery::Positional(forward_index);
                    }
                    container = superclass;
                }
                // The associated field-formal declares its field's type.
                Ty::UNKNOWN => {
                    return self
                        .declared_field_ty(super_container, super_owner, associated.name)
                        .unwrap_or(Ty::UNKNOWN);
                }
                // The associated parameter's declared type lives in
                // `super_owner`'s pool — project it before it becomes the
                // patched formal's `formal.ty`.
                ty => return self.boundary.ty(super_owner, ty),
            }
        }
    }

    /// The unnamed super constructor's formal table one hop up, with the
    /// hop's owning container and name for the next link of a chained
    /// forward — and the super container's owning file, the key every
    /// `formal.ty`/field type read through the returned tables projects by.
    fn associated_super_formals(
        &self,
        container: StringId,
    ) -> Option<(&'a Container, FileId, StringId, &'a Formals)> {
        let (container, _) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )?;
        let superclass = container.superclass?;
        let (super_container, super_owner) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            superclass,
        )?;
        let formals = super_container.ctor_formals.get(&superclass)?;
        Some((super_container, super_owner, superclass, formals))
    }

    /// The instance field's declared type under `name`, the associated
    /// field-formal's own answer — projected out of `owner`'s pool, the
    /// universe the returned `member.var_ty` slots index.
    fn declared_field_ty(
        &self,
        container: &Container,
        owner: FileId,
        name: StringId,
    ) -> Option<Ty> {
        let member = container.members.get(&name)?;
        (member.kind == MemberKind::Field && !member.is_static)
            .then(|| self.boundary.ty(owner, member.var_ty))
    }

    /// The bare-invocation head that names a type alias the creation can
    /// never build on (`typedef DioMediaType = MediaType;` with the RHS
    /// unresolved — the head is a type, not a value, so the oracle's plain
    /// method invocation reports `invocation_of_non_function` at the head,
    /// element_resolver.dart's function-invocation face). A `const` head
    /// takes the creation face instead: `const Alias()` reports
    /// `creation_with_non_type` naming the alias (`instance_creation_
    /// resolver`'s non-type face). `true` when the row emitted and the
    /// miss handler must stay silent; `false` leaves the latch to
    /// [`Engine::report_bare_creation_miss`].
    pub(crate) fn report_alias_invocation_of_non_function(&mut self) -> bool {
        let Some((name, range)) = self.pending_bare_creation else {
            return false;
        };
        // The gate asks only about visible type aliases; every other head
        // shape keeps its existing face. A foreign alias (the declaring
        // unit imported — `generic_inferrer.dart` invoking the imported
        // `TypeConstraintFromArgument`) reaches through the scoped
        // lookup: its TopDecl's `var_ty` carries the right-hand side's
        // head as written, and the groundedness question decides over
        // the graph-wide declaration face.
        if self.alias_head_grounded(name) != Some(false) {
            return false;
        }
        let name_text = self.strings.resolve(name).to_string();
        if self.creation_head_is_const(range.offset) {
            self.emit(SemanticCode::CreationWithNonType, range, &name_text, "");
        } else {
            self.emit(SemanticCode::InvocationOfNonFunction, range, &name_text, "");
        }
        true
    }

    /// The dot-preceded latch head that names an ungrounded type alias
    /// (`Alias.named()` over `typedef Alias = shared.Foreign<Node>;`):
    /// the oracle resolves the selector against the alias's `Type` face —
    /// `The method 'named' isn't defined for the type 'Type'.` at the
    /// selector — and a `const` head takes the creation face at the alias
    /// spelling instead (`creation_with_non_type`). Every other
    /// dot-preceded head keeps its existing fold.
    ///
    /// The latch holds the selector (`named` at its own range); the class
    /// segment the selector hangs off is re-derived from the spelling
    /// ([`Engine::creation_class_before`]), which also skips a balanced
    /// `<…>` group (`Gen<int>.named(`).
    pub(crate) fn report_alias_selector_face(&mut self) {
        let Some((selector, selector_range)) = self.pending_bare_creation else {
            return;
        };
        let Some((name, name_start)) = self.creation_class_before(selector_range) else {
            return;
        };
        if !matches!(
            self.top_level_decl(name).as_deref(),
            Some(decl) if decl.kind == MicroKind::TypeAlias
        ) {
            return;
        }
        if self.alias_creation_grounded(name, 0) {
            return;
        }
        if self.creation_head_is_const(name_start) {
            let name_text = self.strings.resolve(name).to_string();
            let range = SourceRange {
                offset: name_start,
                length: name_text.len() as u32,
            };
            self.emit(SemanticCode::CreationWithNonType, range, &name_text, "");
            return;
        }
        let selector_text = self.strings.resolve(selector).to_string();
        self.emit(
            SemanticCode::UndefinedMethod,
            selector_range,
            &selector_text,
            "Type",
        );
    }

    /// Whether the head at the UTF-16 `offset` is written after the `const`
    /// keyword (trivia may intervene): `const Alias()` marks the creation
    /// const. `const` is reserved, so an identifier run spelling `const`
    /// before a creation head is always the keyword.
    fn creation_head_is_const(&self, offset: u32) -> bool {
        let bytes = self.source.as_bytes();
        let mut at = self.byte_of(offset);
        while at > 0 && bytes[at - 1].is_ascii_whitespace() {
            at -= 1;
        }
        let end = at;
        while at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_' || bytes[at - 1] == b'$') {
            at -= 1;
        }
        end - at == 5 && &bytes[at..end] == b"const"
    }

    /// The import prefix the head at the UTF-16 `offset` is written behind
    /// (`shared.Foreign` — a dot before the head across trivia), with the
    /// prefix identifier's run start. `None` for a bare head.
    fn written_prefix_before(&self, offset: u32) -> Option<(StringId, u32)> {
        let bytes = self.source.as_bytes();
        let mut at = self.byte_of(offset);
        while at > 0 && bytes[at - 1].is_ascii_whitespace() {
            at -= 1;
        }
        if at == 0 || bytes[at - 1] != b'.' {
            return None;
        }
        at -= 1;
        while at > 0 && bytes[at - 1].is_ascii_whitespace() {
            at -= 1;
        }
        let end = at;
        while at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_' || bytes[at - 1] == b'$') {
            at -= 1;
        }
        if at == end {
            return None;
        }
        let text = self.source.get(at..end)?;
        Some((self.strings.intern(text), self.utf16_of(at)))
    }

    /// The `(prefix, real head)` pair of a captured alias RHS written
    /// `p.C<…>`: the capture's first leaf is the spelling's first token —
    /// the prefix for a qualified name (`handle_type` completes on it and
    /// the `TypeReferenceContinuation` joins no leaf, so a non-generic
    /// `p.C` keeps only the `p` leaf). A `.`+identifier read straight off
    /// the source after the head token splits the pair; a bare head or a
    /// generic head whose tree names the real class already returns
    /// `None` (the unprefixed walk handles it).
    fn prefixed_alias_head(&self, tree: &crate::capture::TypeTree) -> Option<(StringId, StringId)> {
        let bytes = self.source.as_bytes();
        let mut at = self.byte_of(tree.range.offset)
            + self.strings.resolve(tree.head).len();
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        if at >= bytes.len() || bytes[at] != b'.' {
            return None;
        }
        at += 1;
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let start = at;
        while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_' || bytes[at] == b'$') {
            at += 1;
        }
        if at == start {
            return None;
        }
        let real = self.source.get(start..at)?;
        Some((tree.head, self.strings.intern(real)))
    }

    /// Whether the bare creation `name(...)` can build on the alias chain:
    /// the right-hand type grounds in a container (a class creation through
    /// the alias), a dart:core primitive, or a non-alias declaration. The
    /// walk rides the this-file captures only — a foreign alias's RHS is
    /// invisible, the verdict stays undecided and the face keeps silent
    /// (the creation faces' under-report posture); the ≤8-hop bound ends
    /// recovered cycles.
    fn alias_creation_grounded(&self, name: StringId, depth: u8) -> bool {
        match self.bounds.alias_named_shape(name) {
            // No named capture: a this-file function-shaped RHS is decided
            // (`Binary(1, 2)` over `typedef Binary = int Function(int, int)`
            // is the registry's own non-function example — emit), a foreign
            // alias's invisible RHS is undecided (keep silent).
            None => self.bounds.alias_fn_shape(name).is_none(),
            Some((_, tree)) if depth < 8 => {
                let head = tree.head;
                // A head written behind an import prefix (`typedef Alias =
                // shared.Foreign<Node>;`) grounds only through that
                // prefix's namespace — the bare-name walk never sees the
                // aliased class (`io.File` grounds, `shared.Foreign` with
                // an empty prefix namespace does not). Two spellings the
                // capture stores: a generic RHS keeps the real head's
                // tree (`Foreign<Node>`) whose token a dot + identifier
                // BEFORE it qualifies; a non-generic `p.C` keeps only the
                // prefix leaf (`handle_type` completes on the prefix, the
                // continuation joins no leaf) whose real head the dot +
                // identifier AFTER it carries. The spelling decides the
                // split either way.
                let pair = self
                    .written_prefix_before(tree.range.offset)
                    .map(|(prefix, _)| (prefix, head))
                    .or_else(|| self.prefixed_alias_head(tree));
                if let Some((prefix, real)) = pair {
                    return prefixed_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        prefix,
                        real,
                    )
                    .is_some();
                }
                if resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    head,
                )
                .is_some()
                    || self.members.primitive_ids.contains(&head)
                {
                    return true;
                }
                match self.top_level_decl(head).as_deref() {
                    // A visible non-alias declaration grounds the chain.
                    Some(decl) if decl.kind != MicroKind::TypeAlias => true,
                    // Another alias: keep hopping.
                    Some(_) => self.alias_creation_grounded(head, depth + 1),
                    // Nothing grounds the head: the alias names a type this
                    // unit cannot build on.
                    None => false,
                }
            }
            // The hop bound hit first: undecided, keep silent.
            Some(_) => true,
        }
    }

    /// Pairs the closed argument list against the callee's declared formals:
    /// positional by index, named by spelling. The count decisions precede
    /// the assignability walk (`extra_positional_arguments[_could_be_named]`
    /// / `not_enough_positional_arguments` — `resolveArgumentsToParameters`,
    /// upstream Dart SDK source, `pkg/analyzer/lib/src/generated/resolver.dart`
    /// :5594-5725 in the corpus registry, `type_analyzer.dart` in the pinned
    /// SDK: positional deficit against the required count, else surplus
    /// against the positional capacity, mutually exclusive); unresolved
    /// callees suppress.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn check_arguments(
        &mut self,
        level: Option<ArgLevel>,
        positional: u16,
        close: SourceRange,
    ) {
        let Some(level) = level else { return };
        // The enum constant's own argument list is an implicit constructor
        // invocation with no callee row — it checks against the enum's
        // selected constructor's formals directly and returns (the
        // row-stack callee machinery below has nothing to walk).
        if level.enum_const {
            self.check_enum_const_arguments(&level, positional, close);
            return;
        }
        // The explicit super-constructor invocation (`: super(...)`,
        // `: super.named(...)`) has no callee row the send machinery can
        // walk — its argument faces run against the resolved super
        // constructor's formals here, and the generic walk below keeps its
        // existing (silent) posture.
        if self.initializer_depth > 0
            && let Some((container, selector, bare)) = self.super_invocation_callee(&level)
        {
            self.check_super_invocation_arguments(
                &level, positional, close, container, selector, bare,
            );
        }
        let Some(callee) = self.invokes.last().and_then(|slot| *slot) else {
            // A dot-preceded latch head is a named selector (`C.named(`),
            // not an unnamed head — the latch also arms for the
            // named-selector creation fold, whose class-segment
            // resolution owns the spelling; the unnamed face skips it.
            let dot_preceded = self.pending_bare_creation.is_some_and(|(_, range)| {
                let start = self.byte_of(range.offset);
                start > 0 && self.source.as_bytes()[start - 1] == b'.'
            });
            if dot_preceded {
                self.report_alias_selector_face();
            } else if !self.report_alias_invocation_of_non_function() {
                self.report_bare_creation_miss();
            }
            return;
        };
        // The resolved-typedef send faces (#2452 F4): a callee row spelling
        // a type alias whose RHS grounds nothing. The empty-invokes latch
        // above cannot reach these — the typedef pushes a resolved callee
        // row — so the row-shape arm decides here.
        //
        // - `Alias(...)`: the oracle's function-invocation face reports
        //   `invocation_of_non_function` at the alias spelling ('Alias'
        //   isn't a function); a `const` head takes the creation face
        //   `creation_with_non_type` instead.
        // - `Alias.named(...)`: the selector resolves against the alias's
        //   `Type` face — `undefined_method` at `named` — and a `const`
        //   head reports `creation_with_non_type` at the class segment.
        //
        // A grounded alias (its RHS resolves a class through the bare or
        // prefixed namespace) is a plain creation and keeps its existing
        // fold.
        {
            let alias_head: Option<(StringId, SourceRange, bool)> = self
                .rows
                .get(callee)
                .and_then(|row| {
                    if row.qualified {
                        // `Alias.named(` — the selector callee: the class
                        // segment is the row directly below.
                        let head = callee
                            .checked_sub(1)
                            .and_then(|index| self.rows.get(index))?;
                        (head.range.end() <= row.range.offset
                            && !head.qualified
                            && matches!(head.shape, RowShape::Bare(BareInfo::Plain { .. }))
                            && self.alias_head_grounded(head.name) == Some(false))
                        .then_some((head.name, head.range, false))
                    } else if matches!(row.shape, RowShape::Bare(BareInfo::Plain { .. }))
                        && self.alias_head_grounded(row.name) == Some(false)
                    {
                        Some((row.name, row.range, true))
                    } else {
                        None
                    }
                });
            if let Some((name, head_range, bare)) = alias_head {
                let const_head = self.creation_head_is_const(head_range.offset);
                let name_text = self.strings.resolve(name).to_string();
                if const_head {
                    self.emit(SemanticCode::CreationWithNonType, head_range, &name_text, "");
                    return;
                }
                if !bare
                    && let Some(selector) = self.rows.get(callee)
                {
                    let selector_text = self.strings.resolve(selector.name).to_string();
                    self.emit(
                        SemanticCode::UndefinedMethod,
                        selector.range,
                        &selector_text,
                        "Type",
                    );
                }
                // A bare non-const head reports through
                // `check_invocation_face`'s spelled-alias leg — the same
                // `invocation_of_non_function` at the same range — so
                // this arm leaves it to fire downstream rather than
                // double-reporting.
                return;
            }
        }
        // The null-argument face runs before the row borrow opens (it
        // re-reads the rows itself and emits).
        self.check_null_argument(callee, &level.args);
        // The explicit extension override (`Ext(arg).member`) carries no
        // formals — its single argument is the receiver, checked against the
        // extension's declared `on` type
        // (`extension_member_resolver.dart:290-313`):
        // `!isAssignableTo(receiverType, extendedType)` reports at the whole
        // receiver expression. An `on` clause that never produced a name
        // (`on_type: None`) is inapplicable and suppresses. This runs before
        // the row borrow below (the check needs `&mut self` to emit).
        let extension_override = match self.rows.get(callee).map(|row| &row.shape) {
            Some(RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Extension,
            })) => Some(*name),
            _ => None,
        };
        if let Some(name) = extension_override {
            self.check_extension_override_argument(name, &level.args);
        }
        // The unnamed constructor's existence face (`_resolveConstructor`'s
        // unnamed arm, constructor_invocation_resolver.dart): an in-file
        // class whose declaration leaves the unnamed face absent
        // (`has_unnamed_constructor` false — every constructor named, as in
        // `factory H()` inside `class G`) reports
        // `new_with_undefined_constructor_default` at the creation head.
        // Foreign and unresolved containers defer (the under-report posture
        // every creation face shares). Decided in a scoped borrow before
        // the callee row's borrow below.
        let unnamed_creation_miss: Option<(StringId, SourceRange)> =
            self.rows.get(callee).and_then(|head| {
                let RowShape::Bare(BareInfo::Container {
                    name,
                    kind: ContainerKind::Class,
                }) = &head.shape
                else {
                    return None;
                };
                if head.qualified {
                    return None;
                }
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    *name,
                )
                .filter(|(decl, _)| !decl.has_unnamed_constructor)
                .map(|_| (*name, head.range))
            });
        if let Some((container, head_range)) = unnamed_creation_miss {
            let name_text = self.strings.resolve(container).to_string();
            self.emit(
                SemanticCode::NewWithUndefinedConstructorDefault,
                head_range,
                &name_text,
                "",
            );
        } else if let Some(head) = self.rows.get(callee)
            && !head.qualified
            && let Some(name) = self.bare_creation_head_name(head.range)
            && resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            )
            .is_some_and(|(decl, _)| !decl.has_unnamed_constructor)
        {
            // The row-transport under-report seam: a bare creation whose
            // callee row resolved outside the container shape (the
            // unresolved-selector posture) still names its class in the
            // source — the face reads the spelling directly and re-asks the
            // same unnamed-face question (`constructor_invocation_resolver`
            // has no row-shape gate). Qualified and non-identifier heads
            // keep the under-report posture.
            let name_text = self.strings.resolve(name).to_string();
            self.emit(
                SemanticCode::NewWithUndefinedConstructorDefault,
                head.range,
                &name_text,
                "",
            );
        }
        // The const-declaration initializer's selector-miss creation
        // (`const y = C.named()` over `class C { const C(); }`): the const
        // capture defers member invocations, so the face reads the send's
        // own rows — a creation-shaped member invocation whose selector no
        // constructor answers fails the initializer beside the resolution
        // row. The tail pass stages the
        // `const_initialized_with_non_constant_value` row at the creation
        // (the const_with_undefined_constructor fixture pins the span).
        if let Some(init) = self.pending_init_names.last().copied() {
            let miss: Option<(StringId, SourceRange)> = self.rows.get(callee).and_then(|row| {
                let RowShape::Bare(BareInfo::Plain { ty: Ty::UNKNOWN }) = &row.shape else {
                    return None;
                };
                if !row.qualified {
                    return None;
                }
                let receiver = callee.checked_sub(1).and_then(|i| self.rows.get(i))?;
                let container = match &receiver.shape {
                    RowShape::Receiver(Receiver::Created {
                        name,
                        kind: ContainerKind::Class,
                    })
                    | RowShape::Bare(BareInfo::Container {
                        name,
                        kind: ContainerKind::Class,
                    }) => *name,
                    _ => return None,
                };
                let resolves = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    container,
                )
                .is_some_and(|(decl, _)| decl.constructors.contains(&row.name));
                if resolves || !self.const_ctor_target(container) {
                    return None;
                }
                let start = receiver.range.offset;
                Some((
                    container,
                    SourceRange {
                        offset: start,
                        length: close.end().saturating_sub(start),
                    },
                ))
            });
            if let Some((container, span)) = miss {
                self.open_const_creations.push((
                    init,
                    span,
                    ConstCreationEntry::Creation { container },
                ));
            }
        }
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        // The usage cluster's given-at-call index (the
        // `_addParametersForArguments` face) keys by callee spelling alone
        // and must not wait on the formals: a receiver this face cannot
        // type (a creation-folded `C().m(...)` head) still supplies its
        // arguments. The parser's count is exact where the row recovery is
        // not: wide positional windows (compound operands) stay
        // unattributed above, so the positional supply is the total count
        // less the attributed named entries. Callee-spelling keys — a
        // same-named member elsewhere suppresses, never false-positives.
        let callee_spelling = row.name;
        let named = level
            .args
            .iter()
            .filter(|(name, ..)| name.is_some())
            .count() as u16;
        if positional > named {
            let given = self.positional_given.entry(callee_spelling).or_default();
            *given = (*given).max(positional - named);
        }
        // The given-named face registers before the formals gate: an
        // invocation whose formals cannot resolve here (an
        // initializer-list redirect `this._(v: v)`, an enum-constant
        // argument list) still supplies what it wrote — the
        // `unused_element_parameter` rule counts call-site givens, not
        // resolutions.
        for (given, ..) in &level.args {
            if let Some(given) = given {
                self.named_given.insert((callee_spelling, *given));
            }
        }
        // Cloned: the assignability path below reads the table while the
        // named-argument marking mutates it. The captured signature arm
        // records which callee row lent the formals — the parameter and
        // return substitution below reads its written types, which the
        // element model's formals cannot carry (function-typed variable
        // annotations are poisoned in the written-type capture). The
        // signature's count face rides the same gate as the model's —
        // upstream resolves the variable's declared function type and
        // counts against it (`resolveArgumentsToParameters`).
        let mut sig_from_row: Option<Box<FnVarSig>> = None;
        // The member send's resolved receiver (`x.m(...)`, `this.m(...)`,
        // the merged `RowShape::Member` send): the generic-inference face
        // below reads it for the member's own type parameters and the
        // receiver-instantiation seeds.
        let mut send_recv: Option<Receiver> = None;
        // A same-named member claims the unqualified callee ahead of the
        // namespace (the oracle resolves `this.x` before any top-level
        // spelling): the return face below reads the flag — a member-owned
        // call must not inherit the same-spelled top-level function's
        // return type (`predicate(result)` reads the field's callable
        // return, never `Parser<String>`).
        let mut member_owns_callee = false;
        let formals: Option<Formals> = match &row.shape {
            // A top-level function's declared formals — or the enclosing
            // container's own method under the implicit-this face (#1351):
            // the class's member wins (the official resolves an
            // unqualified invocation against `this.m` before the top-level
            // namespace), a same-named local suppresses (its function type
            // invokes dynamically), and the top-level namespace answers
            // what the member face missed. A qualified plain row is the
            // member placeholder below — a same-named top-level function
            // must not lend its formals to a member column.
            RowShape::Bare(BareInfo::Plain { .. }) if !row.qualified => {
                let shadowed = self.frames.iter().any(|f| f.locals.contains(&row.name));
                // #1881: the unnamed extension's own-member face answers
                // first — the frame's placeholder container has no collector
                // entry, so the named-container walk below always breaks
                // there (#1701's resolve_bare route, mirrored on the
                // invocation arity face; the named twin resolves the same
                // call through its real container).
                let unnamed = (!shadowed)
                    .then(|| self.unnamed_extension_method_formals(row.name))
                    .flatten()
                    .map(std::borrow::Cow::Owned);
                let member = match unnamed {
                    Some(formals) => Some(formals),
                    None => (!shadowed)
                        .then(|| self.container_context())
                        .flatten()
                        .filter(|(container, kind)| {
                            matches!(
                                self.bare_member_walk(*container, *kind, row.name),
                                Walk::Found(entry) if entry.kind == MemberKind::Method
                            )
                        })
                        .and_then(|(container, kind)| {
                            send_recv = Some(Receiver::This { container, kind });
                            self.member_formals_full(&Receiver::This { container, kind }, row.name)
                        }),
                };
                // A same-named member that is NOT a method still owns the
                // callee: `predicate(result)` inside `PredicateParser`
                // resolves the function-typed field (implicit this), never
                // the same-spelled top-level function — the oracle's scope
                // rules put `this.predicate` ahead of the namespace. The
                // member's callable signature comes from its declared
                // type's alias chain (`alias_fn_sig`); a foreign alias or
                // an unmodeled shape yields no formals — the under-report
                // — but the top-level fallback must never lend its formals
                // to a call the member already claimed.
                let member_found = !shadowed
                    && self.container_context().is_some_and(|(container, kind)| {
                        matches!(
                            self.bare_member_walk(container, kind, row.name),
                            Walk::Found(_)
                        )
                    });
                member_owns_callee = member_found;
                let member_sig: Option<Box<FnVarSig>> = match member {
                    Some(_) => None,
                    None if member_found => (!shadowed)
                        .then(|| self.container_context())
                        .flatten()
                        .and_then(
                            |(container, kind)| {
                                match self.bare_member_walk(container, kind, row.name) {
                                    Walk::Found(entry) if entry.kind == MemberKind::Field => {
                                        match self.fold_alias_ty(entry.var_ty) {
                                            Ty::Named { name, .. } => Some(name),
                                            _ => None,
                                        }
                                    }
                                    Walk::Found(entry) if entry.kind == MemberKind::Getter => {
                                        match self.fold_alias_ty(entry.return_ty) {
                                            Ty::Named { name, .. } => Some(name),
                                            _ => None,
                                        }
                                    }
                                    _ => None,
                                }
                            },
                        )
                        .and_then(|name| self.alias_fn_sig(name)),
                    None => None,
                };
                let member: Option<std::borrow::Cow<'_, Formals>> = match member {
                    Some(formals) => Some(formals),
                    None => member_sig
                        .as_ref()
                        .map(|sig| std::borrow::Cow::Owned(sig.formals(self.intern("")))),
                };
                // A function-typed variable's written signature (the
                // captured `T Function<T>(T)` face) precedes the element
                // model's — the model poisons such annotations and carries
                // no formals for a variable. The shadow gate blocks only
                // the namespace fallbacks below (a same-named top-level
                // function must not lend its formals to a local's call):
                // the shadowing local's OWN captured signature is exactly
                // what `resolveArgumentsToParameters` counts against (a
                // function-typed parameter called inside the body,
                // `Future<I> Function(void Function(Object), Object) spawn`
                // → `spawn(f, m)`).
                let sig_formals = member.is_none().then(|| {
                    // A shadowed local still counts against its OWN written
                    // signature when the name is a parameter whose
                    // annotation the formal claim captured (`Future<I>
                    // Function(void Function(Object), Object) isolateSpawn`
                    // → `isolateSpawn(f, m)`); every other shadowing local
                    // (a local function, a plain variable) keeps the
                    // suppress — the general table carries stale-stash
                    // attributions the parameter-claim table never does.
                    if shadowed {
                        self.fn_param_sigs
                            .get(&row.name)
                            .map(|sig| (sig.formals(self.intern("")), sig.clone()))
                    } else {
                        self.fn_var_sigs
                            .get(&row.name)
                            .map(|sig| (sig.formals(self.intern("")), sig.clone()))
                    }
                });
                // A local callee typed by a function-type alias (`Predicate<T>
                // test` / a folded `FailureJoiner joiner` invoked as
                // `test(item)`): the oracle's `resolveArgumentsToParameters`
                // counts against the alias's signature. The shadowed-local
                // path above answers `Some(None)` — the member gate opened,
                // the written-signature table had no entry — so the empty
                // inner must flatten before the alias arm can run.
                let sig_formals = sig_formals.filter(Option::is_some).or_else(|| {
                    let RowShape::Bare(BareInfo::Plain { ty }) = &row.shape else {
                        return None;
                    };
                    match *ty {
                        // A foreign alias's declared type already folded
                        // to the alias's function RHS (`bind_parameter_type`
                        // rewrites `FailureJoiner joiner` to the raw
                        // `Ty::Function` — the element model's alias row
                        // never reaches the callee): the pool shape is the
                        // signature `resolveArgumentsToParameters` counts
                        // against. Same shape the assignability face's
                        // `function_shape_sig` reads.
                        Ty::Function {
                            shape,
                            nullable: false,
                        } => {
                            let (return_ty, positional, named) = {
                                let pool = self.boundary.pool();
                                let shape = pool.function_shape(shape);
                                (
                                    shape.return_ty,
                                    shape.positional.to_vec(),
                                    shape.named.to_vec(),
                                )
                            };
                            let sig = Box::new(FnVarSig {
                                params: Box::default(),
                                required_positional: u16::try_from(positional.len())
                                    .unwrap_or(u16::MAX),
                                positional: positional.into(),
                                named: named.iter().map(|(name, ty)| (*name, *ty, false)).collect(),
                                return_ty,
                            });
                            Some(Some((sig.formals(self.intern("")), sig)))
                        }
                        Ty::Named { name, .. } => self
                            .alias_fn_sig(name)
                            .map(|sig| Some((sig.formals(self.intern("")), sig))),
                        _ => None,
                    }
                });
                match (member, sig_formals) {
                    (Some(formals), _) => {
                        // The member's alias signature doubles as the
                        // return face (`predicate(result)` reads `bool`,
                        // the alias RHS's return), like a variable's
                        // written signature does.
                        sig_from_row = member_sig;
                        Some(formals.into_owned())
                    }
                    (None, Some(Some((formals, sig)))) => {
                        sig_from_row = Some(sig);
                        Some(formals)
                    }
                    (None, _) => {
                        sig_from_row = None;
                        // A member already claimed the callee (field or
                        // getter of any shape): the namespace fallback
                        // belongs to a spelling no member carries, so a
                        // same-named top-level function must not lend its
                        // formals to the member's call.
                        if member_found {
                            None
                        } else {
                            self.top_level_decl(row.name)
                                .and_then(|d| d.formals.as_deref().cloned())
                        }
                    }
                }
            }
            // The member invocation's send closes BEFORE the dot-access
            // event merges `receiver.name` (`parse_send_from_identifier`
            // parses the argument list inside `parse_primary`): the callee
            // is the unmerged member placeholder (qualified, unresolved)
            // with the receiver row directly below — the
            // `check_invocation_arity` shape. Classify the receiver and
            // resolve the member's formals now; the merged face cannot
            // carry the check (this runs once, at the group's close).
            RowShape::Bare(BareInfo::Plain { .. }) => {
                let recv = callee
                    .checked_sub(1)
                    .and_then(|index| self.rows.get(index))
                    .map_or(Receiver::Unknown, |below| self.argument_receiver(below));
                send_recv = Some(recv);
                match self.member_formals_full(&recv, row.name) {
                    Some(formals) => Some(formals.into_owned()),
                    // The named creation's selector (`B.named(x)`, #1351):
                    // the receiver names the class — the constructor's
                    // formals key on the selector.
                    None => match recv {
                        Receiver::Created {
                            name,
                            kind: ContainerKind::Class,
                        }
                        | Receiver::Container {
                            name,
                            kind: ContainerKind::Class,
                        } => {
                            let formals = self
                                .index
                                .get(self.file)
                                .and_then(|f| f.container(name))
                                .and_then(|c| c.ctor_formals.get(&row.name))
                                .map(|f| (**f).clone());
                            formals.map(|mut formals| {
                                self.patch_field_formals(name, &mut formals);
                                formals
                            })
                        }
                        _ => None,
                    },
                }
            }
            // The unnamed creation's head (`B(x)`, #1351): the folded
            // container row carries the class — its unnamed face's formals
            // (the collector keys the container's own name).
            RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Class,
            }) if !row.qualified => {
                let formals = self
                    .index
                    .get(self.file)
                    .and_then(|f| f.container(*name))
                    .and_then(|c| c.ctor_formals.get(name))
                    .map(|f| (**f).clone());
                formals.map(|mut formals| {
                    self.patch_field_formals(*name, &mut formals);
                    formals
                })
            }
            RowShape::Member { recv } => {
                send_recv = Some(*recv);
                self.member_formals_full(recv, row.name)
                    .map(std::borrow::Cow::into_owned)
            }
            _ => None,
        };
        // The implicit-`call` face (the callee's type names a container
        // whose `call` method answers — bare `body(inner)` over
        // `XPathExpression body` and the dot-spelled `body.call(inner)`
        // alike): the invocation checks against the call method's formals
        // — `inner` against `XPathContext` (`XPathExpression.call`'s
        // formal), the oracle's CallInvocation `.call` resolution
        // (function_expression_invocation_resolver.dart:141-203). The
        // face's executable facts ride along for the parameter
        // substitution and the result-type write below (the call method's
        // own type parameters stand in for the element model's, and the
        // call's declared return becomes the send's return face).
        let mut implicit_call: Option<(Ty, Box<[StringId]>)> = None;
        let formals = formals.or_else(|| {
            let row = self.rows.get(callee)?;
            let ty = self.receiver_static_ty(row);
            let (formals, ret, params) = self.implicit_call_face(ty)?;
            implicit_call = Some((ret, params));
            Some(formals)
        });
        let Some(formals) = formals else {
            return;
        };
        // The required-named coverage (`RequiredParametersVerifier._check`,
        // required_parameters_verifier.dart:123-156): a `required` named
        // formal with no same-named argument reports
        // `missing_required_argument` at the invocation's callee entity
        // (the constructor reference / invoked name — the row's span). The
        // enclosing-constructor super-formal forwarding face
        // (`_containsNamedExpression`'s second walk) is the under-report
        // direction this model documents elsewhere. Collected here, emitted
        // once the callee-row borrow ends (the count block below is the
        // row's last reader).
        let missing_required: Vec<StringId> = formals
            .named
            .iter()
            .filter(|f| f.required)
            .filter(|f| {
                // A required `this._x`/`super._x` formal is covered when any
                // named argument resolves to it — by the public spelling
                // (the element's name) or the raw one.
                !level.args.iter().any(|(name, ..)| {
                    name.is_some_and(|n| {
                        self.named_formal_for(&formals, n)
                            .is_some_and(|m| m.name == f.name)
                    })
                })
            })
            // The reported name is the element's public spelling — a
            // `required this._error` formal reports `'error'` upstream.
            .map(|f| self.formal_public_name(f.name))
            .collect();
        let callee_range = row.range;
        // #1314 FAM10: the callee's declared type parameters (same-file
        // generic executables) admit the inference substitution — a
        // parameter type that is exactly a type parameter checks and
        // displays as its solution. The solution is the context type when
        // the return type is exactly the parameter (downward inference,
        // the display the oracle's solver prints), else the first matched
        // argument's type; a solution violating the declared bound clamps
        // to the bound (the constraint solver's bound-respecting solution
        // — `f<T extends num>(null)` reports against `num`), and an
        // unbounded unsolved parameter stands for itself (the prior
        // behavior).
        // The creation heads (#1351): the unnamed shape's callee row is
        // the folded container; the named shape's placeholder resolves
        // through the receiver below. Their inference parameters are the
        // class's own. The declared return face rides along for the
        // substituted-return write at the close (the row borrow must not
        // reach the emits below).
        let (creation_container, mut return_face) = match &row.shape {
            RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Class,
            }) if !row.qualified => (
                Some(*name),
                self.top_level_decl(row.name).map(|decl| decl.return_ty),
            ),
            RowShape::Bare(BareInfo::Plain { .. }) if row.qualified => {
                let recv = callee
                    .checked_sub(1)
                    .and_then(|index| self.rows.get(index))
                    .map_or(Receiver::Unknown, |below| self.argument_receiver(below));
                let container = match recv {
                    Receiver::Created {
                        name,
                        kind: ContainerKind::Class,
                    }
                    | Receiver::Container {
                        name,
                        kind: ContainerKind::Class,
                    } => Some(name),
                    _ => None,
                };
                // The member send's declared return face (`r.readEnum<T>()`
                // before the dot merge folds it): the receiver's member
                // walk answers the method's written return, which the
                // solved map instantiates below — the unmerged placeholder
                // carries no `Member` shape for `callee_return_ty` to read.
                // A class-head receiver (`C.sm()`) is the creation arm's
                // territory and keeps the prior `None` face.
                let member_return = container.is_none().then(|| {
                    let (container, kind) = match send_recv {
                        Some(Receiver::This { container, kind }) => (container, kind),
                        Some(
                            Receiver::Super { container, .. }
                            | Receiver::Typed {
                                type_name: container,
                                ..
                            },
                        ) => (container, ContainerKind::Class),
                        _ => return None,
                    };
                    match self.bare_member_walk(container, kind, row.name) {
                        Walk::Found(entry)
                            if matches!(entry.kind, MemberKind::Method | MemberKind::Getter) =>
                        {
                            Some(entry.return_ty)
                        }
                        _ => None,
                    }
                });
                (container, member_return.flatten())
            }
            RowShape::Bare(BareInfo::Plain { .. }) => (
                None,
                // The written signature's return precedes the element
                // model's (a function-typed variable's annotation is
                // poisoned in the declared-type slot). A member-owned
                // callee never reads the namespace: the same-spelled
                // top-level function's return is not the member's
                // callable's return — the member's own declared return
                // rides the receiver's member walk instead
                // (`this.readEnum(xs)` inside the declaring class) so the
                // solved map below can instantiate it.
                sig_from_row
                    .as_ref()
                    .map(|sig| sig.return_ty)
                    .or_else(|| {
                        (!member_owns_callee)
                            .then(|| self.top_level_decl(row.name).map(|decl| decl.return_ty))
                            .flatten()
                    })
                    .or_else(|| {
                        let (container, kind) = match send_recv {
                            Some(Receiver::This { container, kind }) => (container, kind),
                            Some(
                                Receiver::Super { container, .. }
                                | Receiver::Typed {
                                    type_name: container,
                                    ..
                                },
                            ) => (container, ContainerKind::Class),
                            _ => return None,
                        };
                        match self.bare_member_walk(container, kind, row.name) {
                            Walk::Found(entry)
                                if matches!(
                                    entry.kind,
                                    MemberKind::Method | MemberKind::Getter
                                ) =>
                            {
                                Some(entry.return_ty)
                            }
                            _ => None,
                        }
                    }),
            ),
            // The member's declared return rides the element walk (the
            // instantiation write below grounds `T` returns).
            RowShape::Member { .. } => (None, Some(self.callee_return_ty(row))),
            _ => (None, None),
        };
        // The implicit-`call` face's return: the call method's declared
        // return is the send's return face, whatever spelling the callee
        // row carries (a bare `body(...)`'s top-level lookup answered
        // nothing; a dot-spelled `body.call(...)` whose member walk missed
        // must not inherit a same-spelled top-level `call`'s return).
        if let Some((ret, _)) = implicit_call {
            return_face = Some(ret);
        }
        // Read before the first mutation below: the row borrow must not
        // reach the emits.
        let unnamed_creation = matches!(&row.shape, RowShape::Bare(BareInfo::Container { .. }));
        let mut exec_params: Option<Box<[(StringId, Option<Ty>)]>> = self
            .exec_bounds
            .get(&callee_spelling)
            .cloned()
            .filter(|params| !params.is_empty());
        // #1351: the creation faces' parameters come from the container's
        // bounds capture (its type parameters with their written bounds).
        if exec_params.is_none()
            && let Some(container) = creation_container
            && let Some(params) = self.bounds.container_params(container)
            && !params.is_empty()
        {
            exec_params = Some(
                params
                    .iter()
                    .map(|(param, bound, _)| {
                        (*param, bound.as_ref().map(|tree| self.tree_ty(tree)))
                    })
                    .collect(),
            );
        }
        // The bounds capture registers at the container's close, so a
        // creation whose callee is declared LATER in the file (the
        // extension method above its own return type's class,
        // `optional()` before `OptionalParser`) reads no capture
        // mid-parse. The element model's `type_param_names` — complete
        // before the parse runs — stand in unbounded: explicit type
        // arguments still solve every slot, and an unsolved slot keeps
        // the raw spelling.
        if exec_params.is_none()
            && let Some(container) = creation_container
            && let Some(names) = self.foreign_container_params(container)
        {
            exec_params = Some(names.into_iter().map(|name| (name, None)).collect());
        }
        // The function-typed variable's written signature (the captured
        // `T Function<T>(T)` annotation): its declared type parameters
        // stand in for the element model's — the model poisons the
        // annotation and carries no executable bounds for a variable.
        if exec_params.is_none()
            && let Some(sig) = sig_from_row.as_ref()
            && !sig.params.is_empty()
        {
            exec_params = Some(sig.params.iter().copied().collect());
        }
        // The implicit-`call` face's parameters (the call method's own
        // type parameters — `R call<R>(R x)` solves `R` like any generic
        // send's, downward from the context type, upward from the first
        // matched argument).
        if exec_params.is_none()
            && let Some((_, params)) = &implicit_call
            && !params.is_empty()
        {
            exec_params = Some(params.iter().map(|name| (*name, None)).collect());
        }
        // The member send's inference parameters (`reader.readEnum(...)`,
        // `this.adopt(...)`, `box.get()`): the found method's own
        // `type_param_names` — keyed by the member walk, so a same-spelled
        // member elsewhere never lends them — plus the declaring
        // container's captured parameters, which the receiver's written
        // instantiation seeds (`Box<int>.get()` binds `T := int`). Member
        // type-parameter bounds are not modeled on the element entry, so
        // every parameter lands unbounded (the under-report direction the
        // name-keyed model already carries).
        let mut receiver_seeds: Vec<(StringId, Ty)> = Vec::new();
        if exec_params.is_none()
            && let Some(recv) = send_recv
            && let Some((params, seeds)) = self.member_send_params(&recv, row.name)
        {
            receiver_seeds = seeds;
            exec_params = Some(params);
        }
        // The count decisions. The parser's total is exact where the row
        // recovery is not, so the positional supply is the total less the
        // named entries (the same given-count the usage index records); a
        // recovered list whose named entries exceed it suppresses. The
        // anchors: upstream reports the deficit after the last positional
        // argument (the token after it — the closing paren, exact for an
        // empty or named-only tail) and the surplus at the first positional
        // argument beyond the capacity; a compound operand's window stays
        // unattributed, so the surplus anchor degrades to the closing paren
        // (a documented span divergence, the code identical).
        if let Some(given) = positional.checked_sub(named) {
            let given = usize::from(given);
            let required = formals.positional.iter().filter(|f| f.required).count();
            let capacity = formals.positional.len();
            // The name face: upstream names every invocation through a
            // written function or method name — an unqualified call AND a
            // qualified member send (`NamedFunctionInvocation`, upstream
            // Dart SDK source, pkg/analyzer/lib/src/dart/ast/ast.dart
            // :37828-37834; capture `c.m(b: 1)` reports "expected by 'm'").
            // Implicit calls over non-simple receivers stay nameless.
            let named_face = matches!(&row.shape, RowShape::Bare(BareInfo::Plain { .. }));
            if given < required {
                // The deficit anchor: the token after the last positional
                // argument — the first token inside the parens (the label)
                // at zero positional arguments, else the closing paren
                // (`lastPositionalArgument?.endToken.next ??
                // argumentList.leftParenthesis.next ??
                // argumentList.rightParenthesis`, upstream Dart SDK source,
                // type_analyzer.dart:5843-5849). A compound operand's window
                // stays unattributed, so the anchor degrades to the closing
                // paren (the same documented span divergence as the surplus
                // face; the code is identical).
                let anchor = level
                    .args
                    .iter()
                    .filter(|(name, .., range)| name.is_none() && range.length > 0)
                    .next_back()
                    .map(|(.., range)| SourceRange {
                        offset: range.offset + range.length,
                        length: 1,
                    })
                    .or(level.first_label)
                    .unwrap_or(close);
                let (shape, a0, a1) = match (named_face, required) {
                    (true, 1) => (
                        ArgCountShape::NameSingular,
                        self.strings.resolve(row.name).to_string(),
                        String::new(),
                    ),
                    (true, _) => (
                        ArgCountShape::NamePlural,
                        required.to_string(),
                        given.to_string(),
                    ),
                    (false, 1) => (ArgCountShape::Singular, String::new(), String::new()),
                    (false, _) => (
                        ArgCountShape::Plural,
                        required.to_string(),
                        given.to_string(),
                    ),
                };
                let code = SemanticCode::NotEnoughPositionalArguments(shape);
                let message = match shape {
                    ArgCountShape::NamePlural => {
                        let name = self.strings.resolve(row.name).to_string();
                        code.message3(&a0, &a1, &name)
                    }
                    _ => code.message(&a0, &a1),
                };
                self.emit_message_with_correction(code, anchor, message);
            } else if given > capacity {
                // The could-be-named verdict: the callee declares more named
                // parameters than the call's distinct named arguments
                // (`namedParameterCount > namedArgumentCount`).
                let distinct = level
                    .args
                    .iter()
                    .filter_map(|(name, ..)| *name)
                    .collect::<FxHashSet<_>>()
                    .len();
                let code = if formals.named.len() > distinct {
                    SemanticCode::ExtraPositionalArgumentsCouldBeNamed
                } else {
                    SemanticCode::ExtraPositionalArguments
                };
                let mut seen = 0usize;
                let mut surplus = None;
                for (name, _, _, range) in &level.args {
                    if name.is_some() {
                        continue;
                    }
                    seen += 1;
                    if seen == capacity + 1 {
                        surplus = Some(*range);
                    }
                }
                // Every positional window attributed: the walked spans are
                // the argument positions, and `surplus` is the first one
                // past the capacity.
                let anchor = (seen == given)
                    .then_some(surplus)
                    .flatten()
                    .unwrap_or(close);
                let message = code.message(&capacity.to_string(), &given.to_string());
                self.emit_message_with_correction(code, anchor, message);
            }
        }
        // The required-named misses emit in formal-declaration order
        // (upstream walks `parameters` in order, one row per missing
        // required named).
        for name in missing_required {
            let text = self.strings.resolve(name).to_string();
            self.emit(
                SemanticCode::MissingRequiredArgument,
                callee_range,
                &text,
                "",
            );
        }
        // Per-parameter solution slots (`None` = unsolved).
        let mut solutions: Box<[Option<Ty>]> = exec_params
            .as_ref()
            .map(|params| vec![None; params.len()].into())
            .unwrap_or_default();
        // The invocation's explicit type arguments (the callee's own
        // type-argument group, keyed by the callee row): the official
        // downward phase fixes them before any inference
        // (`_typesInferredSoFar`) and the final phase never revisits a
        // fixed type. Consumed even without parameters so a stale entry
        // cannot answer a later send on a reused row.
        let explicit_args = self.call_args_for(callee);
        if unnamed_creation
            && explicit_args.is_none()
            && let (Some(container), Some(params)) = (creation_container, &exec_params)
        {
            self.check_could_not_infer(callee, container, params, &formals, &level, close);
        }
        if let Some(params) = &exec_params {
            let param_index = |name: StringId| params.iter().position(|(param, _)| *param == name);
            // The explicit fix precedes every inference face.
            if let Some(explicit) = &explicit_args {
                for (slot, ty) in solutions.iter_mut().zip(explicit.iter()) {
                    *slot = Some(*ty);
                }
            }
            // The receiver's written instantiation seeds the declaring
            // container's own parameters (`Box<int>.get()` solves `T` before
            // any argument reads) — explicit type arguments above win, so a
            // slot already fixed never re-seeds.
            for (name, ty) in &receiver_seeds {
                if let Some(index) = param_index(*name)
                    && solutions[index].is_none()
                {
                    solutions[index] = Some(*ty);
                }
            }
            // The bare-type-parameter shape: the only form the substitution
            // answers (written arguments keep their own face).
            let param_of = |ty: Ty| match ty {
                Ty::Named {
                    name,
                    nullable: false,
                    args,
                } if args == ArgListId::EMPTY => Some(name),
                _ => None,
            };
            // Downward face: the return type is exactly one parameter and
            // the surrounding declaration declared a type.
            if let Some(return_ty) = return_face
                && let Some(param) = param_of(return_ty)
                && let Some(index) = param_index(param)
                && let Some(ctx) = self.pending_decl_ty
                && ctx != Ty::Unknown
                && solutions[index].is_none()
            {
                solutions[index] = Some(ctx);
            }
            // The creation's downward face (#1351): the context type's
            // written arguments name every parameter (`A<int> a =
            // A(null);` — the official solver's downward inference).
            if let Some(container) = creation_container
                && let Some(capture) = self.bounds.container_params(container)
                && let Some(Ty::Named {
                    name: head,
                    args,
                    nullable: false,
                }) = self.pending_decl_ty
                && head == container
                && args != ArgListId::EMPTY
            {
                let pool = self.boundary.args.borrow();
                let ctx_args = pool.get(args);
                if ctx_args.len() == capture.len() {
                    for ((param, _, _), ty) in capture.iter().zip(ctx_args) {
                        if let Some(index) = param_index(*param)
                            && solutions[index].is_none()
                        {
                            solutions[index] = Some(*ty);
                        }
                    }
                }
            }
            // Upward face: the argument constraints solve each parameter
            // (`generic_inferrer.dart`'s `TypeConstraintFromArgument`
            // origins). Every formal the write pairs unifies structurally
            // against the argument's computed type — a bare `T` formal
            // binds its whole argument; a `List<T>`/`Map<K, V>` formal
            // binds the matched argument slots through the same-head
            // argument lists; the `List<E>`/`Set<E>` → `Iterable<E>` edge
            // (the builtins table's only single-slot interface climb)
            // crosses one head hop. A parameter solved from exactly one
            // decidable position — or several positions all agreeing —
            // materializes; positions that disagree leave the slot empty
            // (the upstream solver's LUB merge is outside this model —
            // under-report beats an invented binding).
            let mut candidates: Vec<(StringId, Ty)> = Vec::new();
            let mut positional_index = 0usize;
            for (name, ty, _literal, _range) in &level.args {
                let formal = if let Some(n) = name {
                    self.named_formal_for(&formals, *n)
                } else {
                    let formal = formals.positional.get(positional_index);
                    positional_index += 1;
                    formal
                };
                let Some(formal) = formal else {
                    continue;
                };
                if *ty != Ty::Unknown {
                    self.unify_upward(formal.ty, *ty, params, &mut candidates);
                }
            }
            // Slots an earlier phase already fixed (explicit arguments,
            // the downward context, the receiver seeds) are authoritative:
            // upward candidates compete only for the still-open slots. A
            // parameter whose candidates disagree stays empty — and a
            // later agreeing candidate cannot resurrect a poisoned slot.
            let prior: Box<[bool]> = solutions.iter().map(|s| s.is_some()).collect();
            let mut poisoned: FxHashSet<usize> = FxHashSet::default();
            for (param, ty) in candidates {
                let Some(index) = param_index(param) else {
                    continue;
                };
                if prior[index] || poisoned.contains(&index) {
                    continue;
                }
                match solutions[index] {
                    None => solutions[index] = Some(ty),
                    Some(kept) if kept == ty => {}
                    Some(_) => {
                        poisoned.insert(index);
                        solutions[index] = None;
                    }
                }
            }
        }
        // The effective parameter type: the solution, clamped to the
        // declared bound when it violates it (the constraint solver's
        // bound-respecting solution — `f<T extends num>(null)` reports
        // against `num`); an unsolved bounded parameter stands for its
        // bound; an unsolved unbounded one stands for itself (the prior
        // behavior).
        let effective: Box<[Ty]> = match &exec_params {
            None => Box::default(),
            Some(params) => params
                .iter()
                .zip(solutions.iter())
                .map(|((param, bound), solution)| match (solution, bound) {
                    (Some(ty), None) => *ty,
                    (Some(ty), Some(bound))
                        if self.assignable(*ty, *bound, false) != Some(false) =>
                    {
                        *ty
                    }
                    (_, Some(bound)) => *bound,
                    (None, None) => Ty::named(*param),
                })
                .collect(),
        };

        let mut positional_index = 0usize;
        for (name, ty, literal, range) in &level.args {
            let mut positional_slot = None;
            let formal = if let Some(n) = name {
                self.named_formal_for(&formals, *n)
            } else {
                let formal = formals.positional.get(positional_index);
                positional_slot = Some(positional_index);
                positional_index += 1;
                formal
            };
            if let Some(n) = name {
                self.named_given.insert((callee_spelling, *n));
            }
            let Some(formal) = formal else {
                continue;
            };
            // The function-subtype face (`_isFunctionSubtypeOf`,
            // upstream Dart SDK source, pkg/analyzer/lib/src/dart/element/
            // subtype.dart): a formal carrying a written generic
            // function-type signature against a tear-off argument decides
            // on the two signatures — the element model's `Ty` pair is
            // Unknown/Unknown (rule 0c) and cannot decide. `Some(true)` is
            // sound and silent; `Some(false)` reports with the written
            // shapes; `None` falls through to the ordinary face, which
            // suppresses on the Unknown pair.
            let formal_sig = match name {
                Some(n) => self
                    .fn_formal_sigs
                    .get(&callee_spelling)
                    .and_then(|table| table.named.get(n))
                    .cloned(),
                None => positional_slot
                    .and_then(|slot| {
                        self.fn_formal_sigs
                            .get(&callee_spelling)
                            .and_then(|table| table.positional.get(slot))
                    })
                    .cloned(),
            };
            let arg_sig = self.arg_sigs.remove(&range.offset);
            if let Some(arg_sig) = arg_sig {
                if let Some(formal_sig) = formal_sig {
                    match self.fn_sig_subtype(&arg_sig, &formal_sig) {
                        Some(true) => continue,
                        Some(false) => {
                            let code = SemanticCode::ArgumentTypeNotAssignable;
                            let from_text = self.fn_sig_display(&arg_sig);
                            let to_text = self.fn_sig_display(&formal_sig);
                            let message = code.message3(&from_text, &to_text, "");
                            // The why-not-promoted rows hang on the argument's
                            // read (#1877): the oracle reports this family
                            // through the context-wired argument check
                            // (error_detection_helpers.dart:359-365).
                            let index = self.report.diagnostics.len();
                            self.emit_message_with_correction(code, *range, message);
                            self.pending_non_promotion.push((index, range.offset));
                            continue;
                        }
                        None => {}
                    }
                } else {
                    // A function-type alias formal (`typedef Pred = bool
                    // Function(String)`): the alias RHS is the written
                    // signature the oracle decides against
                    // (`_isFunctionSubtypeOf` unfolds the alias), so the
                    // tear-off decides on the two signatures — not on the
                    // alias head. No captured RHS (or an undecidable pair)
                    // falls through to the ordinary face below.
                    if let Some(alias_sig) = match formal.ty {
                        Ty::Named {
                            name,
                            args: ArgListId::EMPTY,
                            ..
                        } => self.alias_fn_sig(name),
                        _ => None,
                    } {
                        match self.fn_sig_subtype(&arg_sig, &alias_sig) {
                            Some(true) => continue,
                            Some(false) => {
                                let code = SemanticCode::ArgumentTypeNotAssignable;
                                let from_text = self.fn_sig_display(&arg_sig);
                                let to_text = self.ty_display(formal.ty);
                                let message = code.message3(&from_text, &to_text, "");
                                let index = self.report.diagnostics.len();
                                self.emit_message_with_correction(code, *range, message);
                                self.pending_non_promotion.push((index, range.offset));
                                continue;
                            }
                            None => {}
                        }
                    }
                    // A tear-off against a formal with no written function
                    // signature: a function value is assignable only to the
                    // function supertypes (`dynamic`, `Object`, `Function`,
                    // `void`); every other resolvable written formal
                    // refuses — the message renders the tear-off's written
                    // shape (the `c(print)` face, `void Function(Object?)`
                    // vs `int`). Unknown, a bound type parameter, and the
                    // structured formals keep the under-report posture.
                    let refused = match formal.ty {
                        Ty::Named {
                            name,
                            args: ArgListId::EMPTY,
                            ..
                        } => {
                            let head = self.strings.resolve(name);
                            let unbound = self.type_params.contains(&name)
                                || self.pending_type_params.contains(&name);
                            !unbound && head != "Object" && head != "Function"
                        }
                        _ => false,
                    };
                    if refused {
                        let code = SemanticCode::ArgumentTypeNotAssignable;
                        let from_text = self.fn_sig_display(&arg_sig);
                        let to_text = self.ty_display(formal.ty);
                        let message = code.message3(&from_text, &to_text, "");
                        let index = self.report.diagnostics.len();
                        self.emit_message_with_correction(code, *range, message);
                        self.pending_non_promotion.push((index, range.offset));
                        continue;
                    }
                }
            }
            // The substituted parameter type (the substitution keeps
            // every non-type-parameter callee byte-identical). A bare
            // parameter leaf maps to its solved slot; a COMPOUND face
            // (`Stream.transform`'s `StreamTransformer<T, S>` over the
            // receiver-seeded `T` and the argument-solved `S`) substitutes
            // the whole solved map through `subst_params` — an unsolved
            // parameter maps to its own spelling, so only solved slots
            // move.
            let formal_ty = match (&exec_params, formal.ty) {
                (
                    Some(params),
                    Ty::Named {
                        name: param,
                        nullable: false,
                        args,
                    },
                ) if args == ArgListId::EMPTY => params
                    .iter()
                    .position(|(p, _)| *p == param)
                    .map(|index| effective[index])
                    .unwrap_or(formal.ty),
                (Some(params), formal_ty)
                    if !matches!(formal_ty, Ty::Unknown)
                        && params
                            .iter()
                            .any(|(p, _)| self.formal_mentions_param(formal_ty, *p)) =>
                {
                    let map: Vec<(StringId, Ty)> = params
                        .iter()
                        .zip(effective.iter())
                        .map(|((param, _), ty)| (*param, *ty))
                        .collect();
                    self.subst_params(formal_ty, &map).unwrap_or(formal_ty)
                }
                _ => formal.ty,
            };
            if self.argument_verdict(*ty, formal_ty, *literal) == Some(false) {
                // The raw-generic face (#2172's `_base.equals(e1, e2)`): a
                // member-invocation formal leaf that spells the declaring
                // container's own type parameter (`Equality` used raw
                // leaves `equals`'s `E` unsubstituted) is not a resolvable
                // interface type — the oracle's raw-type erasure answers
                // the bound, and the argument face stays silent. Creations
                // keep the face (the unsolved formal of a raw creation is
                // the dynamic-erasure argument the oracle reports — the
                // #1351 pins); only an out-of-scope, unresolvable named
                // head on a member send suppresses.
                let creation = unnamed_creation || creation_container.is_some();
                let unbound_formal = !creation
                    && match formal_ty {
                        Ty::Named {
                            name,
                            args: ArgListId::EMPTY,
                            ..
                        } => {
                            !self.type_params.contains(&name)
                                && !self.pending_type_params.contains(&name)
                                // The final primitives never resolve as
                                // containers (the harness's bare world, and
                                // the unindexed SDK units) — their formals
                                // keep the face.
                                && !self.members.primitive_ids.contains(&name)
                                && resolve_container(
                                    self.graph,
                                    self.namespaces,
                                    self.index,
                                    self.lib,
                                    self.file,
                                    name,
                                )
                                .is_none()
                        }
                        _ => false,
                    };
                if !unbound_formal {
                    self.stage(StagedKind::Argument, *range, *ty, formal_ty, *literal);
                }
            }
        }
        // The substituted return type (#1351, part d): a generic send's
        // static type is the solved return — the bare-parameter shape maps
        // to its slot, and a compound return (`List<T>`) substitutes the
        // solved map through `subst_params` — both forms the oracle's
        // `instantiate` writes on the invocation node. A parameter-less or
        // unsubstitutable return (a record/function shape, a `T?` leaf the
        // substitution declines) keeps the declared face — the
        // under-report direction a wrong binding would violate.
        // The implicit-`call` face's result type (the concrete-return
        // shape — the parameter-return shape above already wrote the
        // solved one): `return body(inner)` reads `XPathSequence`
        // (`Expr.call`'s), never the callee's own `Expr` class — the
        // oracle types the invocation node with the called member's
        // return (function_expression_invocation_resolver.dart:230-256).
        let mut return_written = false;
        if let Some(params) = &exec_params
            && let Some(rf) = return_face
        {
            let map: Vec<(StringId, Ty)> = params
                .iter()
                .zip(effective.iter())
                .map(|((param, _), ty)| (*param, *ty))
                .collect();
            if let Some(instantiated) = self.subst_params(rf, &map)
                && instantiated != rf
            {
                self.rows[callee].ty = instantiated;
                return_written = true;
            }
        }
        if !return_written && let Some((ret, _)) = implicit_call {
            self.rows[callee].ty = ret;
        }
        // The creation's instantiated self type: the send merge replaces
        // the folded row (its type lost), so the solved face rides in
        // `pending_send_ty` and lands on the merged row at the fold
        // (`UnboundedArg<int> v = UnboundedArg<num>(0);` — the assignment
        // check reads the instantiation, not the unknown). A partially
        // solved instantiation keeps the prior unknown face: an
        // ungrounded slot has no type the oracle would print either.
        if let Some(container) = creation_container
            && unnamed_creation
            && let Some(params) = &exec_params
            && solutions.iter().all(|slot| slot.is_some())
        {
            let args: Box<[Ty]> = params
                .iter()
                .zip(solutions.iter())
                .map(|(param, slot)| slot.unwrap_or_else(|| Ty::named(param.0)))
                .collect();
            let ty = self
                .boundary
                .args
                .borrow_mut()
                .named(container, false, &args);
            self.pending_send_ty = Some((callee, ty));
        }
    }

    /// Consumes a completed send: the closed group's callee row becomes an
    /// invocation. A send without arguments never opened a group, so a bare
    /// read stays a read (`foo;` vs `foo()`). An explicit type-argument group
    /// on the callee checks its arity (`_element`, or `_extension` for an
    /// extension override). `end` is the send's last consumed token (the
    /// argument group's closer) — the creation fold's whole-invocation
    /// report anchor.
    ///
    /// A container-name callee (`E('x')`, `C()`) folds into a receiver row
    /// carrying the container: the argument rows belong to the send (truncated away), and
    /// a following `.member` / operator merge must resolve against the
    /// container's face, not the last argument's row. Member callees
    /// (`C.named()`, methods) are not folds — their own rows stand.
    ///
    /// Returns the `handleExit` face when the send resolves bottom: the
    /// listener records `Flow::handle_exit` for it (official decision order —
    /// the resolver's bottom-callee check at the name's end, then the
    /// expression-level bottom check at the expression's end).
    /// Whether `this` binds inside a field initializer at the current
    /// frame depth: the oracle's `bindsThis` (`variable_declaration_
    /// resolver.dart:47-49` — `!element.isStatic && element.isLate`), a
    /// `withThisAccessibility` region spanning the whole initializer —
    /// nested closures included, which is why the closure latches OR this
    /// into their `instance` bit rather than re-asking.
    pub(crate) fn field_initializer_binds_this(&self) -> bool {
        self.field_group_open && !self.open_member_static && self.group_late
    }

    /// The `this`/`super` context verdict (`SuperContext.of`,
    /// super_context.dart:37-85, and the `isThisAccessible` flag): valid
    /// inside an instance member of a class-family container (constructor
    /// bodies included — the initializer list is the invalid half),
    /// inside a `late` instance field's initializer, and — for `this` —
    /// inside an extension or extension-type instance member. Invalid in
    /// every static context: top level, static members, non-late field
    /// initializers, constructor initializer lists, factory bodies, and
    /// annotations. `Extension`/`ExtensionType` answer `Skip` for `super`
    /// (the oracle's `superInExtension`/`superInExtensionType` faces —
    /// different codes, outside this face).
    pub(crate) fn keyword_receiver_state(&self, is_super: bool) -> ReceiverState {
        // The innermost executable answers (closures inherit the enclosing
        // instance bit); a container-initializer list reads through the
        // body-brace latch: the constructor's own frame is open but its
        // body has not begun.
        for frame in self.frames.iter().rev() {
            match &frame.kind {
                FrameKind::Executable {
                    instance,
                    body_brace,
                    ..
                } => {
                    // The initializer window closes at the ctor's OWN body
                    // brace — a braceless closure (`() => f(this)`) opened
                    // in the body has no `body_brace` of its own, so the
                    // ctor-level `ctor_body_open` latch answers for it.
                    let in_ctor_initializers = self.ctor_pending.is_some()
                        && body_brace.is_none()
                        && !self.ctor_body_open;
                    dq_probe!(
                        "[kw-recv] is_super={} instance={} body_brace={:?} ctor_body_open={} -> in_init={}",
                        is_super,
                        instance,
                        body_brace.is_some(),
                        self.ctor_body_open,
                        in_ctor_initializers
                    );
                    if !*instance || in_ctor_initializers {
                        return ReceiverState::Invalid;
                    }
                    let Some((_, kind)) = self.container_context() else {
                        return ReceiverState::Invalid;
                    };
                    if is_super
                        && !matches!(
                            kind,
                            ContainerKind::Class | ContainerKind::Mixin | ContainerKind::Enum
                        )
                    {
                        return ReceiverState::Skip;
                    }
                    return ReceiverState::Valid;
                }
                FrameKind::Container { .. } => {
                    // A container frame below no executable: a field
                    // initializer or an enum constant's argument
                    // expressions. A `late` instance group keeps both
                    // keywords legal (super_context.dart's FieldDeclaration
                    // arm falls through to the declaring class); a static
                    // or non-late group does not.
                    return if self.field_initializer_binds_this() {
                        ReceiverState::Valid
                    } else {
                        ReceiverState::Invalid
                    };
                }
                FrameKind::Block => {}
            }
        }
        ReceiverState::Invalid
    }

    /// The needs-getter face of one completed send
    /// (`function_expression_invocation_resolver.dart:137-144`): the
    /// invoked expression's static type is known and not a function — a
    /// statically-`null` callee reports `invalid_use_of_null_value`
    /// instead, a valid `this`/`super` receiver carries its enclosing
    /// class type (never a function, short of a `call` member), and an
    /// invalid receiver keeps its declaration-context face alone (its
    /// static type is `InvalidType` upstream — `isAssignableTo` answers
    /// true, nothing reports).
    pub(crate) fn check_invocation_face(&mut self, callee: usize) {
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        match &row.shape {
            RowShape::Value => {
                let range = row.range;
                if row.ty == Ty::Null {
                    // The null-typed callee reports through the
                    // nullable-dereference verifier's face upstream, whose
                    // context rows ride the same entity (#1877; upstream
                    // Dart SDK source,
                    // pkg/analyzer/lib/src/error/nullable_dereference_verifier.dart:57-73).
                    self.emit_non_promotion(
                        SemanticCode::InvalidUseOfNullValue,
                        range,
                        "",
                        "",
                        range.offset,
                    );
                } else if row.ty != Ty::UNKNOWN
                    // An index-result callee (`handlers[i](x)`) reads the
                    // element argument's written face, where a function-type
                    // argument (`List<F Function(String)>`) collapses to its
                    // return head — the callable shape is lost, so the face
                    // keeps silent; a `Function` head is callable outright.
                    && self.source.as_bytes().get(self.byte_of(range.offset)) != Some(&b']')
                    && !matches!(row.ty, Ty::Named { name, .. } if self.strings.resolve(name) == "Function")
                {
                    
                    self.emit(
                        SemanticCode::InvocationOfNonFunctionExpression,
                        range,
                        "",
                        "",
                    );
                }
            }
            RowShape::Bare(BareInfo::Plain { ty }) => {
                // A variable whose written annotation is a function type
                // (`T Function<T>(T) f`) is callable — the element model
                // poisons that annotation into a `Named`, but the captured
                // signature keeps the callable face.
                if self.fn_var_sigs.contains_key(&row.name) {
                    return;
                }
                // The callee that SPELLS a type alias (`DioMediaType(...)`)
                // is a type used as a value: upstream resolves the
                // identifier to the alias element and reports
                // `invocation_of_non_function` at the spelling
                // (element_resolver.dart's function-invocation face). The
                // row's own type is already the alias EXPANSION head
                // (`Named(MediaType)` under `typedef DioMediaType =
                // MediaType;`), so the question rides the source spelling —
                // and a foreign alias's unresolvable expansion leaves the
                // row's type `Unknown`, so the check precedes the named
                // destructure; every other callee shape (a variable, a
                // member) keeps its spelling and the leg below never fires
                // for it.
                if let Some(spelled) = self.bare_creation_head_name(row.range)
                    && self.alias_head_grounded(spelled) == Some(false)
                {
                    let name_text = self.strings.resolve(spelled).to_string();
                    let range = row.range;
                    self.emit(SemanticCode::InvocationOfNonFunction, range, &name_text, "");
                    return;
                }
                let Ty::Named { name, .. } = ty else {
                    return;
                };
                // A `Function`-typed value is the function-type supertype —
                // callable.
                if self.strings.resolve(*name) == "Function" {
                    return;
                }
                // A parameterized function-type alias is callable the same
                // way: `typedef P<T> = bool Function(T)` types `P<T> test`,
                // and `test(item)` invokes the aliased signature (the
                // captured alias RHS stands in for the unmodeled
                // substitution). An alias chain (`typedef B = A<int>` where
                // `A` is the fn alias) chases to its terminal function shape.
                if self.alias_fn_sig(*name).is_some() {
                    return;
                }
                // An interface-typed callee resolves its `call` member like
                // the oracle's type-property resolve (`sum(1, 2)` over
                // `Adder` with `int call(int, int)`); a present `call`
                // method is callable, anything else reports.
                if matches!(
                    self.resolved_member(*name, self.intern("call")),
                    Some(entry) if entry.kind == MemberKind::Method
                ) {
                    return;
                }
                // A cross-file function-type alias is callable like the
                // local capture: the alias declaration is library-visible
                // (`top_level_decl`), only its RHS shape is this-file
                // (`alias_fn_sig`'s documented capture boundary) — a
                // same-file non-function RHS was already answered `None`
                // above and still reports, while a foreign one keeps the
                // under-report posture (`Predicate<T> predicate` over
                // petitparser's shared `typedef Predicate<T> =
                // Callback<T, bool>`).
                if self.bounds.alias_fn_shape(*name).is_none()
                    && self.bounds.alias_named_shape(*name).is_none()
                    && matches!(
                        self.top_level_decl(*name).as_deref(),
                        Some(decl) if decl.kind == MicroKind::TypeAlias
                    )
                {
                    return;
                }
                // An out-of-scope head is a foreign projection this unit
                // cannot resolve (an unexported function-type alias rides
                // inference without an import — the oracle resolves the
                // callable truth structurally): call-ability is undecidable
                // here, so the face keeps silent. The dart:core primitives
                // have no declaring stub, yet their non-callable verdicts
                // are known.
                if self.top_level_decl(*name).is_none()
                    && !self.members.primitive_ids.contains(name)
                {
                    return;
                }
                // A local callee may stand flow-promoted at the send (a
                // switch-case pattern narrowed the local to a `call`
                // class — the flow read-type fact drains only at pass 2):
                // the row defers its re-decision instead of reporting the
                // declared face (#2547).
                if self
                    .frames
                    .iter()
                    .rev()
                    .any(|f| f.locals.contains(&row.name))
                {
                    self.deferred_invocation_faces.push((row.range, row.name));
                    return;
                }
                let range = row.range;
                self.emit(
                    SemanticCode::InvocationOfNonFunctionExpression,
                    range,
                    "",
                    "",
                );
            }
            RowShape::Receiver(Receiver::This { .. } | Receiver::Super { .. }) => {
                let is_super = matches!(&row.shape, RowShape::Receiver(Receiver::Super { .. }));
                if self.keyword_receiver_state(is_super) != ReceiverState::Valid {
                    return;
                }
                // `this.x()` resolves through the enclosing container;
                // `super.x()` resolves through its superclass chain —
                // only a `call` member on the resolved target (inherited
                // members included) makes the receiver itself invocable.
                let start = if is_super {
                    self.container_context()
                        .and_then(|(container, _)| {
                            self.index
                                .get(self.file)
                                .and_then(|f| f.container(container))
                        })
                        .and_then(|c| c.superclass)
                } else {
                    // An extension's `this` types as its `on` type
                    // ([`Self::this_static_ty`]) — the alias head is the
                    // callable face the climb asks (`extension
                    // ComparatorExtension<T> on Comparator<T>`'s
                    // `this(...)` sends, #2172).
                    self.container_context().map(|(name, kind)| {
                        match self.this_static_ty(name, kind) {
                            Ty::Named { name: head, .. } => head,
                            _ => name,
                        }
                    })
                };
                let call = self.intern("call");
                let mut callable = false;
                let mut link = start;
                // A recovered cyclic `extends` terminates on the set's
                // repeat bound; a valid chain ends at a missing superclass.
                let mut seen = rustc_hash::FxHashSet::default();
                while let Some(name) = link {
                    if !seen.insert(name) {
                        break;
                    }
                    // The unindexed function-alias face (the
                    // `this(...)` arm of the dart:core `Comparator<T>`
                    // extension — `compareBy`'s `(R a, R b) => this(...)`,
                    // #2172): the `on` type's head spells an alias the
                    // bounds tables never carry, and the aliased signature
                    // is the callable face.
                    if self.alias_fn_sig(name).is_some() {
                        callable = true;
                        break;
                    }
                    let Some(container) = self.index.get(self.file).and_then(|f| f.container(name))
                    else {
                        break;
                    };
                    if container.members.contains_key(&call) {
                        callable = true;
                        break;
                    }
                    link = if is_super { container.superclass } else { None };
                }
                if !callable {
                    let range = row.range;
                    self.emit(
                        SemanticCode::InvocationOfNonFunctionExpression,
                        range,
                        "",
                        "",
                    );
                }
            }
            _ => {}
        }
    }

    pub(crate) fn consume_send(&mut self, end: Token) -> Option<SendExit> {
        // Honor the armed link only for the send whose closing token is the
        // argument group's own `)` — a constructor invocation's `()` arms it
        // without a following `send` event, so a later statement's send must
        // not drain the follower's rows onto the stale callee.
        let (callee, close_end) = self.pending_send.take()?;
        if end.range.end() != close_end {
            return None;
        }
        // The #749 adjudication runs before the fold replaces the callee
        // row: the container/selector face reads the pre-fold shape.
        if self
            .literal_ctor_stash
            .last()
            .is_some_and(|(owner, _, _)| *owner == callee)
        {
            let (_, args, span) = self.literal_ctor_stash.pop().expect("paired stash");
            self.check_literal_ctor_invocation(callee, &args, span);
        }
        if let Some(row) = self.rows.get(callee) {
            let callee_offset = row.range.offset;
            // The discarded-value machine: every completed send is a
            // candidate until its context seals it.
            self.ur_register(callee_offset);
            // The F-H displacement latch: the resolved send's member row
            // stays in-stack, so the next `.member` merge can re-anchor on
            // it when the argument window's recovery rows displaced the
            // geometry (single slot — the next send re-arms it).
            self.last_send_callee = Some((callee, end.range.end()));
            // The invocation span record (callee offset → closing `)`): the
            // staged-join anchors a call operand's assignability verdict at
            // the whole invocation expression, the oracle's
            // `FunctionExpressionInvocation` span — the callee row keeps
            // its name-only range for the count faces. Nested sends
            // complete inner-first, so the insert re-sorts rather than
            // pushes.
            if let Err(index) = self
                .call_ends
                .binary_search_by_key(&callee_offset, |(offset, _)| *offset)
            {
                self.call_ends
                    .insert(index, (callee_offset, end.range.end()));
            }
        } else {
            return None;
        }
        if let Some(row) = self.rows.get_mut(callee)
            && row.intent == Intent::Read
        {
            row.intent = Intent::Invoke;
        }
        // The implicit-`call` result type: a member value invocation whose
        // type names a container with a `call` method yields the method's
        // return type — `return body.call(inner)` reads `XPathSequence`
        // (`XPathExpression.call`'s), never the receiver's own
        // `XPathExpression` (the oracle's `.call` resolution types the
        // invocation node with the called member's return). A `Never`-typed
        // member keeps its own face (the dead-flow arm above reads it).
        // Bare spellings take the write from the argument-check face at
        // the group's close (the substitution needs the live arguments);
        // this block covers the dot-spelled rows whose member walk
        // resolved the formals ahead of that face, and self-guards
        // against a row the face already retyped (the written return
        // names no `call` member).
        if let Some(row) = self.rows.get(callee)
            && row.qualified
            && !self.is_never_ty(row.ty)
            && let Some((_, call_ret, _)) = self.implicit_call_face(row.ty)
        {
            self.rows[callee].ty = call_ret;
        }
        // The same implicit-call retyping for a BARE local callee: the
        // row's own face is the local's declared type (`Object`), but the
        // flow read at the callee's span carries the promoted spelling
        // (`switch (x) case XPathExpression():` arm) — its `call` member's
        // return types the invocation node exactly as the dot-spelled arm
        // above (and keeps the invocation-face check's callable exception
        // honest).
        if let Some(row) = self.rows.get(callee)
            && !row.qualified
            && !self.is_never_ty(row.ty)
            && let Some(read) = self.read_type_at(row.range)
            && read != row.ty
            && let Some((_, call_ret, _)) = self.implicit_call_face(read)
        {
            self.rows[callee].ty = call_ret;
        }
        // A function-typed callee's own shape: the invocation yields the
        // shape's return type (`handler(x)` over `typedef H = String
        // Function(int)`, a field/param/local spelled either through the
        // alias or the raw function type) — never the whole function type
        // the declared row carries. A nullable callee keeps its face (the
        // unchecked-use verdict owns the value then).
        if let Some(&Ty::Function {
            shape,
            nullable: false,
        }) = self.rows.get(callee).map(|row| &row.ty)
        {
            let ret = self.boundary.pool().function_shape(shape).return_ty;
            self.rows[callee].ty = ret;
        }
        // The function-position face (`function_expression_invocation_
        // resolver.dart:131`): the invoked expression's own static type —
        // pre-invocation, so no callee-result enrichment here.
        let exit = if let Some(row) = self.rows.get(callee)
            && let RowShape::Bare(BareInfo::Plain { ty }) = &row.shape
            && self.is_never_ty(*ty)
        {
            let at = row.range;
            self.stage_receiver_of_type_never(at, *ty);
            Some(SendExit::Callee)
        } else {
            // The expression-level face: the completed send's static type is
            // the callee's declared return type, so a `Never` return makes
            // the flow state die at the send's end.
            let ret = self
                .rows
                .get(callee)
                .map_or(Ty::UNKNOWN, |row| self.callee_return_ty(row));
            // The same resolved return stages for the flow send fold
            // (#2567 R18): the listener picks the slot up at this event
            // and stages it into the recorder, so the name-level fold —
            // whose own callee read is an unknown face — adopts the real
            // return (the send-close inference's parameter solution
            // included, `readEnum(Tag.values)` answering the enum).
            if ret != Ty::UNKNOWN {
                if let Some(row) = self.rows.get(callee) {
                    self.send_return_staging = Some((row.range.offset, ret));
                }
            }
            (self.is_never_ty(ret)).then_some(SendExit::Result)
        };
        // The needs-getter face (`function_expression_invocation_resolver
        // .dart:137-144`): the invoked expression's static type is known
        // and not a function — a statically-`null` callee reports
        // `invalid_use_of_null_value` instead (the same resolver's null
        // swap), and an invalid `this`/`super` receiver keeps its
        // declaration-context face alone (its static type is `InvalidType`
        // upstream — `isAssignableTo` answers true, nothing reports).
        self.check_invocation_face(callee);
        if let Some(pos) = self
            .deferred_args
            .iter()
            .rposition(|(row, _, _)| *row == callee)
        {
            let (_, count, range) = self.deferred_args.remove(pos);
            self.check_invocation_arity(callee, count, range);
        }
        if matches!(
            self.rows.get(callee).map(|r| r.shape.clone()),
            Some(RowShape::Bare(BareInfo::Container { .. }))
        ) {
            // The fold drops the rows above the callee (the folded receiver
            // must own the stack for a following `.member` / operator
            // merge), but every row in the dropped span still owes its
            // pass-2 obligations: the oracle resolves an instance creation's
            // arguments regardless of the fold, so an unresolved reference
            // reports (`B(x)`, `new Missing<int>(x)`) and a RESOLVED one
            // marks its usage — a bare tear-off argument (`onPressed:
            // _bump`) counts as a use of `_bump` (`_useIdentifierElement`),
            // and a member argument counts as its field read. Drain
            // unfiltered, exactly like the plain-send fold below; the
            // pass-2 arms keep non-reporting shapes silent.
            for row in self.rows.drain(callee + 1..) {
                self.deferred_miss_rows.push(row);
            }
            let folded = self.rows.get(callee).and_then(|row| match &row.shape {
                RowShape::Bare(BareInfo::Container { name, kind }) => {
                    Some((*name, *kind, row.range))
                }
                _ => None,
            });
            if let Some((name, kind, range)) = folded {
                // The creation's head reference uses the class (the official
                // `visitNamedType`/identifier face): the row folds into a
                // receiver in place, so pass 2 never resolves it.
                if !self.self_scope_rows.contains(&range.offset) {
                    self.used_names.insert(name);
                }
                // The query face's creation-head type-use (#1869-3). Design
                // decision: the row joins `type_uses` (the `(span, name)`
                // outlet `Analysis::resolve_constructor` keys on) rather
                // than routing the const-family rules through a new
                // send-span creation API. Evidence: (1) the feared
                // persist/fingerprint cascade of the `type_uses` route does
                // not exist — the vec reaches only `UnitFold` → `Analysis`
                // (in-memory fold products, recomputed per unit; no
                // engine-persist, `persisted.rs`, or `signature.rs`
                // consumer), so no version surface moves; (2) every
                // existing rule call site (`resolve_constructor`) queries
                // by the head/type-reference span, and a bare implicit
                // head `G()` IS that position syntactically — the outlet
                // shape needs no new API, where a send-span face would add
                // one publication substrate plus one query entry point;
                // (3) the explicit spellings (`new C()`/`const C()`/
                // `C<int>.new()`) already publish through the
                // constructor-reference type use, and this fold branch
                // runs only for the bare-container shape a plain-send
                // spelling leaves, so the two paths never double-push one
                // position. Extension overrides are not creation heads (no
                // constructor face to query) and keep the outlet's
                // type-reference meaning exact. The push rides parse order
                // here; the query-side offset sort (the #1765 NameMap-order
                // SAFE verdict's outlet discipline) normalizes.
                if kind != ContainerKind::Extension {
                    self.type_uses.push((range, name));
                }
                // The creation fold is an unnamed-constructor invocation
                // candidate (`checkConstructorInvocation`); extensions have
                // no constructors. The anchor spans the whole `A()`
                // (`nodeToAnnotate`).
                if kind != ContainerKind::Extension {
                    let span = SourceRange {
                        offset: range.offset,
                        length: end.range.offset + end.range.length - range.offset,
                    };
                    self.ur_register_creation(range.offset, span, name);
                }
                // The solved instantiation lands here: the fold replaced
                // the callee row, and the assignment/argument checks read
                // the merged face, not the pre-fold unknown.
                let solved = match self.pending_send_ty.take() {
                    Some((row, ty)) if row == callee => ty,
                    _ => Ty::UNKNOWN,
                };
                self.rows[callee] = Row {
                    name,
                    range,
                    qualified: false,
                    shape: RowShape::Receiver(Receiver::Created { name, kind }),
                    intent: Intent::Read,
                    write_op: None,
                    ty: solved,
                };
            }
        } else {
            // The plain-send fold (`f(x)`, `c.m(x)`, `f<T>(x)`): upstream the
            // completed MethodInvocation is one expression node — the send's
            // argument rows belong to the node, not the stack. Drain the
            // rows above the callee the same way the creation fold does —
            // every drained row still owes its report (`print(c.noSuchGetter)`'s
            // `undefined_getter`, `f(c + c)`'s `undefined_operator`, a
            // resolved operand's `used_names` mark), so all of them defer to
            // pass 2 through `deferred_miss_rows`; only their mergeable
            // stack position dies here (a following `.member` anchors on the
            // callee, not a displaced argument row).
            for row in self.rows.drain(callee + 1..) {
                self.deferred_miss_rows.push(row);
            }
        }
        exit
    }

    /// An implicit creation ended (`g<int>(...)`): the oracle rewrites the
    /// shape to a plain invocation when the element resolves to a function
    /// (`ast_rewrite.dart`), so a deferred bare-function head checks its
    /// arity here (`_element`). Class/extension heads already checked at
    /// their group.
    pub(crate) fn end_implicit_creation(&mut self) {
        if self.ctor_pos != 1 {
            return;
        }
        let Some((row, count, range)) = self.deferred_args.pop() else {
            return;
        };
        // The rewritten invocation's group: latch consumed with the arity
        // check (the argument check reads it at the send's close — same
        // row index).
        let _ = self.call_args_for(row);
        let Some(head) = self.rows.get(row) else {
            return;
        };
        // A plain function head (`g<int>(...)`) — the oracle rewrites the
        // shape to a plain invocation (`ast_rewrite.dart`) — and a member
        // head reached through the creation syntax (`C.sm<int, String>(...)`,
        // the `id.id<...>(` form) both check as the `_element` invocation.
        // Class and extension heads checked at their type-argument group.
        if matches!(
            head.shape,
            RowShape::Bare(BareInfo::Plain { .. }) | RowShape::Member { .. }
        ) {
            self.check_invocation_arity(row, count, range);
        }
    }

    /// The invoked-callee arity check (`invocation_inferrer.dart` reports
    /// through the oracle's target dispatch, `type_instantiation_target.
    /// dart`): a resolved executable invoked with explicit type arguments
    /// reports `_element`, naming its kind word — a top-level function is
    /// `function`, a member method is `method`. Function-typed variables and
    /// getters resolve to type displays M1 cannot render (the oracle's
    /// `_function` / kind-`getter` rows) — no declared count, suppress.
    /// `null_argument_to_non_null_type` (`NullSafeApiVerifier._checkTypes`,
    /// null_safe_api_verifier.dart:87-107): `Completer<T>.complete` with a
    /// non-nullable `T` rejects a `null` argument — the call carries no
    /// argument (the missing optional reports at the member name) or one
    /// positional whose static type is `Null` (the `null` literal reads as
    /// `Ty::Null`). The registry's other face, `Future<T>.value`, needs the
    /// creation-row machinery (the documented under-report); more than one
    /// positional or any named argument suppresses (`arguments2.length >
    /// 1`), as does a nullable receiver (`T?` rides the null-aware faces).
    pub(crate) fn check_null_argument(
        &mut self,
        callee: usize,
        args: &[(Option<StringId>, Ty, bool, SourceRange)],
    ) {
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        // The member invocation's send closes BEFORE the dot-access event
        // merges `receiver.name` (`parse_send_from_identifier` parses the
        // argument list inside `parse_primary`): the callee is the unmerged
        // member placeholder (qualified, unresolved) with the receiver row
        // directly below — the `check_invocation_arity` shape.
        if !row.qualified
            || !matches!(row.shape, RowShape::Bare(BareInfo::Plain { .. }))
            || self.strings.resolve(row.name) != "complete"
        {
            return;
        }
        let Some(recv_row) = callee.checked_sub(1).and_then(|index| self.rows.get(index)) else {
            return;
        };
        let recv = match &recv_row.shape {
            RowShape::Bare(info) => Engine::receiver_of_bare(*info),
            RowShape::Receiver(recv) => *recv,
            _ => Receiver::Unknown,
        };
        let Receiver::Typed { ty, .. } = recv else {
            return;
        };
        let Ty::Named {
            name,
            nullable: false,
            ..
        } = ty
        else {
            return;
        };
        if self.strings.resolve(name) != "Completer" {
            return;
        }
        // `!_typeSystem.isNonNullable(type)` — the type argument decides
        // (the receiver type's own pooled slot).
        let Some(t0) = self.receiver_first_type_arg(ty) else {
            return;
        };
        if !t0.is_strictly_non_nullable() {
            return;
        }
        let (range, a0) = match args {
            // `c.complete()` — the missing optional argument reports at the
            // invocation (the member-name token stands in; metric-neutral
            // on the (code, file) gate).
            [] => (row.range, "Completer.complete"),
            [(None, ty, _, range)] if *ty == Ty::Null => (*range, "Completer.complete"),
            _ => return,
        };
        let a1 = self.ty_display(t0);
        self.stage_typed(SemanticCode::NullArgumentToNonNullType, range, a0, &a1);
    }

    pub(crate) fn check_invocation_arity(&mut self, callee: usize, count: u16, range: SourceRange) {
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        let name = row.name;
        let qualified = row.qualified;
        let shape = row.shape.clone();
        if let RowShape::Bare(BareInfo::Container {
            name,
            kind: ContainerKind::Extension,
        }) = &shape
            && self.container_kind_of(*name) == Some(ContainerKind::Extension)
        {
            // An explicit extension override reached through the send
            // (`Ex<int>(list)`) — the group's deferred check dispatches by
            // shape here (`extension_member_resolver.dart:380`). The row
            // kind conflates extension types: `E<int>(x)` on an extension
            // type is a creation invocation whose group checks the TYPE's
            // arity like a class's — the resolved kind decides, not the
            // micro tag.
            return self.check_extension_arity(*name, count, range);
        }
        let declared = match &shape {
            // The wave-3 member shape: the send closes before the
            // dot-access event merges `receiver.name`, so the callee is an
            // unmerged member placeholder (qualified, unresolved) with the
            // receiver row directly below — classify the receiver and walk
            // its members (`c.m<D>()` reports `m`).
            RowShape::Bare(BareInfo::Plain { .. }) if qualified => {
                let recv = callee.checked_sub(1).and_then(|i| self.rows.get(i)).map_or(
                    Receiver::Unknown,
                    |below| match &below.shape {
                        RowShape::Bare(info) => Engine::receiver_of_bare(*info),
                        RowShape::Receiver(recv) => *recv,
                        _ => Receiver::Unknown,
                    },
                );
                // A constructor selector carrying type arguments
                // (`C.new<int,int>()`, `C.named<int>()`): constructors never
                // declare type parameters, so any non-empty group reports
                // `_constructor` — the dedicated code, not the `_element`
                // dispatch a member would take (`type_instantiation_target`'s
                // ConstructorElement arm). The receiver must resolve to a
                // class that declares the named/unnamed constructor; a
                // member named `new` that is a plain method keeps the member
                // walk.
                if let Some((class, ctor)) = self.ctor_arity_target(&recv, name)
                    && count != 0
                {
                    let class = self.strings.resolve(class).to_string();
                    let ctor = self.strings.resolve(ctor).to_string();
                    self.emit3(
                        SemanticCode::WrongNumberOfTypeArgumentsConstructor,
                        range,
                        &class,
                        &ctor,
                        "",
                    );
                    return;
                }
                // The prefixed-creation spelling (`fb.ListReader<idl.X>(...)`):
                // the parser keeps `p.C` on the member-send shape — the head
                // `p` latched as the prefix receiver and the class `C` as the
                // member name — so the group never arms `creation_ref`. When
                // the member name resolves to a CONTAINER through the
                // prefix's scope, the invocation is a constructor call whose
                // group counts the class's declared arity (the base code, as
                // the unprefixed `C<T>` head's); a non-container member keeps
                // the member walk (a prefixed method's own arity is
                // unmodeled — under-report).
                if let Receiver::Prefix { name: prefix } = recv
                    && let Some((container, _)) = prefixed_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        prefix,
                        name,
                    )
                {
                    let declared = container.type_params;
                    if declared != count {
                        let name_text = self.strings.resolve(name).to_string();
                        self.emit3(
                            SemanticCode::WrongNumberOfTypeArguments,
                            range,
                            &name_text,
                            &declared.to_string(),
                            &count.to_string(),
                        );
                    }
                    return;
                }
                self.member_exec_arity(&recv, name)
            }
            // A top-level function (`g<int>()`); only real functions carry a
            // parameter list. A getter head is the oracle's
            // function-typed-expression target (`_function` — the head's
            // static type is the getter's return type), whose display the
            // model cannot render: silent, the under-report direction.
            RowShape::Bare(BareInfo::Plain { .. }) => self
                .top_level_decl(name)
                .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some())
                .map(|d| (d.type_params, ExecKind::Function))
                .or_else(|| {
                    // The implicit-this member call (`m<int>(1,"s")` inside
                    // the declaring class): the bare callee is unqualified,
                    // so it never reaches the `Member`/qualified arms — walk
                    // the enclosing container's members for the declared
                    // type-parameter count (#2161).
                    let (container, _) = self.container_context()?;
                    match self.walk_members(container, name) {
                        Walk::Found(entry) if entry.kind == MemberKind::Method => {
                            Some((entry.type_params, ExecKind::Method))
                        }
                        _ => None,
                    }
                }),
            RowShape::Member { recv } => self.member_exec_arity(recv, name),
            _ => None,
        };
        if let Some((declared, kind)) = declared
            && declared != count
        {
            let name_text = self.strings.resolve(name).to_string();
            self.emit4(
                SemanticCode::WrongNumberOfTypeArgumentsElement(kind),
                range,
                kind.word(),
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// A resolved container's declared type-parameter count (`None` when the
    /// name does not resolve — external types suppress).
    pub(crate) fn declared_type_params_of_container(&self, name: StringId) -> Option<u16> {
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .map(|(c, _)| c.type_params)
    }

    /// The `(class, ctor)` pair a constructor selector resolves to, when the
    /// receiver is a container that declares it — `.new` answering the
    /// unnamed constructor (`has_unnamed_constructor`), any other name the
    /// declared-constructor table. The receiver's class resolves by receiver
    /// kind (bare `C` in scope, `io.C` behind its prefix); a non-container or
    /// non-constructor name returns `None` and the member walk proceeds.
    pub(crate) fn ctor_arity_target(
        &self,
        recv: &Receiver,
        name: StringId,
    ) -> Option<(StringId, StringId)> {
        let (resolved, class) = match *recv {
            Receiver::Container { name, .. } => (
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    name,
                )?,
                name,
            ),
            Receiver::PrefixedContainer { prefix, name, .. } => (
                prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    name,
                )?,
                name,
            ),
            _ => return None,
        };
        let declares = resolved.0.constructors.contains(&name)
            || (name == self.intern("new") && resolved.0.has_unnamed_constructor);
        declares.then_some((class, name))
    }

    /// A member callee's declared type-parameter count with its oracle kind
    /// word: methods only. A getter- or field-headed application is the
    /// oracle's function-typed-expression target (`_function`,
    /// `type_instantiation_target.dart`
    /// `InvocationTargetFunctionTypedExpression` — the head's static type is
    /// the member's declared type), whose message displays that function
    /// type: unrenderable at the name-level model, so those finds suppress
    /// (`None`, the under-report direction).
    pub(crate) fn member_exec_arity(
        &self,
        recv: &Receiver,
        name: StringId,
    ) -> Option<(u16, ExecKind)> {
        let (container, super_walk) = match *recv {
            Receiver::Super { container, .. } => (container, true),
            Receiver::This { container, .. }
            | Receiver::Typed {
                type_name: container,
                ..
            }
            | Receiver::Created {
                name: container, ..
            }
            | Receiver::Container {
                name: container, ..
            } => (container, false),
            _ => return None,
        };
        let walk = if super_walk {
            self.walk_super(container, name)
        } else {
            self.walk_members(container, name)
        };
        match walk {
            Walk::Found(entry) if entry.kind == MemberKind::Method => {
                Some((entry.type_params, ExecKind::Method))
            }
            _ => None,
        }
    }

    /// The type-argument application event (`g<int>;`, `c<int>;`): a
    /// resolved executable tear-off reports `_element` naming its kind word
    /// (`function_reference_resolver.dart` → the
    /// `InvocationTargetExecutableElement` dispatch); a type literal reports
    /// the base code; an implicit call reference reports `_element` naming
    /// `call`. Getters and function-typed variables resolve to oracle shapes
    /// M1 cannot name (the getter kind word, the `_function` type display) —
    /// suppress.
    pub(crate) fn consume_type_argument_application(&mut self) {
        // The application applies to the most recent expression group —
        // its head row was resolved at `end_type_arguments` through the
        // group's base mark, so the group's own `TypeUse` rows above it
        // never hide the entry (LIFO, as in [`Engine::end_implicit_creation`]).
        let Some((row_index, count, range)) = self.deferred_args.pop() else {
            return;
        };
        // The tear-off consumed its group: the latched written arguments
        // die with it (no call follows to ground).
        let _ = self.call_args_for(row_index);
        let Some(row) = self.rows.get(row_index) else {
            return;
        };
        match &row.shape {
            RowShape::Bare(BareInfo::Container { name, .. }) => {
                // Type-literal instantiation `C<int>`: the base code.
                self.check_annotation_arity(*name, count, range);
            }
            RowShape::Bare(BareInfo::Plain { ty }) => {
                // A named top-level function tear-off, or an implicit call
                // reference through a declaration-typed receiver (`c<int>`
                // checks `call`), or a function-typed local's
                // instantiation (`var t = C<int>().m; t<A,B>` — the
                // tear-off's static type carries the parameter count the
                // `_function` face reports against, upstream's
                // `InvocationTargetFunctionTypedExpression` shape).
                let named_fn = self
                    .top_level_decl(row.name)
                    .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some())
                    .map(|d| (d.type_params, ExecKind::Function));
                if let Some((declared, kind)) = named_fn {
                    self.emit_element_arity(row.name, declared, kind, count, range);
                } else if let Ty::Function { shape, .. } = *ty {
                    let declared = self.boundary.pool().function_shape(shape).type_params;
                    if declared != count {
                        let display = self.ty_display(*ty);
                        self.emit3(
                            SemanticCode::WrongNumberOfTypeArgumentsFunction,
                            range,
                            &display,
                            &declared.to_string(),
                            &count.to_string(),
                        );
                    }
                } else if let Ty::Named { name, .. } = *ty
                    && let Some(entry) = self.resolved_member(name, self.intern("call"))
                    && entry.kind == MemberKind::Method
                {
                    self.emit_element_arity(
                        self.intern("call"),
                        entry.type_params,
                        ExecKind::Method,
                        count,
                        range,
                    );
                }
            }
            RowShape::Member { recv } => {
                // A property-access tear-off (`C.sm<int, String>;`): the
                // oracle's `InvocationTargetFunctionTypedExpression` reports
                // `_function` leading with the function type — rendered from
                // the captured signature. A method tear-off is the only
                // function-typed shape the model can render: getters, fields,
                // and unknown receivers suppress. A name that is no member at
                // all is the constructor-reference shape `C<...>.named` — the
                // base type-argument code against the container. A
                // created-instance receiver carries the created class's face
                // (`C().m<int, String>`): the target is type-agnostic —
                // `InvocationTargetFunctionTypedExpression.orNull` accepts
                // any function-typed expression
                // (type_instantiation_target.dart:148-155) — mirroring
                // `member_exec_arity`'s Created arm.
                let container = match *recv {
                    Receiver::This { container, .. }
                    | Receiver::Super { container, .. }
                    | Receiver::Typed {
                        type_name: container,
                        ..
                    }
                    | Receiver::Container {
                        name: container, ..
                    }
                    | Receiver::Created {
                        name: container, ..
                    } => Some(container),
                    _ => None,
                };
                let Some(container) = container else {
                    return;
                };
                match self.walk_members(container, row.name) {
                    Walk::Found(entry) if entry.kind == MemberKind::Method => {
                        if entry.type_params != count {
                            let display = self.function_type_display(&entry);
                            self.emit3(
                                SemanticCode::WrongNumberOfTypeArgumentsFunction,
                                range,
                                &display,
                                &entry.type_params.to_string(),
                                &count.to_string(),
                            );
                        }
                    }
                    // Non-method finds (getter/field tear-offs) are the
                    // oracle's function-typed-expression target — the
                    // `_function` row, whose display needs the function-type
                    // face the model cannot render; a broken chain is
                    // opaque. Both suppress (the under-report direction).
                    Walk::Found(_) | Walk::ChainBroken => {}
                    Walk::Missing => self.check_annotation_arity(container, count, range),
                }
            }
            _ => {}
        }
    }

    /// The `_function` message's function-type display from a captured
    /// method signature: `{return} Function<{type params}>({formal types})`
    /// — `T m<T>(T x)` renders `T Function<T>(T)`, the oracle's rendering of
    /// the generic instantiation's uninstantiated type.
    pub(crate) fn function_type_display(&self, entry: &crate::model::MemberEntry) -> String {
        let ret = self.ty_display(entry.return_ty);
        let params = entry
            .type_param_names
            .iter()
            .map(|p| self.strings.resolve(*p))
            .collect::<Vec<_>>()
            .join(", ");
        let formals = entry
            .formals
            .as_ref()
            .map(|f| {
                f.positional
                    .iter()
                    .map(|formal| self.ty_display(formal.ty))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let group = if params.is_empty() {
            String::new()
        } else {
            format!("<{params}>")
        };
        format!("{ret} Function{group}({formals})")
    }

    /// The `_element` emit: kind word, executable name, declared vs given.
    pub(crate) fn emit_element_arity(
        &mut self,
        name: StringId,
        declared: u16,
        kind: ExecKind,
        count: u16,
        range: SourceRange,
    ) {
        if declared != count {
            let name_text = self.strings.resolve(name).to_string();
            self.emit4(
                SemanticCode::WrongNumberOfTypeArgumentsElement(kind),
                range,
                kind.word(),
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// The resolved member entry of `container.member` from a clean walk.
    pub(crate) fn resolved_member(
        &self,
        container: StringId,
        member: StringId,
    ) -> Option<Projected<'a, MemberEntry>> {
        match self.walk_members(container, member) {
            Walk::Found(entry) => Some(entry),
            _ => None,
        }
    }

    /// The annotation/creation arity check: the head's declared
    /// type-parameter count against the given count (`dynamic` and
    /// in-scope type parameters declare none; unresolved and external heads
    /// suppress).
    pub(crate) fn check_annotation_arity(
        &mut self,
        name: StringId,
        count: u16,
        range: SourceRange,
    ) {
        let Some(declared) = self.declared_type_params(name) else {
            return;
        };
        if declared != count {
            let name_text = self.strings.resolve(name).to_string();
            self.emit3(
                SemanticCode::WrongNumberOfTypeArguments,
                range,
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// The continuation head of a prefixed type (`C` of `p.C<T>`): the
    /// written name directly precedes the head's span as `prefix.`, and the
    /// declared arity resolves through the import-prefix scope — a bare
    /// lookup can land on a resolution-elsewhere homonym whose parameter
    /// count is not the named element's (`shared.X<A,B,C,D>` inside a
    /// typedef RHS counted against the local typedef of the same spelling).
    /// Textual recovery keeps the armed-head tuple prefix-free: a bare head
    /// finds no `.` before its span and resolves through the ordinary path.
    /// A prefix segment that does not resolve as a registered import prefix
    /// suppresses the check (the dead-prefix face already disarmed this
    /// head upstream; reaching here with one is a parse-shape anomaly —
    /// under-report).
    fn prefixed_head(&self, head_range: SourceRange) -> Option<StringId> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(head_range.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor == 0 || bytes[cursor - 1] != b'.' {
            return None;
        }
        cursor -= 1;
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        let end = cursor;
        while cursor > 0
            && matches!(bytes[cursor - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
        {
            cursor -= 1;
        }
        if cursor == end {
            return None;
        }
        let prefix = self.strings.intern(&self.source[cursor..end]);
        // Only a live import prefix qualifies — `C.named` (a constructor
        // selector spelling) or a local shadow keeps the ordinary face.
        matches!(
            self.namespaces.lookup(self.graph, self.lib, prefix),
            dartq_resolution::Lookup::Found(NamespaceEntry::Prefix)
        )
        .then_some(prefix)
    }

    /// The declared type-parameter count of `name` resolved through the
    /// import prefix `prefix` (`p.C<T>`): the container model answers a
    /// class/mixin/enum count, the declaration index a typedef's or
    /// function's. An unresolved or unindexed element suppresses — the
    /// prefixed resolution face owns its own report.
    fn declared_type_params_prefixed(&self, prefix: StringId, name: StringId) -> Option<u16> {
        if let Some((container, _)) = prefixed_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            prefix,
            name,
        ) {
            return Some(container.type_params);
        }
        match self
            .namespaces
            .lookup_prefixed(self.graph, self.lib, prefix, name)
        {
            dartq_resolution::Lookup::Found(NamespaceEntry::Decl(file)) => self
                .index
                .get(file)
                .and_then(|f| f.decl(name))
                .map(|decl| decl.type_params),
            _ => None,
        }
    }

    /// [`Engine::check_annotation_arity`] for a continuation head: recover
    /// the written prefix from the head's span and count against the
    /// prefix-scoped element when one exists; bare heads (and continuations
    /// whose prefix never registered) keep the ordinary lookup.
    fn check_prefixed_annotation_arity(
        &mut self,
        name: StringId,
        head_range: SourceRange,
        count: u16,
        range: SourceRange,
    ) {
        let declared = match self.prefixed_head(head_range) {
            Some(prefix) => self.declared_type_params_prefixed(prefix, name),
            None => self.declared_type_params(name),
        };
        let Some(declared) = declared else {
            return;
        };
        if declared != count {
            let name_text = self.strings.resolve(name).to_string();
            self.emit3(
                SemanticCode::WrongNumberOfTypeArguments,
                range,
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// A type head's declared type-parameter count: `dynamic` and type
    /// parameters declare none, a container keeps its own count, a
    /// typedef/function keeps its declaration's; anything unresolved is
    /// `None` (suppress — `undefined_class` already reports). A header's
    /// parked parameters count as none while their header parses. A name
    /// that resolved into another file but carries no indexed declaration
    /// here is an external dart: type — [`EXTERNAL_CORE_TYPE_PARAMS`] answers
    /// (`_buildTypeArguments`, upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart`:
    /// the count comes from the resolved element, whatever file declared it).
    pub(crate) fn declared_type_params(&self, name: StringId) -> Option<u16> {
        if self.members.builtin_types.contains(&name)
            || self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
        {
            return Some(0);
        }
        let (info, target) = self.resolve_bare(name);
        match info {
            BareInfo::Container { name, .. } => resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            )
            .map(|(c, _)| c.type_params)
            .or_else(|| {
                // The unindexed-decl face (the synthetic SDK nodes —
                // `bare_info_of_entry` classifies the primitive spellings as
                // containers): the frozen external arities answer.
                matches!(
                    target,
                    RefTarget::Decl { file } if self.index.get(file).is_none()
                )
                .then(|| {
                    self.external_type_params
                        .iter()
                        .find(|(id, _)| *id == name)
                        .map(|(_, count)| *count)
                })
                .flatten()
            }),
            BareInfo::Plain { .. } => {
                // A container can surface as `Plain`: `resolve_bare`'s
                // enclosing-frame member arm answers `Plain` whenever that
                // frame's member walk breaks — an unnamed extension's frame
                // name resolves to nothing (unnamed extensions are absent
                // from the container index), so its on-clause heads land
                // here behind a member `Plain`. The bare `TopDecl` the
                // fallback below would read carries no container count (0);
                // the container model does
                // (`_buildTypeArguments`, upstream Dart SDK source,
                // `pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart`:
                // the count comes from the resolved element).
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    name,
                )
                .map(|(c, _)| c.type_params)
                .or_else(|| {
                    self.top_level_decl(name)
                        .map(|d| d.type_params)
                        .or_else(|| {
                            // Defined in a file the index does not carry (the
                            // dart: face — indexed declarations answered
                            // above): the frozen SDK arities decide, an
                            // unknown spelling suppresses.
                            matches!(
                                target,
                                RefTarget::Decl { file } if self.index.get(file).is_none()
                            )
                            .then(|| {
                                self.external_type_params
                                    .iter()
                                    .find(|(id, _)| *id == name)
                                    .map(|(_, count)| *count)
                            })
                            .flatten()
                        })
                })
            }
            _ => None,
        }
    }

    /// The `end_type_arguments` dispatch — the code choice by use-site shape.
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    pub(crate) fn end_type_arguments(&mut self, count: usize, begin: Token, end: Token) {
        let count = u16::try_from(count).unwrap_or(u16::MAX);
        let range = SourceRange {
            offset: begin.range.offset,
            length: end.range.end() - begin.range.offset,
        };
        // A type head makes the group an annotation or a creation type
        // reference.
        let (head, base) = self.group_heads.pop().unwrap_or((None, 0));
        // The row the group applies to anchors the group only when it abuts
        // the group across trivia: the head token is the group's syntactic
        // predecessor, so a row ending earlier belongs to an older
        // expression. A switch-case object pattern's named type pushes its
        // row only at `handle_object_pattern`, after its group closed, so
        // the group would otherwise classify against whatever invocation row
        // the previous case body left open (#2491: the pattern's `<`
        // reported the previous arm's callee's arity as
        // `wrong_number_of_type_arguments_element`).
        let anchored = base
            .checked_sub(1)
            .and_then(|i| self.rows.get(i))
            .is_some_and(|row| self.trivia_between(row.range.end(), begin.range.offset));
        // The written-argument capture closes with its own (headless)
        // group; the head row is the one the group applies to. An
        // unanchored headless group grounds no callee — the capture dies
        // here so its members never latch onto the stale slot (the same
        // under-report posture as the merge drains' entry-drop rule).
        if head.is_none() && anchored {
            self.call_args_end(base.saturating_sub(1));
        }
        // The clause-entry capture (#1289 E1): the group closed — an
        // outermost one re-arms the entry span's closer latch, consumed by
        // the head's completion re-fire (a headless group latches nothing).
        if let Some(deferred) = self.deferred_modifier.as_mut() {
            let spans = &mut deferred.spans;
            if spans.arg_depth > 0 {
                spans.arg_depth -= 1;
                if spans.arg_depth == 0 && head.is_some() {
                    spans.args_closer = Some(end.range);
                }
            }
        }
        // The #610 site capture: the site's own group closed with a root —
        // record the check for the finalize pass (the whole-file snapshot).
        // The root's written arguments also latch for the declaration that
        // completes this head ([`Engine::completed_group`]): the body-level
        // pool stays capture-immutable, so a local's declared type carries
        // its arguments through this side channel.
        if let Some(mut roots) = self.bounds.end_group(range)
            && let Some(tree) = roots.pop()
        {
            let args = tree.args.iter().map(|arg| self.tree_ty(arg)).collect();
            self.completed_group = Some((tree.head, tree.range.offset, args));
            self.pending_bounded_checks.push(PendingBoundedCheck::Site {
                head: tree.head,
                args: tree.args,
                in_scope: self.type_params.clone(),
                site: tree.range,
            });
        }
        // The #1351 capture: the open typedef RHS's group closed.
        self.bounds.alias_end_group(range);
        // The #612 capture: the header's group closed into its frame.
        self.var_end_group();
        if let Some((head, head_range)) = head {
            self.check_prefixed_annotation_arity(head, head_range, count, range);
            return;
        }
        // An enum constant's constructor type arguments (`a<int>()`,
        // `a.named<int>()`): the group at the constant's own position —
        // before its `(` opens an argument list — checks against the
        // enclosing enum's declared count. Groups inside the constructor
        // arguments (`a(h<int>())`) run while that list is open and check
        // their own heads instead; the snapshot itself clears at
        // [`Engine::leave_enum_element`].
        if self.arg_levels.is_empty()
            && let Some(declared) = self.in_enum_element
            && declared != count
        {
            self.emit3(
                SemanticCode::WrongNumberOfTypeArgumentsEnum,
                range,
                &declared.to_string(),
                &count.to_string(),
                "",
            );
            return;
        }
        // The wave-3 constructor-reference shape, checked before the row
        // stack: reference heads fire as `ConstructorReference` type uses
        // (a resolved name pushes no row, so the stack may be empty) — the
        // reference's own group matches through `creation_ref`. `C<int>`
        // checks the class face (base code); `C.named<int>` reports
        // `_constructor` for any count (constructors never declare type
        // parameters; the class face must resolve — a missing class already
        // reports `undefined_class`).
        // A collection literal's own type-argument group (`<String,int>{}`,
        // `<Set<E>>{}`) is never a constructor's group — it is headless and
        // the `>` is immediately followed by the literal's `[`/`{` opener.
        // Bail before every invocation/creation attribution: the literal's
        // elements are the set/map face's own checks (`check_list_literal_
        // elements`, `handle_literal_set_or_map`), not a callee's (#2161).
        {
            let bytes = self.source.as_bytes();
            // `end.range.end()` is a UTF-16 offset — convert before the byte
            // walk so a wide char upstream does not desync the `[`/`{` probe.
            let mut cursor = self.byte_of(end.range.end());
            while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor < bytes.len() && matches!(bytes[cursor], b'[' | b'{') {
                return;
            }
        }
        // A literal's type-argument group inside the call's argument list
        // (`A._(<String,int>{})`) must not attribute its count to the
        // enclosing constructor — `creation_ref` stays latched until the
        // reference closes (after the arg list), so only a group opened at
        // argument-list depth 0 is the constructor's own (#2161).
        if self.ctor_pos > 0
            && self.arg_levels.is_empty()
            && let Some((class, ctor)) = self.creation_ref
        {
            match ctor {
                Some((ctor, ctor_range)) => {
                    // The `.new` selector (or one spelled as the class) is
                    // the unnamed face: the explicit arguments check against
                    // the CLASS's declared count, not the constructor's
                    // (constructors never declare type parameters).
                    if ctor == class || self.strings.resolve(ctor) == "new" {
                        self.check_annotation_arity(class, count, range);
                    } else if let BareInfo::Prefix { .. } = self.resolve_bare(class).0 {
                        // The wave-3 reference is `prefix.Class<T>` — the
                        // parser cannot tell a prefixed class head from a
                        // `C.ctor` pair, so the class segment latched as the
                        // head and the real class as its selector. The group
                        // counts against the continuation resolved through
                        // the prefix's scope (`fb.ListReader<idl.X>`). A
                        // third segment (`p.C.named<T>` — a `.` follows the
                        // selector) keeps the `_constructor` face: the group
                        // there applies to the named constructor.
                        let mut cursor = self.byte_of(ctor_range.end());
                        while cursor < self.source.len()
                            && self.source.as_bytes()[cursor].is_ascii_whitespace()
                        {
                            cursor += 1;
                        }
                        if cursor < self.source.len() && self.source.as_bytes()[cursor] == b'.' {
                            let class = self.strings.resolve(class).to_string();
                            let ctor = self.strings.resolve(ctor).to_string();
                            self.emit3(
                                SemanticCode::WrongNumberOfTypeArgumentsConstructor,
                                range,
                                &class,
                                &ctor,
                                "",
                            );
                        } else {
                            self.check_prefixed_annotation_arity(ctor, ctor_range, count, range);
                        }
                    } else if !matches!(self.resolve_bare(class).0, BareInfo::Missing { .. }) {
                        let class = self.strings.resolve(class).to_string();
                        let ctor = self.strings.resolve(ctor).to_string();
                        self.emit3(
                            SemanticCode::WrongNumberOfTypeArgumentsConstructor,
                            range,
                            &class,
                            &ctor,
                            "",
                        );
                    }
                }
                None => self.check_annotation_arity(class, count, range),
            }
            return;
        }
        let Some(callee) = self.rows.len().checked_sub(1) else {
            return;
        };
        // A type-argument group inside an open argument list belongs to the
        // argument expression's own head — never the enclosing creation's
        // callee row (`Report(List<String>.of)` reads `rows[callee]`/`callee
        // -1` for `Report`'s send head, mis-attributing `List`'s group to the
        // outer constructor). The `creation_ref` block above already gates on
        // `arg_levels.is_empty()`; this `ctor_pos` branch must too — the
        // argument's arity defers to its own send through `deferred_args`.
        if self.ctor_pos > 0 && self.arg_levels.is_empty() {
            // A creation reference head. `parse_constructor_reference` keeps
            // its segments unmerged on the row stack: `C<int>` is a bare
            // container row (base code), `C.named<int>` is two bare rows —
            // the class face then the constructor segment (any count
            // reports: constructors never declare type parameters).
            let named_ctor = (callee >= 1)
                .then(|| {
                    let ctor = &self.rows[callee];
                    let class = &self.rows[callee - 1];
                    match (&class.shape, &ctor.shape) {
                        (
                            RowShape::Bare(BareInfo::Container {
                                name: class_name,
                                kind: ContainerKind::Class,
                            }),
                            RowShape::Bare(_),
                        ) => Some((*class_name, ctor.name)),
                        // An extension type's constructor selector
                        // (`E.named<int>()`) — the row kind carries the
                        // conflated `Extension` tag, so the resolved kind
                        // discriminates: only an `ExtensionType` counts as
                        // a constructable container here (a plain
                        // extension's `Ext.named` is a member, not a
                        // constructor).
                        (
                            RowShape::Bare(BareInfo::Container {
                                name: class_name,
                                kind: ContainerKind::Extension,
                            }),
                            RowShape::Bare(_),
                        ) if self.container_kind_of(*class_name)
                            == Some(ContainerKind::ExtensionType) =>
                        {
                            Some((*class_name, ctor.name))
                        }
                        _ => None,
                    }
                })
                .flatten();
            // A selector equal to the class name (the `.new` spelling
            // normalizes to it) is the unnamed face: the class arity owns
            // the check, falling through to it below.
            if let Some((class, ctor)) = named_ctor
                && ctor != class
            {
                let class = self.strings.resolve(class).to_string();
                let ctor = self.strings.resolve(ctor).to_string();
                self.emit3(
                    SemanticCode::WrongNumberOfTypeArgumentsConstructor,
                    range,
                    &class,
                    &ctor,
                    "",
                );
                return;
            }
            if let Some((class, _)) = named_ctor {
                self.check_annotation_arity(class, count, range);
                return;
            }
            let row = &self.rows[callee];
            match &row.shape {
                RowShape::Bare(BareInfo::Container {
                    name,
                    kind: ContainerKind::Extension,
                }) if self.container_kind_of(*name) == Some(ContainerKind::Extension) => {
                    // Explicit extension override `Ext<int>(list)` — any
                    // arity mismatch reports at the group
                    // (`extension_member_resolver.dart:380`). An extension
                    // TYPE falls through to the container arm: `E<int>(x)`
                    // is a constructor invocation, and its group checks the
                    // type's declared arity like a class's.
                    self.check_extension_arity(*name, count, range);
                }
                RowShape::Bare(BareInfo::Container { name, .. }) => {
                    let name = *name;
                    self.check_annotation_arity(name, count, range);
                }
                _ => {
                    // A function invoked through the implicit-creation
                    // syntax resolves at the send; defer.
                    self.deferred_args.push((callee, count, range));
                }
            }
            return;
        }
        // The head row the group applies to is the last row pushed before
        // `<` (`base - 1`); rows pushed inside the group are its own type
        // arguments — unresolved ones are `TypeUse` rows. An unresolved head
        // pushes its own `TypeUse` row, which the deferred checks then
        // ignore; a row from an older expression never matches — and only a
        // trivia-adjacent row is this group's head at all (`anchored`): a
        // pattern named type's group, whose own row lands later at
        // `handle_object_pattern`, defers against nothing.
        if !anchored {
            return;
        }
        let Some(target) = base.checked_sub(1).and_then(|i| self.rows.get(i)) else {
            return;
        };
        if matches!(target.shape, RowShape::Bare(_) | RowShape::Member { .. }) {
            self.deferred_args.push((base - 1, count, range));
        }
    }

    /// The explicit extension override's type-argument check
    /// (`extension_member_resolver.dart:380`): any arity mismatch reports
    /// `_extension` at the group.
    pub(crate) fn check_extension_arity(&mut self, name: StringId, count: u16, range: SourceRange) {
        if let Some(declared) = self.declared_type_params_of_container(name)
            && declared != count
        {
            let name_text = self.strings.resolve(name).to_string();
            self.emit3(
                SemanticCode::WrongNumberOfTypeArgumentsExtension,
                range,
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
        }
    }

    /// A typed list literal's `List<E>` static type, `None` for the
    /// untyped/inferred shape or when the element type does not resolve to
    /// a type (the same gates [`Self::check_list_literal_elements`] applies
    /// to its assignability loop — the row face keeps that posture).
    /// Whether a `?` nullability suffix immediately follows the type-use
    /// token ending at `end` (`<String?>[…]`, `String? x`): the type-use
    /// row names `String` but the `?` rides the suffix token, so the
    /// written type's own nullability lives one token past the name (#2161).
    pub(crate) fn written_nullable_suffix(&self, end: u32) -> bool {
        // `end` is a UTF-16 offset: convert before reading source bytes so a
        // wide char anywhere upstream does not desync the `?` probe.
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(end);
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        bytes.get(cursor) == Some(&b'?')
    }
}

/// Pins for the S2 member-dispatch faces (#2213): the frozen
/// SDK-interface member tables the `@override` walk consults through
/// [`Engine::face_member_frozen`] (the `implements`-face climb cannot reach
/// an unindexed `dart:core`/`dart:async`/`dart:io` declaration, so the
/// oracle's `getOverridden` answer comes from
/// [`crate::member_face::CORE_INTERFACE_MEMBERS`]), and the dynamic-receiver
/// dispatch silence — a `dynamic` static type never carries a member face
/// the `undefined_*` trio can report against
/// (`type_property_resolver.dart`'s `DynamicType` arm answers the access
/// without a lookup result).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_resolution::LibraryGraphBuilder;
    use dartq_scanner::ScanConfig;
    use dartq_source::FileTable;
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library with no imports —
    /// the frozen faces under pin answer by spelling, so an unresolved
    /// `implements Match` exercises the same path the corpus hits behind a
    /// real SDK import edge.
    fn report(source: &str) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        let mut report = fold.report;
        report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
        report
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// The `implements` clause names an unindexed SDK interface in these
    /// pins, so the reference reports `undefined_class` beside the member
    /// face — the assertion reads only the override-annotation family.
    fn override_codes(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "override_on_non_overriding_member")
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// The petitparser `ParserException implements FormatException` shape:
    /// the three `@override` getters (`message`/`offset`/`source`) are all
    /// `FormatException`'s declared face — the frozen table spares each.
    #[test]
    fn format_exception_interface_spares_the_override_rows() {
        let report = report(
            "class ParserException implements FormatException { \
               @override String get message => ''; \
               @override int get offset => 0; \
               @override String get source => ''; \
             }",
        );
        assert!(
            override_codes(&report).is_empty(),
            "{:?}",
            code_names(&report)
        );
    }

    /// The petitparser `ParserMatch implements Match` shape: every
    /// `@override` member names a `Match` interface member
    /// (`sdk/lib/core/pattern.dart:85` — `group`/`[]`/`groups`/
    /// `groupCount` methods, the `pattern`/`input`/`start`/`end` getters).
    #[test]
    fn match_interface_spares_the_override_rows() {
        let report = report(
            "class ParserMatch implements Match { \
               @override int get start => 0; \
               @override int get end => 0; \
               @override String group(int group) => ''; \
               @override String operator [](int group) => ''; \
               @override List<String> groups(List<int> groupIndices) => []; \
               @override int get groupCount => 0; \
               @override String get input => ''; \
               @override String get pattern => ''; \
             }",
        );
        assert!(
            override_codes(&report).is_empty(),
            "{:?}",
            code_names(&report)
        );
    }

    /// The dio `_DefaultIfEmptyStreamSink implements EventSink` shape:
    /// `add`/`addError`/`close` are `EventSink`'s declared face
    /// (`sdk/lib/async/stream.dart:2295` — `Sink`'s `add`/`close` ride the
    /// union because the unindexed `implements Sink` edge is invisible to
    /// the frozen climb).
    #[test]
    fn event_sink_interface_spares_the_override_rows() {
        let report = report(
            "class S implements EventSink { \
               @override void add(Object event) {} \
               @override void addError(Object error, [List<int>? st]) {} \
               @override void close() {} \
             }",
        );
        assert!(
            override_codes(&report).is_empty(),
            "{:?}",
            code_names(&report)
        );
    }

    /// The http `_ClientSocketException implements SocketException` shape:
    /// `address`/`osError`/`port`/`message` are `SocketException`'s
    /// declared fields (`sdk/lib/io/socket.dart:1398`).
    #[test]
    fn socket_exception_interface_spares_the_override_rows() {
        let report = report(
            "class ClientException implements SocketException { \
               @override int get port => 0; \
               @override int get osError => 0; \
               @override int get address => 0; \
             }",
        );
        assert!(
            override_codes(&report).is_empty(),
            "{:?}",
            code_names(&report)
        );
    }

    /// The spare is exact, not a blanket silence: an `@override` member no
    /// interface declares still reports `override_on_non_overriding_member`
    /// (the frozen face answers `false` for absent spellings).
    #[test]
    fn absent_interface_member_keeps_the_override_row() {
        let report = report("class C implements Match { @override int get absent => 0; }");
        assert_eq!(
            override_codes(&report),
            ["override_on_non_overriding_member"],
            "{:?}",
            code_names(&report)
        );
    }

    /// A `dynamic` receiver never reports the member family: the oracle's
    /// resolver routes the access to dynamic dispatch with no lookup
    /// result (`type_property_resolver.dart`'s `DynamicType` arm), so a
    /// declared `dynamic` local's reads and invocations stay silent.
    #[test]
    fn dynamic_receiver_reports_no_member_face() {
        let report = report("void f(dynamic x) { x.gone; x.missing(); x.absent = 1; }");
        let member_rows: Vec<&'static str> = code_names(&report)
            .into_iter()
            .filter(|name| {
                matches!(
                    *name,
                    "undefined_getter" | "undefined_method" | "undefined_setter"
                )
            })
            .collect();
        assert!(member_rows.is_empty(), "{:?}", code_names(&report));
    }

    /// The `(x as dynamic).y` single-root repro (dio
    /// `io_adapter.dart:220`): the cast's own boolish-target latch covers
    /// the folded `Value` row's exact span, collapsing the parenthesized
    /// group's static type to `bool` — the merge re-reads the operand's
    /// real type off the folded row below, so the member face resolves
    /// against `dynamic` and stays silent.
    #[test]
    fn cast_to_dynamic_receiver_reports_no_member_face() {
        for source in [
            "void f(Object x) { (x as dynamic).gone; }",
            "void f(int x) { (x as dynamic).missing(); }",
            "void f(Object x) { (x as dynamic).absent = 1; }",
            "void f() { (1 as dynamic).gone; }",
            "void f() { (null as dynamic).gone; }",
            "void f(Object x) { ((x as dynamic)).gone; }",
        ] {
            let report = report(source);
            let member_rows: Vec<&'static str> = code_names(&report)
                .into_iter()
                .filter(|name| {
                    matches!(
                        *name,
                        "undefined_getter" | "undefined_method" | "undefined_setter"
                    )
                })
                .collect();
            assert!(
                member_rows.is_empty(),
                "{source}: {:?}",
                code_names(&report)
            );
        }
    }

    /// The re-read is exact, not a blanket silence: a cast to a concrete
    /// declared type keeps the member face on the cast target — `(x as int)`
    /// reports `undefined_getter` against `int` (the same `Typed` receiver
    /// the bare-read shape resolves).
    #[test]
    fn cast_to_named_receiver_keeps_the_member_face() {
        let report = report("void f(Object x) { (x as int).gone; }");
        assert!(
            code_names(&report).contains(&"undefined_getter"),
            "{:?}",
            code_names(&report)
        );
    }

    // —— #2214 S3: private-named field-formal call-site spelling ————————————

    /// A `this._x` named formal is spelled `x:` at call sites — the element
    /// stores the public name (`correspondingPublicName`,
    /// token_impl.dart:364), so the oracle's `AsyncData._(…, error: e)` binds
    /// `required this._error` and stays silent. The pin reproduces the
    /// riverpod `async_value.dart` shape: a private named required field
    /// formal satisfied by its public-spelled argument.
    #[test]
    fn private_named_field_formal_binds_by_public_spelling() {
        let report = report(
            "class C { \
               final int _err; \
               final int _load; \
               const C._(this._val, {required this._err, required this._load}); \
               final int _val; \
             } \
             void f(C c) { C._(0, err: 1, load: 2); }",
        );
        assert!(
            !code_names(&report).contains(&"missing_required_argument"),
            "{:?}",
            code_names(&report)
        );
        assert!(
            !code_names(&report).contains(&"undefined_named_parameter"),
            "{:?}",
            code_names(&report)
        );
    }

    /// The public spelling is exact, not a renamed free-for-all: a private
    /// named required formal never supplied still reports
    /// `missing_required_argument` — rendered with the public face (the
    /// element's `name`, not the private `privateName`).
    #[test]
    fn private_named_field_formal_reports_public_name() {
        let report = report(
            "class C { \
               final int _err; \
               const C._(this._val, {required this._err}); \
               final int _val; \
             } \
             void f(C c) { C._(0); }",
        );
        let report_names: Vec<&str> = code_names(&report);
        assert!(
            report_names.contains(&"missing_required_argument"),
            "{:?}",
            report_names
        );
    }

    /// A `super._x` formal associates against the superclass constructor's
    /// `this._x` named formal through the public face — upstream matches the
    /// element names (`superConstructorParameter` keys both on the public
    /// spelling), so the forwarding is associated, never
    /// `super_formal_parameter_without_associated_named`.
    #[test]
    fn super_formal_associates_by_public_name() {
        let report = report(
            "class B { \
               final int _err; \
               const B({required this._err}); \
             } \
             class C extends B { \
               const C({required super._err}); \
             }",
        );
        assert!(
            !code_names(&report).contains(&"super_formal_parameter_without_associated_named"),
            "{:?}",
            code_names(&report)
        );
    }

    // —— #2214 S4-B: cascade target collapse ——————————————————————————————

    /// A `..p = literal` section's RHS row must not become the next
    /// section's receiver: `C()..p = 1..q = true` resolves every section
    /// against the cascade target `C` (`cascadeReceiver`,
    /// type_analyzer.dart:611-618), so `q` — declared on `C` — stays silent.
    #[test]
    fn cascade_section_resolves_against_the_target_not_the_rhs() {
        let report = report(
            "class C { \
               int p = 0; \
               bool q = false; \
             } \
             void f() { C()..p = 1..q = true; }",
        );
        assert!(
            !code_names(&report).contains(&"undefined_setter"),
            "{:?}",
            code_names(&report)
        );
    }

    /// The collapse is receiver-correct, not a blanket silence: a cascade
    /// section naming a member the target does not declare still reports —
    /// the latched `Receiver` resolves `missing` on `C` the same way a
    /// first-section miss does.
    #[test]
    fn cascade_section_still_reports_a_target_miss() {
        let report = report(
            "class C { int p = 0; } \
             void f() { C()..p = 1..missing = true; }",
        );
        assert!(
            code_names(&report).contains(&"undefined_setter"),
            "{:?}",
            code_names(&report)
        );
    }

    /// A fresh cascade re-latches its own target: the second cascade in
    /// `C()..p = 1; D()..q = true;` resolves `q` on `D`, not the first
    /// cascade's `C` (the latched `member_reads` entry is gated on the popped
    /// leftover being a `Value`, so `D()`'s real receiver row wins).
    #[test]
    fn second_cascade_latches_its_own_target() {
        let report = report(
            "class C { int p = 0; } \
             class D { bool q = false; } \
             void f() { C()..p = 1; D()..q = true; }",
        );
        assert!(
            !code_names(&report).contains(&"undefined_setter"),
            "{:?}",
            code_names(&report)
        );
    }

    /// A bare-identifier RHS leaves a `Bare` row, not a `Value` one —
    /// `Decl()..version = version..encoding = encoding` must still resolve
    /// `encoding` against the latched cascade target `Decl`, never against
    /// the parameter's `String` (upstream `cascadeReceiver`,
    /// type_analyzer.dart:611-618). xml `builder.dart`'s `declaration`
    /// shape.
    #[test]
    fn cascade_section_with_bare_rhs_keeps_the_latched_target() {
        let report = report(
            "class Decl { \
               String? get version => null; \
               set version(String? value) {} \
               String? get encoding => null; \
               set encoding(String? value) {} \
             } \
             void build({String? version, String? encoding}) { \
               final d = Decl()..version = version..encoding = encoding; \
             }",
        );
        assert!(
            !code_names(&report).contains(&"undefined_setter"),
            "{:?}",
            code_names(&report)
        );
    }

    /// The real-file shape (xml `builder.dart`'s `declaration`): getter +
    /// setter pairs and three chained sections — every section resolves
    /// against `Decl`, never against the previous section's RHS type.
    #[test]
    fn cascade_getter_setter_chain_keeps_the_latched_target() {
        let report = report(
            "class Decl { \
               String? get version => null; \
               set version(String? value) {} \
               String? get encoding => null; \
               set encoding(String? value) {} \
               bool get standalone => false; \
               set standalone(bool? value) {} \
             } \
             void build({String? version, String? encoding, bool? standalone}) { \
               final d = Decl() \
                 ..version = version \
                 ..encoding = encoding \
                 ..standalone = standalone; \
             }",
        );
        assert!(
            !code_names(&report).contains(&"undefined_setter"),
            "{:?}",
            code_names(&report)
        );
    }

    /// The collapse stays receiver-correct for the bare-RHS shape too:
    /// `Decl()..version = version..missing = encoding` resolves `missing`
    /// on `Decl` and reports the target miss.
    #[test]
    fn cascade_section_after_bare_rhs_still_reports_a_target_miss() {
        let report = report(
            "class Decl { \
               String? get version => null; \
               set version(String? value) {} \
             } \
             void build({String? version, String? encoding}) { \
               final d = Decl()..version = version..missing = encoding; \
             }",
        );
        assert!(
            code_names(&report).contains(&"undefined_setter"),
            "{:?}",
            code_names(&report)
        );
    }

    /// A member-read RHS records its own read AFTER the section's selector,
    /// so the latch must answer the newest CASCADE-SECTION read, not
    /// `member_reads`' plain tail: `request..contentLength =
    /// original.contentLength..followRedirects = original.followRedirects`
    /// still resolves every section against `request` (http `retry.dart`'s
    /// build — the tail answered the RHS face and the next section reported
    /// `undefined_setter` against `int`).
    #[test]
    fn cascade_section_after_member_read_rhs_keeps_the_latched_target() {
        let report = report(
            "class Req { \
               int? get contentLength => null; \
               set contentLength(int? value) {} \
               bool get followRedirects => false; \
               set followRedirects(bool value) {} \
               bool get maxRedirects => false; \
               set maxRedirects(bool value) {} \
             } \
             class Orig { \
               int? get contentLength => null; \
               bool get followRedirects => true; \
               bool get maxRedirects => true; \
             } \
             void build(Req request, Orig original) { \
               request \
                 ..contentLength = original.contentLength \
                 ..followRedirects = original.followRedirects \
                 ..maxRedirects = original.maxRedirects; \
             }",
        );
        assert!(
            !code_names(&report).contains(&"undefined_setter"),
            "{:?}",
            code_names(&report)
        );
    }

    /// The member-read-RHS latch stays receiver-correct for a target miss:
    /// `req..contentLength = orig.contentLength..missing = 1` resolves
    /// `missing` on `Req` and reports the setter miss against it.
    #[test]
    fn cascade_section_after_member_read_rhs_still_reports_a_target_miss() {
        let report = report(
            "class Req { \
               int? get contentLength => null; \
               set contentLength(int? value) {} \
             } \
             class Orig { \
               int? get contentLength => null; \
             } \
             void build(Req req, Orig orig) { \
               req..contentLength = orig.contentLength..missing = 1; \
             }",
        );
        assert!(
            code_names(&report).contains(&"undefined_setter"),
            "{:?}",
            code_names(&report)
        );
    }

    /// A chained access ON a section (`..headers.addAll(x)`) pops the
    /// section's own selector row — that send resolves against the
    /// section's member type (`headers`' map), never the latched cascade
    /// target (`..headers.addAll` must not report `undefined_method`
    /// against `Req`; the http `mock_client.dart` shape).
    #[test]
    fn cascade_chained_send_on_a_section_resolves_against_the_section_type() {
        let report = report(
            "class Req { \
               final Map<String, String> headers = {}; \
               int? get contentLength => null; \
               set contentLength(int? value) {} \
             } \
             void build(Req request, Map<String, String> other) { \
               request \
                 ..contentLength = 1 \
                 ..headers.addAll(other); \
             }",
        );
        assert!(
            !code_names(&report).contains(&"undefined_method"),
            "{:?}",
            code_names(&report)
        );
    }

    /// A function-typed field claims the unqualified callee before the
    /// namespace: `predicate(result)` inside `PredicateParser` resolves
    /// `this.predicate`, never the same-spelled top-level function — the
    /// oracle's scope order. The field's alias signature answers arity
    /// (1 == 1), and no top-level formals may leak in.
    #[test]
    fn function_typed_field_claims_the_bare_callee() {
        let report = report(
            "typedef Pred = bool Function(String); \
             Pred predicate(String s, int n) => true; \
             class P { \
               final Pred predicate; \
               const P(this.predicate); \
               bool test(String r) => predicate(r); \
             }",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"not_enough_positional_arguments")
                && !names.contains(&"invocation_of_non_function_expression"),
            "{names:?}"
        );
    }

    /// A non-callable member still claims the callee: `count(1)` inside a
    /// class declaring `int count` reports the non-function face against
    /// the field — a same-spelled top-level function must not silently
    /// supply the signature either (the member wins the scope race).
    #[test]
    fn non_function_field_blocks_the_top_level_fallback() {
        let report = report(
            "int count(int a, int b) => a; \
             class P { \
               final int count; \
               const P(this.count); \
               int test() => count(1); \
             }",
        );
        let names = code_names(&report);
        assert!(
            !names.contains(&"not_enough_positional_arguments"),
            "{names:?}"
        );
    }

    // —— #2194-F3: argument-depth type-argument delimitation ———————————————

    /// A type-argument group inside a call's argument list belongs to the
    /// argument's own head: `Report(List<String>.of(rows))` checks `List`'s
    /// arity (1 == 1 → silent), never attributes the group to `Report` —
    /// `end_type_arguments`' creation branches gate on `arg_levels.is_empty()`
    /// so the argument's send resolves the arity itself.
    #[test]
    fn nested_type_arguments_belong_to_the_argument_head() {
        let report = report(
            "class Report { \
               final List<String> rows; \
               const Report(this.rows); \
             } \
             void f(List<String> rows) { Report(List<String>.of(rows)); }",
        );
        let arity: Vec<&'static str> = code_names(&report)
            .into_iter()
            .filter(|name| name.starts_with("wrong_number_of_type_arguments"))
            .collect();
        assert!(arity.is_empty(), "{:?}", code_names(&report));
    }

    // —— #2218 S8: `this` accessibility ——————————————————————————————

    /// `this` inside a `late` instance field's initializer is bound — the
    /// oracle's `bindsThis` (`variable_declaration_resolver.dart:47-49`:
    /// `!isStatic && isLate`) wraps the whole initializer, arguments and
    /// nested closures included. The webview `late final _webView =
    /// WebView(x: withWeakReferenceTo(this, (r) => r))` shape reports
    /// nothing.
    #[test]
    fn this_inside_late_field_initializer_arguments() {
        let report = report(
            "Object g(Object a, Object b) => a; \
             class C { \
               late final Object f = g(this, (int r) => r); \
             }",
        );
        assert!(
            !code_names(&report).contains(&"invalid_reference_to_this"),
            "{:?}",
            code_names(&report)
        );
    }

    /// A closure nested inside a `late` instance field's initializer keeps
    /// `this` bound — the accessibility latch survives the closure's own
    /// executable frame (upstream never re-gates `this` at function
    /// boundaries inside an accessible span).
    #[test]
    fn this_inside_closure_in_late_field_initializer() {
        let report = report(
            "class C { \
               late final Object f = () => this; \
             }",
        );
        assert!(
            !code_names(&report).contains(&"invalid_reference_to_this"),
            "{:?}",
            code_names(&report)
        );
    }

    /// The accessibility is scoped, not leaked: `this` inside a NON-`late`
    /// instance field's initializer still reports — the oracle's
    /// `bindsThis` is `false` there and the enclosing `thisType` is not
    /// consulted at field level.
    #[test]
    fn this_inside_plain_field_initializer_reports() {
        let report = report(
            "class C { \
               final Object f = () => this; \
             }",
        );
        assert!(
            code_names(&report).contains(&"invalid_reference_to_this"),
            "{:?}",
            code_names(&report)
        );
    }

    // —— #2283: promotion-refined chained reads ——————————————————————————

    /// The shared fixture, in same-file types (the no-import rig resolves
    /// no external names, so the oracle's `double`/`Object` pair maps to
    /// `Refined`/`Frozen`): the declared face's accessor answers `Frozen`,
    /// the promoted face's field narrows it to `Refined` (a subtype, so
    /// the override is sound).
    const PROMOTED_CHAIN: &str = "class Frozen {} \
       class Refined extends Frozen { Flag get isNaN => Flag(); } \
       class Flag {} \
       class Base { Frozen get value => Frozen(); } \
       class Sub extends Base { \
         Sub(this.value); \
         @override final Refined value; \
       } \
       void chain(Base a) { \
         if (a is Sub) {";

    /// The xml `general.dart` family: `a.value.isNaN` over an `a is Sub`
    /// promotion resolves `value` on the promoted face, whose field type
    /// declares `isNaN` — the oracle stays silent, and the settlement
    /// retracts the frozen `Frozen`-faced trio.
    #[test]
    fn promoted_chain_read_resolves_on_the_promoted_result_face() {
        let report = report(&format!(
            "{} \
             a.value.isNaN; \
             }}",
            PROMOTED_CHAIN
        ));
        let undefined: Vec<_> = code_names(&report)
            .into_iter()
            .filter(|name| name.starts_with("undefined_"))
            .collect();
        assert!(undefined.is_empty(), "{undefined:?}");
    }

    /// The getter-face variant (`value` stays a getter on the promoted
    /// subclass, returning the narrowed type): the settlement walks the
    /// promoted face's accessor the same way.
    #[test]
    fn promoted_chain_getter_override_resolves_on_the_promoted_face() {
        let report = report(
            "class Frozen {} \
             class Refined extends Frozen { Flag get isNaN => Flag(); } \
             class Flag {} \
             class Base { Frozen get value => Frozen(); } \
             class Sub extends Base { \
               @override Refined get value => Refined(); \
             } \
             void chain(Base a) { \
               if (a is Sub) { a.value.isNaN; } \
             }",
        );
        let undefined: Vec<_> = code_names(&report)
            .into_iter()
            .filter(|name| name.starts_with("undefined_"))
            .collect();
        assert!(undefined.is_empty(), "{undefined:?}");
    }

    /// The settlement re-binds, not silences: a member the promoted result
    /// face also lacks still reports — naming the promoted face's display
    /// (`Refined`), never the frozen `Frozen`.
    #[test]
    fn promoted_chain_miss_names_the_settled_face() {
        let report = report(&format!(
            "{} \
             a.value.missing; \
             }}",
            PROMOTED_CHAIN
        ));
        let rows: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_getter")
            .map(|d| d.message.to_string())
            .collect();
        assert_eq!(rows.len(), 1, "{:?}", code_names(&report));
        assert!(rows[0].contains("for the type 'Refined'"), "{rows:?}");
    }

    /// Without a promotion the frozen face stands: the chain miss names
    /// the declared accessor's `Frozen` exactly as before — the settlement
    /// is the promotion's consequence, not a blanket re-render.
    #[test]
    fn unpromoted_chain_miss_keeps_the_declared_face() {
        let report = report(
            "class Frozen {} \
             class Base { Frozen get value => Frozen(); } \
             void chain(Base a) { \
               a.value.missing; \
             }",
        );
        let rows: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_getter")
            .map(|d| d.message.to_string())
            .collect();
        assert_eq!(rows.len(), 1, "{:?}", code_names(&report));
        assert!(rows[0].contains("for the type 'Frozen'"), "{rows:?}");
    }
}
