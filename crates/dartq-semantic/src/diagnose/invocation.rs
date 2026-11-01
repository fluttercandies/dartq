use super::ffi::matching_close;
use super::scopes_bodies::SchemaSlot;
use super::*;
use dartq_source::byte_offset_to_utf16;

/// The #2931 pins (the constructor-use `member_refs` publication — the
/// `ctor_annotations` fallback reaching `check_target_annotations`).
/// Kept as a standalone `#[path]` submodule so the shared `tests` rig and
/// this file's own module stay untouched by the registration.
#[cfg(test)]
#[path = "ticket_2931_tests.rs"]
mod ticket_2931_tests;

/// The #3510 pins (raw generic alias-headed bounds — the written/judged
/// split of `check_call_type_arg_bounds`'s parameter rows and the
/// snapshot-expanded bound the verdict reads). Registered the same
/// `#[path]` way: the shared `tests` rig stays single-writer.
#[cfg(test)]
#[path = "ticket_3510_tests.rs"]
mod ticket_3510_tests;

/// The #3575 pins (the `type_argument_not_matching_bounds` over-fire
/// family: the promoted extension receiver, the facts-carrying message
/// inversion, the alias-decorated written spellings, and the lazy
/// parameter id collision). Registered the same `#[path]` way: the
/// shared `tests` rig stays single-writer.
#[cfg(test)]
#[path = "ticket_3575_tests.rs"]
mod ticket_3575_tests;

/// The #2938 pins (the generic-send `couldNotInfer` face — the real
/// constraint gather/choose/check pipeline over `foo<T>(args)` and the
/// context-driven `F<args> = source` solve).
#[cfg(test)]
#[path = "ticket_2938_tests.rs"]
mod ticket_2938_tests;

/// The #2500 pins (the downward closure-formal re-bind — an untyped
/// literal formal under a member send whose signature stays unsolved at
/// pass 1 gets the callee's substituted answer through
/// `infer_rebind_closure_formals`).
#[cfg(test)]
#[path = "ticket_2500_tests.rs"]
mod ticket_2500_tests;

/// The #2516 pins (the upward twin: element-bearing member-read args
/// feed the constraint gather through `solve_arg_member_ty` without
/// lending their type to the `check_arguments` verdict face).
#[cfg(test)]
#[path = "ticket_2516_tests.rs"]
mod ticket_2516_tests;

/// The #2498 pins (the A8-family handback consolidation's two landed
/// faces: the `Error.throwWithStackTrace`/`Future.error` SDK static
/// returns plus the object-pattern `NamedType` arity verdict —
/// `case Box<int, String>():` emitting `wrong_number_of_type_arguments`
/// through the `pending_pattern_type_args` latch).
#[cfg(test)]
#[path = "ticket_2498_tests.rs"]
mod ticket_2498_tests;

/// The #2463 pins (the census ⑨ small-family handback: A9 arity on a
/// pattern-promoted receiver, A10 extension selection, A11 `.map` name
/// shadowing, A12's three const shapes, A14's mixin/implements slice, A15's
/// cross-library super formal).
#[cfg(test)]
#[path = "ticket_2463_tests.rs"]
mod ticket_2463_tests;

/// The #2272 pins (the `?.`/`?..`/`?[` null-shorting RESULT-TYPE face:
/// the `?.` member result lifts to `nullable(memberType)`, the `?..`
/// cascade result keeps the receiver's own type, the `?[` result is the
/// nullable element type, and a chained continuation inherits the lift —
/// upstream's `handleNullShortingFinished` /
/// `recordNullShortedType` arm).
#[cfg(test)]
#[path = "ticket_2272_tests.rs"]
mod ticket_2272_tests;

/// Where one merged constraint came from — upstream's
/// `TypeConstraintOrigin` (`type_constraint.dart`), the error-message
/// column identity. Each gather round stamps one origin; equal origins
/// group, and `_formatError` prints each surviving group once.
#[derive(Clone)]
enum InferOrigin {
    /// `TypeConstraintFromArgument` — one argument constrained against
    /// its written formal.
    Argument {
        parameter: StringId,
        declared: Ty,
        argument: Ty,
    },
    /// `TypeConstraintFromReturnType` — the declared return matched
    /// the send's downward context.
    ReturnType { declared: Ty, context: Ty },
    /// `TypeConstraintFromFunctionContext` — a generic tear-off
    /// argument's own instantiation infer (`inferTearOff`).
    FunctionContext { function: Ty, context: Ty },
    /// `TypeConstraintFromExtendsClause` — the parameter's substituted
    /// bound, checked last in the per-parameter sequence.
    ExtendsClause {
        parameter: StringId,
        raw_bound: Ty,
        bound: Ty,
    },
}

impl InferOrigin {
    /// `formatError` (upstream `type_constraint.dart`) — the one-to-three
    /// part line (`prefix`, `middle`, optional `end`) `_formatConstraints`
    /// pads into columns.
    fn format_error(&self, engine: &Engine<'_>) -> Vec<String> {
        match self {
            Self::Argument {
                parameter,
                declared,
                argument,
            } => vec![
                format!("Parameter '{}'", engine.strings.resolve(*parameter)),
                format!("declared as     '{}'", engine.ty_display(*declared)),
                format!("but argument is '{}'.", engine.ty_display(*argument)),
            ],
            Self::ReturnType { declared, context } => vec![
                "Return type".to_string(),
                format!("declared as '{}'", engine.ty_display(*declared)),
                format!("used where  '{}' is required.", engine.ty_display(*context)),
            ],
            Self::FunctionContext { function, context } => vec![
                "Function type".to_string(),
                format!("declared as '{}'", engine.ty_display(*function)),
                format!("used where  '{}' is required.", engine.ty_display(*context)),
            ],
            Self::ExtendsClause {
                parameter,
                raw_bound,
                bound,
            } => vec![
                format!("Type parameter '{}'", engine.strings.resolve(*parameter)),
                format!(
                    "is declared to extend '{}' producing '{}'.",
                    engine.ty_display(*raw_bound),
                    engine.ty_display(*bound)
                ),
            ],
        }
    }
}

/// One round's merged constraint on one parameter — upstream's
/// `MergedTypeConstraint` (`lower <: T <: upper`; `Ty::Unknown` is the
/// `_` unconstrained side) with its origin's slot in the round table.
#[derive(Clone)]
struct InferConstraint {
    lower: Ty,
    upper: Ty,
    origin: usize,
}

/// The chain-root static type behind a receiver classification: the
/// declared type of a `Typed` receiver's head (`data` of `data.address`,
/// `struct` of `struct.y.address`'s chain), [`Ty::UNKNOWN`] for every
/// other shape.
fn recv_head_ty(recv: &Receiver) -> Ty {
    match recv {
        Receiver::Typed { ty, .. } => *ty,
        _ => Ty::UNKNOWN,
    }
}

/// Which associated parameter one parked `super.name` sentinel reads: the
/// forward's own index among the constructor's super formals, or the named
/// entry's spelling.
#[derive(Clone, Copy)]
enum SuperQuery {
    Positional(usize),
    Named(StringId),
}

/// One parameter row of a type-argument bounds verdict (#3510): the two
/// faces one upstream `DartType` carries at once — the structural type the
/// subtype test reads and the alias decoration `getDisplayString` prints
/// (display_string_builder.dart:379-382). `written` is the bound as the
/// capture spelled it, the MESSAGE face (`ty_display` fills a raw head to
/// its instantiate-to-bounds form, so `T extends A` renders `'A<dynamic>'`
/// — the oracle's decorated spelling); `judged` is the bound the verdict
/// compares against, the snapshot-expanded shape
/// ([`crate::bounds::Bounds::expanded_params`] — `C<dynamic, dynamic>`
/// for that same `A`). An expanded shape never reaches a message, a
/// written alias head never reaches the verdict. The two coincide for
/// every bound that is already a type (an executable window, an element
/// model `type_param_bounds` row): [`From`] builds that row.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TypeArgBound {
    pub(crate) name: StringId,
    pub(crate) written: Option<Ty>,
    pub(crate) judged: Option<Ty>,
}

impl From<(StringId, Option<Ty>)> for TypeArgBound {
    fn from((name, bound): (StringId, Option<Ty>)) -> Self {
        Self {
            name,
            written: bound,
            judged: bound,
        }
    }
}

impl<'a> Engine<'a> {
    /// Whether `name` resolves to a declaration of a `dart:` library — the
    /// unindexed SDK class head the member merge classifies as a
    /// [`Receiver::Container`] through its names-only spelling (no
    /// container table, so no constructor set: `Future.value`,
    /// `Uri.http` ride this face).
    fn sdk_type_head(&self, name: StringId) -> bool {
        matches!(
            self.namespaces.lookup(self.graph, self.lib, name),
            Lookup::Found(NamespaceEntry::Decl(owner))
                if self
                    .graph
                    .library_of_file(owner)
                    .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"))
        )
    }

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
        member_ident: bool,
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
        // The `?[` receiver's PRE-LIFT element face: the index fold bakes
        // the short's `?` into the result row's stored type (`a?[0]` keeps
        // `E?`), but every null-aware face that follows reads the element's
        // own written type — the `?.` verdict's receiver check (`a?[0]?.b`
        // reports `invalid_null_aware_operator` on `E`) and the chain
        // continuation's lift gate (`a?[0].b` types `B?` only while `E`
        // reads non-null — a `List<E?>` keeps `.b`'s unchecked verdict).
        // The cascade `?..` verdict deliberately reads the lifted row face
        // (the `?..`'s target IS `E?`), so this answer stays scoped to the
        // access operators' continuation face.
        let mut shorted_elem_ty = None;
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
                // A folded `?[` result row reads `E?` through the same
                // receiver face; the continuation's pre-lift element face
                // re-derives through the indexed receiver's own row.
                if !self.in_cascade {
                    shorted_elem_ty = self.shorted_index_elem_ty(popped.range.offset);
                }
                self.stage_receiver_of_type_never(popped.range, receiver_ty);
                recv_span = popped.range;
                was_local = self.local_hits.contains_key(&recv_span.offset);
                // The parenthesized receiver (`_verifyUnassignedSimpleIdentifier`'s
                // `target.unParenthesized`, dead_code_verifier.dart:486-490):
                // `(i)?.x` folds `i` into a group row — the `local_hits` entry
                // stays at `i`'s own offset inside the group's span, so the
                // bare-receiver lookup misses. Unwrap the group the same way
                // the official helper does: the chain roots on the inner local
                // read, and the join's `unassigned`/`promotions` facts key on
                // that inner span, never the parens.
                if !was_local
                    && !popped.qualified
                    && matches!(popped.shape, RowShape::Bare(BareInfo::Plain { .. }))
                    && self.strings.resolve(popped.name).is_empty()
                {
                    if let Some((&inner_offset, _)) = self
                        .local_hits
                        .iter()
                        .find(|&(&off, _)| recv_span.offset < off && off < recv_span.end())
                    {
                        // The inner read's own span: the identifier token at
                        // the hit offset (`local_hits` keys the read's
                        // offset), spelled in UTF-16 like every row range.
                        if let Some(token) = self
                            .tokens
                            .as_slice()
                            .get(self.tokens.as_slice().partition_point(|t| {
                                t.utf16_offset < inner_offset
                            }))
                            .filter(|t| t.utf16_offset == inner_offset)
                        {
                            let raw = &self.source[token.byte_range()];
                            let length =
                                raw.chars().map(char::len_utf16).sum::<usize>() as u32;
                            recv_span = SourceRange {
                                offset: inner_offset,
                                length,
                            };
                            was_local = true;
                        }
                    }
                }
                // The dead-chain candidate: a null-aware operator over a
                // local's nullable read (`c1?.g()`, `c1?..g()`). A merge
                // INSIDE a latched chain's extent (`c1?.a.b`'s `.b`) is the
                // upstream parent climb, not a new chain — the row stays
                // anchored at the first operator and extends to the outer
                // access's end (dead_code_verifier.dart:560-566).
                // The chain face ends exactly where the next selector
                // begins, so the boundary case (`<=`) is the chain's own
                // continuation — a selector further right starts a
                // different access and must not touch the row. A
                // continuation (plain or aware, latched) climbs the parent
                // chain unconditionally (the official walk answers on the
                // chain, not the selector's own receiver); a NEW chain
                // still needs the unassigned-nullable local root.
                let inside_chain = self
                    .dead_null_chains
                    .iter()
                    .any(|(op, end, ..)| *op <= op_range.offset && op_range.offset <= *end);
                // The dead-chain root's receiver nullability reads the
                // written face AND the `FutureOr` argument's nullability
                // (`FutureOr<int?>` is potentially nullable upstream —
                // `?[i]`/`?..` reads through it — while `FutureOr<int>`
                // stays non-nullable; `isNull`/`isPotentiallyNullable`,
                // type_system.dart's `FutureOr` arm).
                let rooted = is_null_aware
                    && was_local
                    && (Self::is_nullable_ty(receiver_ty)
                        || self.future_or_potentially_nullable(receiver_ty));
                if inside_chain || rooted {
                    // The root kind fixes the verifier face the chain answers
                    // (dead_code_verifier.dart's call-site split): a `?..`
                    // section head answers `verifyCascadeExpression`, whose
                    // body-class gate holds only while the section stays one
                    // bare selector; a plain `?.` member answers
                    // `verifyNullAwareAccess`'s parent climb.
                    let root_kind = if is_section {
                        crate::diagnose::types::DeadChainKind::CascadeMember
                    } else {
                        crate::diagnose::types::DeadChainKind::Member
                    };
                    // The boundary test needs the chain's own end BEFORE any
                    // growth: a cascade-kind chain dies when a plain `.`
                    // continuation selector lands at its end (the section
                    // body chained — `?..foo().length`), while a section head
                    // (`..x27`) grows the extent to cover the later sections
                    // (`verifyCascadeExpression` reports operator..cascade
                    // end) and retires the root section's own write face —
                    // the write then targets a later section, which the
                    // verifier never sees.
                    let containing = self.dead_null_chains.iter().position(|(op, end, ..)| {
                        *op <= op_range.offset && op_range.offset <= *end
                    });
                    match containing {
                        Some(index) => {
                            let (_, end, _, kind, _) = self.dead_null_chains[index];
                            let boundary = op_range.offset >= end;
                            match kind {
                                crate::diagnose::types::DeadChainKind::Member
                                | crate::diagnose::types::DeadChainKind::Index => {
                                    // A cascade section (`..x`/`?..x`) after
                                    // a `?.`/`?[` chain is a NEW
                                    // CascadeExpression, not an access parent
                                    // — the climb's
                                    // `parent is MethodInvocation |
                                    // PropertyAccess | IndexExpression` walk
                                    // stops there, so `x?.y..z` reports `?.y`
                                    // only (the cascade's own `?..` arm
                                    // checks the cascade TARGET, which is the
                                    // `x?.y` access — never a
                                    // SimpleIdentifier). Only plain `.`/`?.`
                                    // continuations grow the climb faces.
                                    if !is_section {
                                        self.dead_null_chains[index].1 =
                                            self.dead_null_chains[index].1.max(chain_end);
                                    }
                                }
                                crate::diagnose::types::DeadChainKind::CascadeMember
                                | crate::diagnose::types::DeadChainKind::CascadeIndex => {
                                    if is_section {
                                        self.dead_null_chains[index].1 =
                                            self.dead_null_chains[index].1.max(chain_end);
                                        self.dead_null_chains[index].4 = true;
                                    } else if boundary {
                                        self.dead_null_chains.remove(index);
                                    }
                                }
                            }
                        }
                        None => {
                            // The member face needs a real selector: the
                            // parser folds `?.{ ... }`'s recovery braces
                            // into a literal "member", a shape the official
                            // parser never produces (its recovery drops the
                            // body) — no chain roots there. Cascade roots
                            // keep their latch: the bare-body gate and the
                            // write cancellation key on it.
                            if rooted && (is_section || member_ident) {
                                self.dead_null_chains.push((
                                    op_range.offset,
                                    chain_end,
                                    recv_span,
                                    root_kind,
                                    false,
                                ));
                            }
                        }
                    }
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
                // The keyword-receiver link (#2769): an explicit `this`
                // receiver's member read latches exactly like the
                // implicit-`this` bare read above — the official promotion
                // lookup keys the property slot on the same `this` identity
                // either way (`_handleProperty` reads the target SSA node,
                // and `ThisPropertyTarget._getSsaNode` answers the same node
                // an `ExpressionPropertyTarget` naming `this` does, upstream
                // Dart SDK source,
                // pkg/_fe_analyzer_shared/lib/src/flow_analysis/
                // flow_analysis.dart), so the guarded chain links through
                // the #2283 latch below (`this._p.isEven` under
                // `if (this._p != null)` — the flow marker stands at `_p`'s
                // token for both spellings). The cascade-section and
                // invoked-callee exclusions mirror the bare-read latch
                // (`this..a`'s selector re-classifies to the latched
                // cascade target; recovery rows carrying an invoke intent
                // keep the callee-value face). `super` takes no link: a
                // `super.x` read has no promotion identity — the flow
                // stages no marker for it.
                if matches!(popped_shape, RowShape::Receiver(Receiver::This { .. }))
                    && !(self.in_cascade && is_section)
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
                // The index-result candidate: a member folded over a folded
                // `recv[i]` result row whose receiver is itself re-derivable
                // (a bare read or a chain link) owes the same pass-2
                // re-derivation — the fold typed the element from the
                // receiver's unpromoted face. The latch keys the result
                // row's own span, the index-result leg of
                // `recv_expression_face` answers it.
                if matches!(popped_shape, RowShape::Value)
                    && !(self.in_cascade && is_section)
                    && self.index_result_recvs.contains_key(&popped.range.offset)
                {
                    let member_offset = member.range.offset;
                    if let Err(index) = self
                        .promoted_recv_reads
                        .binary_search_by(|(row, _)| row.cmp(&member_offset))
                    {
                        self.promoted_recv_reads
                            .insert(index, (member_offset, popped.range));
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
                // A constructor-reference send whose instantiated result the
                // name-level read leaves `Unknown` (`G.value(1)` over a
                // GENERIC `G` — the declared return needs the inference that
                // read cannot run, [`Engine::callee_return_ty`]'s
                // generic-ctor arm) still denotes the CREATED instance,
                // never the type literal: upstream types the
                // `InstanceCreationExpression` and resolves the next member
                // on the created class (probe 3.13.7: `G<T> { G.value(T t);
                // void n(); }` — `G.value(1)..n()` is silent, `..gone()`
                // reports "The method 'gone' isn't defined for the type
                // 'G'."). Without it the merged row falls to the unknown
                // dispatch, whose pass-2 re-read of the target span answers
                // the bare head's `Type` face and reports every cascade
                // section against `Type` (the #3394 T2 residual: the corpus
                // `Future.value(C())..expectStaticType<…>()` cluster). Gated
                // on the selector naming a DECLARED constructor, so a
                // static-member send whose return is unknown keeps the
                // unknown dispatch; an unindexed SDK class head carries no
                // constructor table, and its dot-sends are the SDK's own
                // factories (`Future.value`, `Uri.http` — the oracle types
                // the created instance there), so the head spelling stands
                // as the instance.
                let ctor_send_instance = if popped.intent == Intent::Invoke {
                    match &popped_shape {
                        RowShape::Member {
                            recv: Receiver::Container { name, .. },
                            ..
                        } => {
                            let indexed_ctor = resolve_container(
                                self.graph,
                                self.namespaces,
                                self.index,
                                self.lib,
                                self.file,
                                *name,
                            )
                            .is_some_and(|(decl, _)| {
                                self.ctor_face_exists(decl, *name, popped.name)
                            });
                            (indexed_ctor || self.sdk_type_head(*name)).then_some(*name)
                        }
                        _ => None,
                    }
                } else {
                    None
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
                    // A bare head that spells a GROUNDED type alias
                    // (`Alias.parse(…)` under `typedef Alias =
                    // MediaType;`) denotes the aliased class itself: the
                    // dot-send is the class's constructor/static face —
                    // `Alias.parse` names MediaType's factory constructor,
                    // a creation, never an instance-member send on the
                    // expansion type. The receiver classifies as the
                    // expansion container (alias chains fold first), so
                    // the send takes the direct-spelling fold: the
                    // constructor table, static members, and the
                    // class-named miss faces. An ungrounded alias keeps
                    // its own faces (`report_alias_selector_face` names
                    // 'Type'); a local-shadowed spelling stays the value
                    // receiver (`was_local`).
                    RowShape::Bare(BareInfo::Plain { ty })
                        if !was_local
                            && popped.intent != Intent::Invoke
                            && self.alias_head_grounded(popped.name) == Some(true)
                            && let Ty::Named { name: head, .. } = self.fold_alias_ty(*ty)
                            && resolve_container(
                                self.graph,
                                self.namespaces,
                                self.index,
                                self.lib,
                                self.file,
                                head,
                            )
                            .is_some() =>
                    {
                        Receiver::Container {
                            name: head,
                            kind: self.container_kind_of(head).unwrap_or(ContainerKind::Class),
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
                            // reports. A head whose read already carries a
                            // static type is a value (`globalContext`, an SDK
                            // top-level getter), never a container — the
                            // typed fall-through below owns it.
                            Receiver::Unknown
                                if !was_local
                                    && receiver_ty == Ty::UNKNOWN
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
                                // A function-typed receiver has none either:
                                // its extension face (`extension on
                                // PostMessage`, a function typedef) rides the
                                // same table.
                                (Receiver::Unknown, Ty::Record { .. } | Ty::Function { .. }) => {
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
                        && matches!(resolved.kind, ContainerKind::Class) =>
                    {
                        // The typed receiver carries no owner, so the bare
                        // spelling must name the SAME declaration (or
                        // nothing) in this scope; a same-named local class
                        // (`Link` of the file-system API beside `io.Link`)
                        // keeps the dot-joined head, which the member walk
                        // resolves through the prefix's table.
                        let head = if resolve_container(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            popped.name,
                        )
                        .is_none_or(|(local, _)| std::ptr::eq(local, resolved))
                        {
                            popped.name
                        } else {
                            let joined = format!(
                                "{}.{}",
                                self.strings.resolve(*prefix),
                                self.strings.resolve(popped.name)
                            );
                            self.intern(&joined)
                        };
                        Receiver::Typed {
                            type_name: head,
                            ty: Ty::named(head),
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
                        // A prefixed top-level value (`diag.deadCode.m`,
                        // `p.f().m`): the read's static type (the declared
                        // variable/getter type, or the callee's return)
                        // types the next receiver like the chained face.
                        None => match receiver_ty {
                            Ty::Named { name, .. } => Receiver::Typed {
                                type_name: name,
                                ty: receiver_ty,
                            },
                            _ => Receiver::Unknown,
                        },
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
                        _ => match ctor_send_instance {
                            Some(name) => Receiver::Typed {
                                type_name: name,
                                ty: Ty::named(name),
                            },
                            None => Receiver::Unknown,
                        },
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
            let receiver_ty = if let Some(elem) = shorted_elem_ty {
                // The `?[` continuation reads the pre-lift element face:
                // `a?[0]?.b` decides on `E` (the `?[` already shorted the
                // receiver), never the `E?` the result row stores.
                elem
            } else if merged_receiver_ty != Ty::UNKNOWN {
                merged_receiver_ty
            } else {
                match recv {
                    Receiver::This { container, kind } => self.this_static_ty(container, kind),
                    Receiver::Super { .. } => Ty::UNKNOWN,
                    Receiver::Created { name, kind, .. } if kind != ContainerKind::Extension => {
                        Ty::named(name)
                    }
                    Receiver::Typed { ty, .. } => ty,
                    _ => Ty::UNKNOWN,
                }
            };
            // The operator is the anchor (`_checkForUnnecessaryNullAware`'s
            // `.at(operator)`), not the member row the merge folded into.
            // The section selector `?..` spells the cascade face; a `?.`
            // INSIDE a cascade (a section's own null-aware access,
            // `x?..a?.b`) spells the access face — the lexemes follow the
            // written operator, not the enclosing expression.
            let kind = if is_section {
                NullAwareKind::Cascade
            } else {
                NullAwareKind::Access
            };
            // The static-qualifier face (`C?.x`, `p.C?.x`, `C?.x = v`,
            // `C?.f()`): a type spelling in the receiver slot carries no
            // InstanceReceiver, so upstream's per-visit callers report the
            // operator unconditionally — no type check at all
            // (error_verifier.dart's visitReceiverMethodInvocation /
            // visitReceiverPropertyExtraction /
            // visitReceiverPropertyAssignmentTarget `StaticQualifier` arms,
            // :2048-2053/:2083/:2119: the receiver IS the type, the `?.`
            // can never be meaningful). `Receiver::Container` is the
            // bare-head static face (`C`, `E`, a `dart:`-synthetic head);
            // `PrefixedContainer` is the `p.C` join. A `Receiver::Created`
            // (`C()?.x`) is an instance value — its declared type answers
            // below like any expression receiver.
            if matches!(
                recv,
                Receiver::Container { .. } | Receiver::PrefixedContainer { .. }
            ) {
                let (op, replacement) = kind.lexemes();
                self.emit3(
                    SemanticCode::InvalidNullAwareOperator,
                    op_range,
                    op,
                    replacement,
                    "",
                );
            // The bound-aware strictly face: an in-scope parameter
            // receiver decides through its written bound (`T extends
            // AstNodeImpl?` keeps `?.` legal, `T extends C` reports).
            } else if self.strictly_non_nullable(receiver_ty) {
                // The short-circuit variant
                // (`_checkForUnnecessaryNullAware`'s
                // `previousShortCircuitingOperator` walk): a `?.`/`?..`/`?[`
                // whose receiver is non-null only because an EARLIER
                // null-aware operator in the same chain already shorted it
                // reports the `after_short_circuit` message and carries the
                // causing operator as a context row — `s?.length?.isEven`'s
                // second `?.`. The chain ledger's windows key the search:
                // this operator's offset inside a prior chain's extent means
                // that chain's `?.`/`?[` did the shorting (error_verifier
                // .dart:7721-7740). The receiver's own declared-nullability
                // handles the `strictly` verdict either way — the variant
                // swaps the message text and adds the context, never the
                // anchor or severity.
                // The causing operator's own spelling drives the context
                // lexeme and span (`?.`/2 for a member root, `?..`/3 for a
                // cascade-section root, `?`/1 for a `?[` question token —
                // `previousShortCircuitingOperator`'s IndexExpression arm
                // answers the question TOKEN, error_verifier.dart:7314-7318).
                let prior = self
                    .dead_null_chains
                    .iter()
                    .find(|(op, end, ..)| *op <= op_range.offset && op_range.offset <= *end)
                    .map(|(op, _, _, prior_kind, _)| match prior_kind {
                        // The member face answers the written `?.` token;
                        // the INDEX face answers the index expression's
                        // `question` token alone (`?`, 1 column) —
                        // `previousShortCircuitingOperator`'s
                        // IndexExpression arm (:7314-7318) never returns the
                        // `[` its anchor pair carries.
                        crate::diagnose::types::DeadChainKind::Member => (*op, "?.", 2u32),
                        crate::diagnose::types::DeadChainKind::Index => (*op, "?", 1u32),
                        crate::diagnose::types::DeadChainKind::CascadeMember
                        | crate::diagnose::types::DeadChainKind::CascadeIndex => {
                            (*op, "?..", 3u32)
                        }
                    })
                    // A `?[` receiver's own question is the prior
                    // short-circuiter when no latched chain covers this
                    // operator — `l?[0]?.length`'s `?.` reads the pre-lift
                    // element, so the causing operator is the `?[` whose
                    // dead-chain extent already ended at `]`. The context
                    // anchors the question token itself, so whitespace
                    // ahead of the `?` stays outside it.
                    .or_else(|| {
                        if shorted_elem_ty.is_some() && !self.in_cascade {
                            receiver_offset.and_then(|offset| {
                                self.index_result_recvs
                                    .get(&offset)
                                    .and_then(|(indexed, null_aware)| {
                                        null_aware.then(|| {
                                            (self.skip_whitespace(indexed.end()), "?", 1u32)
                                        })
                                    })
                            })
                        } else {
                            None
                        }
                    });
                let (op, replacement) = kind.lexemes();
                if let Some((prior_offset, prior_lexeme, prior_len)) = prior {
                    let message = format!(
                        "The receiver can't be 'null' because of short-circuiting, so the null-aware operator '{op}' can't be used."
                    );
                    let context = vec![ContextMessage {
                        message: SharedStr::from(format!(
                            "The operator '{prior_lexeme}' is causing the short circuiting."
                        )),
                        file: self.file,
                        range: SourceRange {
                            offset: prior_offset,
                            length: prior_len,
                        },
                    }];
                    let code = SemanticCode::InvalidNullAwareOperator;
                    self.report.diagnostics.push(
                        Diagnostic::new(
                            code.severity(),
                            code.code_info(),
                            message,
                            self.file,
                            op_range,
                            // The verbatim replace correction
                            // (diagnostic.g.dart's
                            // invalidNullAwareOperatorAfterShortCircuit
                            // correctionMessage :9473-9478).
                            Some(format!(
                                "Try replacing the operator '{op}' with '{replacement}'."
                            )),
                        )
                        .with_context(context),
                    );
                } else {
                    self.emit3(
                        SemanticCode::InvalidNullAwareOperator,
                        op_range,
                        op,
                        replacement,
                        "",
                    );
                }
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
            self.boundary.args.borrow_mut().named(
                self.intern("List"),
                false,
                &[Ty::named(container)],
            )
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
        // is a no-op. The token-end index rides along (last insert wins,
        // matching the reverse scan it replaced — see `chain_node_read`).
        if let Err(index) = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&member.range.offset))
        {
            let end = member.range.offset + self.member_name_units(member.name);
            self.member_read_ends.insert(end, index as u32);
            self.member_reads.insert(
                index,
                MemberRead {
                    offset: member.range.offset,
                    name: member.name,
                    recv,
                    recv_span,
                },
            );
            // The invalid-receiver witness must be taken HERE, while the
            // enclosing scopes still live: a typed head that names a
            // method type parameter (`DB extends …`, an extension's `on
            // DB` receiver) is a valid static type now but answers
            // "invalid" to the finalization-time re-ask, whose scope walk
            // no longer sees the parameter. Only a head that is invalid
            // NOW is recorded.
            if let Receiver::Typed { type_name, .. } = recv
                && self.is_invalid_ty(Ty::named(type_name))
            {
                self.invalid_typed_member_rows.insert(member.range.offset);
            }
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
            // A folded `?[` result receiver carries the same shorted-chain
            // fact (`a?[0].b`'s `.b` inherits the lift — the whole node
            // types `B?`). The row's stored `E?` is the SHORT's face, so the
            // gate reads the pre-lift element type instead: `List<E>`
            // latches `.b`, `List<E?>` keeps the continuation's own
            // unchecked verdict.
            let (shorted, receiver_non_null) = if self
                .index_result_recvs
                .get(&receiver_offset)
                .is_some_and(|(_, null_aware)| *null_aware)
            {
                (
                    true,
                    shorted_elem_ty.is_some_and(|elem| {
                        !Self::is_nullable_ty(elem)
                            && !self.future_or_potentially_nullable(elem)
                    }),
                )
            } else {
                (
                    self.null_aware_members.contains(&receiver_offset),
                    !(Self::is_nullable_ty(merged_receiver_ty)
                        || self.future_or_potentially_nullable(merged_receiver_ty)),
                )
            };
            if !self.in_cascade && shorted && (receiver_non_null || record_short) {
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
        // The `dart:ffi` verifier's address-read capture ([`super::ffi`]):
        // the `.address` getters whose receiver wears an FFI value shape —
        // a `Pointer`/`Array`/native-primitive receiver, a typed-data
        // receiver, or a compound-subtype chain root. Other spellings
        // (`userObject.address`) are the user's own getters, never
        // captured.
        // An unresolved receiver face keeps the capture too: the
        // extension-resolution the oracle decides from the static type
        // cannot be replayed here, and the receiver face decides on the
        // same facts at finalize (an unresolved receiver reports).
        if member.name == self.intern("address")
            && (self.ffi_value_shape(merged_receiver_ty)
                || self.ffi_value_shape(recv_head_ty(&recv))
                || merged_receiver_ty == Ty::UNKNOWN
                || recv_head_ty(&recv) == Ty::UNKNOWN)
        {
            let recv_is_index = {
                let end = self.byte_of(recv_span.end());
                self.source.as_bytes().get(end.saturating_sub(1)) == Some(&b']')
            };
            self.ffi_address_reads.push(FfiAddressRead {
                selector: member.range,
                recv_span,
                recv_ty: merged_receiver_ty,
                head_ty: recv_head_ty(&recv),
                recv_is_index,
            });
        }
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

    /// The `?[` result's PRE-LIFT element face at `offset` (the `]` row's
    /// own span): `handle_indexed_expression` stores the shorted face on
    /// the result row (`nullable_form(element)`), so every consumer that
    /// needs the continuation's semantics — the `?.` receiver check and the
    /// chain-lift gate — re-derives the element through the folded
    /// receiver row's own static type. `index_result_recvs` carries the
    /// `?`-flag; a plain `a[i]` answers `None` exactly like a non-index
    /// receiver, and a receiver row the fold no longer holds keeps the
    /// under-report posture.
    fn shorted_index_elem_ty(&self, offset: u32) -> Option<Ty> {
        let &(indexed, null_aware) = self.index_result_recvs.get(&offset)?;
        if !null_aware {
            return None;
        }
        let row = self
            .rows
            .iter()
            .rev()
            .find(|row| row.range.offset == indexed.offset)?;
        let ty = Engine::receiver_row_ty(row);
        let ty = if ty == Ty::UNKNOWN {
            self.receiver_static_ty(row)
        } else {
            ty
        };
        Some(self.index_result_ty(ty))
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
    ///
    /// A named mixin application (`class X = S with M;`) declares no
    /// constructor of its own — the analyzer SYNTHESIZES one per generative
    /// constructor of `S` (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart:223`'s
    /// `getMember` superclass climb), so the face reads the application's
    /// [`Self::effective_ctor_container`] terminal instead. Its own
    /// `has_unnamed_constructor` flag is the source-declared row — always
    /// the implicit default — and answers nothing.
    ///
    /// The forwarding check re-resolves `container` in the diagnosing
    /// scope and requires the hit to BE `decl` (`std::ptr::eq`): a
    /// prefixed or shadowed spelling's container answers its own face
    /// alone (the pre-existing under-report posture — the effective face
    /// cannot be re-derived without the declaring unit).
    pub(crate) fn ctor_face_exists(
        &self,
        decl: &Container,
        container: StringId,
        selector: StringId,
    ) -> bool {
        let unnamed = selector == container || selector == self.intern("new");
        let own = |face: &Container| {
            face.constructors.contains(&selector)
                || (unnamed && face.has_unnamed_constructor)
        };
        let effective = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .and_then(|(resolved, owner)| {
            std::ptr::eq(resolved, decl)
                .then(|| self.effective_ctor_container(resolved, owner, container))
                .flatten()
        });
        let Some((face, face_owner, face_name)) = effective else {
            return own(decl);
        };
        if std::ptr::eq(face, decl) {
            return own(decl);
        }
        // The application's forwarded set is the superclass's GENERATIVE
        // constructors — factories never forward (the spec's mixin-
        // application clause, `mixin_application_constructor_forwarding`),
        // so a selector the terminal face declares only as a factory is no
        // constructor of the application.
        if unnamed && face.has_unnamed_constructor {
            return !self.ctor_is_factory(face, face_owner, face_name);
        }
        face.constructors.contains(&selector)
            && !self.ctor_is_factory(face, face_owner, selector)
    }

    /// The container whose constructor tables answer `name`'s constructor
    /// face — `(decl, owner, name)` itself, unless `decl` is a named mixin
    /// application, whose constructor set is forwarded from the
    /// superclass chain (each mixin-application hop climbs to its own
    /// superclass, so `class Y = X with N;` over the mixin application `X`
    /// lands on the terminal face `X`'s forwards read). Returns the
    /// terminal container with its declaring unit and lookup name — the
    /// `ctor_formals` unnamed face keys on THAT spelling, not `name`'s.
    ///
    /// The `is_mixin_application` fact rides the declaring unit's
    /// `decl_kinds` rows (introductory spellings only — an `augment` head
    /// never sets it). A container without both a superclass and a `with`
    /// list cannot be the sugar form, which short-circuits the scan on
    /// the ordinary path. `None` when a superclass spelling resolves
    /// nowhere or the chain cycles — the under-report posture every
    /// creation face shares (the declaration face reports the miss
    /// elsewhere).
    pub(crate) fn effective_ctor_container(
        &self,
        decl: &'a Container,
        owner: FileId,
        name: StringId,
    ) -> Option<(&'a Container, FileId, StringId)> {
        let is_mixin_app = |owner: FileId, name: StringId| {
            self.index.get(owner).is_some_and(|elements| {
                elements
                    .decl_kinds()
                    .iter()
                    .any(|target| target.is_mixin_application && target.name == name)
            })
        };
        if decl.superclass.is_none()
            || decl.with_clause.is_empty()
            || !is_mixin_app(owner, name)
        {
            return Some((decl, owner, name));
        }
        let mut seen: FxHashSet<StringId> = FxHashSet::from_iter([name]);
        let (mut cur, mut cur_owner, mut cur_name) = (decl, owner, name);
        let mut scope_lib = self.lib;
        loop {
            // A mixin application always writes a head type (`= S with`),
            // so `superclass` is present on every hop the flag marks.
            let super_name = cur.superclass?;
            let super_lib = owner_library(self.graph, self.lib, cur_owner, cur_name)
                .unwrap_or(scope_lib);
            let (super_decl, super_owner) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                super_lib,
                cur_owner,
                super_name,
            )?;
            if !seen.insert(super_name) {
                return None; // malformed cyclic chain: unresolved posture
            }
            let chained = super_decl.superclass.is_some()
                && !super_decl.with_clause.is_empty()
                && is_mixin_app(super_owner, super_name);
            if !chained {
                return Some((super_decl, super_owner, super_name));
            }
            scope_lib = super_lib;
            (cur, cur_owner, cur_name) = (super_decl, super_owner, super_name);
        }
    }

    /// Whether `key`'s constructor on `face` is a FACTORY — the selector a
    /// mixin application never forwards. `key` is the `ctor_formals`
    /// lookup spelling (the selector for a named face, the container's own
    /// name for the unnamed face). The declaring unit's `decl_kinds` row
    /// is authoritative; `face`'s `factory_formals` table backstops it for
    /// a declaration whose target row never sealed.
    pub(crate) fn ctor_is_factory(
        &self,
        face: &Container,
        owner: FileId,
        key: StringId,
    ) -> bool {
        face.factory_formals.contains_key(&key)
            || self.index.get(owner).is_some_and(|elements| {
                elements
                    .decl_kinds()
                    .iter()
                    .any(|target| target.is_factory && target.name == key)
            })
    }

    /// The unnamed-constructor existence face on `(decl, owner, name)`
    /// through [`Self::effective_ctor_container`]: the terminal
    /// container's `has_unnamed_constructor`, minus the factory-only
    /// unnamed shape a mixin application never forwards (a plain class's
    /// own factory unnamed face answers `C()` directly — the exclusion is
    /// the forwarding hop's alone). A broken forwarding chain answers the
    /// container's own flag (the under-report posture).
    fn unnamed_ctor_exists(&self, decl: &'a Container, owner: FileId, name: StringId) -> bool {
        let Some((face, face_owner, face_name)) =
            self.effective_ctor_container(decl, owner, name)
        else {
            return decl.has_unnamed_constructor;
        };
        face.has_unnamed_constructor
            && (std::ptr::eq(face, decl)
                || !self.ctor_is_factory(face, face_owner, face_name))
    }

    /// The formal table `selector`'s GENERATIVE constructor carries on
    /// `class`'s [`Self::effective_ctor_container`] face — the mixin-
    /// application forwarding lane [`Self::ctor_schema_formals`]'s plain
    /// read cannot reach (the application's own table is empty). `None`
    /// for an ordinary class, a broken forwarding chain, or a selector
    /// the terminal face does not declare — the caller's own
    /// `factory_formals` lane or under-report posture continues. The
    /// `new`/class-name spellings key on the terminal container's own
    /// name (the collector's `ctor_formals` unnamed-face convention).
    fn forwarded_ctor_formals(&self, class: StringId, selector: StringId) -> Option<Formals> {
        let (container, owner) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            class,
        )?;
        let (face, face_owner, face_name) =
            self.effective_ctor_container(container, owner, class)?;
        if std::ptr::eq(face, container) {
            return None; // no forwarding: the plain table owns this read
        }
        let key = if selector == class || selector == self.intern("new") {
            face_name
        } else {
            selector
        };
        if self.ctor_is_factory(face, face_owner, key) {
            return None;
        }
        let mut formals = (**face.ctor_formals.get(&key)?).clone();
        self.project_formals(face_owner, &mut formals);
        // An untyped `this.f` formal reads its field's declared type off
        // the declaring unit — same-file only, the field-formal patch's
        // ordinary reach (a same-named local class must never patch a
        // foreign face's formals).
        if face_owner == self.file {
            self.patch_field_formals(face_name, &mut formals);
        }
        Some(formals)
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
            } else if self.index_result_recvs.contains_key(&span.offset) {
                self.recv_expression_ty(span)
            } else {
                self.rebound_read_face(span)
                    .or_else(|| self.read_type_at(span))
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
            let declaring = self.member_faces.last_origin().unwrap_or(name);
            let result = match entry.kind {
                MemberKind::Field => self.fold_alias_ty(entry.var_ty),
                _ => self.fold_alias_ty(entry.return_ty),
            };
            // The settled result carries the receiver's instantiation (the
            // official `_executeGetter` face): `List<Tk>.first` reads `Tk`,
            // never the declaration's raw `E` — an inherited member reads
            // the receiver viewed as the declaring container. An
            // uninstantiable face keeps no derived entry (under-report).
            let recv = Receiver::Typed {
                type_name: name,
                ty: face,
            };
            let result = match self.member_recv_env(face, declaring, &entry) {
                Some(env) => self.subst_params(result, &env).unwrap_or(Ty::UNKNOWN),
                None => self.instantiate_accessor_static(name, &recv, result),
            };
            if result == Ty::UNKNOWN {
                continue;
            }
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
                // The re-rendered method face names the settled face's bare
                // element display — never the written instantiation or
                // nullability (the oracle renders `undefined_method` through
                // the element's display name; the getter/setter faces keep
                // the full `getDisplayString` render).
                let face_text = if matches!(code, SemanticCode::UndefinedMethod) {
                    self.strings.resolve(face_head).to_string()
                } else {
                    self.render_ty(face)
                };
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
            // null redirectedElement arm skips); the enum posture lives in
            // the query (an enum's generative constructors are implicitly
            // const).
            if edge.is_const
                && self.redirect_target_is_const(edge.target_container, edge.target) == Some(false)
            {
                let span = if edge.factory {
                    edge.span
                } else {
                    edge.target_span
                };
                self.emit(SemanticCode::RedirectToNonConstConstructor, span, "", "");
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
        let Some((super_container, super_owner)) = resolved else {
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
        // A mixin-application superclass answers through its own forwarded
        // face: `class S = T with M;` synthesizes `T`'s generative
        // constructors, so `super.t1` in a subclass reaches them through
        // the application's terminal container. A broken forwarding chain
        // keeps the application's own row (the under-report posture).
        let (face, face_owner, _) = self
            .effective_ctor_container(super_container, super_owner, superclass)
            .unwrap_or((super_container, super_owner, superclass));
        if face.constructors.contains(&name) && !self.ctor_is_factory(face, face_owner, name) {
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
                // The `extends` face as written: a prefixed clause
                // (`extends p.J`) binds through its import prefix — the
                // collector keeps only the bare leaf.
                match self.resolve_clause_face(container, self.file, self.lib, superclass) {
                    Some((super_container, super_owner)) => {
                        // A mixin-application superclass's super formal
                        // associates against the forwarded table on the
                        // terminal face — `class S = T with M;` declares
                        // `T`'s constructors, keyed on `T`'s own spelling.
                        self.effective_ctor_container(
                            super_container,
                            super_owner,
                            superclass,
                        )
                        .and_then(|(face, _, face_name)| {
                            face.ctor_formals.get(&target.unwrap_or(face_name))
                        })
                    }
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
        while end > 0
            && (bytes[end - 1].is_ascii_alphanumeric()
                || bytes[end - 1] == b'_'
                || bytes[end - 1] == b'$')
        {
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

    /// The UTF-16 offset of the `.` a named-selector creation head writes
    /// before its selector (`C<int>.named(` → the `.`), trivia skipped —
    /// the anchor [`Engine::literal_group_args`] reads the class segment's
    /// group against.
    fn selector_dot_before(&self, selector: SourceRange) -> Option<u32> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(selector.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        (cursor > 0 && bytes[cursor - 1] == b'.').then(|| self.utf16_of(cursor - 1))
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
        while cursor > 0
            && (bytes[cursor - 1].is_ascii_alphanumeric()
                || bytes[cursor - 1] == b'_'
                || bytes[cursor - 1] == b'$')
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
            if start == 0
                || !(bytes[start - 1].is_ascii_alphanumeric()
                    || bytes[start - 1] == b'_'
                    || bytes[start - 1] == b'$')
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
        while cursor > 0
            && (bytes[cursor - 1].is_ascii_alphanumeric()
                || bytes[cursor - 1] == b'_'
                || bytes[cursor - 1] == b'$')
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
            && (bytes[cursor].is_ascii_alphabetic()
                || bytes[cursor] == b'_'
                || bytes[cursor] == b'$')
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
        let callee = if self.ctor_pos > 0 && self.at_creation_level() {
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
            self.rows[index].shape = RowShape::Bare(BareInfo::Container { name: class, kind });
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
            named_head: bare_head.filter(|&(_, range)| self.creation_class_before(range).is_some()),
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
            let mut closure_spans: Vec<(u32, u32)> = level
                .closures
                .iter()
                .map(|(start, end, _)| (*start, *end))
                .collect();
            closure_spans.extend(self.invalid_closure_spans.iter().copied());
            self.solve_call_params(callee, &row, &level.args, &closure_spans);
            self.refine_numeric_call_return(callee, &row, &level.args);
        }
        // Re-arm this level's own creation head: the engine slot is single,
        // so a nested argument group's open overwrote it with the inner
        // head (and the inner close's `report_bare_creation_miss` path may
        // have consumed it) — the fold below must see the creation it
        // belongs to (`const C<T>(inner())` folds the outer creation, never
        // the inner head the interleaving left in the slot).
        let bare_head = levels
            .as_ref()
            .and_then(|level| level.named_head)
            .or(self.pending_bare_creation)
            .or_else(|| levels.as_ref().and_then(|level| level.unnamed_head));
        // #3208 — the creation's call-site instantiation. A `new`/`const`
        // creation and a named-selector creation (`C.named(`) carry no
        // callee row (the constructor reference resolves the head as a
        // type use and the selector pushes none — the fold arms below own
        // the group), so the send-close solver never ran and the fold
        // falls back to the bare head: `C<dynamic>`. Solve the class's
        // parameters from the group's own argument types first — the same
        // upward constraints a member send's close solve reads — and hand
        // the instantiated face to the fold.
        let creation_ty = self.creation_call_site_ty(levels.as_ref(), bare_head);
        if let Some(level) = &levels {
            self.capture_ffi_invocation(level, close);
        } else {
        }
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
                // The ctor-use edge (#2931): the `ConstructorName` node —
                // the class+selector compound — resolves the constructor
                // face, whose annotations the verifier's `member: Some`
                // arm reads through `ctor_annotations`.
                self.note_ctor_use(
                    class,
                    head,
                    SourceRange {
                        offset: class_offset,
                        length: head_range.end().saturating_sub(class_offset),
                    },
                );
                // The class segment's written group instantiates the created
                // type (`Future<Never>.error(..)` is `Future<Never>`), the
                // same face the unnamed fold reads; the group sits right
                // before the selector's `.`.
                let ty = self
                    .selector_dot_before(head_range)
                    .and_then(|dot| {
                        self.literal_group_args(Token {
                            id: 0,
                            range: SourceRange {
                                offset: dot,
                                length: 1,
                            },
                        })
                    })
                    .map_or_else(
                        || creation_ty.unwrap_or_else(|| Ty::named(class)),
                        |args| self.boundary.args.borrow_mut().named(class, false, &args),
                    );
                self.publish_creation_instantiation(class_offset, ty);
                self.rows.push(Row {
                    name: class,
                    range,
                    qualified: false,
                    shape: RowShape::Receiver(Receiver::Created {
                        name: class,
                        kind: ContainerKind::Class,
                        // #3398: the creation's resolved `ty` rides along —
                        // populated by #3394 (`ty: Some(ty)`).
                        ty: None,
                    }),
                    intent: Intent::Read,
                    write_op: None,
                    ty,
                });
            } else if self.keyword_creation_head(head_range.offset)
                && matches!(self.type_resolution(head).0, TypeResolution::Type)
            {
                self.fold_unnamed_creation_row(
                    head_range.offset,
                    head_range,
                    close.end(),
                    head,
                    ContainerKind::Class,
                    None,
                    creation_ty,
                );
            }
            // The prefix-resolved creation (`const fb.Reader().vTableGet`,
            // the unprefixed-spelled `fb.Reader(...)`): the constructor
            // reference `p.C` resolves the head as a type use behind the
            // prefix, so no callee row transports and the same stale-pop
            // the keyword arm folds. The resolved container's own face is
            // the created type; the member verdicts on the folded row
            // under-report through the bare scope the same way the
            // non-keyword prefixed send already does (the class name sits
            // behind the prefix table). The ctor-use edge keys on the
            // resolved owner — the bare namespace walk would read a
            // same-named container in the wrong scope.
            else if let Some((prefix, prefix_offset)) =
                self.written_prefix_before(head_range.offset)
                && let Some((resolved, owner)) = prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    head,
                )
            {
                let solved = self.creation_context_instantiation(head, prefix_offset);
                self.fold_unnamed_creation_row(
                    prefix_offset,
                    head_range,
                    close.end(),
                    head,
                    resolved.kind,
                    Some((resolved, owner)),
                    solved,
                );
            }
        }
    }

    /// The `dart:ffi` verifier's invocation capture ([`super::ffi`]): the
    /// group's facts ride out to the post-parse pass when the send wears
    /// one of the verifier's callee spellings (`Pointer.fromFunction`,
    /// `sizeOf`, `Native.addressOf`, `Struct.create`, `asFunction`,
    /// `elementAt`) or the head is a creation whose target may be an FFI
    /// class (`C()`, `NativeCallable<T>.listener(f)`). Every other group
    /// drops — the capture stays proportional to the file's FFI use, never
    /// to its call count.
    pub(crate) fn capture_ffi_invocation(&mut self, level: &ArgLevel, close: SourceRange) {
        let mut captured: Option<FfiInvocation> = None;
        // The send arm: a linked callee row whose selector spelling is one
        // of the verifier's own. The receiver's static type rides from the
        // row below the callee (past the written type arguments).
        // A `@Native`-annotated declaration's calls ride along too: the
        // `.address` position face needs every native call's argument
        // spans to decide leaf containment.
        if let Some(index) = self.invokes.last().and_then(|slot| *slot)
            && let Some(row) = self.rows.get(index)
            && (self.ffi_callee_spelling(row.name)
                || self.decl_sites.iter().any(|site| {
                    site.name == row.name && site.annotations.contains(&self.intern("Native"))
                }))
        {
            let mut recv_ty = Ty::UNKNOWN;
            let mut recv_found = false;
            let mut head_use: Option<StringId> = None;
            let mut back = index;
            while back > 0 {
                back -= 1;
                let below = &self.rows[back];
                if below.range.end() > row.range.offset {
                    continue;
                }
                // A class-qualified send (`Native.addressOf`, the static
                // factory shapes) keeps the named type use as the receiver
                // face when no value row sits below.
                if matches!(below.shape, RowShape::TypeUse(..)) {
                    if head_use.is_none() {
                        head_use = Some(below.name);
                    }
                    continue;
                }
                // The class-qualified send's merged receiver row keeps
                // the bare class spelling with an unresolved face
                // (`Native.addressOf`): the receiver IS the named type
                // the row spells.
                if matches!(below.shape, RowShape::Bare(..))
                    && matches!(self.type_resolution(below.name).0, TypeResolution::Type)
                {
                    recv_ty = Ty::named(below.name);
                    recv_found = true;
                    break;
                }
                recv_ty = below.ty;
                recv_found = true;
                break;
            }
            if !recv_found && let Some(name) = head_use {
                recv_ty = Ty::named(name);
            }
            captured = Some(FfiInvocation {
                callee: row.name,
                selector: None,
                head: row.range,
                type_arg: self.ffi_written_type_arg(row.range),
                args: Vec::new(),
                recv_ty,
                whole: SourceRange {
                    offset: row.range.offset,
                    length: close.end().saturating_sub(row.range.offset),
                },
                is_creation: false,
            });
        }
        // The creation arm: a bare head (`C(`, `const C(`, the
        // named-selector `NativeCallable<T>.listener(`). The target must
        // resolve to an FFI compound or `NativeCallable` — a plain user
        // class's creations are never captured. The fold latches the
        // named-selector creation's SELECTOR (`listener`), so the class
        // rides the source walk before the `.` when the latch itself is
        // no FFI target.
        if captured.is_none()
            && let Some((head_name, head_range)) = level
                .named_head
                .or(self.pending_bare_creation)
                .or(level.unnamed_head)
        {
            let direct = self.ffi_creation_target(head_name);
            let (class_name, head) = if direct {
                (head_name, head_range)
            } else {
                match self
                    .creation_class_before(head_range)
                    .filter(|(class, _)| self.ffi_creation_target(*class))
                {
                    Some((class, offset)) => {
                        // The head span is the CLASS NAME only: the
                        // written type-argument walk starts right after
                        // it (`NativeCallable<T>.listener`).
                        let bytes = self.source.as_bytes();
                        let mut end_byte = self.byte_of(offset);
                        while bytes
                            .get(end_byte)
                            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'$')
                        {
                            end_byte += 1;
                        }
                        (
                            class,
                            SourceRange {
                                offset,
                                length: self.utf16_of(end_byte).saturating_sub(offset),
                            },
                        )
                    }
                    None => return,
                }
            };
            captured = Some(FfiInvocation {
                callee: class_name,
                selector: if direct {
                    self.selector_after_head(head)
                } else {
                    Some(head_name)
                },
                head,
                type_arg: self.ffi_written_type_arg(head),
                args: Vec::new(),
                recv_ty: Ty::UNKNOWN,
                whole: SourceRange {
                    offset: head.offset,
                    length: close.end().saturating_sub(head.offset),
                },
                is_creation: true,
            });
        }
        // The bare-send fallback: a creation the fold carried as a bare
        // send (`C(...)` closing with no creation latch) whose name
        // resolves to an FFI compound — the struct/union creation faces'
        // subject. The send arm's callee-spelling gate has already
        // rejected the row as a verifier callee by the time this runs.
        if captured.is_none()
            && let Some(index) = self.invokes.last().copied().flatten()
            && let Some(row) = self.rows.get(index)
            && matches!(row.shape, RowShape::Bare(..))
            && self.ffi_creation_target(row.name)
        {
            let head = row.range;
            captured = Some(FfiInvocation {
                callee: row.name,
                selector: None,
                head,
                type_arg: self.ffi_written_type_arg(head),
                args: Vec::new(),
                recv_ty: Ty::UNKNOWN,
                whole: SourceRange {
                    offset: head.offset,
                    length: close.end().saturating_sub(head.offset),
                },
                is_creation: true,
            });
        }
        let Some(mut invocation) = captured else {
            return;
        };
        invocation.args = level
            .args
            .iter()
            .map(|(name, ty, _literal, range)| {
                let (invocation_shaped, head) = self.arg_invocation_head(*range);
                (
                    *name,
                    FfiArgShape {
                        range: *range,
                        ty: *ty,
                        invocation_shaped,
                        head,
                    },
                )
            })
            .collect();
        self.ffi_invocations.push(invocation);
    }

    /// The named-selector creation's selector segment: between the head's
    /// span end and the argument list's `(` sit an optional `<...>` type
    /// group and `.name` — that name is the selector. Any other shape
    /// (a direct `(`, a comment) answers `None`.
    fn selector_after_head(&self, head: SourceRange) -> Option<StringId> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(head.end());
        let mut depth = 0i32;
        while let Some(&b) = bytes.get(cursor) {
            match b {
                b'<' => depth += 1,
                b'>' => depth = (depth - 1).max(0),
                b'.' if depth == 0 => {
                    let mut end = cursor + 1;
                    while bytes
                        .get(end)
                        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
                    {
                        end += 1;
                    }
                    return (end > cursor + 1)
                        .then(|| self.strings.intern(&self.source[cursor + 1..end]));
                }
                _ if b.is_ascii_whitespace() || depth > 0 => {}
                _ => return None,
            }
            cursor += 1;
        }
        None
    }

    /// The written `<...>` type argument after an invocation head
    /// (`fromFunction<Int8 Function(Int8)>`,
    /// `NativeCallable<T>.listener`), source-parsed — see
    /// [`FfiTypeArg`]. `None` when no group follows the head.
    fn ffi_written_type_arg(&self, head: SourceRange) -> Option<FfiTypeArg> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(head.end());
        while bytes.get(cursor).is_some_and(|b| b.is_ascii_whitespace()) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'<') {
            return None;
        }
        let Some(close) = matching_close(bytes, cursor, b'<', b'>') else {
            return None;
        };
        let inner = &self.source[cursor + 1..close];
        let trimmed = inner.trim();
        if trimmed.is_empty() {
            return None;
        }
        let head_len: usize = trimmed
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .map(char::len_utf8)
            .sum();
        if head_len == 0 {
            return None;
        }
        let start = cursor + 1 + (inner.len() - inner.trim_start().len());
        let end = start + trimmed.len();
        Some(FfiTypeArg {
            display: trimmed.into(),
            span: SourceRange {
                offset: byte_offset_to_utf16(self.source, start),
                length: byte_offset_to_utf16(self.source, end)
                    .saturating_sub(byte_offset_to_utf16(self.source, start)),
            },
            head: self.strings.intern(&trimmed[..head_len]),
            fn_sig: self.ffi_written_fn_sig(trimmed),
        })
    }

    /// The written `R Function(P...)` form: the last top-level `Function`
    /// word (a return type never nests a bare `Function` outside groups)
    /// whose group closes at the text's end. Each parameter contributes
    /// its first identifier.
    fn ffi_written_fn_sig(&self, text: &str) -> Option<(StringId, Box<[StringId]>)> {
        let bytes = text.as_bytes();
        let mut depth_angle = 0i32;
        let mut depth_paren = 0i32;
        let mut best: Option<(usize, usize)> = None;
        for (i, &b) in bytes.iter().enumerate() {
            match b {
                b'<' => depth_angle += 1,
                b'>' => depth_angle -= 1,
                b'(' => depth_paren += 1,
                b')' => depth_paren -= 1,
                b'F' if depth_angle == 0 && depth_paren == 0 => {
                    let word_end = i + "Function".len();
                    let word_before =
                        i == 0 || bytes.get(i - 1).is_some_and(|b| b.is_ascii_whitespace());
                    if word_before
                        && text[i..].starts_with("Function")
                        && bytes.get(word_end) == Some(&b'(')
                        && let Some(close) = matching_close(bytes, word_end, b'(', b')')
                    {
                        best = Some((i, close));
                    }
                }
                _ => {}
            }
        }
        let (ret_end, group_close) = best?;
        let ret = text[..ret_end].trim();
        let params_text = &text[ret_end + "Function".len() + 1..group_close];
        let ret_head: String = ret
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if ret_head.is_empty() {
            return None;
        }
        let params: Vec<StringId> = params_text
            .split(',')
            .filter_map(|param| {
                let head: String = param
                    .trim()
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
                    .collect();
                (!head.is_empty()).then(|| self.strings.intern(&head))
            })
            .collect();
        Some((self.strings.intern(&ret_head), params.into_boxed_slice()))
    }

    /// Whether the argument expression at `range` reads as an invocation
    /// (its last source byte is the call's `)`), and the invocation head's
    /// spelling (the first identifier of the span) — the
    /// `argument_must_be_a_constant` face's source facts.
    fn arg_invocation_head(&self, range: SourceRange) -> (bool, StringId) {
        let start = self.byte_of(range.offset);
        let end = self.byte_of(range.end()).min(self.source.len());
        let text = self.source.get(start..end).unwrap_or_default();
        // The fold's argument span may carry the callee head only (the
        // `(` sits just past the span's end) or the whole call; both read
        // as an invocation.
        let trimmed_end = text.trim_end();
        let after = self.source[self.byte_of(range.end()).min(self.source.len())..].trim_start();
        let invocation_shaped = trimmed_end.ends_with(')') || after.starts_with('(');
        let head: String = text
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let head = if head.is_empty() {
            StringId::NONE
        } else {
            self.strings.intern(&head)
        };
        (invocation_shaped, head)
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
    /// `ctor` is the declaring container and owner file a prefix-resolved
    /// head already carried — the bare namespace walk would read a
    /// same-named container in the wrong scope (the prefixed member
    /// verdicts' own posture). `solved` is the call site's instantiation
    /// ([`Engine::creation_call_site_ty`]): it types the created row when
    /// the head spells no written `<...>` group.
    fn fold_unnamed_creation_row(
        &mut self,
        start: u32,
        head_range: SourceRange,
        close_end: u32,
        head: StringId,
        kind: ContainerKind,
        ctor: Option<(&'a Container, FileId)>,
        solved: Option<Ty>,
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
        // The ctor-use edge (#2931): the `ConstructorName` node resolves the
        // unnamed constructor face — the head token is the oracle's anchor
        // for a bare head; a prefixed head's node covers the `p.C` compound.
        // Classes alone reach it — an enum's `E(` is no creation upstream.
        if kind == ContainerKind::Class {
            match ctor {
                Some((decl, file)) => self.note_ctor_use_in(
                    decl,
                    file,
                    head,
                    head,
                    SourceRange {
                        offset: start,
                        length: head_range.end().saturating_sub(start),
                    },
                ),
                None => self.note_ctor_use(head, head, head_range),
            }
        }
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
                || solved.unwrap_or_else(|| Ty::named(head)),
                |args| self.boundary.args.borrow_mut().named(head, false, &args),
            );
        self.publish_creation_instantiation(start, ty);
        self.rows.push(Row {
            name: head,
            range,
            qualified: false,
            shape: RowShape::Receiver(Receiver::Created {
                name: head,
                kind,
                // #3398: the creation's resolved `ty` rides along —
                // populated by #3394 (`ty: Some(ty)`).
                ty: None,
            }),
            intent: Intent::Read,
            write_op: None,
            ty,
        });
    }

    /// The created type a creation's own arguments instantiate: the
    /// class's declared parameters solved from the argument types'
    /// upward constraints ([`Engine::unify_ctor_upward`]) — the same
    /// structural walk the member send's close solve runs, which a
    /// constructor send never reaches (constructors are not member rows,
    /// and the creation's head resolves as a type use, so no callee row
    /// links). Every parameter materializes: a solved slot keeps its
    /// single decidable candidate (an agreeing repeat holds; a
    /// disagreement keeps no binding — under-report beats an invented
    /// LUB merge), an unsolved one takes its declared bound, else
    /// `dynamic` (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/generic_inferrer.dart's
    /// `chooseFinalTypes`, whose still-free parameters read
    /// `_inferer.typeVariableForSubtype`'s bound over the
    /// instantiate-to-bounds fill).
    ///
    /// `None` keeps the caller's raw-head face (the to-bounds `<dynamic>`
    /// display residual): an explicit `<...>` group always outranks
    /// inference, a linked callee row already ran the close solve, and an
    /// unresolvable head or constructor table leaves nothing to pair.
    fn creation_call_site_ty(
        &mut self,
        level: Option<&ArgLevel>,
        bare_head: Option<(StringId, SourceRange)>,
    ) -> Option<Ty> {
        let level = level?;
        let (head, head_range) = bare_head?;
        // The head this group belongs to: the class segment of a named
        // selector (`C.named(`) or the keyworded head itself (`new C(`).
        // A resolved class is the only creation head; an implicit send
        // (`x.foo(`) keeps its own faces.
        let (class, selector, class_offset) = match self.creation_class_before(head_range) {
            Some((class, class_offset)) => (class, head, class_offset),
            None if self.keyword_creation_head(head_range.offset) => (head, head, head_range.offset),
            None => return None,
        };
        if self.type_resolution(class).0 != TypeResolution::Type {
            return None;
        }
        // Explicit type arguments outrank inference: a written `<...>`
        // group fixes the instantiation even when the reduced parser
        // cannot recover its members (`new C<int, dynamic>(1, "x")` keeps
        // the written face, never the argument-solved one).
        if self.creation_written_type_args(class, class_offset) {
            return None;
        }
        // A linked callee row already ran the send close's own solve.
        if self.invokes.last().is_some_and(|slot| slot.is_some()) {
            return None;
        }
        // No argument constrains the class's parameters: the context type
        // is the only downward source (`LinkedHashSet<int> s =
        // LinkedHashSet()` infers `<int>`; upstream resolves every
        // instance creation under the enclosing expression's context).
        if level.args.is_empty() {
            return self.creation_context_instantiation(class, class_offset);
        }
        let params: Vec<(StringId, Option<Ty>)> = match self.bounds.container_params(class) {
            Some(params) if !params.is_empty() => params
                .iter()
                .map(|(param, bound, _)| (*param, bound.as_ref().map(|tree| self.tree_ty(tree))))
                .collect(),
            // A creation whose class declaration closes after the site
            // reads no bounds capture mid-parse; the element model's
            // parameter names stand in unbounded (the same fallback the
            // argument check's `exec_params` takes).
            _ => self
                .foreign_container_params(class)?
                .into_iter()
                .map(|param| (param, None))
                .collect(),
        };
        let formals = self.ctor_schema_formals(class, selector)?;
        let mut candidates: Vec<(StringId, Ty)> = Vec::new();
        let mut positional = 0usize;
        for (name, ty, _literal, range) in &level.args {
            let formal = match name {
                Some(name) => self.named_formal_for(&formals, *name),
                None => {
                    let formal = formals.positional.get(positional);
                    positional += 1;
                    formal
                }
            };
            let Some(formal) = formal else {
                continue;
            };
            // A function-typed argument's row may carry no `Ty` — a
            // closure literal's signature rides `level.closures`, a
            // tear-off's `arg_sigs` — and still constrains the class's
            // parameters (`C((int x) => x)` binds `T := int` through
            // `T Function(T)`). A member-read argument resolves its
            // accessor return for the solve only, the same face the
            // send-close walk reads.
            let arg_ty = if *ty != Ty::UNKNOWN {
                *ty
            } else {
                let sig: Option<Box<FnVarSig>> =
                    self.arg_sigs.get(&range.offset).cloned().or_else(|| {
                        level
                            .closures
                            .iter()
                            .find(|(start, end, _)| range.offset >= *start && range.offset <= *end)
                            .map(|(.., sig)| sig.clone())
                    });
                match sig {
                    Some(sig) => self.fn_sig_ty(&sig),
                    None => match self.solve_arg_member_ty(range.offset) {
                        Some(ty) => ty,
                        None => continue,
                    },
                }
            };
            if arg_ty != Ty::UNKNOWN {
                self.unify_ctor_upward(formal.ty, arg_ty, &params, &mut candidates);
            }
        }
        let mut solved: Vec<Option<Ty>> = vec![None; params.len()];
        // The downward seed: a context type naming the same class pairs its
        // type arguments with the declared parameters ahead of the
        // argument-side constraints (upstream's `InferenceContext` supplies
        // the `T` mappings first and `chooseFinalTypes` upgrades them with
        // the argument types). A seeded slot is PINNED, never poisoned.
        let mut pinned: Vec<bool> = vec![false; params.len()];
        if let Some(seed) = self.creation_context_seed(class, class_offset, params.len()) {
            for ((slot, pin), ty) in solved.iter_mut().zip(pinned.iter_mut()).zip(seed) {
                if !matches!(ty, Ty::Dynamic | Ty::Unknown) {
                    *slot = Some(ty);
                    *pin = true;
                }
            }
        }
        let mut poisoned: FxHashSet<usize> = FxHashSet::default();
        for (param, ty) in candidates {
            let Some(index) = params.iter().position(|(candidate, _)| *candidate == param) else {
                continue;
            };
            if poisoned.contains(&index) || !self.schema_type_is_decidable(ty) {
                continue;
            }
            match solved[index] {
                None => solved[index] = Some(ty),
                Some(kept) if kept == ty => {}
                // The covariant pin (`inferTypeParameterFromAll`'s
                // `typeFromPreviousInference` arm, upstream Dart SDK source,
                // pkg/_fe_analyzer_shared/lib/src/type_inference/
                // type_analyzer_operations.dart:1237-1241): a parameter the
                // DOWNWARD inference already bound returns that binding and
                // ignores the argument constraint — the constraint's
                // conflict reports at the argument row through the ordinary
                // assignability check, never by overwriting the created
                // type (`A<int, String> x = new A('s', 1)` keeps
                // `A<int, String>` and reports `'s'`).
                Some(_) if pinned[index] => {}
                Some(_) => {
                    poisoned.insert(index);
                    solved[index] = None;
                }
            }
        }
        if solved.iter().all(Option::is_none) {
            return None;
        }
        let args: Vec<Ty> = params
            .iter()
            .zip(&solved)
            .map(|((_param, bound), slot)| slot.or(*bound).unwrap_or(Ty::Dynamic))
            .collect();
        Some(self.boundary.args.borrow_mut().named(class, false, &args))
    }

    /// Publishes a creation's instantiation where the created row's readers
    /// resolve it: `call_type_args`, the slot a written `<...>` group rides
    /// and [`Engine::creation_row_ty`] peeks — the return/arrow-body and
    /// cascade faces type a `Created` row through that face, never its
    /// `row.ty`. A written group outranks inference before every caller
    /// reaches here, so the entry carries the solved (downward) arguments
    /// alone; zero-length spans keep the bounds faces (which only read
    /// consumed written groups) inert. A `Named` without arguments (the
    /// to-bounds residual) publishes nothing.
    fn publish_creation_instantiation(&mut self, offset: u32, ty: Ty) {
        let Ty::Named { args, .. } = ty else {
            return;
        };
        if args == ArgListId::EMPTY {
            return;
        }
        // A written group already owns the slot (its real spans feed the
        // bounds diagnostics): the written instantiation equals the
        // published one, so the entry stands.
        if self.call_type_args.iter().any(|(at, _)| *at == offset) {
            return;
        }
        let tys = self.boundary.args.borrow().get(args).to_vec();
        let spans = vec![
            SourceRange {
                offset,
                length: 0,
            };
            tys.len()
        ];
        self.call_type_args.push((
            offset,
            CallTypeArgs {
                tys: tys.into_boxed_slice(),
                spans: spans.into_boxed_slice(),
            },
        ));
    }

    /// The creation's static type when only its context names the class's
    /// parameters: `LinkedHashSet<int> s = LinkedHashSet()` instantiates
    /// the created type with the context's own arguments (upstream resolves
    /// every `InstanceCreationExpression` under the enclosing expression's
    /// context type, so the constructor's parameters are solved downward
    /// before the argument-side constraints). A context naming another
    /// class — the supertype-context shapes (`Set<int> s = LinkedHashSet()`
    /// needs the supertype walk this face does not run) — or an untyped
    /// context keeps the caller's to-bounds fallback.
    fn creation_context_instantiation(&mut self, class: StringId, start: u32) -> Option<Ty> {
        let context = self.creation_context_ty(start)?;
        let Ty::Named { name, args, .. } = self.fold_alias_ty(context) else {
            return None;
        };
        (name == class && args != ArgListId::EMPTY).then_some(Ty::Named {
            name: class,
            nullable: false,
            args,
        })
    }

    /// [`Engine::creation_context_instantiation`]'s positional split: the
    /// context's type arguments when they pair 1:1 with `count` declared
    /// parameters. `None` keeps the argument-side solve alone.
    fn creation_context_seed(
        &mut self,
        class: StringId,
        start: u32,
        count: usize,
    ) -> Option<Vec<Ty>> {
        let context = self.creation_context_ty(start)?;
        let Ty::Named { name, args, .. } = self.fold_alias_ty(context) else {
            return None;
        };
        if name != class || args == ArgListId::EMPTY {
            return None;
        }
        let args = self.boundary.args.borrow().get(args).to_vec();
        (args.len() == count).then_some(args)
    }

    /// The context type the creation at `start` resolves under — the
    /// downward half of upstream's instance-creation inference
    /// (`visitInstanceCreationExpression` analyzes the creation under the
    /// enclosing expression's context type). The channels the surrounding
    /// grammar reaches, innermost first: an enclosing grouping parenthesis
    /// (climbed, a `const`/`new` keyworded head included), the declaration
    /// initializer's armed annotation, an assignment target's write type,
    /// the host executable's declared return (a function literal's
    /// `ctx_ret` stands in for its function-typed context), and the
    /// conditional a `?`/`:` branch operand sits in (both branches inherit
    /// the whole conditional's context). Every other position — an
    /// argument slot, a collection element, a binary operand — answers
    /// `None` (the no-context posture).
    fn creation_context_ty(&mut self, start: u32) -> Option<Ty> {
        self.creation_context_ty_at(start, 0)
    }

    fn creation_context_ty_at(&mut self, start: u32, depth: u8) -> Option<Ty> {
        use dartq_scanner::{Keyword, TokenKind as K};
        if depth > 8 {
            return None;
        }
        let tokens = self.tokens.as_slice();
        let mut at = tokens.partition_point(|token| token.utf16_offset < start);
        loop {
            let index = at.checked_sub(1)?;
            let token = tokens[index];
            match token.kind {
                // An enclosing grouping parenthesis: the expression's own
                // position is the group's. A `(` left of an operand-ending
                // token is a call/index argument list instead — an
                // argument slot this face does not resolve.
                K::OpenParen => {
                    if Self::creation_operand_end(tokens[index.checked_sub(1)?].kind) {
                        return None;
                    }
                    at = index;
                }
                // A `const`/`new` keyworded head and an `await` operand
                // inherit the whole expression's position.
                K::Keyword(Keyword::Const)
                | K::Keyword(Keyword::New)
                | K::Keyword(Keyword::Await) => {
                    at = index;
                }
                K::Eq => {
                    return if self.in_initializer && self.frames.len() == self.init_frame_depth {
                        self.pending_decl_ty.map(|ty| self.fold_alias_ty(ty))
                    } else {
                        self.assignment_context_at(token.utf16_offset)
                    };
                }
                K::QuestionQuestionEq => return self.assignment_context_at(token.utf16_offset),
                // The host's declared return, or a function literal's
                // context return (`arm_closure_completion`'s `ctx_ret`).
                K::Function | K::Keyword(Keyword::Return) => return self.enclosing_return_ty(),
                // A conditional operand inherits the conditional's own
                // context: `a = c ? b : C()` reads the `=` target through
                // the else operand exactly as the then operand does.
                K::Question => {
                    let condition = self.conditional_condition_start(index)?;
                    return self.creation_context_ty_at(condition, depth + 1);
                }
                K::Colon => {
                    let question = self.conditional_question_before(index)?;
                    let condition = self.conditional_condition_start(question)?;
                    return self.creation_context_ty_at(condition, depth + 1);
                }
                _ => return None,
            }
        }
    }

    /// Whether `kind` can end an operand (the token left of a `(` decides
    /// grouping from call argument list): identifiers, literals, and the
    /// closing brackets.
    fn creation_operand_end(kind: dartq_scanner::TokenKind) -> bool {
        use dartq_scanner::{Keyword, TokenKind as K};
        matches!(
            kind,
            K::Identifier
                | K::Int
                | K::Double
                | K::Hexadecimal
                | K::String
                | K::StringInterpolationIdentifier
                | K::CloseParen
                | K::CloseSquare
                | K::Keyword(Keyword::This)
                | K::Keyword(Keyword::Super)
                | K::Keyword(Keyword::True)
                | K::Keyword(Keyword::False)
                | K::Keyword(Keyword::Null)
        )
    }

    /// The write type of the `=`/`??=` operator at `limit`: the target row
    /// read through the assignment check's own face — the constructor
    /// initializer's bare target resolves through the field table so a
    /// same-named formal never lends its type. An index write (`c[i] = e`,
    /// the target row under an operator mark) answers `None`.
    fn assignment_context_at(&mut self, limit: u32) -> Option<Ty> {
        let target = self
            .rows
            .iter()
            .rev()
            .find(|row| row.range.end() <= limit && !matches!(row.shape, RowShape::Value))?
            .clone();
        if matches!(target.intent, Intent::Operator(..)) {
            return None;
        }
        let ty = match (self.initializer_depth > 0, &target.shape) {
            (true, RowShape::Bare(_)) if !target.qualified => {
                match self.init_rows.get(&target.range.offset).copied() {
                    Some(container) => self.member_write_ty(container, target.name),
                    None => self.assignment_target_ty(&target),
                }
            }
            _ => self.assignment_target_ty(&target),
        };
        Some(ty)
    }

    /// The declared return of the innermost executable frame — the host
    /// `return e` / `=> e` context — with a function literal's context
    /// return standing in when the literal's own frame declares none.
    fn enclosing_return_ty(&self) -> Option<Ty> {
        let frame = self
            .frames
            .iter()
            .rev()
            .find(|frame| matches!(frame.kind, FrameKind::Executable { .. }))?;
        match &frame.kind {
            FrameKind::Executable { ret: Some(ret), .. } => Some(self.fold_alias_ty(ret.ty)),
            FrameKind::Executable { ctx_ret, .. } => *ctx_ret,
            _ => None,
        }
    }

    /// The `?` the conditional's `:` at `colon` pairs with: a backward scan
    /// over the else operand that skips bracket groups and nested
    /// conditionals (`a ? b ? c : d : e` pairs the outer `?` with the last
    /// `:`). `None` when no pairing `?` sits before it.
    fn conditional_question_before(&self, colon: usize) -> Option<usize> {
        use dartq_scanner::TokenKind as K;
        let tokens = self.tokens.as_slice();
        let mut depth = 0usize;
        let mut index = colon;
        while index > 0 {
            index -= 1;
            match tokens[index].kind {
                K::CloseParen | K::CloseSquare | K::CloseCurly => depth += 1,
                K::OpenParen | K::OpenSquare | K::OpenCurly => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                }
                K::Colon => depth += 1,
                K::Question => {
                    if depth == 0 {
                        return Some(index);
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        None
    }

    /// The first token offset of the condition operand of the conditional
    /// whose `?` sits at `question`: the token after the nearest depth-0
    /// expression boundary (`=`, `,`, `;`, `return`, `=>`, an opening
    /// bracket, or a preceding conditional operator).
    fn conditional_condition_start(&self, question: usize) -> Option<u32> {
        use dartq_scanner::{Keyword, TokenKind as K};
        let tokens = self.tokens.as_slice();
        let mut depth = 0usize;
        let mut index = question;
        while index > 0 {
            index -= 1;
            let token = tokens[index];
            match token.kind {
                K::CloseParen | K::CloseSquare | K::CloseCurly => depth += 1,
                K::OpenParen | K::OpenSquare | K::OpenCurly => {
                    if depth == 0 {
                        return tokens.get(index + 1).map(|token| token.utf16_offset);
                    }
                    depth -= 1;
                }
                K::Eq
                | K::Comma
                | K::Semicolon
                | K::Colon
                | K::Question
                | K::Function
                | K::Keyword(Keyword::Return)
                    if depth == 0 =>
                {
                    return tokens.get(index + 1).map(|token| token.utf16_offset);
                }
                _ => {}
            }
        }
        tokens.first().map(|token| token.utf16_offset)
    }

    /// Whether the creation's class segment spells a written `<...>`
    /// group (trivia aside): the head's own bytes open an angle group
    /// before the selector or the argument list.
    fn creation_written_type_args(&self, name: StringId, offset: u32) -> bool {
        let head_len = self.strings.resolve(name).encode_utf16().count() as u32;
        let mut cursor = self.byte_of(offset.saturating_add(head_len));
        let bytes = self.source.as_bytes();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        cursor < bytes.len() && bytes[cursor] == b'<'
    }

    /// The upward constraint for one constructor argument: the send-close
    /// solver's structural walk ([`Engine::unify_upward`]) plus the
    /// nullable-writable formal arm — a `T?` parameter (`C(T? v)`) binds
    /// `T` from the argument's non-null head (official `A? <: T?`; the
    /// same rule the closure-schema unifier applies).
    fn unify_ctor_upward(
        &self,
        formal: Ty,
        arg: Ty,
        params: &[(StringId, Option<Ty>)],
        out: &mut Vec<(StringId, Ty)>,
    ) {
        if let Ty::Named {
            name,
            nullable: true,
            args: ArgListId::EMPTY,
        } = formal
            && params.iter().any(|(param, _)| *param == name)
        {
            let bound = match arg {
                Ty::Named {
                    name,
                    args,
                    nullable: true,
                } => Ty::Named {
                    name,
                    args,
                    nullable: false,
                },
                other => other,
            };
            out.push((name, bound));
            return;
        }
        self.unify_upward(formal, arg, params, out);
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
            // A `?.`-merged argument reads the short-circuit's nullable
            // result (`g(a?.b)` offers `B?`).
            let ty = self.expression_row_ty(row);
            // The await flatten re-applies on the on-demand read — the same
            // face `flush_argument` gives the claimed argument (a member/send
            // operand's close-time row still carries the raw `Future<T>`).
            let ty = if ty != Ty::UNKNOWN
                && self
                    .await_operand_extents
                    .iter()
                    .any(|&(start, end)| row.range.offset >= start && row.range.end() <= end)
            {
                self.flatten(ty)
            } else {
                ty
            };
            (
                ty,
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
                    // An `await` operand's row rewrite (`end_await_expression`)
                    // flattened only the close-time stored `ty` — a
                    // member/send operand resolves in the later pass, so the
                    // on-demand read here answers the raw `Future<T>` face.
                    // The operand's static type IS `flatten(operand)`
                    // (`analyzeAwaitExpression`, type_analyzer.dart:542-548 —
                    // the same face the var-inferred bind, the assignment
                    // staging, and the prefix operand re-apply), so the
                    // argument ask (`checkForArgumentTypesNotAssignableInList`
                    // reads each argument's resolved `staticType`) must see
                    // the flattened `T` too (#3061: `f(await fut)` offered
                    // `Future<dynamic>` against `num`).
                    let ty = if ty != Ty::UNKNOWN
                        && self
                            .await_operand_extents
                            .iter()
                            .any(|&(start, end)| row.range.offset >= start && row.range.end() <= end)
                    {
                        self.flatten(ty)
                    } else {
                        ty
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
                        let anchor = hi.checked_sub(1).and_then(|i| self.rows.get(i)).map_or(
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
    /// type-parameter names paired with their declared bounds (a `Dynamic`
    /// entry = absent, matching the executable windows' `Option<Ty>`
    /// contract the bounds face reads).
    pub(crate) fn implicit_call_face(
        &self,
        ty: Ty,
    ) -> Option<(Formals, Ty, Box<[(StringId, Option<Ty>)]>)> {
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
                        e.type_param_names
                            .iter()
                            .enumerate()
                            .map(|(index, name)| {
                                (*name, e.type_param_bounds.get(index).copied().filter(|b| *b != Ty::Dynamic))
                            })
                            .collect(),
                    ),
                    Projected::Foreign(e) => (
                        e.formals.as_deref()?.to_owned(),
                        e.return_ty,
                        e.type_param_names
                            .iter()
                            .enumerate()
                            .map(|(index, name)| {
                                (*name, e.type_param_bounds.get(index).copied().filter(|b| *b != Ty::Dynamic))
                            })
                            .collect(),
                    ),
                };
                Some((formals, return_ty, params))
            }
            _ => None,
        }
    }

    /// The tear-off signature of an argument row: a bare unqualified read
    /// of a same-file top-level function — or of a declared local
    /// function, whose own value IS the tear-off (#2842) — carries the
    /// function's own signature as its type-as-a-value (the row's `Ty`
    /// stays `Unknown` — the model has no function variant; the signature
    /// is the structural face). Getters carry no formals and synthesize
    /// nothing; member and prefixed tear-offs stay untyped (under-report).
    /// Only a value READ
    /// qualifies: a completed send's callee row keeps its bare shape (the
    /// send flips `intent` to [`Intent::Invoke`] alone), so an ungated
    /// shape check reads `use(f())`'s call result as a tear-off of `f`
    /// and the non-function-formal face co-reports a spurious
    /// `argument_type_not_assignable` (oracle 3.13.1 is silent there).
    pub(crate) fn tear_off_sig(&self, row: &Row) -> Option<Box<FnVarSig>> {
        if row.qualified || row.intent != Intent::Read {
            return None;
        }
        // A declared local FUNCTION's own value IS the tear-off (#2842):
        // the frame capture answers before the shadow refusals — the read
        // carries the declaration's own signature (the formal-compare
        // face here and the type-check operand face alike). A plain local
        // of the same spelling finds no capture in
        // [`Engine::local_fn_sig`] and falls to the refusal below.
        if let Some(sig) = self.local_fn_sig(row.name) {
            return Some(Box::new(sig.clone()));
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
            // A top-level getter shares the tag but captures no formal
            // group at all (a zero-parameter function captures an empty
            // one): its bare read is the getter's value, never a tear-off.
            decl.formals.as_ref()?;
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

    /// A captured [`FnVarSig`] as a pool `Ty::Function` — the inference
    /// face's argument-type materialization: a tear-off or closure literal
    /// constrains the call's type parameters through its signature
    /// (`unify_upward` consumes `Ty::Function` pairs position-by-position,
    /// return included). The named section drops the `required` bit — the
    /// shape table records types alone. Also the verdict surface's
    /// tear-off materialization (the assignment/return positions read the
    /// same face the argument position does).
    pub(crate) fn fn_sig_ty(&self, sig: &FnVarSig) -> Ty {
        let positional: Vec<Ty> = sig.positional.to_vec();
        let named: Vec<(StringId, Ty)> =
            sig.named.iter().map(|(name, ty, _)| (*name, *ty)).collect();
        // The signature's own required-positional split rides the shape: it
        // is part of the function-type identity, and a count-blind intern
        // here would give the same spelling a second `FnShapeId` whenever
        // another producer recorded the honest split.
        self.boundary.args.borrow_mut().function_generic_required(
            &sig.params,
            sig.required_positional,
            sig.return_ty,
            &positional,
            &named,
            false,
        )
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
                Some(
                    self.boundary
                        .args
                        .borrow_mut()
                        .record(&positional, &named, nullable),
                )
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
                Some(self.boundary.intern_function_ty_required(
                    pool_shape.type_params,
                    pool_shape.required_positional,
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

    // ---------------------------------------------------------------------
    // The real constraint solve (#2938): upstream `GenericInferrer`'s
    // gather → preliminary-choose → gather-args → final-choose → check
    // pipeline, on the element model's `Ty`. `Ty::Unknown` IS the schema
    // `_` — `isKnownType` answers false on it, `isSatisfiedBy` treats it as
    // the unconstrained side, and `chooseTypeFromConstraint` reads a
    // `(_ , X)` constraint through the closure arms exactly as upstream's
    // `UnknownInferredType` does.
    // ---------------------------------------------------------------------

    /// Whether `ty` is fully grounded — upstream's `isKnownType`: a bare
    /// `_` hole or any hole inside a compound face makes the type
    /// "not known". A type-parameter leaf is known (it names a concrete
    /// declaration-site variable, never an inference hole).
    fn infer_known(&self, ty: Ty) -> bool {
        match ty {
            Ty::Unknown => false,
            Ty::Named { args, .. } => {
                let list: Vec<Ty> = self.boundary.args.borrow().get(args).to_vec();
                list.iter().all(|&arg| self.infer_known(arg))
            }
            Ty::Function { shape, .. } => {
                let shape = {
                    let pool = self.boundary.pool();
                    pool.function_shape(shape).clone()
                };
                self.infer_known(shape.return_ty)
                    && shape.positional.iter().all(|&t| self.infer_known(t))
                    && shape.named.iter().all(|&(_, t)| self.infer_known(t))
            }
            Ty::Record { shape, .. } => {
                let shape = {
                    let pool = self.boundary.pool();
                    pool.record_shape(shape).clone()
                };
                shape.positional.iter().all(|&t| self.infer_known(t))
                    && shape.named.iter().all(|&(_, t)| self.infer_known(t))
            }
            _ => true,
        }
    }

    /// The greatest/least closure of `ty` over the inference parameters
    /// (`greatest_closure`, upstream `type_analyzer_operations.dart`'s
    /// closure of a type schema): a covariant occurrence of a parameter
    /// reads `Object?`, a contravariant one `Never`; a bare parameter leaf
    /// in the target set reads `Object?` under `covariant` else `Never`.
    /// Named/record args stay covariant; a function shape's return is
    /// covariant and its formals contravariant (the polarity flip the
    /// `T Function(T)` → `Object? Function(Never)` closure carries).
    fn infer_closure(&self, ty: Ty, params: &[(StringId, Option<Ty>)], covariant: bool) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if args == ArgListId::EMPTY && params.iter().any(|(p, _)| *p == name) {
                    return if covariant {
                        Ty::Named {
                            name: self.intern("Object"),
                            nullable: true,
                            args: ArgListId::EMPTY,
                        }
                    } else {
                        Ty::Never
                    };
                }
                if args == ArgListId::EMPTY {
                    return ty;
                }
                // Materialize the argument list BEFORE recursing — the
                // recursion re-interns through the same `args` pool
                // (`intern_list`/`function_generic` take `borrow_mut`), and a
                // read borrow held across it panics the RefCell.
                let list: Vec<Ty> = self.boundary.args.borrow().get(args).to_vec();
                let list: Vec<Ty> = list
                    .iter()
                    .map(|&arg| self.infer_closure(arg, params, covariant))
                    .collect();
                Ty::Named {
                    name,
                    nullable,
                    args: self.boundary.intern_list(&list),
                }
            }
            Ty::Function { shape, nullable } => {
                let pool_shape = {
                    let pool = self.boundary.pool();
                    pool.function_shape(shape).clone()
                };
                let return_ty = self.infer_closure(pool_shape.return_ty, params, covariant);
                let positional: Vec<Ty> = pool_shape
                    .positional
                    .iter()
                    .map(|&t| self.infer_closure(t, params, !covariant))
                    .collect();
                let named: Vec<(StringId, Ty)> = pool_shape
                    .named
                    .iter()
                    .map(|&(n, t)| (n, self.infer_closure(t, params, !covariant)))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .function_generic(
                        &pool_shape.type_params_named,
                        return_ty,
                        &positional,
                        &named,
                        nullable,
                    )
            }
            Ty::Record { shape, nullable } => {
                let pool_shape = {
                    let pool = self.boundary.pool();
                    pool.record_shape(shape).clone()
                };
                let positional: Vec<Ty> = pool_shape
                    .positional
                    .iter()
                    .map(|&t| self.infer_closure(t, params, covariant))
                    .collect();
                let named: Vec<(StringId, Ty)> = pool_shape
                    .named
                    .iter()
                    .map(|&(n, t)| (n, self.infer_closure(t, params, covariant)))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, nullable)
            }
            _ => ty,
        }
    }

    /// `P <: Q` constraint generation over the inference parameters —
    /// `performSubtypeConstraintGenerationInternal` (upstream
    /// `type_analyzer_operations.dart:2196-2441`). `round` receives each
    /// implied constraint as `(param_index, lower, upper)`; the caller
    /// merges them into the round's per-param `(lower, upper)` pair.
    /// `_` (`Ty::Unknown`) on either side, an `isQuestionType` P
    /// occurrence, or a non-member parameter short-circuits without a
    /// constraint. The Q-side parameter arm carries the bound-gate
    /// (`inference_using_bounds`, default on): `P <: T` registers only
    /// when `T` declares no bound or `P <: greatestClosure(bound)` — the
    /// gate that keeps a failed-bound argument silent (the constructor
    /// `C(myF)` face, where `NotA` never reaches `A`'s closure).
    fn infer_gather(
        &self,
        p: Ty,
        q: Ty,
        params: &[(StringId, Option<Ty>)],
        left_schema: bool,
        round: &mut Vec<(usize, Ty, Ty)>,
    ) -> bool {
        let param_index = |name: StringId| params.iter().position(|(param, _)| *param == name);
        let push = |round: &mut Vec<(usize, Ty, Ty)>, index: usize, lower: Ty, upper: Ty| {
            round.push((index, lower, upper));
        };
        // `_` shorts: a hole on either side matches anything.
        if p == Ty::Unknown || q == Ty::Unknown {
            return true;
        }
        // P is an inference parameter → `Q <: P` upper.
        if let Ty::Named {
            name,
            nullable: false,
            args,
        } = p
            && args == ArgListId::EMPTY
            && let Some(index) = param_index(name)
        {
            push(round, index, Ty::Unknown, q);
            return true;
        }
        // Q is an inference parameter → `P <: Q` lower, with the bound gate.
        if let Ty::Named {
            name,
            nullable: false,
            args,
        } = q
            && args == ArgListId::EMPTY
            && let Some(index) = param_index(name)
        {
            let gate_passes = match params[index].1 {
                None => true,
                Some(bound) => {
                    let closed = self.infer_closure(bound, params, true);
                    self.assignable(p, closed, false) == Some(true)
                }
            };
            if gate_passes {
                push(round, index, p, Ty::Unknown);
            }
            return true;
        }
        if p == q {
            return true;
        }
        match (p, q) {
            (
                Ty::Named {
                    name: p_head,
                    args: p_args,
                    ..
                },
                Ty::Named {
                    name: q_head,
                    args: q_args,
                    ..
                },
            ) => {
                let pool = self.boundary.args.borrow();
                let (p_args, q_args) = (pool.get(p_args).to_vec(), pool.get(q_args).to_vec());
                drop(pool);
                if p_head == q_head {
                    if p_args.len() != q_args.len() {
                        return false;
                    }
                    // Covariant argwise (the parser drops the written
                    // variance token, so `inout` faces read covariant —
                    // under-report against a double-constraint oracle).
                    for (&pa, &qa) in p_args.iter().zip(q_args.iter()) {
                        if !self.infer_gather(pa, qa, params, left_schema, round) {
                            return false;
                        }
                    }
                    return true;
                }
                // Different heads: the P-side climbs its super-interface
                // chain to the Q head and substitutes (`asInstanceOf`).
                let Some(inst) = self.super_instantiation(p_head, q_head) else {
                    return false;
                };
                if inst.len() != q_args.len() {
                    return false;
                }
                let map: Vec<(StringId, Ty)> = self
                    .bounds
                    .container_params(p_head)
                    .map(|capture| {
                        capture
                            .iter()
                            .map(|(param, _, _)| *param)
                            .zip(inst.iter().copied())
                            .collect()
                    })
                    .unwrap_or_default();
                // `p_args`/`q_args` already materialized above — the map
                // substitutes the p-side's own parameters through the
                // super-instantiation.
                let p_inst_args: Vec<Ty> = p_args
                    .iter()
                    .map(|&arg| self.subst_params(arg, &map).unwrap_or(arg))
                    .collect();
                for (&pa, &qa) in p_inst_args.iter().zip(q_args.iter()) {
                    if !self.infer_gather(pa, qa, params, left_schema, round) {
                        return false;
                    }
                }
                true
            }
            (
                Ty::Function {
                    shape: p_shape, ..
                },
                Ty::Function {
                    shape: q_shape, ..
                },
            ) => {
                let (p_shape, q_shape) = {
                    let pool = self.boundary.pool();
                    (
                        pool.function_shape(p_shape).clone(),
                        pool.function_shape(q_shape).clone(),
                    )
                };
                // Return covariant: `P.ret <: Q.ret`.
                if !self.infer_gather(p_shape.return_ty, q_shape.return_ty, params, left_schema, round)
                {
                    return false;
                }
                // Positional formals contravariant: `Q.arg <: P.arg`.
                for (&qa, &pa) in q_shape
                    .positional
                    .iter()
                    .zip(p_shape.positional.iter())
                {
                    if !self.infer_gather(qa, pa, params, !left_schema, round) {
                        return false;
                    }
                }
                // Named formals by name, contravariant; a missing pair fails.
                for &(qn, qt) in q_shape.named.iter() {
                    let Some(&(_, pt)) = p_shape.named.iter().find(|&&(pn, _)| pn == qn) else {
                        return false;
                    };
                    if !self.infer_gather(qt, pt, params, !left_schema, round) {
                        return false;
                    }
                }
                true
            }
            (
                Ty::Record {
                    shape: p_shape, ..
                },
                Ty::Record {
                    shape: q_shape, ..
                },
            ) => {
                let (p_shape, q_shape) = {
                    let pool = self.boundary.pool();
                    (
                        pool.record_shape(p_shape).clone(),
                        pool.record_shape(q_shape).clone(),
                    )
                };
                for (&pa, &qa) in p_shape
                    .positional
                    .iter()
                    .zip(q_shape.positional.iter())
                {
                    if !self.infer_gather(pa, qa, params, left_schema, round) {
                        return false;
                    }
                }
                for &(qn, qt) in q_shape.named.iter() {
                    let Some(&(_, pt)) = p_shape.named.iter().find(|&&(pn, _)| pn == qn) else {
                        return false;
                    };
                    if !self.infer_gather(pt, qt, params, left_schema, round) {
                        return false;
                    }
                }
                true
            }
            // A non-inference pair decides by the ordinary subtype verdict —
            // the constraint-generator's trailing `isSubtypeOf` fallback
            // (a ground `int <: num` match contributes no constraint and
            // keeps the round's other constraints live; a ground failure
            // rewinds the whole round upstream — the `_tryMatchSubtypeOf`
            // `success` gate).
            _ => self.assignable(p, q, false) == Some(true),
        }
    }

    /// Merge one gathered round's raw constraints into the solver's
    /// per-parameter `(lower, upper)` list — `_squashConstraints`'s per-round
    /// form: `lower = LUB(lowers)`, `upper = GLB(uppers)`, one origin stamp
    /// per round.
    fn infer_lub(&self, a: Ty, b: Ty) -> Ty {
        if a == Ty::Unknown {
            return b;
        }
        if b == Ty::Unknown || a == b {
            return a;
        }
        if a == Ty::Never {
            return b;
        }
        if b == Ty::Never {
            return a;
        }
        // `dynamic`/`void` tops absorb; `Null` joins toward the nullable
        // partner (the nullability face `LUB(Null, T) = T?`).
        match (a, b) {
            (Ty::Dynamic, _) | (Ty::Void, _) => return a,
            (_, Ty::Dynamic) | (_, Ty::Void) => return b,
            (Ty::Null, other) | (other, Ty::Null) => {
                return match other {
                    Ty::Named {
                        name, args, ..
                    } => Ty::Named {
                        name,
                        nullable: true,
                        args,
                    },
                    _ => other,
                };
            }
            _ => {}
        }
        // Subtype-related pairs collapse to the wider side.
        if self.assignable(a, b, false) == Some(true) {
            return b;
        }
        if self.assignable(b, a, false) == Some(true) {
            return a;
        }
        // The numeric-builtin face (`LUB(int, double) = num`): `int` and
        // `double` both declare `extends num`, so the climb below answers
        // the same face — this arm stands when the clause capture has not
        // (or cannot) instantiate the pair, keeping the builtin numeric
        // triangle decidable on its own terms.
        {
            let num = self.intern("num");
            let int = self.intern("int");
            let double = self.intern("double");
            let numeric = |ty: Ty| {
                matches!(ty, Ty::Named { name, nullable: false, args, .. }
                    if args == ArgListId::EMPTY && (name == int || name == double || name == num))
            };
            if numeric(a) && numeric(b) {
                return Ty::named(num);
            }
        }
        // The declared-supertype climb: `LUB(int, double) = num` — walk
        // `a`'s declared clause chain for a head both instantiate into.
        match (a, b) {
            (
                Ty::Named {
                    name: a_head,
                    args: a_args,
                    ..
                },
                Ty::Named {
                    name: b_head,
                    args: b_args,
                    ..
                },
            ) => {
                let candidates: [StringId; 3] =
                    [self.intern("num"), self.intern("Object"), self.intern("Comparable")];
                for target in candidates {
                    let a_inst = if a_head == target {
                        Some(
                            self.boundary
                                .args
                                .borrow()
                                .get(a_args)
                                .to_vec(),
                        )
                    } else {
                        self.super_instantiation(a_head, target)
                    };
                    let b_inst = if b_head == target {
                        Some(
                            self.boundary
                                .args
                                .borrow()
                                .get(b_args)
                                .to_vec(),
                        )
                    } else {
                        self.super_instantiation(b_head, target)
                    };
                    let (Some(a_inst), Some(b_inst)) = (a_inst, b_inst) else {
                        continue;
                    };
                    if a_inst.len() != b_inst.len() {
                        continue;
                    }
                    let merged: Vec<Ty> = a_inst
                        .iter()
                        .zip(b_inst.iter())
                        .map(|(&x, &y)| self.infer_lub(x, y))
                        .collect();
                    return Ty::Named {
                        name: target,
                        nullable: false,
                        args: self.boundary.intern_list(&merged),
                    };
                }
            }
            _ => {}
        }
        // The function pair joins pointwise (return LUB, formals GLB —
        // the upstream `FunctionType` LUB's structural arms), shapes of
        // equal arity only.
        if let (
            Ty::Function {
                shape: a_shape, ..
            },
            Ty::Function {
                shape: b_shape, ..
            },
        ) = (a, b)
        {
            let (a_shape, b_shape) = {
                let pool = self.boundary.pool();
                (
                    pool.function_shape(a_shape).clone(),
                    pool.function_shape(b_shape).clone(),
                )
            };
            if a_shape.positional.len() == b_shape.positional.len()
                && a_shape.named.len() == b_shape.named.len()
            {
                let return_ty = self.infer_lub(a_shape.return_ty, b_shape.return_ty);
                let positional: Vec<Ty> = a_shape
                    .positional
                    .iter()
                    .zip(b_shape.positional.iter())
                    .map(|(&x, &y)| self.infer_glb(x, y))
                    .collect();
                let named: Vec<(StringId, Ty)> = a_shape
                    .named
                    .iter()
                    .filter_map(|&(n, x)| {
                        b_shape
                            .named
                            .iter()
                            .find(|&&(bn, _)| bn == n)
                            .map(|&(_, y)| (n, self.infer_glb(x, y)))
                    })
                    .collect();
                if named.len() == a_shape.named.len() {
                    return self.boundary.args.borrow_mut().function_generic(
                        &[],
                        return_ty,
                        &positional,
                        &named,
                        false,
                    );
                }
            }
        }
        // Unrelated non-nullable grounds join at `Object` (Dart's LUB of
        // unrelated classes is the shared supertype — `int`/`String` →
        // `Object`; `Object` itself when either side is already there).
        Ty::Named {
            name: self.intern("Object"),
            nullable: false,
            args: ArgListId::EMPTY,
        }
    }

    /// The meet half of the merge (`upper = GLB(uppers)`): `_` absorbs,
    /// the subtype-related pair keeps the narrower side, equal types
    /// collapse, an unrelated pair reads `Never` (`GLB(String, num)` —
    //  the `extends`-refine arm's `Never` the oracle prints).
    fn infer_glb(&self, a: Ty, b: Ty) -> Ty {
        if a == Ty::Unknown {
            return b;
        }
        if b == Ty::Unknown || a == b {
            return a;
        }
        if a == Ty::Never || b == Ty::Never {
            return Ty::Never;
        }
        if self.assignable(a, b, false) == Some(true) {
            return a;
        }
        if self.assignable(b, a, false) == Some(true) {
            return b;
        }
        Ty::Never
    }

    /// The instantiate-to-bounds fill (`type_system.dart`'s
    /// `instantiateTypeFormalsToBounds`): every parameter without a known
    /// solution falls to its bound, recursively — a bound naming another
    /// parameter resolves through that parameter's own seed, a cycle
    /// (`X extends Y`, `Y extends A<X>`) grounds the in-flight leaf to
    /// `Object?`, and an absent bound reads `dynamic` (the analyzer's
    /// default for `<T>`). Returns the solved vector in declaration
    /// order.
    fn infer_i2b(&self, params: &[(StringId, Option<Ty>)]) -> Vec<Ty> {
        fn seed(
            engine: &Engine<'_>,
            params: &[(StringId, Option<Ty>)],
            index: usize,
            out: &mut Vec<Option<Ty>>,
            visiting: &mut Vec<usize>,
        ) -> Ty {
            if let Some(ty) = out[index] {
                return ty;
            }
            if visiting.contains(&index) {
                // The cycle leaf (`X` inside `A<X>` while `X` resolves):
                // grounds to `Object?` exactly as the oracle's i2b
                // recursion break prints it.
                return Ty::Named {
                    name: engine.intern("Object"),
                    nullable: true,
                    args: ArgListId::EMPTY,
                };
            }
            visiting.push(index);
            let seed = match params[index].1 {
                None => Ty::Dynamic,
                Some(bound) => {
                    let map: Vec<(StringId, Ty)> = params
                        .iter()
                        .enumerate()
                        .map(|(other, (param, _))| {
                            let ty = if other == index {
                                Ty::Unknown
                            } else {
                                seed(engine, params, other, out, visiting)
                            };
                            (*param, ty)
                        })
                        .collect();
                    engine.subst_params(bound, &map).unwrap_or(bound)
                }
            };
            visiting.pop();
            out[index] = Some(seed);
            seed
        }
        let mut out: Vec<Option<Ty>> = vec![None; params.len()];
        let mut visiting: Vec<usize> = Vec::new();
        for index in 0..params.len() {
            seed(self, params, index, &mut out, &mut visiting);
        }
        out.into_iter().map(|slot| slot.unwrap_or(Ty::Dynamic)).collect()
    }

    /// One inference round's merged per-parameter constraint — upstream's
    /// `MergedTypeConstraint` (`lower <: T <: upper`, `_` = `Ty::Unknown`)
    /// stamped with the round's origin.
    fn infer_merge_round(
        &self,
        params: &[(StringId, Option<Ty>)],
        raw: &[(usize, Ty, Ty)],
        origin: usize,
        constraints: &mut Vec<Vec<InferConstraint>>,
        fixed: &[bool],
    ) {
        let mut lower: Vec<Ty> = vec![Ty::Unknown; params.len()];
        let mut upper: Vec<Ty> = vec![Ty::Unknown; params.len()];
        let mut touched: Vec<bool> = vec![false; params.len()];
        for &(index, l, u) in raw {
            lower[index] = self.infer_lub(lower[index], l);
            upper[index] = self.infer_glb(upper[index], u);
            touched[index] = true;
        }
        for index in 0..params.len() {
            if !touched[index] || fixed[index] {
                continue;
            }
            constraints[index].push(InferConstraint {
                lower: lower[index],
                upper: upper[index],
                origin,
            });
        }
    }

    /// The closure of a SCHEMA — `TypeSchemaEliminationVisitor` (upstream
    /// `type_system.dart`'s `greatestClosureOfSchema`): every covariant
    /// `_` becomes the top (`Object?`), every contravariant `_` the bottom
    /// (`Never`); the least closure swaps them. Unlike `infer_closure`
    /// (which eliminates the inference's own parameters), this eliminates
    /// the `_` holes a substituted bound carries.
    fn infer_schema_closure(&self, ty: Ty, greatest: bool) -> Ty {
        let object_q = || Ty::Named {
            name: self.intern("Object"),
            nullable: true,
            args: ArgListId::EMPTY,
        };
        match ty {
            Ty::Unknown => {
                // covariant position: greatest→top, least→bottom.
                if greatest { object_q() } else { Ty::Never }
            }
            Ty::Named {
                name,
                nullable,
                args,
            } if args != ArgListId::EMPTY => {
                // Same reentrancy guard as `infer_closure`: materialize the
                // list before the recursive fold re-interns through the pool.
                let list: Vec<Ty> = self.boundary.args.borrow().get(args).to_vec();
                let list: Vec<Ty> = list
                    .iter()
                    .map(|&arg| self.infer_schema_closure(arg, greatest))
                    .collect();
                Ty::Named {
                    name,
                    nullable,
                    args: self.boundary.intern_list(&list),
                }
            }
            Ty::Function { shape, nullable } => {
                let pool_shape = {
                    let pool = self.boundary.pool();
                    pool.function_shape(shape).clone()
                };
                let return_ty = self.infer_schema_closure(pool_shape.return_ty, greatest);
                let positional: Vec<Ty> = pool_shape
                    .positional
                    .iter()
                    .map(|&t| self.infer_schema_closure(t, !greatest))
                    .collect();
                let named: Vec<(StringId, Ty)> = pool_shape
                    .named
                    .iter()
                    .map(|&(n, t)| (n, self.infer_schema_closure(t, !greatest)))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .function_generic(
                        &pool_shape.type_params_named,
                        return_ty,
                        &positional,
                        &named,
                        nullable,
                    )
            }
            Ty::Record { shape, nullable } => {
                let pool_shape = {
                    let pool = self.boundary.pool();
                    pool.record_shape(shape).clone()
                };
                let positional: Vec<Ty> = pool_shape
                    .positional
                    .iter()
                    .map(|&t| self.infer_schema_closure(t, greatest))
                    .collect();
                let named: Vec<(StringId, Ty)> = pool_shape
                    .named
                    .iter()
                    .map(|&(n, t)| (n, self.infer_schema_closure(t, greatest)))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, nullable)
            }
            _ => ty,
        }
    }

    /// `_squashConstraints` for one parameter: `lower = LUB(lowers)`,
    /// `upper = GLB(uppers)` across its recorded rounds.
    fn infer_squash(&self, cs: &[InferConstraint]) -> (Ty, Ty) {
        let mut lower = Ty::Unknown;
        let mut upper = Ty::Unknown;
        for c in cs {
            upper = self.infer_glb(upper, c.upper);
            lower = self.infer_lub(lower, c.lower);
        }
        (lower, upper)
    }

    /// `chooseTypeFromConstraint` (upstream
    /// `type_analyzer_operations.dart:1015-1074`): prefer the known side —
    /// lower first for the covariant parameter, upper under a
    /// contravariant parameter — then the partially-known side closed
    /// under `grounded` (least closure of a lower schema, greatest of an
    /// upper), else `_`.
    fn infer_choose_from(
        &self,
        lower: Ty,
        upper: Ty,
        params: &[(StringId, Option<Ty>)],
        grounded: bool,
    ) -> Ty {
        let _ = params;
        if self.infer_known(lower) {
            return lower;
        }
        if self.infer_known(upper) {
            return upper;
        }
        if lower != Ty::Unknown {
            return if grounded {
                self.infer_schema_closure(lower, false)
            } else {
                lower
            };
        }
        if upper != Ty::Unknown {
            return if grounded {
                self.infer_schema_closure(upper, true)
            } else {
                upper
            };
        }
        Ty::Unknown
    }

    /// The invocation's downward-context type: a declaration's written
    /// annotation or an assignment target's LHS type — the
    /// `contextReturnType` `setupGenericTypeInference` carries. `_`
    /// (`Ty::Unknown`) when the send infers context-free.
    fn infer_send_context(&mut self, callee: usize) -> Ty {
        if let Some(ctx) = self.pending_decl_ty
            && ctx != Ty::Unknown
        {
            return ctx;
        }
        self.assignment_write_ctx(callee)
            .filter(|ctx| *ctx != Ty::Unknown)
            .unwrap_or(Ty::Unknown)
    }

    /// The constraint solver's verdict for one invocation — the real
    /// `GenericInferrer` pipeline (upstream `generic_inferrer.dart`'s
    /// `tryChooseFinalTypes` + `_checkArgumentsNotMatchingBounds`, and
    /// `invocation_inferrer.dart`'s setup → args → final sequence):
    ///
    /// 1. `constrainReturnType` — the declared return against the
    ///    send's downward context (declaration annotation or assignment
    ///    target), `leftSchema` covariant.
    /// 2. `choosePreliminaryTypes` — per-parameter context-only choice;
    ///    a known result fixes the parameter (`_typesInferredSoFar`),
    ///    and later argument constraints on it drop silently — the
    ///    oracle's `String v = id2(3)` face (the fixed `String` never
    ///    reports `couldNotInfer`, only `argument_type_not_assignable`).
    /// 3. `constrainArgument` per argument (with the bound-gate on every
    ///    `P <: T` lower) — plus `constrainGenericFunctionInContext`
    ///    for a generic tear-off argument (`inferTearOff`).
    /// 4. `tryChooseFinalTypes` — squash, choose (bound-merge +
    ///    extends-merge inside `inferTypeParameterFromAll`), then
    ///    per-parameter `isSatisfiedBy` over its recorded rounds and the
    ///    substituted extends clause; the first failing parameter emits
    ///    `couldNotInfer`'s Tried-to-infer form at the callee name.
    /// 5. No failures → `_checkArgumentsNotMatchingBounds` — each
    ///    i2b-completed argument against its substituted bound emits
    ///    the instantiated-form `couldNotInfer` instead.
    ///
    /// `unnamed_creation` suppresses the bare-creation extends-only
    /// failure — `check_creation_inference`'s i2b face already reported
    /// that exact row upstream (one `couldNotInfer` per parameter, never
    /// two). `recv`/`below` carry the qualified callee's receiver identity
    /// for the downward closure-formal re-bind (`DeferredClosureFormal`
    /// entries match on `callee_name` + receiver span + argument slot).
    ///
    /// Returns the solved argument vector; known entries also fill
    /// `solutions`'s still-open slots (the substituted-formals and return
    /// writes below read them — `_` never fills).
    #[allow(clippy::too_many_arguments)]
    fn infer_invocation(
        &mut self,
        callee: usize,
        callee_range: SourceRange,
        callee_spelling: StringId,
        recv: Option<Receiver>,
        below: Option<SourceRange>,
        params: &[(StringId, Option<Ty>)],
        formals: &Formals,
        level: &ArgLevel,
        return_face: Option<Ty>,
        unnamed_creation: bool,
        solutions: &mut [Option<Ty>],
    ) -> Vec<Ty> {
        let mut constraints: Vec<Vec<InferConstraint>> = vec![Vec::new(); params.len()];
        let mut origins: Vec<InferOrigin> = Vec::new();
        let mut fixed: Vec<bool> = vec![false; params.len()];
        let mut inferred_so_far: Vec<Ty> = vec![Ty::Unknown; params.len()];

        // Round 0 — the downward context (`constrainReturnType`, origin
        // ReturnType). The declared face folds its aliases first — the
        // oracle's printed `declared as` is the EXPANDED type (`F<T,T>`
        // spells `T Function(T)`).
        if let Some(declared) = return_face {
            let context = self.infer_send_context(callee);
            if context != Ty::Unknown {
                let declared = self.fold_alias_ty(declared);
                let context = self.fold_alias_ty(context);
                let origin = origins.len();
                origins.push(InferOrigin::ReturnType { declared, context });
                let mut raw = Vec::new();
                self.infer_gather(declared, context, params, true, &mut raw);
                self.infer_merge_round(params, &raw, origin, &mut constraints, &fixed);
            }
        }

        // Preliminary choose (`choosePreliminaryTypes`): context-only,
        // `grounded: false`; known results fix the parameter.
        for index in 0..params.len() {
            if inferred_so_far[index] != Ty::Unknown {
                continue;
            }
            let (lower, upper) = self.infer_squash(&constraints[index]);
            let chosen = self.infer_choose_param(
                params,
                index,
                lower,
                upper,
                &inferred_so_far,
                &mut constraints,
                &mut origins,
                false,
            );
            inferred_so_far[index] = chosen;
            if self.infer_known(chosen) {
                fixed[index] = true;
            }
        }

        // The argument rounds: each `constrainArgument` carries its own
        // Argument origin (`parameterType` = the written formal, the
        // oracle's `declared as` column). A generic tear-off argument
        // (the signature's own `<T>` group is non-empty) against a
        // function-typed formal runs `inferTearOff` instead — its own
        // inferrer on the tear-off's parameters, anchored at the
        // argument's node.
        let mut positional_index = 0usize;
        for (name, ty, _literal, range) in &level.args {
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
            // The tear-off/closure signature recovery the upward face
            // already runs (`arg_sigs` keyed by offset, `level.closures`
            // by containment) — reused here for both the argument's type
            // and the `inferTearOff` detection.
            let arg_sig: Option<Box<FnVarSig>> =
                self.arg_sigs.get(&range.offset).cloned().or_else(|| {
                    level
                        .closures
                        .iter()
                        .find(|(start, end, _)| {
                            range.offset >= *start && range.offset <= *end
                        })
                        .map(|(.., sig)| sig.clone())
                });
            let arg_ty = if *ty != Ty::Unknown {
                *ty
            } else {
                match &arg_sig {
                    Some(sig) => {
                        let mut fty = self.fn_sig_ty(sig);
                        // The literal closure's `void` STUB return is
                        // never a real inference — upstream's
                        // `staticType` reads the body's inferred return
                        // (`(double x) => 0` is `int Function(double)`,
                        // and the oracle's origin column prints THAT).
                        // The tear-off arm keeps its written signature.
                        if sig.return_ty == Ty::Void
                            && let Some((lit_start, lit_end)) = level
                                .closures
                                .iter()
                                .find(|(start, end, _)| {
                                    range.offset >= *start && range.offset <= *end
                                })
                                .map(|(start, end, _)| (*start, *end))
                            && let Some(ret) = self.literal_inferred_return(lit_start, lit_end)
                            && let Ty::Function {
                                shape: fn_shape,
                                nullable: fn_nullable,
                            } = fty
                        {
                            let shape = {
                                let pool = self.boundary.pool();
                                pool.function_shape(fn_shape).clone()
                            };
                            let positional: Vec<Ty> = shape.positional.to_vec();
                            let named: Vec<(StringId, Ty)> =
                                shape.named.iter().map(|(n, t)| (*n, *t)).collect();
                            fty = self.boundary.args.borrow_mut().function_generic(
                                &shape.type_params_named,
                                ret,
                                &positional,
                                &named,
                                fn_nullable,
                            );
                        }
                        fty
                    }
                    // #2516: the solve-only member-read answer — an
                    // element-bearing accessor/getter argument constrains
                    // the call's parameters through its resolved return,
                    // while `level.args` keeps `Unknown` for the verdict.
                    None => match self.solve_arg_member_ty(range.offset) {
                        Some(ty) => ty,
                        None => continue,
                    },
                }
            };
            if arg_ty == Ty::Unknown {
                continue;
            }
            // `inferFunctionTypeInstantiation`: a GENERIC argument's
            // signature infers against the formal's function context —
            // its own solve (origin FunctionContext), reported at the
            // argument's own range. The instantiated signature then feeds
            // the outer `constrainArgument` round exactly as upstream's
            // `inferTearOff` return value does — the generic argument's
            // chosen types constrain the call's own parameters.
            let tear_params = self.tear_off_arg_params(range.offset);
            if let Some(sig) = &arg_sig
                && !tear_params.as_ref().is_none_or(|p| p.is_empty())
                && let Ty::Function { .. } = self.fold_alias_ty(formal.ty)
            {
                let mut sig = (**sig).clone();
                if let Some(p) = tear_params {
                    sig.params = p;
                }
                let origin = origins.len();
                origins.push(InferOrigin::Argument {
                    parameter: formal.name,
                    declared: self.fold_alias_ty(formal.ty),
                    argument: arg_ty,
                });
                if let Some(instantiated) = self.infer_tear_off(&sig, formal.ty, *range) {
                    let mut raw = Vec::new();
                    self.infer_gather(instantiated, formal.ty, params, false, &mut raw);
                    self.infer_merge_round(params, &raw, origin, &mut constraints, &fixed);
                }
                continue;
            }
            let origin = origins.len();
            origins.push(InferOrigin::Argument {
                parameter: formal.name,
                declared: self.fold_alias_ty(formal.ty),
                argument: arg_ty,
            });
            let mut raw = Vec::new();
            self.infer_gather(arg_ty, formal.ty, params, false, &mut raw);
            self.infer_merge_round(params, &raw, origin, &mut constraints, &fixed);
        }

        // Final choose (`tryChooseFinalTypes`): the same per-parameter
        // solve, `grounded: true`, then the per-parameter satisfaction
        // check — a failing parameter emits the Tried-to-infer form.
        let mut inferred: Vec<Ty> = Vec::with_capacity(params.len());
        for index in 0..params.len() {
            let chosen = if fixed[index] && self.infer_known(inferred_so_far[index]) {
                inferred_so_far[index]
            } else {
                let (lower, upper) = self.infer_squash(&constraints[index]);
                self.infer_choose_param(
                    params,
                    index,
                    lower,
                    upper,
                    &inferred,
                    &mut constraints,
                    &mut origins,
                    true,
                )
            };
            inferred.push(chosen);
        }

        let mut reported = false;
        for index in 0..params.len() {
            let inferred_ty = inferred[index];
            let mut success = constraints[index]
                .iter()
                .all(|c| self.infer_satisfied(c, inferred_ty));
            if success && let Some(raw_bound) = params[index].1 {
                let map: Vec<(StringId, Ty)> = params
                    .iter()
                    .zip(inferred.iter())
                    .map(|((param, _), ty)| (*param, *ty))
                    .collect();
                let bound = self.subst_params(raw_bound, &map).unwrap_or(raw_bound);
                let origin = origins.len();
                origins.push(InferOrigin::ExtendsClause {
                    parameter: params[index].0,
                    raw_bound,
                    bound,
                });
                let extends = InferConstraint {
                    lower: Ty::Unknown,
                    upper: bound,
                    origin,
                };
                constraints[index].push(extends.clone());
                success = self.infer_satisfied(&extends, inferred_ty);
            }
            // Creation dedup: `check_creation_inference` already reported
            // the bare-creation bound-seed failure (the i2b face walks the
            // same extends arm). When this solve's ONLY unsatisfied
            // origins are ExtendsClauses, the seam's row is byte-identical
            // — emit nothing a second time (`B()` where `B<X extends B<X>>`
            // would otherwise double-report). The arm still counts as
            // reported: upstream sets `hasErrorReported` on ANY unsatisfied
            // parameter (generic_inferrer.dart `tryChooseFinalTypes`) and
            // gates `_checkArgumentsNotMatchingBounds` on it, so the seam's
            // row must also silence the instantiated-bound twin.
            if !success
                && unnamed_creation
                && constraints[index].iter().all(|c| {
                    self.infer_satisfied(c, inferred_ty)
                        || matches!(origins[c.origin], InferOrigin::ExtendsClause { .. })
                })
            {
                reported = true;
                continue;
            }

            if !success {
                let detail = self.infer_format_error(params[index].0, inferred_ty, &constraints[index], &origins);
                let parameter = self.strings.resolve(params[index].0).to_string();
                let message = SemanticCode::CouldNotInfer.message(&parameter, &detail);
                self.emit_message_with_correction(SemanticCode::CouldNotInfer, callee_range, message);
                reported = true;
            }
        }

        if !reported {
            // `_checkArgumentsNotMatchingBounds`: the i2b-completed
            // arguments against each parameter's substituted bound —
            // the instantiated-form report, anchored identically.
            let result = self.infer_i2b_full(params, &inferred);
            for index in 0..params.len() {
                let Some(raw_bound) = params[index].1 else {
                    continue;
                };
                let argument = result[index];
                let map: Vec<(StringId, Ty)> = params
                    .iter()
                    .zip(result.iter())
                    .map(|((param, _), ty)| (*param, *ty))
                    .collect();
                let bound = self.subst_params(raw_bound, &map).unwrap_or(raw_bound);
                if self.assignable(argument, bound, false) == Some(true) {
                    continue;
                }
                let argument_text = self.ty_display(argument);
                let bound_text = self.ty_display(bound);
                let raw_text = self.ty_display(raw_bound);
                let list: Vec<String> = result.iter().map(|&ty| self.ty_display(ty)).collect();
                let detail = format!(
                    "\n'{argument_text}' doesn't conform to the bound '{bound_text}', \
                     instantiated from '{raw_text}' using type arguments [{}].",
                    list.join(", ")
                );
                let parameter = self.strings.resolve(params[index].0).to_string();
                let message = SemanticCode::CouldNotInfer.message(&parameter, &detail);
                self.emit_message_with_correction(
                    SemanticCode::CouldNotInfer,
                    callee_range,
                    message,
                );
            }
        }

        // Solved parameters fill the still-open solution slots — the
        // substituted-formals and return writes below read them
        // (`_typesInferredSoFar`'s winners alone; `_` never fills).
        for (index, slot) in solutions.iter_mut().enumerate() {
            if slot.is_none()
                && index < inferred.len()
                && self.infer_known(inferred[index])
            {
                *slot = Some(inferred[index]);
            }
        }
        inferred
    }

    /// `inferTypeParameterFromContext`/`inferTypeParameterFromAll`'s
    /// shared body: `chooseTypeFromConstraint` over the squashed
    /// `(lower, upper)`, then — when the raw choice is known — the
    /// bound-merge (`lower <: bound` gathers constraints into the OTHER
    /// parameters' sets) and the extends-clause merge (the substituted
    /// bound joins the parameter's upper). `preliminary` selects the
    /// non-grounded choose arm.
    #[allow(clippy::too_many_arguments)]
    fn infer_choose_param(
        &mut self,
        params: &[(StringId, Option<Ty>)],
        index: usize,
        lower: Ty,
        upper: Ty,
        inferred_prior: &[Ty],
        constraints: &mut Vec<Vec<InferConstraint>>,
        origins: &mut Vec<InferOrigin>,
        preliminary: bool,
    ) -> Ty {
        let grounded = !preliminary;
        let chosen = self.infer_choose_from(lower, upper, params, grounded);
        if preliminary && !self.infer_known(chosen) {
            return chosen;
        }
        let mut lower = lower;
        let mut upper = upper;
        // `mergeInConstraintsFromBound`: only when a real lower and a
        // declared bound both stand — `lower <: bound` gathers fresh
        // constraints (into `constraints`, origin ExtendsClause over
        // THIS parameter's bound), and its own returned constraint
        // merges back into this parameter's.
        if lower != Ty::Unknown
            && let Some(raw_bound) = params[index].1
        {
            let origin = origins.len();
            origins.push(InferOrigin::ExtendsClause {
                parameter: params[index].0,
                raw_bound,
                bound: raw_bound,
            });
            let mut raw = Vec::new();
            // The bound's OWN spelling matches against the lower — the
            // raw declared face (`T1 Function(T1)`), not the closure.
            self.infer_gather(lower, raw_bound, params, true, &mut raw);
            let mut lower_new = Ty::Unknown;
            let mut upper_new = Ty::Unknown;
            for &(target, l, u) in &raw {
                lower_new = self.infer_lub(lower_new, l);
                upper_new = self.infer_glb(upper_new, u);
                if target == index {
                    continue;
                }
                // Cross-parameter propagation: the bound's constraint on
                // ANOTHER parameter merges into that parameter's set
                // (`f(C())` → `B` through `X extends A<X>`).
                constraints[target].push(InferConstraint {
                    lower: l,
                    upper: u,
                    origin,
                });
            }
            // The bound's self-constraint merges back (an ExtendsClause-
            // stamped bound face on the parameter itself).
            lower = self.infer_lub(lower, lower_new);
            upper = self.infer_glb(upper, upper_new);
        }
        // The extends-clause merge: the bound substituted with the
        // already-chosen parameters joins the upper (`Func1<_,_>` under
        // an unchosen sibling).
        if let Some(raw_bound) = params[index].1 {
            let map: Vec<(StringId, Ty)> = params
                .iter()
                .enumerate()
                .map(|(other, (param, _))| {
                    let ty = inferred_prior
                        .get(other)
                        .copied()
                        .unwrap_or(Ty::Unknown);
                    (*param, ty)
                })
                .collect();
            let bound = self.subst_params(raw_bound, &map).unwrap_or(raw_bound);
            upper = self.infer_glb(upper, bound);
        }
        self.infer_choose_from(lower, upper, params, grounded)
    }

    /// `MergedTypeConstraint::isSatisfiedBy`: `inferred <: upper` AND
    /// `lower <: inferred` — a `_` side accepts everything. The ordinary
    /// subtype verdict answers both; an undecidable pair reads satisfied
    /// (the under-report posture — a wrongly-decided `false` invents a
    /// `couldNotInfer` the oracle never printed).
    fn infer_satisfied(&self, c: &InferConstraint, inferred: Ty) -> bool {
        let upper_ok = c.upper == Ty::Unknown
            || self.assignable(inferred, c.upper, false) != Some(false);
        let lower_ok = c.lower == Ty::Unknown
            || self.assignable(c.lower, inferred, false) != Some(false);
        // The `Object` fallback's own face: the merged answer `Object`
        // satisfies only `Object`/`Object?`/`dynamic`-headed uppers — a
        // concrete head's ask is decidable-false even when the head's
        // supertype chain cannot complete in this model's scope (the
        // names-only `Object` stub has no `double` climb).
        if upper_ok
            && let (
                Ty::Named { name: i_head, nullable: false, .. },
                Ty::Named {
                    name: u_head,
                    nullable: u_nullable,
                    ..
                },
            ) = (inferred, c.upper)
            && i_head == self.intern("Object")
            && u_head != i_head
            && !u_nullable
        {
            return false;
        }
        upper_ok && lower_ok
    }

    /// `_formatError` (upstream `generic_inferrer.dart:538-585`): the
    /// Tried-to-infer intro, the unsatisfied origins block, then "The
    /// type 'I' was inferred from:" + the satisfied block — each block
    /// the `_formatConstraints` column layout (prefix padded to the
    /// widest, the `end` part on a second indented line), duplicate
    /// lines deduped.
    fn infer_format_error(
        &self,
        parameter: StringId,
        inferred: Ty,
        constraints: &[InferConstraint],
        origins: &[InferOrigin],
    ) -> String {
        let inferred_text = self.ty_display(inferred);
        let parameter_name = self.strings.resolve(parameter).to_string();
        // Group constraints by origin; an origin is unsatisfied when ANY
        // of its constraints fails the inferred type.
        let mut order: Vec<usize> = Vec::new();
        let mut satisfied: Vec<bool> = Vec::new();
        for c in constraints {
            match order.iter().position(|&o| o == c.origin) {
                Some(slot) => {
                    satisfied[slot] =
                        satisfied[slot] && self.infer_satisfied(c, inferred);
                }
                None => {
                    order.push(c.origin);
                    satisfied.push(self.infer_satisfied(c, inferred));
                }
            }
        }
        let format_lines = |want: bool| -> String {
            let parts: Vec<Vec<String>> = order
                .iter()
                .zip(satisfied.iter())
                .filter(|&(_, s)| *s == want)
                .map(|(&o, _)| origins[o].format_error(self))
                .collect();
            let prefix_max = parts
                .iter()
                .map(|p| p.first().map(|s| s.len()).unwrap_or(0))
                .max()
                .unwrap_or(0);
            let mut seen: Vec<String> = Vec::new();
            for p in parts {
                let prefix = p.first().cloned().unwrap_or_default();
                let middle = p.get(1).cloned().unwrap_or_default();
                let mut line = format!(
                    "  {prefix}{} {middle}",
                    " ".repeat(prefix_max.saturating_sub(prefix.len()))
                );
                if let Some(end) = p.get(2) {
                    line.push_str(&format!("\n  {} {end}", " ".repeat(prefix_max)));
                }
                if !seen.contains(&line) {
                    seen.push(line);
                }
            }
            seen.join("\n")
        };
        let unsatisfied = format_lines(false);
        let satisfied_text = format_lines(true);
        let mut detail = format!(
            "\n\nTried to infer '{inferred_text}' for '{parameter_name}' which doesn't work:\n{unsatisfied}"
        );
        if !satisfied_text.is_empty() {
            detail.push_str(&format!(
                "\n\nThe type '{inferred_text}' was inferred from:\n{satisfied_text}"
            ));
        }
        detail.push_str("\n\nConsider passing explicit type argument(s) to the generic.\n\n");
        detail
    }

    /// The generic tear-off argument's own solve
    /// (`inferFunctionTypeInstantiation` → `constrainGenericFunctionInContext`
    /// — `invocation_inference_helper.dart:122`): the argument's signature
    /// minus its type-parameter group infers against the formal's context
    /// type; a failure reports `couldNotInfer` at the argument with the
    /// FunctionContext origin. Returns the signature's chosen instantiation
    /// — upstream feeds it into the outer `constrainArgument` round as the
    /// argument's type (`inferTearOff`'s return), `None` only when the
    /// signature never was a function type.
    fn infer_tear_off(&mut self, sig: &FnVarSig, context_ty: Ty, range: SourceRange) -> Option<Ty> {
        let context = self.fold_alias_ty(context_ty);
        let fn_ty = self.fn_sig_ty(sig);
        let params: Vec<(StringId, Option<Ty>)> = sig.params.to_vec();
        // The context strips its own `<T>` group upstream only when the
        // CONTEXT is generic — ours never is here (the formal's written
        // type). `constrainGenericFunctionInContext` rebuilds the fn type
        // WITHOUT its type formals (`inferFnType`): the written
        // parameters/formals still spell `T`, and the gather's Q-side
        // parameter arm matches them through `params` — so the
        // context-side leaf binds lowers, the fn-side leaf binds uppers.
        let mut constraints: Vec<Vec<InferConstraint>> = vec![Vec::new(); params.len()];
        let mut origins = vec![InferOrigin::FunctionContext {
            function: fn_ty,
            context,
        }];
        let mut raw = Vec::new();
        let infer_fn = {
            let pool_shape = {
                let pool = self.boundary.pool();
                match fn_ty {
                    Ty::Function { shape, .. } => pool.function_shape(shape).clone(),
                    _ => return None,
                }
            };
            // Re-intern the shape without its own param group —
            // `inferFnType`'s `typeParameters: const []` fold: the
            // parts keep their `T` spellings (they ARE the schema
            // holes the gather matches).
            self.boundary
                .args
                .borrow_mut()
                .function_generic(
                    &[],
                    pool_shape.return_ty,
                    &pool_shape.positional,
                    &pool_shape.named,
                    matches!(fn_ty, Ty::Function { nullable: true, .. }),
                )
        };
        // `fnType <: contextType` covariant — the fn type's pieces are
        // the schema (`leftSchema: true` keeps `P`-side upper arms live
        // for the signature's own parameters).
        self.infer_gather(infer_fn, context, &params, true, &mut raw);
        let fixed = vec![false; params.len()];
        self.infer_merge_round(&params, &raw, 0, &mut constraints, &fixed);
        let mut inferred: Vec<Ty> = Vec::with_capacity(params.len());
        for index in 0..params.len() {
            let (lower, upper) = self.infer_squash(&constraints[index]);
            inferred.push(self.infer_choose_param(
                &params,
                index,
                lower,
                upper,
                &inferred,
                &mut constraints,
                &mut origins,
                false,
            ));
        }
        let mut reported = false;
        for index in 0..params.len() {
            let ok = constraints[index]
                .iter()
                .all(|c| self.infer_satisfied(c, inferred[index]));
            if !ok {
                let detail = self.infer_format_error(
                    params[index].0,
                    inferred[index],
                    &constraints[index],
                    &origins,
                );
                let parameter = self.strings.resolve(params[index].0).to_string();
                let message = SemanticCode::CouldNotInfer.message(&parameter, &detail);
                self.emit_message_with_correction(SemanticCode::CouldNotInfer, range, message);
                reported = true;
            }
        }
        let _ = reported;
        // The instantiated signature the outer `constrainArgument` consumes:
        // the parameter-free shape with the solve's choices substituted —
        // upstream's `inferTearOff` return value. `_` leftovers substitute
        // nothing (a failed or partial solve keeps the schema leaf, which
        // the outer gather matches no constraint against).
        let map: Vec<(StringId, Ty)> = params
            .iter()
            .zip(inferred.iter())
            .map(|((param, _), ty)| (*param, *ty))
            .collect();
        self.subst_params(infer_fn, &map)
    }

    /// The downward closure-formal re-bind's scan half (#2500): every
    /// literal-closure argument whose signature carries untyped formals
    /// re-derives the callee formal's substituted function-shape entry —
    /// the solver's downward answer the pass-1 `closure_param_context_ty`
    /// could not count — and writes `rebound_locals` for the matching
    /// `DeferredClosureFormal`. Runs for any send with a receiver (a
    /// generic or an inherited-non-generic member): `NodeList<Annotation>
    /// .where((annotation) => …)` needs only the receiver env (no callee
    /// parameters), which `closure_callee_params` supplies through the
    /// `member_recv_env` climb. `solutions` carries the callee's own
    /// inference merge when the send had parameters.
    fn infer_rebind_closure_formals(
        &mut self,
        callee_spelling: StringId,
        recv: Option<Receiver>,
        below: Option<SourceRange>,
        params: &[(StringId, Option<Ty>)],
        solutions: &[Option<Ty>],
        level: &ArgLevel,
        formals: &Formals,
    ) {
        let Some(recv) = recv else {
            return;
        };
        if self.deferred_closure_formals.is_empty() {
            return;
        }
        let mut map: Vec<(StringId, Ty)> = params
            .iter()
            .zip(solutions.iter())
            .filter_map(|((param, _), solution)| solution.map(|ty| (*param, ty)))
            .collect();
        if let Some((_, seeds, _)) = self.closure_callee_params(&recv, callee_spelling) {
            for (name, ty) in seeds {
                if !map.iter().any(|(param, _)| *param == name) {
                    map.push((name, ty));
                }
            }
        }
        let mut positional_index = 0usize;
        for (name, _, _literal, range) in &level.args {
            let formal = if let Some(n) = name {
                self.named_formal_for(formals, *n)
            } else {
                let formal = formals.positional.get(positional_index);
                positional_index += 1;
                formal
            };
            let slot = match name {
                Some(n) => SchemaSlot::Named(*n),
                None => SchemaSlot::Positional(positional_index.saturating_sub(1)),
            };
            let Some(formal) = formal else {
                continue;
            };
            // Only literal closures with untyped formals need the re-bind —
            // `arg_sigs`/`level.closures` recover the literal's written
            // signature; `Unknown` slots are the capture's open spots the
            // downward answer fills.
            let arg_sig: Option<Box<FnVarSig>> =
                self.arg_sigs.get(&range.offset).cloned().or_else(|| {
                    level
                        .closures
                        .iter()
                        .find(|(start, end, _)| {
                            range.offset >= *start && range.offset <= *end
                        })
                        .map(|(.., sig)| sig.clone())
                });
            let Some(sig) = arg_sig else {
                continue;
            };
            if !sig.params.is_empty() {
                continue;
            }
            let needs = sig.positional.iter().any(|&t| t == Ty::Unknown)
                || sig.named.iter().any(|&(_, t, _)| t == Ty::Unknown);
            if !needs {
                continue;
            }
            let formal_ty = self.fold_alias_ty(formal.ty);
            let substituted = self.subst_params(formal_ty, &map).unwrap_or(formal_ty);
            let Ty::Function { shape, .. } = substituted else {
                continue;
            };
            let shape = {
                let pool = self.boundary.pool();
                pool.function_shape(shape).clone()
            };
            for (index, &written) in sig.positional.iter().enumerate() {
                if written != Ty::Unknown {
                    continue;
                }
                let Some(&entry) = shape.positional.get(index) else {
                    continue;
                };
                if !self.infer_known(entry) {
                    continue;
                }
                self.infer_bind_closure_formal(
                    callee_spelling,
                    below,
                    slot,
                    SchemaSlot::Positional(index),
                    entry,
                );
            }
            for &(label, written, _) in sig.named.iter() {
                if written != Ty::Unknown {
                    continue;
                }
                let Some(&(_, entry)) = shape.named.iter().find(|&&(n, _)| n == label) else {
                    continue;
                };
                if !self.infer_known(entry) {
                    continue;
                }
                self.infer_bind_closure_formal(
                    callee_spelling,
                    below,
                    slot,
                    SchemaSlot::Named(label),
                    entry,
                );
            }
        }
    }

    /// The literal closure's inferred return for the constraint gather
    /// (#2516): `closure_capture_end` now bakes the `=>` body's resolved
    /// expression type into `FnVarSig.return_ty` (the body's last
    /// completed row still sits on the stack at capture close — the
    /// listener drains it only after). A BLOCK body's captured return
    /// stays the `void` STUB and keeps arriving here; upstream's
    /// `staticType` reads the body's inferred return (`(double x) => 0`
    /// is `int Function(double)`, and the oracle's origin column prints
    /// THAT), so a surviving stub still re-reads the tail. `None` keeps
    /// the stub.
    fn literal_inferred_return(&self, start: u32, end: u32) -> Option<Ty> {
        // The body's expression row, when it survived the fold (`=> expr`
        // literals leave the expression's own row inside the literal
        // span); a primitive literal token is typed by its lexeme.
        if let Some(row) = self
            .rows
            .iter()
            .rev()
            .find(|row| row.range.offset >= start && row.range.end() <= end
                && matches!(row.intent, Intent::Read | Intent::Invoke | Intent::Operator(..)))
        {
            let ty = self.expression_row_ty(row);
            if ty != Ty::UNKNOWN {
                return Some(ty);
            }
        }
        // The folded literal: the `=>`-tail lexeme answers the primitive
        // face (`=> 0` infers `int`, `=> 'x'` `String`, `=> true` `bool`)
        // — upstream's `staticType` for the trivial expression bodies
        // the constraint origin column prints verbatim.
        // The `level.closures` (start, end) pair ends at the literal's
        // `=>`/head boundary — the body sits AFTER it. Scan forward from
        // the `=>` to the argument-list separator (`)`/`,`/`;` at paren
        // depth 0) and classify the primitive lexeme.
        let start = self.byte_of(start);
        let text = self.source.get(start..)?;
        
        let tail = text.split("=>").nth(1)?.trim_start();
        let ty = if tail.starts_with('\'') || tail.starts_with('"')
            || tail.starts_with("r'") || tail.starts_with("r\"")
        {
            self.intern("String")
        } else {
            let head = tail
                .split(|c: char| !(c.is_alphanumeric() || c == '.' || c == '_'))
                .next()?;
            if head.parse::<i64>().is_ok() {
                self.intern("int")
            } else if head.parse::<f64>().is_ok() {
                self.intern("double")
            } else if head == "true" || head == "false" {
                self.intern("bool")
            } else {
                return None;
            }
        };
        Some(Ty::named(ty))
    }

    /// The generic tear-off argument's own `<T>` group, recovered from
    /// the argument row's spelled callee — `tear_off_sig` keeps
    /// `params` empty on purpose (every other consumer reads the
    /// signature's identity, and a non-empty group would re-route them
    /// as inference subjects), so the `inferTearOff` solve looks the
    /// declaration up by name: `type_param_names` for the spellings, the
    /// `exec_bounds` window for any paired declared bound the oracle's
    /// FunctionContext origin spells.
    fn tear_off_arg_params(&self, offset: u32) -> Option<Box<[(StringId, Option<Ty>)]>> {
        let row = self.rows.iter().find(|row| row.range.offset == offset)?;
        let decl = self.top_level_decl(row.name)?;
        if decl.type_param_names.is_empty() {
            return None;
        }
        let bounds = self.exec_bounds.get(&row.name);
        Some(
            decl.type_param_names
                .iter()
                .map(|name| {
                    let bound = bounds
                        .and_then(|window| window.iter().find(|(param, _)| param == name))
                        .and_then(|(_, bound)| *bound);
                    (*name, bound)
                })
                .collect(),
        )
    }

    /// #2516 — the solve-only argument type: a member-read argument whose
    /// merged Value row kept `Unknown` at capture (`flush_argument` types
    /// the multi-row window `Unknown` by design — the assignability
    /// verdict must keep that posture) still owes the constraint gather
    /// its accessor answer. Reads `member_reads` (offset-sorted), rebuilds
    /// the read's row like `apply_deferred_rebinds` does, and re-derives
    /// its static type — consumed ONLY by `infer_invocation`'s
    /// `constrainArgument` rounds and the `unify_upward` pass in
    /// `check_arguments`, never by `level.args`, so the verdict face sees
    /// exactly the same `Unknown` it saw before (the prior
    /// `typed_argument_row` attempt regressed by lending the typed answer
    /// to the verdict). `None` keeps the under-report for any argument
    /// that is not a member read/send.
    fn solve_arg_member_ty(&mut self, offset: u32) -> Option<Ty> {
        let index = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&offset))
            .ok()?;
        let read = self.member_reads[index];
        let name = self.strings.resolve(read.name);
        let units: u32 = if name.is_ascii() {
            name.len() as u32
        } else {
            name.chars().map(char::len_utf16).sum::<usize>() as u32
        };
        let invoked = self.byte_at(read.offset + units) == Some(b'(');
        let row = Row {
            name: read.name,
            range: SourceRange {
                offset: read.offset,
                length: 0,
            },
            qualified: true,
            shape: RowShape::Member { recv: read.recv },
            intent: if invoked {
                Intent::Invoke
            } else {
                Intent::Read
            },
            write_op: None,
            ty: Ty::UNKNOWN,
        };
        let ty = if invoked {
            self.expression_row_ty(&row)
        } else {
            self.initializer_read_ty(&row)
        };
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// One closure formal's downward re-bind (#2500): the deferred entry
    /// the pass-1 schema face staged (`closure_param_context_ty` pushes a
    /// `DeferredClosureFormal` for every unsolved literal formal whose
    /// callee qualified) matches on the callee identity + receiver span +
    /// argument-list slot + the formal's own schema slot; the solved entry
    /// joins `rebound_locals` so the pass-2 read-type re-derive and the
    /// `unused_element` tally both see the real type.
    fn infer_bind_closure_formal(
        &mut self,
        callee_spelling: StringId,
        below: Option<SourceRange>,
        slot: SchemaSlot,
        formal: SchemaSlot,
        entry: Ty,
    ) {
        let Some(below) = below else {
            return;
        };
        let Some(index) = self
            .deferred_closure_formals
            .iter()
            .position(|d| {
                d.callee_name == callee_spelling
                    && d.below == below
                    && match (d.slot, slot) {
                        (SchemaSlot::Positional(a), SchemaSlot::Positional(b)) => a == b,
                        (SchemaSlot::Named(a), SchemaSlot::Named(b)) => a == b,
                        _ => false,
                    }
                    && match (d.formal, formal) {
                        (SchemaSlot::Positional(a), SchemaSlot::Positional(b)) => a == b,
                        (SchemaSlot::Named(a), SchemaSlot::Named(b)) => a == b,
                        _ => false,
                    }
            })
        else {
            return;
        };
        let d = &self.deferred_closure_formals[index];
        let (decl_offset, parse_ty) = (d.decl_offset, d.parse_ty);
        if parse_ty != Ty::Unknown && parse_ty == entry {
            return;
        }
        // The pass-2 re-bind keeps its own entry: when its schema re-derive
        // answers, its `narrows` gate may replace this write with the
        // deferred lane's own verdict (equal or narrower) — last writer
        // wins and both solve the member.
        self.rebound_locals.insert(decl_offset, (parse_ty, entry));
    }

    /// `instantiateTypeFormalsToBounds` over the inference result —
    /// `tryChooseFinalTypes`'s `result`: known inferred types stay, the
    /// `_` leftovers fill from the i2b seeds (the
    /// `_checkArgumentsNotMatchingBounds` argument vector).
    fn infer_i2b_full(
        &self,
        params: &[(StringId, Option<Ty>)],
        inferred: &[Ty],
    ) -> Vec<Ty> {
        let mut result: Vec<Ty> = inferred.to_vec();
        // i2b seeds only where inference left `_`.
        let need_seed: Vec<bool> = inferred
            .iter()
            .map(|&ty| !self.infer_known(ty))
            .collect();
        if !need_seed.iter().any(|&n| n) {
            return result;
        }
        // Reuse the raw i2b fixpoint for the open slots.
        let seeds = self.infer_i2b(params);
        for index in 0..params.len() {
            if need_seed[index] {
                result[index] = seeds[index];
            }
        }
        result
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
                list.iter()
                    .any(|&arg| self.formal_mentions_param(arg, param))
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
    pub(crate) fn foreign_container_params(&self, name: StringId) -> Option<Vec<StringId>> {
        let libs = self.graph.declaring_libraries(name);
        if libs.len() != 1 {
            return None;
        }
        let file = self.graph.unit_files(libs[0]).iter().copied().find(|&f| {
            self.index
                .get(f)
                .is_some_and(|unit| unit.container(name).is_some())
        })?;
        let names = self
            .index
            .get(file)?
            .container(name)?
            .type_param_names
            .to_vec();
        (!names.is_empty()).then_some(names)
    }

    /// The member send's inference parameters plus its declared
    /// bounds facts: `(inference params, receiver seeds, declared
    /// bounds)`. The bounds element is the method's own
    /// `type_param_names` paired with `type_param_bounds` — the facts
    /// `_checkInvocationTypeArguments` decides on, held apart from the
    /// inference `own_bound` seeds (which deliberately answer `None`
    /// outside their narrow context-free case and are never a bound
    /// source).
    fn member_send_params(
        &self,
        recv: &Receiver,
        member: StringId,
    ) -> Option<(
        Box<[(StringId, Option<Ty>)]>,
        Vec<(StringId, Ty)>,
        Box<[(StringId, Option<Ty>)]>,
    )> {
        // The method's declared `(name, bound)` pairs — the bounds
        // verdict's parameter table; `Dynamic` reads as the absent-bound
        // `None` the executable windows spell.
        let decl_bounds = |entry: &crate::model::MemberEntry| -> Box<[(StringId, Option<Ty>)]> {
            entry
                .type_param_names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    (
                        *name,
                        entry
                            .type_param_bounds
                            .get(index)
                            .copied()
                            .filter(|b| *b != Ty::Dynamic),
                    )
                })
                .collect()
        };
        let (container, kind, recv_ty) = match *recv {
            Receiver::Typed { type_name, ty } => (type_name, ContainerKind::Class, Some(ty)),
            Receiver::This { container, kind } => (container, kind, None),
            // A static send (`W.wrap(...)`): the method's own type
            // parameters infer like any generic send's — its formals never
            // spell the caller's same-named in-scope parameters.
            Receiver::Container { name, kind } => {
                let Walk::Found(entry) = self.bare_member_walk(name, kind, member) else {
                    return None;
                };
                if entry.kind != MemberKind::Method
                    || !entry.is_static
                    || entry.type_param_names.is_empty()
                {
                    return None;
                }
                let params: Vec<(StringId, Option<Ty>)> = entry
                    .type_param_names
                    .iter()
                    .map(|name| (*name, None))
                    .collect();
                return Some((
                    params.into_boxed_slice(),
                    Vec::new(),
                    decl_bounds(&entry),
                ));
            }
            // A folded creation's instance send (`new C().m(42)`): the
            // instance face — the constructor-argument membership check
            // walks the superclass chain like a declaration-typed receiver,
            // and the generic method's own parameters solve upward from
            // the arguments (`m<T>(T x)` binds `T := int`). The created
            // receiver carries no written instantiation, so the declaring
            // container's parameters join unseeded — `new Box().get()`
            // stays unsolved where `box.get()` read `int`
            // (generic_inferrer.dart's upward constraint arm; a `C<int>()`
            // creation's explicit arguments live on the callee row, not
            // this receiver — under-report over a wrong binding).
            Receiver::Created { name, kind, .. } if kind != ContainerKind::Extension => {
                (name, kind, None)
            }
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
        // The method's own bounds, read like a top-level function's
        // `exec_bounds`: an unconstrained parameter settles to its bound
        // (`popTypedList<T extends Object>(n)` reads `List<Object>?`, the
        // inferrer's `_chooseTypeFromConstraints` falling back to the
        // bound). An absent bound, or one naming the method's own
        // parameters (`T extends Comparable<T>`), stays unbounded. The
        // fallback only answers a parameter no formal mentions (no
        // argument can constrain it — a closure argument's return the
        // model cannot infer would otherwise settle `Item extends
        // ManifestItem` to the bound) on a context-free send — the whole
        // initializer of an unannotated declaration outside any argument
        // list (the caller already popped the send's own level); a
        // downward context (`take(pop(n)!)`, `List<Foo> x = pop(n)`) would
        // solve the slot from the context, which this model leaves open.
        let context_free = self.in_initializer
            && self.pending_decl_ty.is_none()
            && self.arg_levels.is_empty()
            && self.init_frame_depth == self.frames.len();
        let formal_mentions = |param: StringId| {
            let Some(formals) = entry.formals.as_deref() else {
                return false;
            };
            let pool = self.boundary.pool();
            formals
                .positional
                .iter()
                .chain(formals.named.iter())
                .any(|formal| {
                    // An unmodeled formal type (a function-typed formal) may
                    // spell the parameter anywhere.
                    if formal.ty == Ty::Unknown {
                        return true;
                    }
                    let mut hit = false;
                    pool.visit_named(formal.ty, &mut |named| {
                        if let Ty::Named { name, .. } = named
                            && name == param
                        {
                            hit = true;
                        }
                    });
                    hit
                })
        };
        let own_bound = |index: usize| {
            if !context_free || formal_mentions(entry.type_param_names[index]) {
                return None;
            }
            let bound = entry.type_param_bounds.get(index).copied()?;
            if bound == Ty::Dynamic {
                return None;
            }
            let mut recursive = false;
            self.boundary.pool().visit_named(bound, &mut |named| {
                if let Ty::Named { name, .. } = named
                    && entry.type_param_names.contains(&name)
                {
                    recursive = true;
                }
            });
            (!recursive).then_some(bound)
        };
        // The bounds verdict's parameter table, held apart from the
        // inference seeds below: `own_bound` answers `None` outside its
        // context-free corner deliberately — it is an inference seed,
        // never a bound source — while the declared bounds are the facts
        // `check_call_type_arg_bounds` needs.
        let declared = decl_bounds(&entry);
        let mut params: Vec<(StringId, Option<Ty>)> = entry
            .type_param_names
            .iter()
            .enumerate()
            .map(|(index, name)| (*name, own_bound(index)))
            .collect();
        // The declaring container's parameters join only for the
        // receiver's own member: an inherited member's formals spell the
        // SUPERTYPE's parameter names, which happen to collide with the
        // receiver's own spelling (`C<T> extends B<String>` calling
        // `B`'s `m(T)` — binding `C`'s `T` from the argument would invent
        // a solution the oracle never derives). With no seed the foreign
        // member's bare-parameter formal binds nothing (the formal spells
        // a name outside this param list, so `unify_upward` declines it).
        let container_params: Vec<(StringId, Option<crate::bounds::TypeTree>, bool)> = if own_member
        {
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
        Some((params.into_boxed_slice(), seeds, declared))
    }

    /// The extension-member send's bounds face (#3495) at the formals-less
    /// seam: [`Self::check_arguments`] hands a send with no member formals
    /// here, and a member only an applied EXTENSION declares is exactly
    /// that shape. The written type-argument group checks against the
    /// table [`Self::extension_member_bounds`] builds; a send that carries
    /// no group, no resolved receiver, or no extension application is
    /// silent (the under-report direction).
    fn check_extension_member_type_arg_bounds(
        &mut self,
        callee: usize,
        recv: Option<Receiver>,
    ) {
        let Some(recv) = recv else { return };
        let Some(member) = self.rows.get(callee).map(|row| row.name) else {
            return;
        };
        let Some(args) = self.call_args_for(callee) else {
            return;
        };
        // A receiver whose frozen face is potentially NULLABLE abstains
        // (#3575): pass 1 classified the receiver at its push-time face
        // (`int? i1` under `if (i1 != null)` reads `int?` here), the
        // promoted face exists only after the pass-2 read-type join
        // (`Engine::read_types` fills after pass 1), and the extension's
        // `on T` binding over the unpromoted face checks a bound the
        // oracle — which reads the instantiated member type over the
        // promoted receiver (`checkMethodInvocation`'s
        // `node.function.staticType`) — never sees: the corpus's
        // `expectStaticType<Exactly<int>>()` family fired 1052 rows the
        // oracle does not report. The promotion-latch table cannot gate
        // this seam — its bare-read arm excludes invoked callees
        // (`Intent::Invoke`), and an extension-member bounds check rides
        // an invoked send — so the nullable frozen face itself is the
        // abstain signal. A NON-nullable face cannot be null-narrowed by
        // a re-derivation, so it decides here as before; `this`-receiver
        // and unresolved receivers keep the face below (the extension
        // face's own silence cases decide them).
        if let Receiver::Typed { ty, .. } = &recv
            && self.potentially_nullable(*ty)
        {
            return;
        }
        if let Some(params) = self.extension_member_bounds(&recv, member) {
            self.check_call_type_arg_bounds(&params, &args);
        }
    }

    /// The extension-member send's bounds table (#3495): a member the
    /// receiver's container never declares lives on an APPLIED extension,
    /// whose own parameter table [`Self::member_send_params`]'s container
    /// walk cannot reach. Upstream checks the written type arguments
    /// against the INSTANTIATED member type (`checkMethodInvocation` reads
    /// `node.function.staticType`, upstream Dart SDK source,
    /// pkg/analyzer/lib/src/error/type_arguments_verifier.dart:236-240 +
    /// :570-633): the extension's own parameters bind by the application
    /// first — the `on` clause's argument positions against the receiver's
    /// written arguments, a bare own-parameter `on T` wholesale — so
    /// `R extends Exactly<T>` over an `int` receiver checks against
    /// `Exactly<int>`. The application's verdict keeps the pool's
    /// specificity (`Single`/`UnsafeSingle` resolve the member); an
    /// ambiguous pool, several declarers of the spelling, and a binding
    /// the substitution cannot spell all keep the under-report (`None`).
    fn extension_member_bounds(
        &mut self,
        recv: &Receiver,
        member: StringId,
    ) -> Option<Box<[(StringId, Option<Ty>)]>> {
        let (head, receiver_ty, receiver_nullable) = match *recv {
            Receiver::Typed { type_name, ty } => {
                (type_name, ty, self.potentially_nullable(ty))
            }
            // An extension body's `this`: the extended type is the
            // receiver (`this.m<..>()` re-applies the face like any typed
            // receiver).
            Receiver::This { container, kind } => {
                let ty = self.this_static_ty(container, kind);
                let Ty::Named { name, .. } = ty else {
                    return None;
                };
                (name, ty, self.potentially_nullable(ty))
            }
            _ => return None,
        };
        self.accessible_extension_faces();
        let faces = self.accessible_extensions.as_deref().unwrap_or(&[]);
        let declarers: Vec<(&'a ExtensionFace, FileId)> = faces
            .iter()
            .filter(|(face, _)| {
                face.members
                    .get(&member)
                    .is_some_and(|entry| entry.kind == MemberKind::Method)
            })
            .copied()
            .collect();
        // The oracle resolves ONE extension (`_chooseMostSpecific`); a
        // pool where several faces spell the member decides by
        // applicability, which this table cannot attribute — the
        // under-report direction [`Self::extension_read_ty`] keeps.
        if declarers.len() != 1 {
            return None;
        }
        let (face, face_file) = declarers[0];
        match self.extension_application(head, receiver_ty, member, false, receiver_nullable) {
            ExtensionApp::Single | ExtensionApp::UnsafeSingle => {}
            ExtensionApp::None | ExtensionApp::Ambiguous(_) => return None,
        }
        let entry = face.members.get(&member)?;
        if entry.type_param_names.is_empty() {
            return None;
        }
        // The extension's own parameters instantiate from the receiver: a
        // bare own-parameter `on` head binds `T := receiver` wholesale
        // (`extension StaticType<T> on T`), a generic `on` head binds its
        // argument positions through the receiver's written arguments
        // (`extension M<K, V> on Map<K, V>` — the same positional zip
        // `extension_member_formals` takes). An `on` head the receiver
        // does not spell (a subtype hop) leaves the map empty.
        let mut bindings: Vec<(StringId, Ty)> = Vec::new();
        if let Some(on_head) = face.on_type
            && face.type_param_names.contains(&on_head)
        {
            bindings.push((on_head, receiver_ty));
        } else if let (
            Ty::Named {
                name: on_head,
                args: on_args,
                ..
            },
            Ty::Named {
                name: recv_head,
                args: recv_args,
                ..
            },
        ) = (self.boundary.ty(face_file, face.on_ty), receiver_ty)
            && on_head == recv_head
        {
            let on_args = self.boundary.pool().get(on_args).to_vec();
            let recv_args = self.boundary.pool().get(recv_args).to_vec();
            if on_args.len() == recv_args.len() {
                for (param, arg) in on_args.iter().zip(recv_args) {
                    if let Ty::Named {
                        name,
                        args: ArgListId::EMPTY,
                        ..
                    } = param
                        && face.type_param_names.contains(name)
                    {
                        bindings.push((*name, arg));
                    }
                }
            }
        }
        let params: Vec<(StringId, Option<Ty>)> = entry
            .type_param_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let bound = match entry.type_param_bounds.get(index).copied() {
                    None | Some(Ty::Dynamic) | Some(Ty::Unknown) => None,
                    // The face's payload interns in its declaring file's
                    // pool — project before the substitution reads it.
                    Some(bound) => {
                        let bound = self.boundary.ty(face_file, bound);
                        if bindings.is_empty() {
                            Some(bound)
                        } else {
                            self.subst_params(bound, &bindings)
                        }
                    }
                };
                (*name, bound)
            })
            .collect();
        Some(params.into_boxed_slice())
    }

    /// #2831 P1 — the raw-constructor inference face: an unnamed creation
    /// (`A()`) with no explicit type arguments and no downward context (no
    /// enclosing argument list — an outer call flows its formal downward —
    /// and no annotated declaration being initialized) seeds every
    /// parameter with the greatest closure of its bound
    /// ([`Bounds::check_creation_inference`]) and reports at the
    /// constructor name one `could_not_infer` row per failing parameter —
    /// the extends-clause origin's `_formatConstraints` line — plus the
    /// creation-site `type_argument_not_matching_bounds` twin the failed
    /// inference's kept types carry (upstream Dart SDK source,
    /// type_arguments_verifier.dart:342-540: the implicit arguments are
    /// the erroneous types the solver kept, `_typeArgumentErrorNode`
    /// anchors at the NamedType itself with no written argument, and the
    /// `ConstructorName` parent denies the super-bounded pass, so the twin
    /// spells the un-inverted seed). Emitted eagerly at the creation's
    /// close: the bounds snapshot consult is rebuild-aware, and a bound
    /// forward-referencing a later declaration keeps the under-report.
    pub(crate) fn check_creation_inference(&mut self, callee: usize) {
        let (container, range) = {
            let Some(row) = self.rows.get(callee) else {
                return;
            };
            let RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Class,
            }) = &row.shape
            else {
                return;
            };
            if row.qualified {
                return;
            }
            (*name, row.range)
        };
        self.check_creation_inference_at(container, range, true);
    }

    /// #2841 — the alias-callee route's inference half: a bare
    /// non-function alias spelling is the aliased class's unnamed creation
    /// with implicit arguments, and the face keys the ALIAS's own
    /// parameters (upstream infers the alias application's arguments, so
    /// the `could_not_infer` text spells the alias's type variable —
    /// `nonfunction_FutureOr_l1_t02` 86:3). The creation-site
    /// `type_argument_not_matching_bounds` twin stays out: upstream's twin
    /// is the EXPANDED creation's class-side check (the kept seeds mapped
    /// through the alias RHS against the class's parameters — the corpus
    /// twin spells the class parameter `X`, not the alias's `X1`), a face
    /// the alias-side rows cannot spell; the under-report posture holds
    /// until the bounds creation-site arm owns the expanded args.
    pub(crate) fn check_alias_creation_inference(&mut self, head: StringId, range: SourceRange) {
        self.check_creation_inference_at(head, range, false);
    }

    /// The shared inference tail (`head` keys the snapshot declaration —
    /// a class for the creation face, a type alias for the alias-callee
    /// route; `emit_twin` gates the class-side twin rows).
    fn check_creation_inference_at(&mut self, head: StringId, range: SourceRange, emit_twin: bool) {
        if !self.arg_levels.is_empty() || self.pending_decl_ty.is_some() {
            return;
        }
        // The explicit-argument peek is non-consuming: the generic walk
        // below still reads (and takes) the latch through `call_args_for`.
        if self
            .call_type_args
            .iter()
            .any(|(at, _)| *at == range.offset)
        {
            return;
        }
        let rows = self.bounds.check_creation_inference(self.strings, head);
        // The `couldNotInfer` row anchors the whole constructor-name
        // chain (`B.named()` spans 7, the unnamed `B()` form keeps the
        // bare head) — the oracle writes at the creation node's
        // ConstructorName. The twin below keeps the head anchor (the
        // `_typeArgumentErrorNode` NamedType face).
        let chain = self.written_name_chain_span(range).unwrap_or(range);
        for row in &rows {
            let detail = format!(
                "\n\nTried to infer '{}' for '{}' which doesn't work:\n  \
                 Type parameter '{}' is declared to extend '{}' producing '{}'.\n\n\
                 Consider passing explicit type argument(s) to the generic.\n\n",
                row.seed, row.parameter, row.parameter, row.raw_bound, row.bound
            );
            let message = SemanticCode::CouldNotInfer.message(&row.parameter, &detail);
            self.emit_message_with_correction(SemanticCode::CouldNotInfer, chain, message);
        }
        if !emit_twin {
            return;
        }
        for row in &rows {
            self.emit3(
                SemanticCode::TypeArgumentNotMatchingBounds,
                range,
                &row.seed,
                &row.parameter,
                &row.bound,
            );
        }
    }

    /// The written extent of a constructor-name chain: the anchor grown
    /// through a `.selector` continuation (`B.named()` spans 7, the bare
    /// `B` head alone spans the unnamed form) — the whole ConstructorName
    /// the oracle's `couldNotInfer` report anchors. `None` when no
    /// `.ident` follows the anchor.
    pub(crate) fn written_name_chain_span(&self, anchor: SourceRange) -> Option<SourceRange> {
        let mut at = self.skip_whitespace(anchor.end());
        if self.byte_at(at) != Some(b'.') {
            return None;
        }
        at = self.skip_whitespace(at + 1);
        let mut end = at;
        while self
            .byte_at(end)
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
        {
            end += 1;
        }
        if end == at {
            return None;
        }
        Some(SourceRange {
            offset: anchor.offset,
            length: end - anchor.offset,
        })
    }

    /// The written extent of a function-type-literal type argument: the
    /// captured `Function` keyword head grown through its formals group
    /// (`Function()` spans 10, not the 8-char head) — the whole written
    /// TypeAnnotation the oracle's `_typeArgumentErrorNode` anchors. A
    /// nullable literal already spans past its suffix through the
    /// capture's question face, and a bare `Function` class argument is
    /// always followed by the group's `,`/`>` (a generic invocation's
    /// argument list only opens past the `>`), so both keep their
    /// captured face. `None` for every span the shape does not fit.
    pub(crate) fn written_function_literal_span(&self, span: SourceRange) -> Option<SourceRange> {
        if span.length != 8 {
            return None;
        }
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < span.end());
        let open = tokens.get(at)?;
        if open.utf16_offset != span.end() || open.kind != dartq_scanner::TokenKind::OpenParen {
            return None;
        }
        let close = tokens.get(self.tokens.matching_end(at)?)?;
        Some(SourceRange {
            offset: span.offset,
            length: close.utf16_offset + 1 - span.offset,
        })
    }

    /// The invocation-site type-argument bounds face (#2869,
    /// `_checkInvocationTypeArguments`, upstream Dart SDK source,
    /// pkg/analyzer/lib/src/error/type_arguments_verifier.dart:570-633):
    /// every explicit type argument checks against its parameter's bound,
    /// substituted with the written arguments (the
    /// `<TFrom, TTo extends Iterable<TFrom>>` shapes). An arity mismatch
    /// truncates to the shared prefix (the count reports as
    /// `wrong_number_of_type_arguments`), and the report anchors at the
    /// offending argument's own node (`typeArgumentList[i]`). Each row is
    /// a [`TypeArgBound`] (#3510): the verdict compares the JUDGED bound,
    /// the message renders the WRITTEN one — a `(name, bound)` pair is the
    /// row whose two faces coincide.
    pub(crate) fn check_call_type_arg_bounds<B: Copy + Into<TypeArgBound>>(
        &mut self,
        params: &[B],
        args: &CallTypeArgs,
    ) {
        let len = params.len().min(args.tys.len());
        let map: Vec<(StringId, Ty)> = params[..len]
            .iter()
            .zip(args.tys[..len].iter())
            .map(|(row, ty)| ((*row).into().name, *ty))
            .collect();
        for index in 0..len {
            let TypeArgBound {
                name,
                written,
                judged,
            } = params[index].into();
            let Some(judged) = judged else {
                continue;
            };
            let arg = args.tys[index];
            // An unresolvable argument or bound reads `DynamicType`/null
            // upstream (the resolver substituted dynamic before this face
            // sees the types) and satisfies everything — deciding against
            // an unsolved spelling would over-fire.
            if !self.call_type_arg_decidable(arg) || !self.call_type_arg_decidable(judged) {
                continue;
            }
            let Some(judged) = self.subst_params(judged, &map) else {
                continue;
            };
            if self.assignable(arg, judged, false) != Some(false) {
                continue;
            }
            let argument = self.ty_display(arg);
            let parameter = self.strings.resolve(name).to_string();
            // The message slot is the written bound, substituted the same
            // way: the alias decoration the oracle prints (`'A<dynamic>'`)
            // survives only there — the expanded shape decided, it never
            // renders. A written face that fails to substitute falls back
            // to the judged one (the pre-split reading).
            let shown = written
                .and_then(|written| self.subst_params(written, &map))
                .unwrap_or(judged);
            let bound_text = self.ty_display(shown);
            // The anchor is the written argument's own extent: a bare
            // `Function` head grows through its formals group — the whole
            // TypeAnnotation `_typeArgumentErrorNode` anchors (#3571);
            // every other captured face already spans whole.
            let span = self
                .written_function_literal_span(args.spans[index])
                .unwrap_or(args.spans[index]);
            self.emit3(
                SemanticCode::TypeArgumentNotMatchingBounds,
                span,
                &argument,
                &parameter,
                &bound_text,
            );
        }
    }

    /// Whether a bounds-face type can decide a subtype question: a
    /// compound shape always can; a bare named head must resolve as a
    /// type (a primitive, an in-scope parameter, or a scoped
    /// declaration) — an unresolved spelling is the oracle's
    /// erroneous-type suppression, never a mismatch.
    fn call_type_arg_decidable(&self, ty: Ty) -> bool {
        match ty {
            Ty::Named {
                name,
                args: ArgListId::EMPTY,
                ..
            } => self.named_type_is_type(name),
            _ => true,
        }
    }

    /// The bounds-verdict rows of a captured head (#3510): the written
    /// bounds ([`crate::bounds::Bounds::container_params`] /
    /// [`crate::bounds::Bounds::alias_params`]) as the message face and the
    /// snapshot-expanded bounds
    /// ([`crate::bounds::Bounds::expanded_params`]) as the judged face,
    /// zipped in declaration order. `None` when the head has no capture
    /// (the caller's other parameter sources decide).
    fn captured_type_arg_bounds(&self, head: StringId) -> Option<Vec<TypeArgBound>> {
        let written = self
            .bounds
            .container_params(head)
            .or_else(|| self.bounds.alias_params(head))?;
        let expanded = self.bounds.expanded_params(self.strings, head);
        let rows = written
            .iter()
            .enumerate()
            .map(|(index, (name, bound, _))| {
                let written = bound.as_ref().map(|tree| self.tree_ty(tree));
                let judged = expanded
                    .as_ref()
                    .and_then(|rows| rows.get(index))
                    .map_or(written, |(_, tree, _)| {
                        tree.as_ref().map(|tree| self.tree_ty(tree))
                    });
                TypeArgBound {
                    name: *name,
                    written,
                    judged,
                }
            })
            .collect();
        Some(rows)
    }

    /// #2869 — the creation bounds face at the formals seam: a send whose
    /// head has no formals (a ctor-less class creation, an alias
    /// creation, a formals-less function) returns right below the
    /// `check_creation_inference` call, and its written type arguments
    /// still check against the head's substituted bounds. The head's
    /// declaring parameters come from the container capture, the alias
    /// capture, or the executable's own bound window — the same facts the
    /// formals path gathers. Poisoned bounds keep the name-only shape
    /// (the executable windows' rule), which the decidability gate
    /// suppresses. The latch entry is PEEKED, never consumed: the
    /// creation-type faces downstream (`creation_row_ty`,
    /// `with_explicit_creation_args`) read the same entry to instantiate
    /// the row — a drain here types every formals-less creation as its
    /// bare head (the #2243 regression).
    pub(crate) fn check_creation_type_arg_bounds(&mut self, callee: usize) {
        let Some(offset) = self.rows.get(callee).map(|r| r.range.offset) else {
            return;
        };
        let pos = self
            .call_type_args
            .iter()
            .rposition(|(at, _)| *at == offset);
        let Some(args) = pos.map(|p| self.call_type_args[p].1.clone()) else {
            return;
        };
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        if row.qualified {
            return;
        }
        let head_name = match &row.shape {
            RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Class,
            }) => Some(*name),
            // An extension-type head carries the conflated `Extension`
            // tag; only a resolved `ExtensionType` is a constructable
            // container (a plain extension's `Ext(x)` is the override
            // face, not a creation).
            RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Extension,
            }) if self.container_kind_of(*name) == Some(ContainerKind::ExtensionType) => {
                Some(*name)
            }
            _ => self.bare_creation_head_name(row.range),
        };
        let params = head_name.and_then(|name| self.captured_type_arg_bounds(name));
        let params: Vec<TypeArgBound> = match params {
            Some(params) => params,
            None => match self.exec_bounds.get(&row.name) {
                Some(window) if !window.is_empty() => {
                    window.iter().copied().map(TypeArgBound::from).collect()
                }
                // The generic function-literal invocation `(<T>() {})
                // <String>()`: the callee row is the parenthesized
                // literal's `Bare(Plain)`/`Value` — no declaring name, no
                // `exec_bounds` window — but its own `<T>` window is the
                // last closed outermost type-variables group, still
                // latched in `bounds.pending` (the send's `<String>`
                // group is a call-site group, never a header's
                // `begin_type_variables`, so nothing cleared it). This is
                // the formals-less arm the `(<lit>)<S>()` send reaches
                // (`checkFunctionExpressionInvocation`,
                // type_arguments_verifier.dart:193-196). Gated on the
                // nameless literal fold: a spelled callee `x<S>()` owns
                // its OWN type's parameters (an unmodeled function type
                // under-reports), never an enclosing declaration's stale
                // window — a generic declaration nested in the literal's
                // body re-latches `pending` the same way.
                _ if row.name == self.intern("") => match self.bounds.pending_window() {
                    Some(window) if !window.is_empty() => window
                        .iter()
                        .map(|(param, bound, _)| {
                            TypeArgBound::from((
                                *param,
                                bound.as_ref().map(|tree| self.tree_ty(tree)),
                            ))
                        })
                        .collect(),
                    _ => return,
                },
                _ => return,
            },
        };
        self.check_call_type_arg_bounds(&params, &args);
    }

    /// #2890 — the keyword-form creation's bounds face (`new G<A>()`,
    /// `const G<A>()`, and their `.new` spelling): the constructor name's
    /// type is a NamedType, and the written arguments check exactly like an
    /// annotation's — the substituted container bounds, or a type alias's
    /// own when the head resolves through one (`checkNamedType` →
    /// `_checkForTypeArgumentNotMatchingBounds`, upstream Dart SDK source,
    /// pkg/analyzer/lib/src/error/type_arguments_verifier.dart:243-247).
    /// A resolved creation head pushes no row, so the group closed
    /// unanchored: the capture never latched, and the still-open capture IS
    /// the written group (a headed nested group never re-arms it, and its
    /// members feed at a foreign depth). A missing capture means the
    /// anchored latch already owns the group — the anchored arms read it,
    /// never this seam. The named-constructor spellings (`C.named<int>`)
    /// declare no type parameters — the `_constructor` count face stays
    /// alone there.
    fn check_creation_ref_bounds(&mut self, class: StringId) {
        let Some(params) = self.captured_type_arg_bounds(class) else {
            return;
        };
        let Some(capture) = self.call_args_capture.take() else {
            return;
        };
        let (tys, spans): (Vec<Ty>, Vec<SourceRange>) = capture.members.into_iter().unzip();
        let args = CallTypeArgs {
            tys: tys.into_boxed_slice(),
            spans: spans.into_boxed_slice(),
        };
        self.check_call_type_arg_bounds(&params, &args);
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
        // The built-in `Map<K, V> <: Iterable<E>` relation (the language's
        // Map-as-Iterable face — `Map` declares no supertype clause in the
        // SDK source, so the declared-edge walk sees no `Iterable` hop and
        // answers a definite miss the oracle never makes: `f(someMap)`
        // against `f(Iterable<Object?> x)` over-reported
        // `Map<dynamic,dynamic> => Iterable<Object?>`). The element
        // relation decides: `MapEntry<K, V> <: E`. Raw heads read
        // `dynamic` for their arguments (the raw-type erasure). Core
        // spellings only — a same-file `Map`/`Iterable`/`MapEntry`
        // declaration keeps the ordinary declared-edge verdict.
        if let Some(verdict) = self.builtin_map_iterable_verdict(from, to) {
            return Some(verdict);
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

    /// The built-in `Map<K, V> <: Iterable<E>` relation the argument
    /// verdict asks first ([`Engine::argument_verdict`]'s Map-as-Iterable
    /// face): `Some` only where the heads are the core spellings the
    /// analyzing file does not itself declare — a same-file
    /// `Map`/`Iterable`/`MapEntry` declaration shadows the core face and
    /// keeps the declared-edge verdict. The element relation
    /// `MapEntry<K, V> <: E` decides through the ordinary matrix; raw
    /// heads erase to `dynamic` arguments.
    fn builtin_map_iterable_verdict(&self, from: Ty, to: Ty) -> Option<bool> {
        let Ty::Named {
            name: map,
            args: map_args,
            nullable: false,
        } = from
        else {
            return None;
        };
        let Ty::Named {
            name: iterable,
            args: iter_args,
            nullable: false,
        } = to
        else {
            return None;
        };
        if map != self.intern("Map") || iterable != self.intern("Iterable") {
            return None;
        }
        let shadowed = |engine: &Self, name: StringId| {
            engine
                .index
                .get(engine.file)
                .is_some_and(|unit| unit.container(name).is_some())
        };
        let entry_head = self.intern("MapEntry");
        if shadowed(self, map) || shadowed(self, iterable) || shadowed(self, entry_head) {
            return None;
        }
        // Each read scopes its own pool borrow: `intern_list` below takes
        // the args cell's `borrow_mut`, and a held `Ref` would panic the
        // cell (the #2594 live-repro shape).
        let (k, v) = {
            let pool = self.boundary.pool();
            match pool.get(map_args) {
                [] => (Ty::Dynamic, Ty::Dynamic),
                [k, v] => (*k, *v),
                _ => return None,
            }
        };
        let elem = {
            let pool = self.boundary.pool();
            match pool.get(iter_args) {
                [] => Ty::Dynamic,
                [elem] => *elem,
                _ => return None,
            }
        };
        let entry = Ty::Named {
            name: entry_head,
            nullable: false,
            args: self.boundary.intern_list(&[k, v]),
        };
        self.assignable(entry, elem, false)
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
        let Some((name_range, selector)) = self.enum_const_call else {
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
        // `const E({this.test = 0})` reports nothing). The positional
        // supply joins the same index (`visitEnumConstantDeclaration` →
        // `_addParametersForArguments` over the constant's whole
        // argumentList, upstream unused_local_elements_verifier.dart:146-153):
        // `E(0)` gives the selected constructor's first positional formal.
        for (given, ..) in &level.args {
            if let Some(given) = given {
                self.named_given.insert((key, *given));
            }
        }
        let named = level
            .args
            .iter()
            .filter(|(name, ..)| name.is_some())
            .count() as u16;
        if positional > named {
            let given = self.positional_given.entry(key).or_default();
            *given = (*given).max(positional - named);
        }
        // The unnamed face over an enum with NO declared constructor
        // resolves the synthetic `const E()` — a zero-formal table that
        // still decides the count faces (`v(0)` over a constructor-less
        // enum reports `extra_positional_arguments` "0 expected, but 1
        // found"; upstream resolves the element's implicit generative
        // constructor). A declared-selector miss keeps the under-report
        // posture — `undefined_enum_constructor` owns that face.
        let Some((formals, const_ctor)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            enum_name,
        )
        .and_then(|(container, owner)| {
            let mut formals = match container.ctor_formals.get(&key) {
                Some(formals) => (**formals).clone(),
                None if key == enum_name => Formals {
                    positional: Box::default(),
                    named: Box::default(),
                },
                None => return None,
            };
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
            // The constant's written type arguments instantiate the enum's
            // own parameters (`enum E<T> { a<int>(...) }` — `T` is `int`):
            // the oracle checks each argument against the constructor's
            // INSTANTIATED formal
            // (`correspondingParameter.type`, upstream Dart SDK source,
            // pkg/analyzer/lib/src/generated/error_detection_helpers.dart
            // :72-75), never the declared `T` leaf. The headless group's
            // members ride the `PendingBoundedCheck::Site` capture queued
            // at its close — `head` is the enum (the official
            // `checkEnumConstantDeclaration` face), and this constant's
            // own group is the nearest one at-or-after its name. A
            // formal that spells no parameter substitutes to itself.
            if !container.type_param_names.is_empty() {
                let written = self
                    .pending_bounded_checks
                    .iter()
                    .filter_map(|check| match check {
                        PendingBoundedCheck::Site { head, args, site, .. }
                            if *head == enum_name && site.offset >= name_range.end() =>
                        {
                            Some((site.offset, args))
                        }
                        _ => None,
                    })
                    .min_by_key(|(offset, _)| *offset)
                    .map(|(_, args)| args.clone());
                if let Some(written) = written {
                    let map: Vec<(StringId, Ty)> = container
                        .type_param_names
                        .iter()
                        .zip(written.iter())
                        .map(|(param, tree)| (*param, self.tree_ty(tree)))
                        .collect();
                    if !map.is_empty() {
                        for formal in formals
                            .positional
                            .iter_mut()
                            .chain(formals.named.iter_mut())
                        {
                            if let Some(substituted) = self.subst_params(formal.ty, &map) {
                                formal.ty = substituted;
                            }
                        }
                    }
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
        // The required-named misses emit in formal-declaration order at the
        // constant's own name — upstream's `visitEnumConstantDeclaration`
        // checks `constructorElement`'s formals with `errorEntity: node.name`
        // (required_parameters_verifier.dart:60-66); the enum-constant
        // invocation carries no callee row, so the constant's name range is
        // the `enum_const_call` latch's own span.
        if let Some((name_range, _)) = self.enum_const_call {
            self.emit_missing_required(&formals, &level.args, &[], name_range);
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
                let note = self.record_arity_note(flow_ty, formal.ty);
                let message = SemanticCode::ArgumentTypeNotAssignable.message3(
                    &from_text,
                    &to_text,
                    &note,
                );
                self.emit_message_with_correction(
                    SemanticCode::ArgumentTypeNotAssignable,
                    *range,
                    message,
                );
            } else {
                // The conversion's own face (#2870): a granted `int →
                // double` literal ask doubles as the imprecise-value check
                // (the formal is the double context the oracle's
                // `_checkForOutOfRange` double arm reports from).
                self.check_imprecise_int_literal(flow_ty, formal.ty, *literal, *range);
            }
        }
    }

    /// The index expression's argument check (#2937, upstream
    /// `checkIndexExpressionIndex`, error_detection_helpers.dart:260-289):
    /// the folded index operand checks against the first formal of EVERY
    /// `[]`/`[]=` element the access's write form applies — `[]` for a pure
    /// `a[i]` read, `[]=` for `a[i] = v` (`hasRead: false`, resolver.dart
    /// :1504 via `resolveForWrite`), both for `a[i] += v`/`++a[i]`/`a[i]++`
    /// (`hasRead: true`, the compound `resolveForWrite`). The staged
    /// operand's static type and span ride `index_arg_faces`; the write
    /// form is the operator row's finalized `write_op`. A receiver naming
    /// no container, an operator with an empty formal list, or an
    /// undecidable pair stays silent (the under-report direction — the
    /// dynamic receiver upstream resolves nothing either).
    pub(crate) fn check_index_argument(&mut self, bracket_open: u32, write: Option<OpWrite>) {
        let Some(face) = self.index_arg_faces.remove(&bracket_open) else {
            return;
        };
        let Some(recv) = face.recv else {
            return;
        };
        let (read, write_op) = match write {
            // `=`: only the write element's formal applies
            // (`hasRead: false`).
            Some(OpWrite::Assign) => (None, Some(self.intern("[]="))),
            // `+=`/`++`/`--`: the read and write elements both apply
            // (`hasRead: true`).
            Some(OpWrite::Compound) => (Some(self.intern("[]")), Some(self.intern("[]="))),
            // A pure read: only the `[]` element's formal.
            None => (Some(self.intern("[]")), None),
        };
        // The flow read's re-bound or promoted face outranks the staged
        // snapshot — the same override the call-site walk applies (the
        // oracle checks the argument's static type at the send).
        let flow_ty = self
            .rebound_read_face(face.index_range)
            .or_else(|| self.read_type_at(face.index_range))
            .unwrap_or(face.index_ty);
        // The unindexed SDK collection heads (`Map`, `List`) carry no
        // element model in the graph, so their index formal resolves off
        // the written receiver type instead (#3248).
        let recv_ty = self
            .index_receiver_ty(bracket_open)
            .map(|ty| self.fold_alias_ty(ty));
        for spelling in [read, write_op].into_iter().flatten() {
            let Some(param_ty) = self
                .resolved_member(recv, spelling)
                .and_then(|entry| {
                    entry
                        .formals
                        .as_ref()
                        .and_then(|formals| formals.positional.first().map(|formal| formal.ty))
                })
                .or_else(|| recv_ty.and_then(|ty| self.core_index_formal_ty(ty, spelling)))
            else {
                continue;
            };
            if self.argument_verdict(flow_ty, param_ty, face.literal) == Some(false) {
                let from_text = self.ty_display(flow_ty);
                let to_text = self.ty_display(param_ty);
                let message = SemanticCode::ArgumentTypeNotAssignable
                    .message3(&from_text, &to_text, "");
                self.emit_message_with_correction(
                    SemanticCode::ArgumentTypeNotAssignable,
                    face.index_range,
                    message,
                );
            }
        }
    }

    /// The index expression's receiver type, read off its operator row: the
    /// `[]` fold leaves the receiver's own row on the stack carrying the
    /// bracket span, so the written arguments the head-only
    /// [`IndexArgFace`] spelling drops (`Map<K, V>`'s key) stay reachable.
    /// `None` once the row folded away — the unit-end drain of a pure read
    /// (`a[i]`), whose core formals (`int`, `Object?`) reject nothing.
    fn index_receiver_ty(&self, bracket_open: u32) -> Option<Ty> {
        let index_op = self.intern("[]");
        self.rows.iter().find_map(|row| match &row.intent {
            Intent::Operator(op, bracket) if *op == index_op && bracket.offset == bracket_open => {
                let ty = Self::receiver_row_ty(row);
                (ty != Ty::UNKNOWN).then_some(ty)
            }
            _ => None,
        })
    }

    /// The unindexed SDK collection heads' index formal (#3248): `Map` and
    /// `List` carry no element model in the graph, so
    /// [`Self::resolved_member`] answers nothing for them and the argument
    /// check never ran — `Map<int, int> m; m['x'] = 0` stayed silent.
    /// Upstream resolves the elements and reports the key mismatch through
    /// `checkIndexExpressionIndex` (error_detection_helpers.dart:260-289):
    /// `Map<K, V>` declares `V? operator [](Object? key)` / `void operator
    /// []=(K key, V value)` and `List<E>` declares `E operator [](int
    /// index)` / `void operator []=(int index, E value)` (upstream Dart SDK
    /// source, sdk/lib/core/map.dart:288-306 and list.dart:375-395). Only
    /// `Map`'s write spelling is a claim here — its read key is `Object?`,
    /// which rejects nothing. A receiver without written arguments, or with
    /// any other head, answers nothing (the under-report direction); a
    /// same-file `Map<K, V>`/`List<E>` shadow is not distinguished from the
    /// core head, which costs nothing the corpus carries (no unit declares
    /// either spelling) and would only pair a row with the
    /// `undefined_operator` such a shadow already reports.
    fn core_index_formal_ty(&self, receiver: Ty, spelling: StringId) -> Option<Ty> {
        let Ty::Named { name, args, .. } = receiver else {
            return None;
        };
        let written: Vec<Ty> = self.boundary.pool().get(args).to_vec();
        if name == self.intern("Map") {
            return (spelling == self.intern("[]=") && written.len() == 2).then(|| written[0]);
        }
        if name == self.intern("List") && written.len() == 1 {
            return Some(Ty::named(self.intern("int")));
        }
        None
    }

    /// The oracle's argument-report appendix
    /// (`NonAssignabilityReporterForArgument.createDiagnostic`,
    /// error_detection_helpers.dart:405-441): between two record types the
    /// positional and named arity mismatches — and every named field the
    /// formal does not carry under the same name and type — join the
    /// message after its clause, space-separated. The alias folds decide
    /// (`typedef A = ({int b, int c})` counts as its record), the written
    /// spellings display. Empty for every non-record pair, so the
    /// `message3` slot keeps the template's own terminal space.
    fn record_arity_note(&self, actual: Ty, expected: Ty) -> String {
        let (
            Ty::Record {
                shape: actual_shape,
                ..
            },
            Ty::Record {
                shape: expected_shape,
                ..
            },
        ) = (self.fold_alias_ty(actual), self.fold_alias_ty(expected))
        else {
            return String::new();
        };
        let (actual_positional, actual_named, expected_positional, expected_named, unexpected) = {
            let pool = self.boundary.pool();
            let actual = pool.record_shape(actual_shape);
            let expected = pool.record_shape(expected_shape);
            let unexpected: Vec<(StringId, Ty)> = if expected.named.is_empty() {
                Vec::new()
            } else {
                actual
                    .named
                    .iter()
                    .filter(|(name, ty)| {
                        !expected
                            .named
                            .iter()
                            .any(|(other, other_ty)| other == name && other_ty == ty)
                    })
                    .copied()
                    .collect()
            };
            (
                actual.positional.len(),
                actual.named.len(),
                expected.positional.len(),
                expected.named.len(),
                unexpected,
            )
        };
        let mut note = Vec::new();
        if expected_positional != 0 && actual_positional != expected_positional {
            note.push(format!(
                "Expected {expected_positional} positional arguments, but got {actual_positional} instead."
            ));
        }
        if expected_named != 0 && actual_named != expected_named {
            note.push(format!(
                "Expected {expected_named} named arguments, but got {actual_named} instead."
            ));
        }
        for (name, ty) in unexpected {
            note.push(format!(
                "Unexpected named argument `{}` with type `{}`.",
                self.strings.resolve(name),
                self.ty_display(ty)
            ));
        }
        note.join(" ")
    }

    /// The annotation's argument walk (#2937, upstream Dart SDK source,
    /// resolver.dart:1887 — `visitAnnotation` runs
    /// `checkForArgumentTypesNotAssignableInList` on the resolved
    /// constructor's formals right after `_annotationResolver.resolve`):
    /// a metadata creation's invoke slot carries no callee link, so the
    /// generic `close_arguments` walk never fires — the annotation's own
    /// resolved constructor pairs the argument list instead. `annotation`
    /// is the pending entry's joined spelling (`A`, `A.named`, `p.A.named`,
    /// `A<int>.named`): the class segment keys the container's
    /// `ctor_formals` table (`fromInt` selector or the unnamed face), the
    /// per-argument check runs `argument_verdict` at each argument's span
    /// exactly like the call-site walk. The const-annotation pair
    /// (`const_constructor_param_type_mismatch` + this code, the enum arm's
    /// dual) precedes the assignability row on a `const` constructor — the
    /// oracle's constant evaluator runs the invocation's argument check
    /// before the general assignability verifier reports the same span.
    pub(crate) fn check_annotation_arguments(
        &mut self,
        annotation: StringId,
        args: &[(Option<StringId>, Ty, bool, SourceRange)],
    ) {
        let text = self.strings.resolve(annotation).to_string();
        // The class segment is everything before the constructor selector's
        // last `.`; a generic head (`A<int>.named`) keeps its written
        // arguments, a plain `A.named` splits on the selector dot. The
        // unnamed face (`@A(arg)`) selects the container's own spelling.
        let (class_text, selector) = match text.rsplit_once('.') {
            Some((class, sel)) => (class, Some(sel)),
            None => (text.as_str(), None),
        };
        // The resolution head drops the written type-argument group
        // (`A<int>` resolves `A`) — the container's formal table keys on
        // the bare spelling.
        let head = class_text.split('<').next().unwrap_or(class_text);
        let head_id = self.intern(head);
        // The bare spelling resolves through the diagnosing scope; the
        // dot-joined `p.A` head resolves through its import prefix's table
        // (the same pair `resolve_receiver_row`'s operator faces consult).
        let Some((container, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            head_id,
        )
        .or_else(|| {
            resolve_joined_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.strings,
                head_id,
            )
        }) else {
            return;
        };
        // `ctor_formals` keys on the selector for a named face and on the
        // container's own name for the unnamed face (the collector's
        // `selector.unwrap_or(key)` convention). The unnamed face's key is
        // the container's LEAF spelling (`A` of `p.A`), never the joined
        // prefix head — `prefixed_container` resolved through the prefix,
        // but the table keys on the declaring name itself.
        let leaf = head.rsplit('.').next().unwrap_or(head);
        // A mixin-application head pairs the forwarded table on the
        // terminal face — the unnamed key is THAT container's own
        // spelling. A broken forwarding chain keeps the head's own row.
        let (face, face_owner, face_name) = self
            .effective_ctor_container(container, owner, self.intern(leaf))
            .unwrap_or((container, owner, self.intern(leaf)));
        let key = selector.map_or(face_name, |sel| self.intern(sel));
        let Some(formals) = face.ctor_formals.get(&key) else {
            return;
        };
        let mut formals = (**formals).clone();
        self.project_formals(face_owner, &mut formals);
        let const_ctor = face.has_const_constructor;
        // The per-argument walk pairs positionals by index, named by
        // spelling — the same shape `check_enum_const_arguments` runs (no
        // `undefined_named_parameter` here: the oracle's annotation walk
        // resolves arguments against the constructor's formals and the
        // label miss is the metadata arity face's own code).
        let mut positional_index = 0usize;
        for (name, ty, literal, range) in args {
            let formal = if let Some(n) = name {
                match self.named_formal_for(&formals, *n) {
                    Some(formal) => formal,
                    None => continue,
                }
            } else {
                let formal = formals.positional.get(positional_index);
                positional_index += 1;
                match formal {
                    Some(formal) => formal,
                    None => continue,
                }
            };
            let flow_ty = self
                .rebound_read_face(*range)
                .or_else(|| self.read_type_at(*range))
                .unwrap_or(*ty);
            if self.argument_verdict(flow_ty, formal.ty, *literal) == Some(false) {
                let from_text = self.ty_display(flow_ty);
                let to_text = self.ty_display(formal.ty);
                if const_ctor {
                    let message = SemanticCode::ConstConstructorParamTypeMismatch
                        .message(&from_text, &to_text);
                    self.emit_message_with_correction(
                        SemanticCode::ConstConstructorParamTypeMismatch,
                        *range,
                        message,
                    );
                }
                let note = self.record_arity_note(flow_ty, formal.ty);
                let message = SemanticCode::ArgumentTypeNotAssignable
                    .message3(&from_text, &to_text, &note);
                self.emit_message_with_correction(
                    SemanticCode::ArgumentTypeNotAssignable,
                    *range,
                    message,
                );
            }
        }
        // The count and required faces mirror `resolveArgumentsToParameters`
        // (resolver.dart:4978-5009 — the deficit/surplus blocks are
        // unconditional once a parameter element resolved) and
        // `_checkRequired` (required_parameters_verifier.dart:134-218 —
        // `visitAnnotation` runs it with `errorEntity` on the constructor
        // identifier). A metadata invocation carries no callee row, so the
        // generic `check_arguments` never saw this argument list; the
        // resolved formals above are the only verdict source. The anchor is
        // the first argument's span, or the annotation head's (reconstructed
        // from the written text's start) when the list is empty — position
        // is load-bearing only for the (severity, code, file) key.
        let given = args
            .iter()
            .take_while(|(name, ..)| name.is_none())
            .count();
        let required = formals.positional.iter().filter(|f| f.required).count();
        let anchor = args
            .iter()
            .map(|(.., range)| *range)
            .next()
            .unwrap_or(SourceRange {
                offset: 0,
                length: 0,
            });
        if given < required {
            let ctor_text = selector.map_or_else(
                || head.to_string(),
                |sel| format!("{head}.{sel}"),
            );
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
        } else if given > formals.positional.len() {
            let capacity = formals.positional.len();
            let code = SemanticCode::ExtraPositionalArguments;
            let message = code.message(&capacity.to_string(), &given.to_string());
            self.emit_message_with_correction(code, anchor, message);
        }
        self.emit_missing_required(&formals, args, &[], anchor);
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

    /// The required-named coverage walk
    /// (`RequiredParametersVerifier._check`,
    /// required_parameters_verifier.dart:122-156): a `required` named
    /// formal with no same-named argument reports `missing_required_argument`
    /// at `anchor` — the callee entity (`errorEntity: node` upstream).
    /// `extra_cover` is the enclosing-constructor super-formal forwarding
    /// face (`_containsNamedExpression`'s second walk): a `required`
    /// `super.x` formal of the enclosing constructor stands in for the
    /// same-named argument. The emit walks `parameters` in declaration
    /// order, one row per missing required named — the oracle's exact
    /// ordering.
    pub(crate) fn emit_missing_required(
        &mut self,
        formals: &Formals,
        args: &[(Option<StringId>, Ty, bool, SourceRange)],
        extra_cover: &[(bool, StringId)],
        anchor: SourceRange,
    ) {
        let missing: Vec<StringId> = formals
            .named
            .iter()
            .filter(|f| f.required)
            .filter(|f| {
                !args.iter().any(|(name, ..)| {
                    name.is_some_and(|n| {
                        self.named_formal_for(formals, n)
                            .is_some_and(|m| m.name == f.name)
                    })
                })
            })
            .filter(|f| {
                !extra_cover
                    .iter()
                    .any(|(named, name)| *named && *name == f.name)
            })
            // The reported name is the element's public spelling — a
            // `required this._error` formal reports `'error'` upstream.
            .map(|f| self.formal_public_name(f.name))
            .collect();
        for name in missing {
            let text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::MissingRequiredArgument, anchor, &text, "");
        }
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

    /// The redirecting-constructor invocation's callee
    /// (`this(...)`/`this.named(...)` in an initializer list): the same
    /// two geometries as [`Engine::super_invocation_callee`] — an unmerged
    /// member placeholder over the bare `this` keyword receiver for the
    /// selector shape, or the keyword row the group open latched as
    /// `callee_row` for the bare shape. Returns `(enclosing container,
    /// selector, bare)`; `this.new(...)` normalizes to the unnamed face.
    fn redirecting_invocation_callee(
        &self,
        level: &ArgLevel,
    ) -> Option<(StringId, Option<StringId>, bool)> {
        let callee = self.invokes.last().and_then(|slot| *slot);
        if let Some(index) = callee
            && let Some(row) = self.rows.get(index)
            && row.qualified
            && let RowShape::Bare(BareInfo::Plain { .. }) = row.shape
            && let Some(below) = index.checked_sub(1).and_then(|i| self.rows.get(i))
            && let RowShape::Receiver(Receiver::This { container, .. }) = below.shape
            && below.name == self.intern("")
        {
            let selector = (row.name != self.intern("new")).then_some(row.name);
            return Some((container, selector, false));
        }
        let index = level.callee_row?;
        let row = self.rows.get(index)?;
        match &row.shape {
            RowShape::Receiver(Receiver::This { container, .. })
                if row.name == self.intern("") =>
            {
                Some((*container, None, true))
            }
            _ => None,
        }
    }

    /// The redirecting-constructor invocation's argument faces
    /// (`visitRedirectingConstructorInvocation`, upstream Dart SDK source,
    /// resolver.dart:4057-4085 — `resolveInvocation` pairs the arguments
    /// against the resolved same-class constructor's formals, and
    /// `RequiredParametersVerifier.visitRedirectingConstructorInvocation`
    //  checks the required-named coverage): the count faces
    /// (`extra_positional_arguments[_could_be_named]` /
    /// `not_enough_positional_arguments`), the required-named misses, and
    /// `undefined_named_parameter` at the label — the same decision order
    /// as [`Engine::check_super_invocation_arguments`] minus the superclass
    /// walk (a redirecting target lives in the enclosing container, keyed
    /// by the selector or the container's own name for the unnamed face).
    /// An unresolved selector defers to `undefined_constructor_in_initializer`.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn check_redirecting_invocation_arguments(
        &mut self,
        level: &ArgLevel,
        positional: u16,
        close: SourceRange,
        container: StringId,
        selector: Option<StringId>,
    ) {
        let key = selector
            .filter(|name| *name != self.intern("new"))
            .unwrap_or(container);
        let Some(formals) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.ctor_formals.get(&key))
            .map(|f| (**f).clone())
        else {
            // No such same-class constructor — the existence face owns the
            // report (upstream `undefined_constructor_in_initializer`).
            return;
        };
        let named = level
            .args
            .iter()
            .filter(|(name, ..)| name.is_some())
            .count() as u16;
        let Some(given) = positional.checked_sub(named) else {
            return;
        };
        let given = usize::from(given);
        let required = formals.positional.iter().filter(|f| f.required).count();
        let capacity = formals.positional.len();
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
            // The name face carries the redirecting target's display —
            // upstream spells `'{returnType}.new'` for the unnamed face and
            // the selector for the named one (resolver.dart
            // `_reportNotEnoughPositionalArguments` :5043-5050).
            let mut ctor_text = self.strings.resolve(container).to_string();
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
        // The required-named misses emit in formal-declaration order at the
        // invocation node — upstream `errorEntity` is the invocation itself.
        let required_anchor = level
            .callee_row
            .and_then(|index| self.rows.get(index))
            .map(|row| row.range)
            .unwrap_or(close);
        self.emit_missing_required(&formals, &level.args, &[], required_anchor);
        // The unknown named parameter anchors at its label — the same
        // geometry the super walk applies (the oracle resolves the named
        // table at the same `resolveArgumentsToParameters` pass).
        for (name, .., range) in &level.args {
            let Some(n) = name else {
                continue;
            };
            if self.named_formal_for(&formals, *n).is_none() {
                let text = self.strings.resolve(*n).to_string();
                let label = level
                    .named_labels
                    .iter()
                    .find(|(label, _)| label == n)
                    .map(|(_, range)| *range)
                    .unwrap_or(*range);
                self.emit(SemanticCode::UndefinedNamedParameter, label, &text, "");
            }
        }
    }

    /// The explicit-keyword unnamed creation's argument faces
    /// (`const A(...)`, `new A(...)`): the `unnamed_head` latch carries the
    /// resolved class head; its `ctor_formals` unnamed face keys on the
    /// container's own name. Upstream `visitInstanceCreationExpression` →
    /// `resolveArgumentsToParameters` (resolver.dart:4978-5009) runs the
    /// deficit/surplus verdicts and the required-named walk
    /// (`_checkRequired`, required_parameters_verifier.dart:134-218) on the
    /// resolved constructor — the same geometry
    /// [`Engine::check_redirecting_invocation_arguments`] applies, minus
    /// the redirecting target's `.new` name face.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn check_explicit_creation_arguments(
        &mut self,
        level: &ArgLevel,
        positional: u16,
        close: SourceRange,
        head: StringId,
    ) {
        let Some((container, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            head,
        ) else {
            return;
        };
        // A mixin-application head reads the forwarded table on the
        // terminal face — `new A1(...)` over `class A1 = B<int> with M1;`
        // pairs `B`'s unnamed formals (keyed on `B`, not `A1`). A broken
        // forwarding chain keeps the under-report posture the empty-table
        // miss already carried.
        let Some((face, face_owner, face_name)) =
            self.effective_ctor_container(container, owner, head)
        else {
            return;
        };
        let Some(formals) = face.ctor_formals.get(&face_name).map(|f| (**f).clone())
        else {
            return;
        };
        let mut formals = formals;
        self.project_formals(face_owner, &mut formals);
        let named = level
            .args
            .iter()
            .filter(|(name, ..)| name.is_some())
            .count() as u16;
        let Some(given) = positional.checked_sub(named) else {
            return;
        };
        let given = usize::from(given);
        let required = formals.positional.iter().filter(|f| f.required).count();
        let capacity = formals.positional.len();
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
            let mut ctor_text = self.strings.resolve(head).to_string();
            ctor_text.push_str(".new");
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
        let required_anchor = level
            .args
            .iter()
            .map(|(.., range)| *range)
            .next()
            .unwrap_or(close);
        self.emit_missing_required(&formals, &level.args, &[], required_anchor);
        for (name, .., range) in &level.args {
            let Some(n) = name else {
                continue;
            };
            if self.named_formal_for(&formals, *n).is_none() {
                let text = self.strings.resolve(*n).to_string();
                let label = level
                    .named_labels
                    .iter()
                    .find(|(label, _)| label == n)
                    .map(|(_, range)| *range)
                    .unwrap_or(*range);
                self.emit(SemanticCode::UndefinedNamedParameter, label, &text, "");
            }
        }
        // The per-argument assignability face (`resolveArgumentsToParameters`
        // pairs each argument with its formal's INSTANTIATED type,
        // resolver.dart:4978-5009). No callee row runs the send-close solver
        // for the keyword spelling, so the creation's own solution is the
        // only source of that instantiation: it substitutes the class's
        // parameters in the formals before the verdict, and a
        // context-seeded `A<int, String> a = new A('s', 1)` keeps its
        // created type (`creation_call_site_ty`'s covariant pin) while each
        // conflict reports at the argument's own span — the oracle's two
        // rows.
        let instantiation = self
            .creation_call_site_ty(Some(level), level.unnamed_head)
            .and_then(|ty| match ty {
                Ty::Named { name, args, .. } if name == head && args != ArgListId::EMPTY => {
                    Some(self.boundary.args.borrow().get(args).to_vec())
                }
                _ => None,
            });
        let map: Vec<(StringId, Ty)> = match (instantiation, self.bounds.container_params(head)) {
            (Some(args), Some(params)) if params.len() == args.len() => params
                .iter()
                .map(|(param, _, _)| *param)
                .zip(args)
                .collect(),
            _ => Vec::new(),
        };
        let mut positional_index = 0usize;
        for (name, ty, literal, range) in &level.args {
            let formal = match name {
                Some(name) => self.named_formal_for(&formals, *name),
                None => {
                    let formal = formals.positional.get(positional_index);
                    positional_index += 1;
                    formal
                }
            };
            let Some(formal) = formal else {
                continue;
            };
            let formal_ty = if map.is_empty() {
                formal.ty
            } else {
                self.subst_params(formal.ty, &map).unwrap_or(formal.ty)
            };
            if self.argument_verdict(*ty, formal_ty, *literal) == Some(false) {
                self.stage(StagedKind::Argument, *range, *ty, formal_ty, *literal);
            }
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
        let (formals, super_container, resolved_super) = match superclass {
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
                        // A mixin-application superclass forwards its
                        // terminal class's constructor tables — `super.t1`
                        // in `class X extends S` over `class S = T with M;`
                        // pairs `T`'s `t1` formals (keyed on `T`, not `S`).
                        // A broken forwarding chain keeps the under-report
                        // posture the selector miss already carried.
                        let Some((face, face_owner, face_name)) = self
                            .effective_ctor_container(
                                super_container,
                                super_owner,
                                superclass,
                            )
                        else {
                            return;
                        };
                        let key = selector
                            .filter(|name| *name != self.intern("new"))
                            .unwrap_or(face_name);
                        let Some(formals) = face.ctor_formals.get(&key).map(|f| (**f).clone())
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
                        // `face_owner`'s pool, so the clone re-interns into
                        // the diagnosing pool before the argument walk
                        // derefs a `formal.ty` slot (a same-file owner is a
                        // no-op).
                        let mut formals = formals;
                        self.project_formals(face_owner, &mut formals);
                        self.patch_super_formals(face_name, &mut formals);
                        // An untyped initializing formal reads its field's
                        // declared type BEFORE the clause substitution
                        // (#2873): `Indexable(this._value)` over
                        // `final ReadType _value` captures the formal
                        // `Unknown`, and a substitution running before the
                        // field patch would skim past it — the raw
                        // parameter spelling (`ReadType`) decided and
                        // displayed where the oracle pairs
                        // `Indexable<String?, String?>`'s `String?`.
                        for formal in formals
                            .positional
                            .iter_mut()
                            .chain(formals.named.iter_mut())
                        {
                            if formal.ty == Ty::UNKNOWN
                                && let Some(entry) = face.members.get(&formal.name)
                                && entry.kind == MemberKind::Field
                                && !entry.is_static
                            {
                                formal.ty = self.boundary.ty(face_owner, entry.var_ty);
                            }
                        }
                        // The subclass's written instantiation substitutes
                        // the superclass's own parameters in the formals
                        // (`PathMap<V> extends MapView<String?, V>` pairs
                        // `MapView(Map<K, V>?)`'s `K := String?`) — the
                        // element model's clause face carries names only,
                        // so the written argument group is the only source.
                        // A forwarded face's parameters pair against the
                        // application's own super-edge instantiation, which
                        // the name-level model does not carry — the
                        // substitution stays off there (the under-report
                        // posture).
                        if std::ptr::eq(face, super_container)
                            && let Some(args) = self.clause_instantiation(container, superclass)
                            && !face.type_param_names.is_empty()
                        {
                            // The clause scan records one `Ty` per clause
                            // entry — a single instantiated head carries its
                            // own written arguments, so the positional
                            // pairing reads them out of the pool first.
                            let written: Vec<Ty> = match args.as_slice() {
                                [Ty::Named { args: arg_list, .. }]
                                    if *arg_list != crate::model::ArgListId::EMPTY =>
                                {
                                    self.boundary.args.borrow().get(*arg_list).to_vec()
                                }
                                many => many.to_vec(),
                            };
                            if written.len() == face.type_param_names.len() {
                                let map: Vec<(StringId, Ty)> = face
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
        // invocation: the count adds the forwarded positionals, then the
        // deficit/surplus verdicts run unconditionally
        // (`verifySuperFormalParameters` feeds `positionalArgumentCount`,
        // then `positionalArgumentCount < requiredParameterCount` decides —
        // upstream Dart SDK source, resolver.dart
        // `resolveArgumentsToParameters` :4930-4940/:4978-4991; probed:
        // `G(super.x) : super(1, 2, 3)` against `B(int x, int y)` reports
        // "2 expected, but 4 found", `D(super.x) : super(1)` reports no
        // deficit because 1+1 covers 2, and `B(super.p1) : super()`
        // against `A(int p1, int p2)` reports "2 expected, but 1 found").
        let open_site = self.open_ctor_site;
        let super_formals_here: Vec<(bool, StringId)> = self
            .pending_super_formals
            .iter()
            .filter(|(owner, ..)| open_site.is_some() && *owner == open_site)
            .map(|(_, _, named, name, ..)| (*named, *name))
            .collect();
        given += super_formals_here.iter().filter(|(named, _)| !*named).count();
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
        // The required-named misses emit in formal-declaration order at the
        // invocation's callee entity (`RequiredParametersVerifier._check`,
        // upstream Dart SDK source, required_parameters_verifier.dart
        // :122-132/:144-166) — a `required` named formal with no same-named
        // argument reports `missing_required_argument`. Coverage mirrors
        // `_containsNamedExpression`'s second walk (:197-218): a named
        // `super.x` formal of the enclosing constructor stands in for the
        // same-named argument — `super_formals_here` is that extra cover.
        let required_anchor = level
            .callee_row
            .and_then(|index| self.rows.get(index))
            .map(|row| row.range)
            .unwrap_or(close);
        self.emit_missing_required(&formals, &level.args, &super_formals_here, required_anchor);
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
                let note = self.record_arity_note(flow_ty, formal.ty);
                let message = SemanticCode::ArgumentTypeNotAssignable.message3(
                    &from_text,
                    &to_text,
                    &note,
                );
                self.emit_message_with_correction(
                    SemanticCode::ArgumentTypeNotAssignable,
                    *range,
                    message,
                );
            } else {
                // The conversion's own face (#2870), same as the main walk.
                self.check_imprecise_int_literal(flow_ty, formal.ty, *literal, *range);
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
            ScopedHit::Found(NamespaceEntry::Decl(file)) => self
                .index
                .get(file)
                .and_then(|els| els.decl(name))
                .and_then(|decl| {
                    (decl.kind == MicroKind::TypeAlias)
                        .then(|| self.foreign_alias_rhs_grounded(decl.var_ty))
                }),
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
        // A mixin-application superclass's unnamed table is the terminal
        // face's — the associated `super.name` forward climbs to the
        // class that DECLARES the constructor, whose own superclass is
        // the chained hop's next link.
        let (face, face_owner, face_name) =
            self.effective_ctor_container(super_container, super_owner, superclass)?;
        let formals = face.ctor_formals.get(&face_name)?;
        Some((face, face_owner, face_name, formals))
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
        while at > 0
            && (bytes[at - 1].is_ascii_alphanumeric()
                || bytes[at - 1] == b'_'
                || bytes[at - 1] == b'$')
        {
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
        while at > 0
            && (bytes[at - 1].is_ascii_alphanumeric()
                || bytes[at - 1] == b'_'
                || bytes[at - 1] == b'$')
        {
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
        let mut at = self.byte_of(tree.range.offset) + self.strings.resolve(tree.head).len();
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
        while at < bytes.len()
            && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_' || bytes[at] == b'$')
        {
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

    /// The assignment write target's static type when the rows below the
    /// callee spell exactly `target = <this send>` (or `??=`): receiver
    /// chain links join by `.` gaps and the target row sits behind a gap
    /// whose only non-whitespace bytes are the assignment operator —
    /// `==`/`=>` are an equality and an arrow, `,`/`:` an argument list,
    /// `?` a conditional, none of them a write (`assignment_expression_
    /// resolver.dart` offers the write context before the RHS resolves).
    /// Bounded to the nearest eight rows; every step is a constant-size
    /// byte probe.
    fn assignment_write_ctx(&mut self, callee: usize) -> Option<Ty> {
        let mut above = self.rows.get(callee)?.range.offset;
        for row in self.rows.get(..callee)?.iter().rev().take(8) {
            let mut at = row.range.end();
            while at < above && self.byte_at(at).is_some_and(|b| b.is_ascii_whitespace()) {
                at += 1;
            }
            let (assigned, len) = match self.byte_at(at) {
                Some(b'=') if self.byte_at(at + 1) != Some(b'=') => (true, 1),
                Some(b'?')
                    if self.byte_at(at + 1) == Some(b'?') && self.byte_at(at + 2) == Some(b'=') =>
                {
                    (true, 3)
                }
                Some(b'.') => (false, 1),
                _ => return None,
            };
            at += len;
            while at < above {
                match self.byte_at(at) {
                    Some(b) if b.is_ascii_whitespace() => at += 1,
                    _ => return None,
                }
            }
            if assigned {
                // The LHS folds one pass behind the send (getter-backed
                // member reads resolve in pass 2), so an `Unknown` row
                // re-derives through the accessor face — the getter's
                // declared return instantiated on the receiver (#2481's
                // read face, `initializer_read_ty`).
                let ty = if row.ty != Ty::Unknown {
                    row.ty
                } else {
                    let row = row.clone();
                    self.initializer_read_ty(&row)
                };
                return (ty != Ty::Unknown).then_some(ty);
            }
            above = row.range.offset;
        }
        None
    }

    /// Whether the container's declaration wrote an extends head no scope
    /// can resolve (`class StateController<StateT> extends
    /// StateNotifier<StateT>` behind a missing package import): the head is
    /// written but names no container, so the clause's super formals are
    /// untypeable in the oracle's model too (its `InvalidType` superclass).
    /// The head must resolve in the DECLARING library's scope, not this
    /// one — privacy included (`dart:collection`'s private `_SplayTree` is
    /// a fully typed superclass even though invisible from here, so the
    /// owner-scoped ask keeps SDK classes constrainable).
    fn declared_extends_unresolved(&self, container: StringId) -> bool {
        let Some((face, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        ) else {
            return false;
        };
        let Some(head) = face.superclass else {
            return false;
        };
        let scope = self.owner_library_of(owner, container).unwrap_or(self.lib);
        resolve_container(self.graph, self.namespaces, self.index, scope, owner, head).is_none()
    }

    /// The created container's instantiation OF `target`, expressed over the
    /// container's own parameters — `B<S, T> extends A<T, S>` asks
    /// `super_instantiation(B, A)` → `[T, S]` (a `B<a, b>` IS an `A<b, a>`).
    /// The downward inference the official solver runs for an instance
    /// creation whose context type is a SUPERINTERFACE of the created class
    /// (`A<int, String> x = new B(3, "hello")` solves `B<String, int>` —
    /// `_inferConstructors`'s `asInstanceOf` climb, type_analyzer.dart's
    /// `constructorsToInfer`/`inferrer.constrainArguments`).
    ///
    /// The walk threads one substitution hop at a time through
    /// [`Self::clause_instantiation`]'s recorded clause arguments: the
    /// current head's own written clause `S<sup_args>` substitutes the
    /// current instantiation into `sup_args` (the clause arguments spell the
    /// CURRENT head's parameters — `B`'s own `S`/`T`, not the subclass's),
    /// yielding `target`'s face over the ORIGINAL container's parameters.
    /// A missing clause, a non-`Ty::Named` clause argument (the capture
    /// records only named heads), a cycle, or the 16-hop cap all answer
    /// `None` — the under-report direction.
    fn super_instantiation(&self, container: StringId, target: StringId) -> Option<Vec<Ty>> {
        if container == target {
            return None;
        }
        let file = self.index.get(self.file)?;
        let params_of = |head: StringId| {
            self.bounds
                .container_params(head)
                .map(|capture| {
                    capture
                        .iter()
                        .map(|(name, _, _)| *name)
                        .collect::<Vec<StringId>>()
                })
                .filter(|names| !names.is_empty())
        };
        // The identity instantiation `container<P…>` — each hop rewrites it
        // through the current head's super-clause.
        let mut current_head = container;
        let mut current_args: Vec<Ty> = params_of(container)?
            .iter()
            .map(|name| Ty::named(*name))
            .collect();
        for _ in 0..16 {
            let superclass = file.container(current_head)?.superclass?;
            let clause = self.clause_instantiation(current_head, superclass)?;
            // The clause capture stores the whole `S<args>` Named as its
            // single entry (overrides.rs's `scan_clause_type`); a bare
            // `extends S` stores the unparameterized head.
            let clause_args = match clause.as_slice() {
                [Ty::Named { args, .. }] if *args != ArgListId::EMPTY => {
                    self.boundary.args.borrow().get(*args).to_vec()
                }
                _ => Vec::new(),
            };
            // Substitute the CURRENT instantiation into the clause
            // arguments: they spell `current_head`'s parameters, whose
            // solutions the current map carries.
            let super_params = params_of(current_head)?;
            let map: Vec<(StringId, Ty)> = super_params
                .iter()
                .copied()
                .zip(current_args.iter().copied())
                .collect();
            let mut next_args = Vec::with_capacity(clause_args.len());
            for arg in &clause_args {
                next_args.push(self.subst_params(*arg, &map)?);
            }
            current_head = superclass;
            current_args = next_args;
            if current_head == target {
                return Some(current_args);
            }
            // A bare `extends S` (no written arguments) instantiates the
            // super face to its own parameters' greatest closure — the
            // climb's next hop substitutes nothing useful; keep the
            // walk honest by refusing to continue on a parameter-ful head
            // whose clause gave no arguments.
            if clause_args.is_empty() && params_of(superclass).is_some() {
                return None;
            }
        }
        None
    }

    /// The givens lookup spelling of an invoked selector: the written
    /// spelling, except the reserved `new` selector — the unnamed
    /// constructor's alias — which keys under the class segment it hangs
    /// off (`C.new(...)` credits the `C`-keyed formal table, the
    /// collector's own `A()`/`A.new` normalization). A bare `.new(`
    /// dot-shorthand names no class and credits nothing (`None`).
    fn givens_key(&self, selector: StringId, selector_range: SourceRange) -> Option<StringId> {
        if selector != self.intern("new") {
            return Some(selector);
        }
        self.creation_class_before(selector_range)
            .map(|(class, _)| class)
    }

    /// The call-site givens index (`_addParametersForArguments`, upstream
    /// Dart SDK source, `pkg/analyzer/lib/src/error/
    /// unused_local_elements_verifier.dart:394-399`): the invocation credits
    /// what it wrote — the positional supply (the parser's total count less
    /// the named entries) and each named label — keyed by the callee's
    /// lookup spelling. Runs ahead of any formals resolution: the
    /// `unused_element_parameter` rule counts call-site givens, not
    /// resolutions.
    fn record_argument_givens(&mut self, key: StringId, level: &ArgLevel, positional: u16) {
        let named = level
            .args
            .iter()
            .filter(|(name, ..)| name.is_some())
            .count() as u16;
        if positional > named {
            let given = self.positional_given.entry(key).or_default();
            *given = (*given).max(positional - named);
        }
        for (given, ..) in &level.args {
            if let Some(given) = given {
                self.named_given.insert((key, *given));
            }
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
        if self.initializer_depth > 0 {
            if let Some((container, selector, bare)) = self.super_invocation_callee(&level) {
                self.check_super_invocation_arguments(
                    &level, positional, close, container, selector, bare,
                );
            } else if let Some((container, selector, _bare)) =
                self.redirecting_invocation_callee(&level)
            {
                self.check_redirecting_invocation_arguments(
                    &level, positional, close, container, selector,
                );
            }
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
                // The dot-shorthand/named-selector send credits what it
                // wrote before any resolution face
                // (`visitDotShorthandConstructorInvocation` /
                // `visitDotShorthandInvocation` → `_addParametersForArguments`,
                // upstream unused_local_elements_verifier.dart:121-134): the
                // latch holds the selector the givens index keys (a `new`
                // selector under its class segment), so `C.new(used)` spares
                // `used` even though no callee row answers here.
                if let Some((selector, selector_range)) = self.pending_bare_creation
                    && let Some(key) = self.givens_key(selector, selector_range)
                {
                    self.record_argument_givens(key, &level, positional);
                }
                self.report_alias_selector_face();
            } else if !self.report_alias_invocation_of_non_function() {
                // The explicit-keyword unnamed creation (`const A()`,
                // `new A()`): no callee row, so the generic walk never
                // saw the argument list — its own count faces run here
                // before the miss handler (upstream
                // `visitInstanceCreationExpression` →
                // `resolveArgumentsToParameters`).
                if let Some((head, head_range)) = level.unnamed_head
                    // A `const p.C(...)` head resolves through the import
                    // prefix — `resolve_container` on the bare spelling
                    // would read a same-named local class's formals; the
                    // prefixed fold's own face handles it.
                    && self
                        .written_prefix_before(head_range.offset)
                        .is_none()
                {
                    self.check_explicit_creation_arguments(&level, positional, close, head);
                }
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
            let alias_head: Option<(StringId, SourceRange, bool)> =
                self.rows.get(callee).and_then(|row| {
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
                    self.emit(
                        SemanticCode::CreationWithNonType,
                        head_range,
                        &name_text,
                        "",
                    );
                    return;
                }
                if !bare && let Some(selector) = self.rows.get(callee) {
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
            // The void-receiver face is upstream's first arm
            // (`extension_member_resolver.dart:287-288` — `if (receiverType
            // is VoidType)` reports `use_of_void_result` at the receiver
            // expression before the assignability ask). `check_extension_
            // override_argument` suppresses the not-assignable row on the
            // same void receiver, so the verdict rides here instead of
            // inside that callee (the helper's own suppression keeps its
            // `else if` semantics).
            if let Some((_, ty, _, arg_range)) = level.args.first()
                && *ty == Ty::Void
            {
                // The receiver EXPRESSION is upstream's anchor
                // (`useOfVoidResult.at(receiverExpression)` — the whole
                // invoked expression). The row's `arg_range` spells only
                // the callee name (`v`), so `invocation_span` widens it to
                // the full `v()` (call_ends' callee-offset → closing `)`)
                // — an uninvoked receiver keeps its own span.
                let range = self.invocation_span(*arg_range).unwrap_or(*arg_range);
                self.emit(SemanticCode::UseOfVoidResult, range, "", "");
            }
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
                .filter(|(decl, owner)| {
                    !self.unnamed_ctor_exists(*decl, *owner, *name)
                })
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
            .is_some_and(|(decl, owner)| {
                !self.unnamed_ctor_exists(decl, owner, name)
            })
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
                        ..
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
                .is_some_and(|(decl, owner)| {
                    // The mixin-application face forwards the terminal
                    // class's generative constructors (`const A1.b1(…)`
                    // over `class A1 = B<int> with M1;` resolves `B`'s
                    // `b1`). A broken chain keeps the own-face read.
                    self.effective_ctor_container(decl, owner, container)
                        .is_some_and(|(face, face_owner, _)| {
                            face.constructors.contains(&row.name)
                                && (std::ptr::eq(face, decl)
                                    || !self.ctor_is_factory(face, face_owner, row.name))
                        })
                });
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
        // `_addParametersForArguments` face) keys by callee spelling alone
        // and must not wait on the formals: a receiver this face cannot
        // type (a creation-folded `C().m(...)` head) still supplies its
        // arguments. The parser's count is exact where the row recovery is
        // not: wide positional windows (compound operands) stay
        // unattributed above, so the positional supply is the total count
        // less the attributed named entries. Callee-spelling keys — a
        // same-named member elsewhere suppresses, never false-positives.
        let callee_spelling = row.name;
        // The given-named face registers before the formals gate: an
        // invocation whose formals cannot resolve here (an
        // initializer-list redirect `this._(v: v)`, an enum-constant
        // argument list) still supplies what it wrote — the
        // `unused_element_parameter` rule counts call-site givens, not
        // resolutions. The reserved `new` selector keys under its class
        // segment — the spelling the constructor's formal table sealed on.
        // Field-level writes ([`Self::record_argument_givens`] takes `&mut
        // self`, and the callee row's borrow stays open for the formals
        // match below).
        if let Some(key) = self.givens_key(row.name, row.range) {
            let named = level
                .args
                .iter()
                .filter(|(name, ..)| name.is_some())
                .count() as u16;
            if positional > named {
                let given = self.positional_given.entry(key).or_default();
                *given = (*given).max(positional - named);
            }
            for (given, ..) in &level.args {
                if let Some(given) = given {
                    self.named_given.insert((key, *given));
                }
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
        // The ctor-use edge the named-selector send arms (#2931): latched
        // inside the `Receiver::Container` arm — the callee-row borrow
        // ends with the `formals` match, so the edge publishes just after.
        let mut ctor_use: Option<(StringId, StringId, SourceRange)> = None;
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
                // The lexical lookup sees only the enclosing body's OWN
                // members (scope.dart `InstanceScope` / `ExtensionScope`
                // add the element's declared getters/setters/methods): an
                // inherited or `on`-type member never hides a top-level
                // declaration in scope — `shuffle(this, ...)` inside
                // `extension on List<E>` calls the library's `shuffle`,
                // not `List.shuffle`. The implicit-this face applies only
                // when the lexical lookup found nothing.
                let lexical_top = self.container_context().is_some_and(|(container, _)| {
                    let own = match self.unnamed_extension_members() {
                        Some(members) => Some(members.get(&row.name).is_some()),
                        None => self
                            .index
                            .get(self.file)
                            .and_then(|f| f.container(container))
                            .map(|c| c.members.get(&row.name).is_some()),
                    };
                    own == Some(false) && self.top_level_decl(row.name).is_some()
                });
                let shadowed = self.frames.iter().any(|f| f.locals.contains(&row.name));
                let member_gate = shadowed || lexical_top;
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
                    None => (!member_gate)
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
                let member_found = !member_gate
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
                        .and_then(|(container, kind)| {
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
                        })
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
                    } else if self.fn_param_sigs.contains_key(&row.name) {
                        // A parameter-claimed spelling with no frame
                        // binding it here is another executable's formal
                        // (`rel(int Function() topF)` beside a call of the
                        // top-level `topF`): the out-of-scope parameter
                        // must not lend its signature.
                        None
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
                            ..
                        }
                        | Receiver::Container {
                            name,
                            kind: ContainerKind::Class,
                        } => {
                            // The ctor-use edge (#2931) arms only over the
                            // class-reference receiver (`B.named(`): a
                            // folded `B()` below the callee spells a member
                            // call on the created instance, never the
                            // constructor face — `Created` stays out.
                            if let Receiver::Container { .. } = recv {
                                ctor_use = callee.checked_sub(1).and_then(|index| {
                                    self.rows.get(index).map(|below| {
                                        (
                                            name,
                                            row.name,
                                            SourceRange {
                                                offset: below.range.offset,
                                                length: row
                                                    .range
                                                    .end()
                                                    .saturating_sub(below.range.offset),
                                            },
                                        )
                                    })
                                });
                            }
                            // The factory lane joins the plain table's
                            // lookup (a redirecting factory's own written
                            // formals), but never for a closure-carrying
                            // send: the formals would type the closures'
                            // own parameters through the written schema, a
                            // substitution chain this model under-seeds
                            // (the `StreamTransformer.fromHandlers`
                            // nullability cascade) — the closure chain
                            // keeps the prior under-report.
                            if level.closures.is_empty() {
                                self.ctor_schema_formals(name, row.name)
                                    .or_else(|| self.forwarded_ctor_formals(name, row.name))
                                    .or_else(|| self.factory_ctor_formals(name, row.name))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    },
                }
            }
            // The unnamed creation's head (`B(x)`, #1351): the folded
            // container row carries the class — its unnamed face's formals
            // (the collector keys the container's own name). A foreign
            // class reaches the graph-resolved table only on the
            // provably-unconstrainable face (#2687: its extends head
            // resolves nowhere, so the formals carry no decidable type
            // either and the send needs the instantiation gate below);
            // every other foreign creation keeps the under-report.
            RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Class | ContainerKind::ExtensionType,
            }) if !row.qualified => {
                let formals = self
                    .index
                    .get(self.file)
                    .and_then(|f| f.container(*name))
                    .and_then(|c| c.ctor_formals.get(name))
                    .map(|f| (**f).clone())
                    .or_else(|| self.forwarded_ctor_formals(*name, *name))
                    .or_else(|| {
                        self.declared_extends_unresolved(*name)
                            .then(|| self.ctor_schema_formals(*name, *name))
                            .flatten()
                    });
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
        // The ctor-use edge publishes here (#2931): the callee-row borrow
        // ended with the `formals` match, and the existence gate inside
        // `note_ctor_use` keeps a selector that is no constructor silent
        // (the `undefined_method` family owns that report upstream).
        if let Some((class, selector, span)) = ctor_use {
            self.note_ctor_use(class, selector, span);
        }
        // A nullable receiver whose member a nullable-`on` extension
        // declares resolves through the extension, never the instance
        // member (`extension E on C? { m }` claims `C? x; x.m(a)` — the
        // same verdict `member_defined`'s nullable-dereference face takes):
        // the instance formals must not check the arguments. The
        // extension's own table is not projected here (under-report).
        let formals = match send_recv {
            Some(Receiver::Typed {
                type_name,
                ty: ty @ Ty::Named { nullable: true, .. },
            }) if formals.is_some() => {
                let claimed = matches!(
                    self.extension_application(type_name, ty, callee_spelling, false, true),
                    ExtensionApp::Single
                );
                self.members.ext_ref.set(None);
                if claimed { None } else { formals }
            }
            _ => formals,
        };
        // Scoped: the #2831 P1 call below needs `&mut self` to emit, so
        // the callee-row borrow ends at the copied range and re-opens
        // after the emitting face (which pushes no rows).
        let callee_range = {
            let Some(row) = self.rows.get(callee) else {
                return;
            };
            row.range
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
        let mut implicit_call: Option<(Ty, Box<[(StringId, Option<Ty>)]>)> = None;
        let formals = formals.or_else(|| {
            let row = self.rows.get(callee)?;
            let ty = self.receiver_static_ty(row);
            let (formals, ret, params) = self.implicit_call_face(ty)?;
            implicit_call = Some((ret, params));
            Some(formals)
        });
        // #2831 P1 — the raw-constructor inference face runs before the
        // formal table's early return: a ctor-less class's implicit
        // default has no captured formals, and this face needs none (the
        // `could_not_infer` seam below only admits its pinned
        // outer-call shape and never reaches here for the corpus's bare
        // statement creations). The engine-side wrapper owns the shape,
        // context, and explicit-argument guards and emits both rows.
        self.check_creation_inference(callee);
        // #2869 — the creation bounds face rides the same seam: a send
        // whose head has no formals returns below, and its written type
        // arguments still check against the head's substituted bounds
        // (the class's own, the alias's, or the executable's window).
        // The formals path checks at the argument walk's wiring below.
        if formals.is_none() {
            self.check_creation_type_arg_bounds(callee);
        }
        // A function-TYPE callee (`void Function({required int x}) f;
        // f();`) carries no element formals but the captured signature's
        // `named` table spells `required` — upstream
        // `visitFunctionExpressionInvocation`'s `staticInvokeType is
        // FunctionType` arm checks the required-named coverage against the
        // signature's formals (required_parameters_verifier.dart:60-67).
        // Rebind `formals` from the signature so the coverage walk below
        // sees the same table the member/top-level walks consume.
        let formals = formals.or_else(|| {
            self.rows
                .get(callee)
                .filter(|row| !row.qualified)
                .and_then(|row| self.fn_var_sigs.get(&row.name))
                .map(|sig| sig.formals(self.intern("")))
        });
        let Some(formals) = formals else {
            // #3495 — the EXTENSION-member send's bounds face. A member the
            // receiver's container never declares lives on an APPLIED
            // extension: [`Self::member_formals_full`] walks the container
            // only, so the send lands here with no formals — and with no
            // `member_send_params` table either, which is why the written
            // type arguments checked nothing. Upstream runs
            // `_checkInvocationTypeArguments` over the INSTANTIATED member
            // type (`checkMethodInvocation` reads
            // `node.function.staticType`, upstream Dart SDK source,
            // pkg/analyzer/lib/src/error/type_arguments_verifier.dart
            // :236-240 + :570-633): the extension's own parameters bind by
            // the application first, the member's own by the written
            // group, so `T expectStaticType<R extends Exactly<T>>()` over
            // an `int` receiver reports against `Exactly<int>`.
            self.check_extension_member_type_arg_bounds(callee, send_recv);
            return;
        };
        let Some(row) = self.rows.get(callee) else {
            return;
        };
        // The required-named coverage (`RequiredParametersVerifier._check`,
        // required_parameters_verifier.dart:123-156): a `required` named
        // formal with no same-named argument reports
        // `missing_required_argument` at the invocation's callee entity
        // (the constructor reference / invoked name — the row's span). The
        // enclosing-constructor super-formal forwarding face
        // (`_containsNamedExpression`'s second walk) is the under-report
        // direction this model documents elsewhere. Emitted at the count
        // block below once the callee-row borrow ends (the row's last
        // reader).
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
        // The named-selector creation's class segment (`G.value(1)` → the
        // `G` row's own offset): the creation latches key on it, so the
        // argument-solved instantiation below publishes there for the
        // readers that resolve a created row's type (`creation_row_ty`, and
        // through it `callee_return_ty`'s generic-constructor arm).
        let mut creation_head: Option<u32> = None;
        let (creation_container, mut return_face) = match &row.shape {
            RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Class,
            }) if !row.qualified => (
                Some(*name),
                self.top_level_decl(row.name).map(|decl| decl.return_ty),
            ),
            RowShape::Bare(BareInfo::Plain { .. }) if row.qualified => {
                let below = callee.checked_sub(1).and_then(|index| self.rows.get(index));
                let recv = below.map_or(Receiver::Unknown, |below| self.argument_receiver(below));
                // Only a CLASS-HEAD receiver whose selector names a DECLARED
                // constructor is a creation of that head: a static member on
                // the same head (`G.make(1)`) keeps its declared return, and
                // an instance receiver (`C().m(1)`) is no creation — its
                // send must never type the inner creation.
                if let (Receiver::Container {
                    name,
                    kind: ContainerKind::Class,
                }, Some(below)) = (&recv, below)
                    && resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        *name,
                    )
                    .is_some_and(|(decl, _)| self.ctor_face_exists(decl, *name, row.name))
                {
                    creation_head = Some(below.range.offset);
                }
                let container = match recv {
                    Receiver::Created {
                        name,
                        kind: ContainerKind::Class,
                        ..
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
        // The invocation-site bounds verdict's parameter table — the
        // DECLARED bounds the callee's own element carries, held apart
        // from `exec_params` (the inference seeds, whose `own_bound` arm
        // answers `None` outside its context-free corner by design). The
        // `exec_bounds` window and every signature arm below already
        // spell `(name, declared bound)` — those convert straight in (the
        // two faces of a [`TypeArgBound`] coincide for a bound that is
        // already a type); the container/alias arms split the faces
        // (#3510), and the member/implicit-call arms fill it from their
        // own declared facts.
        let mut decl_bounds: Option<Box<[TypeArgBound]>> = exec_params
            .as_deref()
            .map(|pairs| pairs.iter().copied().map(TypeArgBound::from).collect());
        // #1351: the creation faces' parameters come from the container's
        // bounds capture (its type parameters with their written bounds).
        // The inference seeds keep the written face; the verdict table
        // judges the snapshot-expanded one and renders the written one.
        if exec_params.is_none()
            && let Some(container) = creation_container
            && self
                .bounds
                .container_params(container)
                .is_some_and(|params| !params.is_empty())
            && let Some(rows) = self.captured_type_arg_bounds(container)
        {
            // The instance-creation verdict (`checkInstanceCreation`,
            // type_arguments_verifier.dart:236-240): `C<String>(x)` over
            // `C<T extends num>` — the class's own bounds.
            decl_bounds = Some(rows.iter().copied().collect());
            exec_params = Some(rows.iter().map(|row| (row.name, row.written)).collect());
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
        // The alias-callee route (#2841's bounds half): a creation spelled
        // through a non-function type alias checks the ALIAS's own
        // parameter bounds (`checkNamedType` reads
        // `alias.element.typeParameters` on the alias application,
        // type_arguments_verifier.dart:371-375). A poisoned bound keeps
        // the name-only shape — recovery residue is never a fact, the
        // same rule the executable windows capture by.
        if exec_params.is_none()
            && let Some(head) = creation_container
            && self.bounds.alias_named_shape(head).is_some()
            && let Some(rows) = self.captured_type_arg_bounds(head)
        {
            decl_bounds = Some(rows.iter().copied().collect());
            exec_params = Some(rows.iter().map(|row| (row.name, row.written)).collect());
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
            // The signature's `params` already pair the declared bounds —
            // `fnVar<String>(x)` over `T Function<T extends num>(T)` is
            // the function-expression-invocation verdict.
            decl_bounds = Some(sig.params.iter().copied().map(TypeArgBound::from).collect());
        }
        // A function-typed value invoked with explicit type arguments
        // (`fn<String>(x)` over `fn: T Function<T extends num>(T)`, or a
        // parenthesized generic literal `(<T>() {})<String>()`): the
        // callee row's `Ty::Function` shape carries the declared
        // `type_params_named` — the function-expression-invocation
        // verdict (`checkFunctionExpressionInvocation`,
        // type_arguments_verifier.dart:193-196) — which neither the
        // element model's `exec_bounds` window (no declaration) nor the
        // inference seeds answer.
        if decl_bounds.is_none()
            && let Ty::Function { shape, .. } = row.ty
        {
            let named = self
                .boundary
                .pool()
                .function_shape(shape)
                .type_params_named
                .clone();
            if !named.is_empty() {
                decl_bounds = Some(named.into_iter().map(TypeArgBound::from).collect());
            }
        }
        // The implicit-`call` face's parameters (the call method's own
        // type parameters — `R call<R>(R x)` solves `R` like any generic
        // send's, downward from the context type, upward from the first
        // matched argument).
        if exec_params.is_none()
            && let Some((_, params)) = &implicit_call
            && !params.is_empty()
        {
            exec_params = Some(params.iter().map(|(param, _)| (*param, None)).collect());
            // The call method's declared bounds answer the
            // function-expression-invocation verdict (`checkFunction-
            // ExpressionInvocation`, upstream Dart SDK source,
            // pkg/analyzer/lib/src/error/type_arguments_verifier.dart
            // :193-196 — `c<int>()` resolves `call`'s own parameters).
            decl_bounds = Some(params.iter().copied().map(TypeArgBound::from).collect());
        }
        // The member send's inference parameters (`reader.readEnum(...)`,
        // `this.adopt(...)`, `box.get()`): the found method's own
        // `type_param_names` — keyed by the member walk, so a same-spelled
        // member elsewhere never lends them — plus the declaring
        // container's captured parameters, which the receiver's written
        // instantiation seeds (`Box<int>.get()` binds `T := int`). The
        // method's own bounds ride the entry (see `member_send_params`).
        let mut receiver_seeds: Vec<(StringId, Ty)> = Vec::new();
        if exec_params.is_none()
            && let Some(recv) = send_recv
            && let Some((params, seeds, bounds)) = self.member_send_params(&recv, row.name)
        {
            receiver_seeds = seeds;
            exec_params = Some(params);
            // The method's declared bounds are the method-invocation
            // verdict's table (`checkMethodInvocation`,
            // type_arguments_verifier.dart:236-240): `reader.readEnum<
            // String>(...)` checks `String` against the method's own
            // `T extends Enum` bound — the inference seeds' gated
            // `own_bound` never answers it.
            decl_bounds = Some(bounds.iter().copied().map(TypeArgBound::from).collect());
        }
        // The member send's bounds verdict never rides `exec_bounds`: a
        // same-spelled top-level `m<T>`'s window is not the method's
        // table (the `exec_bounds` spelling collides across a send's
        // receiver). Whenever a receiver drove the send, the method's own
        // `type_param_bounds` — keyed by the member walk — decides; a
        // non-generic member yields an empty table (its own silent face).
        if let Some(recv) = send_recv
            && let Some((_, _, bounds)) = self.member_send_params(&recv, row.name)
        {
            decl_bounds = Some(bounds.iter().copied().map(TypeArgBound::from).collect());
        }
        // #2516 robustness: a same-file top-level generic callee whose
        // `exec_bounds` window never paired (the bounds capture can miss
        // when a header's `<T>` group closes through a path the pairing
        // latch does not see — the element model's declaration is
        // complete regardless) falls back to the projected declaration's
        // own parameter spellings — the same source
        // `closure_callee_params`'s foreign arm reads. Receiver/member
        // sends and member-owned bare callees never reach here: their
        // parameters already answered above or belong to the method's
        // own table.
        if exec_params.is_none()
            && matches!(&row.shape, RowShape::Bare(BareInfo::Plain { .. }))
            && !row.qualified
            && !member_owns_callee
            && let Some(decl) = self.top_level_decl(callee_spelling)
            && decl.type_param_names.len() == usize::from(decl.type_params)
            && !decl.type_param_names.is_empty()
        {
            let params: Vec<(StringId, Option<Ty>)> = decl
                .type_param_names
                .iter()
                .map(|name| (*name, None))
                .collect();
            decl_bounds = Some(params.iter().copied().map(TypeArgBound::from).collect());
            exec_params = Some(params.into());
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
        let named = level
            .args
            .iter()
            .filter(|(name, ..)| name.is_some())
            .count() as u16;
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
        self.emit_missing_required(&formals, &level.args, &[], callee_range);
        // Per-parameter solution slots (`None` = unsolved).
        let mut solutions: Box<[Option<Ty>]> = exec_params
            .as_ref()
            .map(|params| vec![None; params.len()].into())
            .unwrap_or_default();
        // The slots the invocation's own type-argument group fixed
        // positionally: the oracle's downward phase writes them before any
        // inference and the effective face keeps the WRITTEN argument even
        // when it violates the declared bound (the bound check reports the
        // violation separately — upstream Dart SDK source, pkg/analyzer/
        // lib/src/error/type_arguments_verifier.dart:570-633); only an
        // inferred slot clamps to its bound.
        let mut explicit_fixed: FxHashSet<usize> = FxHashSet::default();
        // The slots whose upward candidates disagreed: upstream's LUB merge
        // solves them to a type this model cannot compute, so their formals
        // stay undecidable rather than reading the parameter's own spelling
        // (a callee `S` beside a caller's same-spelled `S`).
        let mut disagreed: FxHashSet<usize> = FxHashSet::default();
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
        // The invocation-site bounds face (#2869 + the C2 wiring): the
        // written type arguments check against the callee's DECLARED
        // bounds before the argument walk
        // (`_checkInvocationTypeArguments`, upstream Dart SDK source,
        // pkg/analyzer/lib/src/error/type_arguments_verifier.dart
        // :570-633 — the same `checkMethodInvocation`/
        // `checkFunctionExpressionInvocation`/`checkInstanceCreation`
        // drive it). The verdict reads `decl_bounds` — the member send's
        // `type_param_bounds`, the implicit-`call` method's bounds, and
        // the container/alias/signature captures — never the inference
        // seeds: an `own_bound`-gated `None` is not an absent bound, and
        // an inference-seeded bound is not the declared one.
        if let (Some(params), Some(args)) = (&decl_bounds, &explicit_args) {
            self.check_call_type_arg_bounds(&params[..], args);
        }
        // #2890 — the super-send bounds face: `super.m<int>(...)` resolves
        // through the SUPERCLASS's member table, which neither
        // `member_send_params` nor the `exec_bounds` window answers, so the
        // written arguments checked nothing (#2869 residual). The send
        // closes before the dot-access merge — the callee is the unmerged
        // qualified member placeholder with the receiver row directly
        // below (the same geometry the formals read) — and the method's
        // own parameters decide, substituted with the written arguments:
        // the same `_checkInvocationTypeArguments` verdict
        // (`checkMethodInvocation`, upstream Dart SDK source,
        // pkg/analyzer/lib/src/error/type_arguments_verifier.dart:236-240),
        // the report anchored at the offending argument's node. Gated on
        // `exec_params.is_none()` so the chain's first-match-wins posture
        // holds and an answered send never checks twice.
        if exec_params.is_none()
            && let Some(args) = &explicit_args
            && let Some(row) = self.rows.get(callee)
            && row.qualified
            && matches!(&row.shape, RowShape::Bare(BareInfo::Plain { .. }))
            && let Receiver::Super { container, .. } = callee
                .checked_sub(1)
                .and_then(|index| self.rows.get(index))
                .map_or(Receiver::Unknown, |below| self.argument_receiver(below))
            && let Walk::Found(entry) = self.walk_super(container, callee_spelling)
            && entry.kind == MemberKind::Method
            && !entry.type_param_names.is_empty()
        {
            let params: Vec<(StringId, Option<Ty>)> = entry
                .type_param_names
                .iter()
                .enumerate()
                .map(|(index, name)| (*name, entry.type_param_bounds.get(index).copied()))
                .collect();
            self.check_call_type_arg_bounds(&params, args);
        }
        
        if let Some(params) = &exec_params {
            let param_index = |name: StringId| params.iter().position(|(param, _)| *param == name);
            // The explicit fix precedes every inference face.
            if let Some(explicit) = &explicit_args {
                for (index, (slot, ty)) in solutions
                    .iter_mut()
                    .zip(explicit.tys.iter())
                    .enumerate()
                {
                    *slot = Some(*ty);
                    explicit_fixed.insert(index);
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
            // A(null);` — the official solver's downward inference). A
            // SUPERINTERFACE context climbs the declared clause chain
            // (`A<int, String> x = new B(3, "hello")` over
            // `B<S, T> extends A<T, S>` solves `B<String, int>` —
            // `asInstanceOf`'s hop substitution), each context argument
            // unifying against the target face's parameter spelling.
            if let Some(container) = creation_container
                && let Some(Ty::Named {
                    name: head,
                    args,
                    nullable: false,
                }) = self.pending_decl_ty
                && args != ArgListId::EMPTY
            {
                let inst: Option<Vec<Ty>> = if head == container {
                    self.bounds
                        .container_params(container)
                        .map(|capture| capture.iter().map(|(p, _, _)| Ty::named(*p)).collect())
                } else {
                    self.super_instantiation(container, head)
                };
                if let Some(inst) = inst {
                    let ctx_args: Vec<Ty> = self.boundary.args.borrow().get(args).to_vec();
                    if ctx_args.len() == inst.len() {
                        for (formal, actual) in inst.iter().zip(ctx_args.iter()) {
                            // Unify the target-face argument (a parameter
                            // spelling or a compound over them) against the
                            // context argument — `unify_upward`'s own
                            // recursion (`A<T, S>` vs `A<int, String>`
                            // binds `T := int`, `S := String`).
                            let mut candidates = Vec::new();
                            self.unify_upward(*formal, *actual, params, &mut candidates);
                            for (param, ty) in candidates {
                                if let Some(index) = param_index(param)
                                    && solutions[index].is_none()
                                {
                                    solutions[index] = Some(ty);
                                }
                            }
                        }
                    }
                }
            }
            // The assignment write context (#2687): the LHS of the `=` the
            // send is the RHS of (`_controllerNotifier.result =
            // $Result.data(controller)` — the field's static type is the
            // send's contextType, assignment_expression_resolver.dart's
            // write context). The rows below the callee carry the folded
            // LHS; the byte gap between it and the send's receiver chain
            // discriminates the assignment (`=`/`??=`) from every other
            // join (`.` chain, `,`/`:` argument, `?` conditional). The
            // send's own parameter list (foreign-aware, unlike the
            // bounds capture) pairs the context's written arguments. A
            // send carrying closure arguments stays out: its formals
            // then type the closures' own parameters, a substitution
            // chain whose nullability face this model under-seeds (the
            // `StreamTransformer.fromHandlers` cascade) — under-report
            // over a wrong binding.
            let write_ctx = self
                .assignment_write_ctx(callee)
                .filter(|_| level.closures.is_empty());
            if let Some(container) = creation_container
                && let Some(Ty::Named {
                    name: head, args, ..
                }) = write_ctx
                && args != ArgListId::EMPTY
            {
                // The same superinterface climb the declared-context arm
                // runs: `x = new B(...)` where `x: A<…>` and
                // `B extends A<…>` solves through the clause chain.
                let inst: Option<Vec<Ty>> = if head == container {
                    self.bounds
                        .container_params(container)
                        .map(|capture| capture.iter().map(|(p, _, _)| Ty::named(*p)).collect())
                } else {
                    self.super_instantiation(container, head)
                };
                if let Some(inst) = inst {
                    let ctx_args: Vec<Ty> = self.boundary.args.borrow().get(args).to_vec();
                    if ctx_args.len() == inst.len() {
                        for (formal, actual) in inst.iter().zip(ctx_args.iter()) {
                            let mut candidates = Vec::new();
                            self.unify_upward(*formal, *actual, params, &mut candidates);
                            for (param, ty) in candidates {
                                if let Some(index) = param_index(param)
                                    && solutions[index].is_none()
                                {
                                    solutions[index] = Some(ty);
                                }
                            }
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
            for (name, ty, _literal, range) in &level.args {
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
                // A function-typed argument whose row carries no `Ty` — a
                // tear-off's signature stashes in `arg_sigs`, a closure
                // literal's in `level.closures` — still constrains the
                // call's parameters: upstream binds `T` from the argument's
                // function type (`test<Function>(foo, bar)` solves
                // `T := void Function()` — generic_inferrer.dart's
                // `TypeConstraintFromArgument`). The signature interns to
                // the pool's `Ty::Function` and unifies like any typed
                // argument.
                let arg_ty = if *ty != Ty::Unknown {
                    *ty
                } else {
                    let sig: Option<Box<FnVarSig>> =
                        self.arg_sigs.get(&range.offset).cloned().or_else(|| {
                            level
                                .closures
                                .iter()
                                .find(|(start, end, _)| {
                                    range.offset >= *start && range.offset <= *end
                                })
                                .map(|(.., sig)| sig.clone())
                        });
                    match sig {
                        Some(sig) => self.fn_sig_ty(&sig),
                        // #2516: solve-only — a member-read/getter
                        // argument's resolved return constrains the
                        // parameters; `level.args` keeps `Unknown` for the
                        // assignability verdict.
                        None => match self.solve_arg_member_ty(range.offset) {
                            Some(ty) => ty,
                            None => continue,
                        },
                    }
                };
                if arg_ty != Ty::Unknown {
                    self.unify_upward(formal.ty, arg_ty, params, &mut candidates);
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
            disagreed = poisoned;
            // The real constraint solve (#2938): `GenericInferrer`'s
            // gather → choose → check pipeline. Written type arguments
            // bypass it entirely (upstream's `typeArgumentList != null`
            // arm never builds an inferrer); the narrow creation seam
            // above keeps its own pinned outer-call shape — every other
            // send with inference parameters runs here. The solved types
            // fill the still-open slots (the substituted formals the
            // argument walk checks and the instantiated return write
            // consume them).
            if explicit_args.is_none() {
                
                let infer_below = self
                    .rows
                    .get(callee)
                    .filter(|row| row.qualified)
                    .and_then(|_| callee.checked_sub(1))
                    .and_then(|index| self.rows.get(index))
                    .map(|row| row.range);
                self.infer_invocation(
                    callee,
                    callee_range,
                    callee_spelling,
                    send_recv,
                    infer_below,
                    params,
                    &formals,
                    &level,
                    return_face,
                    unnamed_creation,
                    &mut solutions,
                );
            }
        }
        // The downward closure-formal re-bind (#2500): literal closure
        // arguments with untyped formals were staged as
        // `DeferredClosureFormal` when the pass-1 schema face could not
        // solve the callee's substituted signature. Now that the send
        // closed, the callee's formals + the merged solutions answer the
        // open formal slots — bind them into `rebound_locals` so the
        // `unused_element` tally sees the member read through the typed
        // formal. Runs regardless of `exec_params`: a non-generic member
        // (e.g. `NodeList<Annotation>.where` inherited from `Iterable`)
        // yields no inference parameters yet still owes the formal its
        // receiver-instantiated `E`.
        {
            let rebind_below = self
                .rows
                .get(callee)
                .filter(|row| row.qualified)
                .and_then(|_| callee.checked_sub(1))
                .and_then(|index| self.rows.get(index))
                .map(|row| row.range);
            let (rebind_params, rebind_solutions) = match &exec_params {
                Some(params) => (&params[..], &solutions[..]),
                None => (&[][..], &[][..]),
            };
            self.infer_rebind_closure_formals(
                callee_spelling,
                send_recv,
                rebind_below,
                rebind_params,
                rebind_solutions,
                &level,
                &formals,
            );
            // The generic tear-off argument's own inference stands alone
            // from the callee's: `h(f)` where `h` is NON-generic still
            // runs `inferFunctionTypeInstantiation` on `f` against the
            // formal's function context (`resolver.dart`'s typedLiteral
            // walk — `constrainGenericFunctionInContext` is per-argument).
            // The `exec_params` send routes the same call inside
            // `infer_invocation`'s argument round, so this arm stays
            // gated to the callee without inference parameters.
            if exec_params.is_none() && explicit_args.is_none() {
                let mut positional_index = 0usize;
                for (name, _, _, range) in &level.args {
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
                    let arg_sig: Option<Box<FnVarSig>> =
                        self.arg_sigs.get(&range.offset).cloned().or_else(|| {
                            level
                                .closures
                                .iter()
                                .find(|(start, end, _)| {
                                    range.offset >= *start && range.offset <= *end
                                })
                                .map(|(.., sig)| sig.clone())
                        });
                    let Some(sig) = arg_sig else {
                        continue;
                    };
                    let Some(tear_params) = self.tear_off_arg_params(range.offset) else {
                        continue;
                    };
                    if tear_params.is_empty() {
                        continue;
                    }
                    if !matches!(self.fold_alias_ty(formal.ty), Ty::Function { .. }) {
                        continue;
                    }
                    let mut sig = (*sig).clone();
                    sig.params = tear_params;
                    let _ = self.infer_tear_off(&sig, formal.ty, *range);
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
                .enumerate()
                .map(
                    |(index, ((param, bound), solution))| match (solution, bound) {
                        _ if disagreed.contains(&index) => Ty::UNKNOWN,
                        (Some(ty), _) if explicit_fixed.contains(&index) => *ty,
                        (Some(ty), None) => *ty,
                        (Some(ty), Some(bound))
                            if self.assignable(*ty, *bound, false) != Some(false) =>
                        {
                            *ty
                        }
                        (_, Some(bound)) => *bound,
                        (None, None) => Ty::named(*param),
                    },
                )
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
            // The substituted parameter type (#2873) — computed BEFORE the
            // function-subtype face so the tear-off rejection decides and
            // displays on the bound parameter (`test<Function>(foo, bar)`
            // pairs `void Function()` against the WRITTEN `Function`, never
            // the callee's own out-of-scope `T` spelling). The substitution
            // keeps every non-type-parameter callee byte-identical: a bare
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
            // The void-operand use (upstream `checkForUseOfVoidResult`
            // over the argument expression, error_detection_helpers.dart:
            // 245-258): `g(v())` consumes the void value unless the formal
            // is itself `void` (`g(void x)` accepts it). The row reports
            // at the argument's use-site span. `operand_static_ty`
            // resolves the callee's `void` return — the level arg's `ty`
            // spells an invoke `Unknown`.
            let operand_row = self
                .rows
                .iter()
                .rev()
                .find(|row| row.range == *range)
                .cloned();
            let operand_ty = operand_row
                .as_ref()
                .map(|row| self.operand_static_ty(row))
                .unwrap_or(*ty);
            if operand_ty == Ty::Void && formal_ty != Ty::Void {
                let mut span = operand_row
                    .as_ref()
                    .map(|row| self.use_site_range(row))
                    .unwrap_or(*range);
                // A NAMED argument's operand is the `name: value` entry —
                // the oracle's `at(expression)` anchors the whole
                // `NamedExpression`, so the span re-opens at the label.
                if name.is_some() {
                    span = self.label_span(span).unwrap_or(span);
                }
                self.emit(
                    SemanticCode::UseOfVoidResult,
                    span,
                    "",
                    "",
                );
                continue;
            }
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
            // The tear-off half reads the bare-read stash; a closure
            // LITERAL argument's signature rides `level.closures` instead
            // (type_capture.rs's `closure_capture_end` — the literal's
            // own Value row spans only its end token or the context-typed
            // literal window, so the pairing key is containment, never
            // the offset key the tear-off table uses). Upstream checks
            // both faces through the same per-argument
            // `correspondingParameter` walk (resolver.dart:522,
            // `checkForArgumentTypesNotAssignableInList`) — a `() => 0`
            // against `void Function(int)` reports exactly like a
            // tear-off. A block-bodied closure capture's `FnVarSig`
            // return stays the `void` STUB (`closure_capture_end` bakes
            // the `=>` body's resolved expression type instead — an
            // `=>` literal's own inference is overwritten by the re-type
            // below either way), never a real inference for the ask:
            // the formal's return is the downward
            // context the literal re-infers under (`inferLocalFunction`'s
            // cap — the `S = R` posture the capture's own Value row
            // carries), so the stub re-types BEFORE the ask and the
            // decision rides the arity and parameter slots (#3061's
            // re-decide, applied up front).
            let tear_off_sig = self.arg_sigs.remove(&range.offset);
            let closure_sig = if tear_off_sig.is_none() {
                level
                    .closures
                    .iter()
                    .find(|(start, end, _)| range.offset >= *start && range.offset <= *end)
                    .map(|(.., sig)| sig.clone())
            } else {
                None
            };
            let arg_sig = tear_off_sig.or(closure_sig.clone());
            if let Some(arg_sig) = arg_sig {
                if let Some(formal_sig) = formal_sig {
                    // The literal's downward face runs BEFORE the ask: a
                    // closure stub's `void` return is never an inference —
                    // the formal's return is the context the body re-infers
                    // under (`inferLocalFunction`'s cap), so the stub
                    // re-types and ONE ask decides on the real faces: the
                    // return covariance reads the context return, and the
                    // arity/parameter slots produce the row the oracle's
                    // `checkForArgumentTypesNotAssignableInList` emits
                    // (resolver.dart:519-536). A satisfying body stays
                    // silent, a genuine mismatch reports the context-typed
                    // literal (the oracle's resolved `FunctionExpression`
                    // type: written formals, context return). A tear-off's
                    // signature is a real written one and asks unchanged.
                    let decided = match closure_sig.as_ref() {
                        Some(stub) => {
                            let mut sig = (**stub).clone();
                            sig.return_ty = formal_sig.return_ty;
                            sig
                        }
                        None => (*arg_sig).clone(),
                    };
                    match self.fn_sig_subtype(&decided, &formal_sig) {
                        Some(true) => continue,
                        Some(false) => {
                            let code = SemanticCode::ArgumentTypeNotAssignable;
                            let from_text = self.fn_sig_display(&decided);
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
                    // falls through to the ordinary face below. The
                    // substituted type decides (#2873): `test<Foo>(foo)`
                    // unfolds the alias instantiation the written argument
                    // bound, never the raw parameter spelling. The closure
                    // stub's return re-infers from the alias RHS's return —
                    // the same downward face the direct formal arm applies.
                    if let Some(alias_sig) = match formal_ty {
                        Ty::Named {
                            name,
                            args: ArgListId::EMPTY,
                            ..
                        } => self.alias_fn_sig(name),
                        _ => None,
                    } {
                        // The same downward face the direct formal arm runs:
                        // the stub re-types from the alias RHS's return (the
                        // written RHS is the context the oracle decides
                        // against) and one ask decides.
                        let decided = match closure_sig.as_ref() {
                            Some(stub) => {
                                let mut sig = (**stub).clone();
                                sig.return_ty = alias_sig.return_ty;
                                sig
                            }
                            None => (*arg_sig).clone(),
                        };
                        match self.fn_sig_subtype(&decided, &alias_sig) {
                            Some(true) => continue,
                            Some(false) => {
                                let code = SemanticCode::ArgumentTypeNotAssignable;
                                let from_text = self.fn_sig_display(&decided);
                                let to_text = self.ty_display(formal_ty);
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
                    // vs `int`). The SUBSTITUTED type decides (#2873): a
                    // solved parameter leaf reads its bound written type
                    // (`test<Function>(foo, bar)` pairs against `Function`
                    // and stays silent), an unsolved one keeps the raw
                    // spelling and the prior posture. Unknown, a bound type
                    // parameter, and the structured formals keep the
                    // under-report posture.
                    let refused = match formal_ty {
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
                        let to_text = self.ty_display(formal_ty);
                        let message = code.message3(&from_text, &to_text, "");
                        let index = self.report.diagnostics.len();
                        self.emit_message_with_correction(code, *range, message);
                        self.pending_non_promotion.push((index, range.offset));
                        continue;
                    }
                    // The unknown-row fallback: a captured signature whose
                    // callee carries no written-signature table entry while
                    // the element formal IS a function shape (the foreign
                    // and SDK callees the `fn_formal_sigs` capture never
                    // sees), on a row that stayed Unknown (no matched
                    // context at the literal's capture — the ordinary face
                    // below cannot ask `Unknown` against anything), or on a
                    // CLOSURE LITERAL's resolved row whose captured face is
                    // still the stub. The stub re-types from the formal
                    // shape's return (the cap posture) and the pair decides
                    // through the ordinary ask — `assignable` unfolds the
                    // formal's function shape and runs the same
                    // `_isFunctionSubtypeOf` (upstream Dart SDK source,
                    // pkg/analyzer/lib/src/dart/element/subtype.dart) the
                    // direct arm decides with; the arity and parameter slots
                    // produce the row (resolver.dart:522). A tear-off's real
                    // return needs no re-typing.
                    //
                    // The closure arm is the #3248 face: a block-bodied
                    // literal's captured `void` return is never an
                    // inference (`inferLocalFunction`'s cap — the formal's
                    // return is the context the body re-infers under), so a
                    // row that resolved to the stub `void Function()`
                    // against a `T Function()` formal would report a
                    // mismatch the oracle never makes. Re-typing here is the
                    // direct arm's own posture, applied to the row the
                    // signature table could not pair.
                    let folded_formal = self.fold_alias_ty(formal_ty);
                    if (*ty == Ty::Unknown || closure_sig.is_some())
                        && matches!(
                            folded_formal,
                            Ty::Function {
                                nullable: false,
                                ..
                            }
                        )
                    {
                        let mut decided = (*arg_sig).clone();
                        if closure_sig.is_some()
                            && let Some(ret) = self.function_shape_return(folded_formal)
                        {
                            decided.return_ty = ret;
                        }
                        match self.assignable(self.fn_sig_ty(&decided), folded_formal, false) {
                            Some(true) => continue,
                            Some(false) => {
                                let code = SemanticCode::ArgumentTypeNotAssignable;
                                let from_text = self.fn_sig_display(&decided);
                                let to_text = self.ty_display(formal_ty);
                                let message = code.message3(&from_text, &to_text, "");
                                let index = self.report.diagnostics.len();
                                self.emit_message_with_correction(code, *range, message);
                                self.pending_non_promotion.push((index, range.offset));
                                continue;
                            }
                            None => {}
                        }
                    }
                }
            }
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
                // A typedef-alias head (`typedef AI = int;`) is not a
                // `Container`, so `resolve_container` cannot see it — yet the
                // alias folds to a resolvable type (`int`) whose formal the
                // verdict must still judge. Fold the unbound head through the
                // alias RHS before the erasure gate: only a head that is
                // genuinely NEITHER a container NOR an alias suppresses the
                // staged argument row (upstream resolves the alias before
                // the raw-type-erasure question, so the face stays live for
                // alias-spelled formals). Display keeps the written
                // `formal_ty`; the latch only decides staging.
                let folded_formal = self.fold_alias_verdict(formal_ty);
                let unbound_formal = !creation
                    && match folded_formal {
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
            } else {
                // The conversion's own face (#2870): a granted `int →
                // double` literal ask doubles as the imprecise-value check
                // (the formal is the double context the oracle's
                // `_checkForOutOfRange` double arm reports from). The
                // rejecting side defers to the staged teardown above.
                self.check_imprecise_int_literal(*ty, formal_ty, *literal, *range);
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
        // solved instantiation keeps the prior unknown face — EXCEPT the
        // provably unconstrainable face: the declaring class wrote an
        // extends head that resolves nowhere, so every super formal of
        // its constructors carries an unresolvable parameter type (the
        // oracle's `InvalidType`) and a still-open slot can never gain an
        // upward candidate from any formal that mentions it; the oracle's
        // unsolved fill (the bound, else dynamic) is then the exact
        // answer (`StateController(super._state)` over an unresolved
        // `StateNotifier` instantiates `StateController<dynamic>`). An
        // under-seeded solve never mimics this face: a resolvable extends
        // clause types its super formals, and a formal that names the
        // parameter keeps the slot out (its argument may still constrain
        // it in the oracle — flat_buffers.dart's context-solved
        // `Offset(_tail)` stays on the fully-solved path alone).
        if let Some(container) = creation_container
            && (unnamed_creation || creation_head.is_some())
            && let Some(params) = &exec_params
        {
            let mentions_param = |ty: Ty| {
                params
                    .iter()
                    .any(|(param, _)| self.formal_mentions_param(ty, *param))
            };
            let unconstrainable = self.declared_extends_unresolved(container)
                && formals
                    .positional
                    .iter()
                    .chain(formals.named.iter())
                    .all(|formal| !mentions_param(formal.ty));
            if solutions.iter().all(|slot| slot.is_some()) || unconstrainable {
                let args: Box<[Ty]> = params
                    .iter()
                    .zip(solutions.iter())
                    .map(|((_param, bound), slot)| slot.or(*bound).unwrap_or(Ty::Dynamic))
                    .collect();
                let ty = self
                    .boundary
                    .args
                    .borrow_mut()
                    .named(container, false, &args);
                if unnamed_creation {
                    // The unnamed creation's fold REPLACES the callee row,
                    // so the solve rides the row-indexed slot and the fold
                    // adopts it as the created row's type.
                    self.pending_send_ty = Some((callee, ty));
                } else if let Some(head) = creation_head {
                    // A NAMED-selector creation (`G.value(1)`) has no fold
                    // to adopt a row-indexed slot — the dot merge replaces
                    // its selector row. The solve publishes where every
                    // created row's TYPE READER looks instead:
                    // `call_type_args`, keyed on the class segment's own
                    // offset, which `creation_row_ty` (and through it
                    // `callee_return_ty`'s generic-constructor arm) resolves
                    // the created instance through. The argument-derived
                    // instantiation is a decided face, not a to-bounds
                    // fill: nothing publishes unless every slot solved (or
                    // the head is provably unconstrainable), so an
                    // undecided send keeps its `Unknown` under-report.
                    self.publish_creation_instantiation(head, ty);
                }
            }
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
                    let in_ctor_initializers =
                        self.ctor_pending.is_some() && body_brace.is_none() && !self.ctor_body_open;
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
                    // A callee typed by an in-scope type parameter resolves
                    // callability through its written bound (the resolver's
                    // `resolveToBound` step — upstream Dart SDK source,
                    // pkg/analyzer/lib/src/dart/resolver/
                    // function_expression_invocation_resolver.dart:66: a
                    // function-form bound (`T extends MyFunction`) answers
                    // `receiverType is FunctionTypeImpl` and never reaches
                    // the report arms).
                    && !matches!(row.ty, Ty::Named { name, .. } if self.is_scope_param(name)
                        && self.param_bound_callable(name))
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
                //
                // A GROUNDED alias (the RHS resolves a constructible class)
                // is the opposite verdict: the spelling is the aliased
                // class's unnamed creation with implicit arguments —
                // upstream routes the alias constructor invocation through
                // the aliased type (`A()` under `typedef A<X extends ...> =
                // C<X>;` is `C<inferred>()`, so no invocation report and
                // the raw-constructor inference face decides the span,
                // #2841). A local shadowing the alias keeps the ordinary
                // callee faces — the spelling then names the variable, not
                // the type. The alias-capture gate keeps foreign aliases
                // (no this-file capture) on their existing verdict.
                match self
                    .bare_creation_head_name(row.range)
                    .map(|spelled| (spelled, self.alias_head_grounded(spelled)))
                {
                    Some((spelled, Some(false))) => {
                        let name_text = self.strings.resolve(spelled).to_string();
                        let range = row.range;
                        self.emit(SemanticCode::InvocationOfNonFunction, range, &name_text, "");
                        return;
                    }
                    Some((spelled, Some(true)))
                        if self.bounds.alias_named_shape(spelled).is_some()
                            && !self
                                .frames
                                .iter()
                                .rev()
                                .any(|f| f.locals.contains(&row.name)) =>
                    {
                        let range = row.range;
                        self.check_alias_creation_inference(spelled, range);
                        return;
                    }
                    _ => {}
                }
                let Ty::Named {
                    name,
                    nullable: ty_nullable,
                    ..
                } = ty
                else {
                    // The void-callee use (`function_expression_invocation_
                    // resolver.dart:86-96`): the resolver reads
                    // `function.typeOrThrow` BEFORE the callability walk and
                    // hands THAT to `_checkForUseOfVoidResult` (:214-222) — an
                    // invoked expression whose own static type is `void`
                    // reports at the callee expression and leaves the
                    // invocation unresolved (`_unresolved(node,
                    // DynamicTypeImpl.instance)`), so no non-function verdict
                    // follows and the send's result face stays unknown here.
                    // The shape is a `void`-annotated parameter or local
                    // (`void f(void x) { x(); }`) or a `void`-typed variable:
                    // a function-typed callee never reaches this arm — its
                    // signature rides `fn_var_sigs` (returned above) or the
                    // `Ty::Named` alias/`Function` legs below, which is why
                    // `void Function() f; f();` stays the legal discard.
                    if *ty == Ty::Void {
                        let range = row.range;
                        self.emit(SemanticCode::UseOfVoidResult, range, "", "");
                        return;
                    }
                    // A record-typed callee reports both ways the record
                    // face answers upstream: a `call`-named field resolves
                    // through the `recordField != null` arm and a record
                    // without one falls to `needsGetterError` — both report
                    // at the callee (upstream Dart SDK source,
                    // pkg/analyzer/lib/src/dart/resolver/
                    // function_expression_invocation_resolver.dart:103-121).
                    if matches!(ty, Ty::Record { .. }) {
                        let range = row.range;
                        self.emit(
                            SemanticCode::InvocationOfNonFunctionExpression,
                            range,
                            "",
                            "",
                        );
                    }
                    return;
                };
                // A `Function`-typed value is the function-type supertype —
                // callable. The borrowed reads copy out here: the extension
                // cache's warm-up below is the arm's one `&mut` call, and
                // every later read consumes the copies, so the scrutinee
                // borrow ends at this point.
                let name = *name;
                let ty_nullable = *ty_nullable;
                let row_name = row.name;
                let row_range = row.range;
                if self.strings.resolve(name) == "Function" {
                    return;
                }
                // A parameterized function-type alias is callable the same
                // way: `typedef P<T> = bool Function(T)` types `P<T> test`,
                // and `test(item)` invokes the aliased signature (the
                // captured alias RHS stands in for the unmodeled
                // substitution). An alias chain (`typedef B = A<int>` where
                // `A` is the fn alias) chases to its terminal function shape.
                if self.alias_fn_sig(name).is_some() {
                    return;
                }
                // An interface-typed callee resolves its `call` member like
                // the oracle's type-property resolve (`sum(1, 2)` over
                // `Adder` with `int call(int, int)`); a present `call`
                // method is callable, anything else reports. An
                // in-scope-extension `call` method answers the same way —
                // the type-property resolve consults the accessible
                // extensions (`extension E on C { int call(int x) }` over
                // `C()`; a nullable receiver needs a nullable `on` type —
                // `extension on int?` over `int? a`), upstream Dart SDK
                // source, pkg/analyzer/lib/src/dart/resolver/
                // function_expression_invocation_resolver.dart:92-100.
                //
                // The answer doubles as the member's use credit (#2894):
                // the send spells no `.call` selector, so no resolution-side
                // capture records the member, and a finalize-time
                // re-derivation cannot — the frame is gone by then. The
                // credit rides the INVOKE event while the frame is alive: a
                // resolved class `call` method joins the name-level pool,
                // and the applied extension's `call` joins the
                // element-attributed set (the face's declaring file + pool
                // slot — the `usedElements.members` key) with its
                // contributing import credited for a foreign face. Only a
                // UNIQUE application credits — an ambiguous pool resolves
                // no member upstream, so every candidate stays
                // unreferenced.
                let call = self.intern("call");
                if matches!(
                    self.resolved_member(name, call),
                    Some(entry) if entry.kind == MemberKind::Method
                ) {
                    self.used_names.insert(call);
                    return;
                }
                let call_faces = self.extension_call_faces(name, ty_nullable);
                // An ambiguous pool keeps the standing callable verdict (any
                // applicable face answered) with no member credit — the
                // oracle resolves no member, so every candidate stays
                // unreferenced.
                if call_faces.len() > 1 {
                    return;
                }
                if let Some((face, face_file)) = call_faces.into_iter().next() {
                    // The element key: the face's position in its
                    // declaring file's `extension_faces()` pool (pointer
                    // identity IS element identity within an analysis —
                    // the pool is frozen before the walk). A face the
                    // index cannot see credits nothing rather than
                    // guessing a slot.
                    if let Some(face_index) = self
                        .index
                        .get(face_file)
                        .and_then(|elements| {
                            elements
                                .extension_faces()
                                .iter()
                                .position(|entry| std::ptr::eq(entry, face))
                        })
                        .and_then(|index| u32::try_from(index).ok())
                    {
                        self.extension_member_used
                            .insert((face_file, face_index, call));
                    }
                    self.credit_extension_import(face, face_file);
                    return;
                }
                // A type-parameter-typed callee resolves through its written
                // bound — the resolver's `resolveToBound` step (upstream Dart
                // SDK source, pkg/analyzer/lib/src/dart/resolver/
                // function_expression_invocation_resolver.dart:66): a
                // function-form bound (`typedef MyFunction = double
                // Function(int _); T extends MyFunction`) is callable, a
                // `call`-bearing bound likewise.
                if self.is_scope_param(name) && self.param_bound_callable(name) {
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
                if self.bounds.alias_fn_shape(name).is_none()
                    && self.bounds.alias_named_shape(name).is_none()
                    && matches!(
                        self.top_level_decl(name).as_deref(),
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
                if self.top_level_decl(name).is_none()
                    && !self.members.primitive_ids.contains(&name)
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
                    .any(|f| f.locals.contains(&row_name))
                {
                    self.deferred_invocation_faces.push((row_range, row_name));
                    return;
                }
                self.emit(
                    SemanticCode::InvocationOfNonFunctionExpression,
                    row_range,
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
                // An enclosing EXTENSION's own `call` method is such a
                // member: `this` types as the `on` type and the `.call`
                // property resolve finds the extension itself applicable
                // (`extension E on C { int call(int x) => 0;
                // int m() => this(2); }`, upstream Dart SDK source,
                // pkg/analyzer/lib/src/dart/resolver/
                // function_expression_invocation_resolver.dart:92-100).
                if !is_super
                    && let Some((container, kind)) = self.container_context()
                    && kind == ContainerKind::Extension
                    && matches!(
                        self.index.get(self.file).and_then(|f| f.container(container))
                            .and_then(|c| c.members.get(&self.intern("call"))),
                        Some(entry) if entry.kind == MemberKind::Method
                    )
                {
                    return;
                }
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

    /// Whether the in-scope type parameter `name`'s written bound is a
    /// callable face: a function-form bound (`T extends void Function()`,
    /// the bound stand-in for the resolver's `receiverType is
    /// FunctionTypeImpl` arm — upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/
    /// function_expression_invocation_resolver.dart:66-79), an aliased
    /// function shape (`typedef MyFunction = double Function(int _); T
    /// extends MyFunction`), or a bound head carrying a `call` method.
    pub(crate) fn param_bound_callable(&self, name: StringId) -> bool {
        let Some(bound) = self.param_bound(name) else {
            return false;
        };
        match bound {
            Ty::Function { .. } => true,
            Ty::Named { name: head, .. } => {
                self.strings.resolve(head) == "Function"
                    || self.alias_fn_sig(head).is_some()
                    || matches!(
                        self.resolved_member(head, self.intern("call")),
                        Some(entry) if entry.kind == MemberKind::Method
                    )
            }
            _ => false,
        }
    }

    /// Whether an accessible extension applies to the head `name` at the
    /// receiver's nullability and declares a `call` METHOD — the
    /// extension-provided `.call` face of the oracle's type-property
    /// resolve (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/
    /// function_expression_invocation_resolver.dart:92-100, the
    /// `typePropertyResolver.resolve` call whose lookup consults the
    /// library's accessible extensions). A nullable receiver matches only
    /// a nullable `on` type (`String?` is not a subtype of `String` — the
    /// `applicableTo` predicate), matching the face's own applicability
    /// gate.
    pub(crate) fn extension_call_member(&mut self, name: StringId, nullable: bool) -> bool {
        !self.extension_call_faces(name, nullable).is_empty()
    }

    /// Every accessible extension face applicable to the head `name` at the
    /// receiver's nullability that declares a `call` METHOD — the candidate
    /// pool behind [`Self::extension_call_member`]'s verdict and the
    /// invoke-site use credit (#2894). A nullable receiver matches only a
    /// nullable `on` type (`String?` is not a subtype of `String` — the
    /// `applicableTo` predicate), matching the face's own applicability
    /// gate.
    fn extension_call_faces(
        &mut self,
        name: StringId,
        nullable: bool,
    ) -> Vec<(&'a ExtensionFace, FileId)> {
        let call = self.intern("call");
        self.accessible_extension_faces()
            .iter()
            .filter(|(face, _)| {
                face.on_type == Some(name)
                    && (!nullable || face.on_type_nullable)
                    && matches!(
                        face.members.get(&call),
                        Some(entry) if entry.kind == MemberKind::Method
                    )
            })
            .copied()
            .collect()
    }

    /// The genuinely-unresolved bare-call-head face (#2860): the send's
    /// callee resolved as nothing — [`Engine::resolve_bare`]'s full scope
    /// chain (frames, the container member faces, the extension faces, the
    /// scoped library lookup) answered `Missing` — so the oracle types the
    /// invocation with `InvalidType` and every relation over it reads
    /// vacuous. The refusal mirrors of the snapshot universe's buckets keep
    /// the over-deading guard honest: a mixin/extension body's `on` face
    /// may still define the name (`on_opaque` — the member could live on
    /// the unindexed hop), a dart:core top-level facade spelling (`print`)
    /// resolves through the implicit core import, and an invokable member
    /// spelling (`int.parse`'s `parse` bare at top level) is the
    /// name-keyed over-approximation the oracle's own scope also refuses —
    /// those keep the unknown face (the conservative under-report).
    fn unresolved_call_head(&self, row: &Row) -> bool {
        if row.qualified {
            return false;
        }
        let RowShape::Bare(BareInfo::Missing { on_opaque, .. }) = &row.shape else {
            return false;
        };
        if *on_opaque
            || super::types::CORE_TOP_LEVEL_FNS.contains(&self.strings.resolve(row.name))
            || self.invokable_member_spelling(row.name)
        {
            return false;
        }
        true
    }

    /// Whether `name` spells an invokable member of some container this
    /// engine's universe registers — the core member tables' spellings
    /// ([`CORE_TYPE_MEMBERS`], [`OBJECT_MEMBERS`], [`ENUM_SYNTH_MEMBERS`],
    /// [`CORE_MEMBER_RETURNS`]' static faces) plus this file's own
    /// container members, the same name-keyed over-approximation the
    /// snapshot's invokable face carries (`LibraryTypes::invokable_members`,
    /// upstream Dart SDK source semantics: an instance or static member
    /// spelling somewhere beats the unresolved-call face). Built once per
    /// engine on the first unresolved-head send; every other send reads the
    /// cached set (one hash probe).
    fn invokable_member_spelling(&self, name: StringId) -> bool {
        self.member_spellings
            .get_or_init(|| {
                let mut set = FxHashSet::default();
                for (_, members) in crate::member_face::CORE_TYPE_MEMBERS {
                    set.extend(members.iter().map(|spelling| self.strings.intern(spelling)));
                }
                set.extend(
                    crate::member_face::OBJECT_MEMBERS
                        .iter()
                        .map(|spelling| self.strings.intern(spelling)),
                );
                set.extend(
                    crate::member_face::ENUM_SYNTH_MEMBERS
                        .iter()
                        .map(|spelling| self.strings.intern(spelling)),
                );
                for (head, member, _) in crate::member_face::CORE_MEMBER_RETURNS {
                    let _ = head;
                    set.insert(self.strings.intern(member));
                }
                if let Some(elements) = self.index.get(self.file) {
                    for &container in elements.container_order() {
                        if let Some(container) = elements.container(container) {
                            set.extend(container.members.keys().copied());
                        }
                    }
                }
                set
            })
            .contains(&name)
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
            // The armed `last` check owns this frame, so the pop reads the
            // same slot with no intervening mutation — an empty pop is
            // only-on-drift. The face skips into the pairing ledger (the
            // lints bridge's `expect_paired` posture) instead of panicking.
            match self.literal_ctor_stash.pop() {
                Some((_, args, span)) => self.check_literal_ctor_invocation(callee, &args, span),
                None => self.pairing.miss("paired literal ctor stash"),
            }
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
        // Whether the invoked expression is itself a completed send
        // (`v()()`): only then is its own static type the inner send's
        // result rather than a callee reference's declared face. Captured
        // before the intent flip below erases the distinction.
        let callee_is_invocation = self
            .rows
            .get(callee)
            .is_some_and(|row| row.intent == Intent::Invoke);
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
        // A PROMOTED function-typed callee: the bare local's declared face
        // is the nullable function type, but the flow read at the callee's
        // span carries the promotion standing there (`if (f != null) return
        // f();` — the invoked expression is the non-null function), and the
        // send yields that shape's return exactly as the declared-non-null
        // callee below. The row's own face must still be a function type
        // (folded through any alias): a non-function promotion rides the
        // implicit-call arm above instead.
        if let Some(row) = self.rows.get(callee)
            && !row.qualified
            && let Some(Ty::Function {
                shape,
                nullable: false,
            }) = self.read_type_at(row.range)
            && matches!(self.fold_alias_ty(row.ty), Ty::Function { .. })
        {
            self.rows[callee].ty = self.boundary.pool().function_shape(shape).return_ty;
        }
        // A function-typed callee's own shape: the invocation yields the
        // shape's return type (`handler(x)` over `typedef H = String
        // Function(int)`, a field/param/local spelled either through the
        // alias or the raw function type) — never the whole function type
        // the declared row carries. A nullable callee keeps its face (the
        // unchecked-use verdict owns the value then).
        // The send-solve's instantiated return keeps its binding: a generic
        // callee whose upward inference solved a parameter
        // (`solved_call_params` holds the callee's offset — `toF(source)`
        // binds `X := G` and :10907 already wrote the substituted `F<G>`)
        // IS the call's static type; folding it to the generic function
        // shape and reading `shape.return_ty` would clobber the solved
        // face to `void` and starve every downstream assignability ask
        // (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
        // function_expression_invocation_resolver.dart:55-78 — the
        // invocation's static type is the inference result's return).
        let solve_written = self
            .rows
            .get(callee)
            .is_some_and(|row| self.solved_call_params.contains_key(&row.range.offset));
        let callee_ty = self.rows.get(callee).map(|row| self.fold_alias_ty(row.ty));
        if !solve_written
            && let Some(Ty::Function {
                shape,
                nullable: false,
            }) = callee_ty
        {
            let ret = self.boundary.pool().function_shape(shape).return_ty;
            self.rows[callee].ty = ret;
        }
        // The void-operand use on the INVOKED EXPRESSION (upstream
        // `function_expression_invocation_resolver.dart:55-56` —
        // `_checkForUseOfVoidResult(function, function.typeOrThrow)`): the
        // checked type is the invoked expression's own static type, never
        // the invocation's result. Only an expression that is itself a
        // completed send carries a `void` face here (`v()()` — invoking the
        // result of a `void` call); a plain callee reference is a function
        // value or a `call`-bearing receiver whatever its own return is, so
        // the alias-typed formal's legal discard (`typedef F<X> = void
        // Function<Y extends X>(); void f(F<int> a) { a(); }`) stays silent.
        // The row's `ty` holds that inner send's result at this point — the
        // residue of its own write-back below. No live shape reaches this
        // today: the send link arms only on a callee row abutting the group
        // (`Engine::open_arguments`'s `abuts`), and a nested invocation's
        // callee row never does — the gate keeps the face upstream-correct
        // for the nested model instead of firing on a callee's own return.
        if callee_is_invocation
            && let Some(row) = self.rows.get(callee)
            && row.ty == Ty::Void
        {
            // The checked node is the invoked EXPRESSION `v()` — the whole
            // inner send, callee kind notwithstanding — so the anchor widens
            // to the completed send's span (callee through the first `)` —
            // the `function` node, not the outer `v()()`).
            let range = self.invocation_span(row.range).unwrap_or(row.range);
            self.emit(SemanticCode::UseOfVoidResult, range, "", "");
        }
        // The completed send's static type (upstream `node.recordStaticType
        // (returnType)`): read once, ahead of both the dead-flow exit below
        // and the write-back onto the row. The read is
        // [`Self::operand_static_ty`] — the ONE fact every verdict over this
        // row consumes ([`Self::value_operand_ty`]'s `is` operand, the
        // null-comparison faces), which answers the row's own post-fold face
        // (a function type's return, the implicit-`call` return, a solved
        // instantiation) and falls back to `callee_return_ty` for a row that
        // still holds none. Reading the verdict's own fact here is what
        // keeps the flow send fold and the verdicts from disagreeing
        // (#3415: the fold above writes a function-typed formal's return
        // onto the row, so `intFunction() is int` types `int` at the verdict
        // while a `callee_return_ty`-only read answered `Unknown` and staged
        // nothing into the recorder — the oracle's `dead_code` rows for the
        // unsatisfiable arm went missing).
        let send_result = self
            .rows
            .get(callee)
            .map_or(Ty::UNKNOWN, |row| self.operand_static_ty(row));
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
            // The same resolved return stages for the flow send fold
            // (#2567 R18): the listener picks the slot up at this event
            // and stages it into the recorder, so the name-level fold —
            // whose own callee read is an unknown face — adopts the real
            // return (the send-close inference's parameter solution
            // included, `readEnum(Tag.values)` answering the enum).
            if send_result != Ty::UNKNOWN {
                if let Some(row) = self.rows.get(callee) {
                    self.send_return_staging = Some((row.range.offset, send_result));
                }
            } else if let Some(row) = self.rows.get(callee)
                && self.unresolved_call_head(row)
            {
                // The genuinely-unresolved head (#2860): the oracle types
                // the invocation `InvalidType`, so the listener stages the
                // head's own name — the flow lattice's invalid stand-in —
                // and a switch over the call answers the vacuous-cover
                // chain instead of the undecidable unknown.
                self.send_invalid_staging = Some((row.range.offset, row.name));
            }
            (self.is_never_ty(send_result)).then_some(SendExit::Result)
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
        // The completed send's static type lands on the callee row (upstream
        // `node.recordStaticType(returnType)`): the invocation NODE is this
        // row for every enclosing consumer, and `receiver_static_ty` reads
        // `row.ty` before any shape-derived face. Without the write the row
        // kept its callee reference face standing (`Unknown` for a bare
        // top-level `void v()`, the alias spelling for a `F<int>` formal)
        // where consumers expect the invocation's own type; the callee
        // reference's face must not outlive the invocation it fed. A
        // creation head keeps its own fold (the row is replaced below with
        // the instantiated/solved face).
        if send_result != Ty::UNKNOWN
            && let Some(row) = self.rows.get_mut(callee)
            && !matches!(row.shape, RowShape::Bare(BareInfo::Container { .. }))
        {
            row.ty = send_result;
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
                // The ctor-use edge (#2931): the bare `A(` head IS the
                // `ConstructorName` — its class key reaches the verifier's
                // `ctor_annotations` fallback.
                if kind == ContainerKind::Class {
                    self.note_ctor_use(name, name, range);
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
                let mut solved = match self.pending_send_ty.take() {
                    Some((row, ty)) if row == callee => ty,
                    _ => Ty::UNKNOWN,
                };
                // The downward half of the creation's inference: with no
                // argument-derived instantiation, the context type names
                // the parameters (`LinkedHashSet<int> s = LinkedHashSet()`
                // infers `<int>`; upstream's `InstanceCreationExpression`
                // resolves under the enclosing context). The instantiation
                // publishes where the return/arrow-body/cascade faces read
                // a created row's type ([`Engine::creation_row_ty`]).
                if solved == Ty::UNKNOWN && !self.creation_written_type_args(name, range.offset) {
                    if let Some(ty) = self.creation_context_instantiation(name, range.offset) {
                        self.publish_creation_instantiation(range.offset, ty);
                        solved = ty;
                    }
                }
                self.rows[callee] = Row {
                    name,
                    range,
                    qualified: false,
                    shape: RowShape::Receiver(Receiver::Created {
                        name,
                        kind,
                        // #3398: the creation's resolved `ty` rides along —
                        // populated by #3394 (`ty: Some(solved)` when
                        // `solved != Ty::UNKNOWN`).
                        ty: None,
                    }),
                    intent: Intent::Read,
                    write_op: None,
                    ty: solved,
                };
                // A cascade target whose instantiation was inferred
                // (`LinkedHashSet<int> s = LinkedHashSet()..addAll(...)`):
                // the cascade reads its target's static type off the row
                // stack's top (`begin_cascade`), and a `Created` row answers
                // only its bare head there — so the solved instantiation
                // rides an explicit value row above the fold row, which the
                // cascade's target read adopts and its own value stand-in
                // then inherits.
                let cascade_follows = {
                    let tokens = self.tokens.as_slice();
                    let next = tokens
                        .partition_point(|token| token.utf16_offset < end.range.end());
                    tokens.get(next).is_some_and(|token| {
                        matches!(
                            token.kind,
                            dartq_scanner::TokenKind::PeriodPeriod
                                | dartq_scanner::TokenKind::QuestionPeriodPeriod
                        )
                    })
                };
                if solved != Ty::UNKNOWN && cascade_follows {
                    self.rows.push(Row {
                        name,
                        range,
                        qualified: false,
                        shape: RowShape::Value,
                        intent: Intent::Read,
                        write_op: None,
                        ty: solved,
                    });
                }
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
        match &head.shape {
            RowShape::Bare(BareInfo::Plain { .. }) | RowShape::Member { .. } => {
                self.check_invocation_arity(row, count, range);
            }
            // A container head the implicit creation applies to
            // (`Map<int>()`, `var c = C<int, int>()`): the group's count
            // checks against the type's declared type-parameter count — the
            // same verdict the type-use face runs (the base code, upstream
            // `named_type_resolver.dart` `_buildTypeArguments`). An
            // extension head checks its own face instead.
            RowShape::Bare(BareInfo::Container {
                name,
                kind: ContainerKind::Extension,
            }) if self.container_kind_of(*name) == Some(ContainerKind::Extension) => {
                self.check_extension_arity(*name, count, range);
            }
            RowShape::Bare(BareInfo::Container { name, .. }) => {
                self.check_annotation_arity(*name, count, range);
            }
            _ => {}
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
            RowShape::Bare(BareInfo::Plain { .. }) => {
                // The unnamed implicit creation's head (`C<int,int>()`):
                // the selector-less spelling never arms `creation_ref` and
                // the callee row stays `Bare(Plain)` — the group counts the
                // CLASS's declared arity, not an executable's (upstream
                // `resolveTypeArguments` on the `ConstructorName`'s `type`).
                // A local shadowing the spelling keeps the send's own face
                // (`resolve_container` answers the library namespace — a
                // local `C` never cancels the class's arity verdict).
                if self.resolve_local(name).is_none()
                    && let Some((container, _)) = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
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
                self.top_level_decl(name)
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
                    })
            }
            RowShape::Member { recv } => self.member_exec_arity(recv, name),
            // A container callee reached through the send (`C<int,int>()` —
            // the unnamed implicit creation keeps its `Bare(Container)`
            // shape when `resolve_bare` classified the head at push): the
            // group counts the TYPE's declared arity (the base code,
            // `_buildTypeArguments`), not an executable's `_element`.
            RowShape::Bare(BareInfo::Container { name, .. }) => {
                return self.check_annotation_arity(*name, count, range);
            }
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
        let (decl, owner) = resolved;
        // The mixin-application head forwards the terminal class's
        // generative set (`p.A1.b1` over `class A1 = B<int> with M1;`
        // resolves `B.b1`'s arity) — the prefixed arm's own walk reaches
        // it the same way (`prefixed_container` already carries the
        // declaring unit). A broken forwarding chain keeps the own-face
        // read.
        let (face, face_owner, face_name) = self
            .effective_ctor_container(decl, owner, class)
            .unwrap_or((decl, owner, class));
        let forwarded = !std::ptr::eq(face, decl);
        let declares = (face.constructors.contains(&name)
            && (!forwarded || !self.ctor_is_factory(face, face_owner, name)))
            || (name == self.intern("new")
                && face.has_unnamed_constructor
                && (!forwarded || !self.ctor_is_factory(face, face_owner, face_name)));
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
        // The application's written arguments, latched for this row when the
        // group anchored: the bounds faces consume them (#2890), every other
        // arm drops them (no call follows to ground the latch).
        let latched_args = self.call_args_for(row_index);
        let Some(row) = self.rows.get(row_index) else {
            return;
        };
        // The facts the arms read past their `&mut` faces: the row borrow
        // must not reach them.
        let row_name = row.name;
        let row_qualified = row.qualified;
        match &row.shape {
            RowShape::Bare(BareInfo::Container { name, .. }) => {
                let class_name = *name;
                // Type-literal instantiation `C<int>`: the base code.
                self.check_annotation_arity(class_name, count, range);
                // #2890 — a `.` after the group is the constructor-reference
                // spelling (`A<String>.new`): the rewrite keeps the type
                // arguments on the constructor name's type whenever any exist
                // (ast_rewrite.dart `_toConstructorReference_propertyAccess` —
                // "it looks more like a constructor tearoff than anything
                // else"), so `checkConstructorReference` checks them against
                // the container's (or the alias's own) substituted bounds.
                // Without the dot the node is a TypeLiteral, whose visit
                // runs no bounds face — the plain `A<int>;` spelling stays
                // silent. A prefixed head keeps the under-report posture
                // (the same `!qualified` rule the creation faces share).
                if !row_qualified
                    && let Some(args) = &latched_args
                    && self.dot_follows_range_end(range)
                    && let Some(params) = self.captured_type_arg_bounds(class_name)
                {
                    self.check_call_type_arg_bounds(&params, args);
                }
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
                    .top_level_decl(row_name)
                    .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some())
                    .map(|d| (d.type_params, ExecKind::Function));
                if let Some((declared, kind)) = named_fn {
                    self.emit_element_arity(row_name, declared, kind, count, range);
                    // #2890 — the tear-off's bounds face
                    // (`checkFunctionReference`, upstream Dart SDK source,
                    // pkg/analyzer/lib/src/error/type_arguments_verifier.dart
                    // :192-196): the written arguments check against the
                    // function's declared bounds, the report anchored at the
                    // offending argument's node. The `exec_bounds` window is
                    // the declaring function's own capture — the same facts
                    // the send path reads.
                    if let Some(args) = &latched_args
                        && let Some(window) = self.exec_bounds.get(&row_name)
                        && !window.is_empty()
                    {
                        let window: Vec<(StringId, Option<Ty>)> = window.iter().copied().collect();
                        self.check_call_type_arg_bounds(&window, args);
                    }
                } else if let Some((declared, kind)) = self
                    .tear_off_element_arity(row_name)
                {
                    // An unqualified reference the top-level table missed:
                    // a declared LOCAL function (`f() { g<int>; }` — its
                    // own value is the tear-off, the `_function`-kind
                    // element face names `function`), or the enclosing
                    // container's own METHOD through implicit `this`
                    // (`bar() { foo<int>; }` — the element's declared
                    // type-parameter count, the `method` word). Upstream's
                    // `InvocationTargetMethod` /
                    // `InvocationTargetFunction` dispatch decides the kind
                    // word from the resolved element — the same split
                    // `check_invocation_arity` applies on the send path.
                    self.emit_element_arity(row_name, declared, kind, count, range);
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
                        // #2890 — the method tear-off's bounds face (the
                        // same `checkFunctionReference` verdict over the
                        // written arguments, the method's own parameters
                        // deciding).
                        if let Some(args) = &latched_args
                            && !entry.type_param_names.is_empty()
                        {
                            let params: Vec<(StringId, Option<Ty>)> = entry
                                .type_param_names
                                .iter()
                                .enumerate()
                                .map(|(index, name)| {
                                    (*name, entry.type_param_bounds.get(index).copied())
                                })
                                .collect();
                            self.check_call_type_arg_bounds(&params, args);
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

    /// Whether a `.` followed by a name starts after `range`'s end (the
    /// group's closer, a UTF-16 offset converted before the byte walk): the
    /// constructor-reference follower (`A<String>.new`). The same source
    /// probe the collection-literal bail uses, member-shaped — a bare `.`
    /// with no name is recovery residue, not a selector.
    fn dot_follows_range_end(&self, range: SourceRange) -> bool {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(range.end());
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        cursor < bytes.len()
            && bytes[cursor] == b'.'
            && bytes
                .get(cursor + 1)
                .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_' || *b == b'$')
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

    /// The declared type-parameter count of an unqualified tear-off the
    /// top-level table missed: a declared local function reads its own
    /// captured signature (`local_fn_sig` — `f() { g<int>; }` reports
    /// `function`), else the enclosing container's own method through
    /// implicit `this` (`bar() { foo<int>; }` reports `method`). The
    /// member face is own-declared only — upstream's lexical scope puts
    /// `this`'s members ahead of the namespace for an unqualified
    /// reference, and an inherited member resolves through the member
    /// walk the same as the invocation face's
    /// (`check_invocation_arity`'s implicit-this arm).
    fn tear_off_element_arity(&self, name: StringId) -> Option<(u16, ExecKind)> {
        if let Some(sig) = self.local_fn_sig(name) {
            return Some((sig.params.len() as u16, ExecKind::Function));
        }
        if self.frames.iter().any(|f| f.locals.contains(&name)) {
            // A plain local of the same spelling shadows every element
            // face — the read is the local's value, never a tear-off.
            return None;
        }
        let (container, _) = self.container_context()?;
        match self.walk_members(container, name) {
            Walk::Found(entry) if entry.kind == MemberKind::Method => {
                Some((entry.type_params, ExecKind::Method))
            }
            _ => None,
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
    pub(crate) fn check_prefixed_annotation_arity(
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
        // A headless group inside a switch-case / if-case pattern with NO
        // anchor row is the object pattern's own NamedType group —
        // `case Box<int, String>():` whose `Box` head row pushes only at
        // `handle_object_pattern` (after the group closed), leaving the
        // arity verdict with no head to read. The group's count and span
        // latch here for `handle_object_pattern` to consume — it anchors
        // the group's `<` against the pattern head's token (the arity row
        // the oracle emits as `wrong_number_of_type_arguments_element` on
        // `NamedType`, named_type_resolver.dart `_buildTypeArguments`).
        if head.is_none()
            && !anchored
            && (self.switch.case_sink_open || self.switch.pattern_depth > 0)
        {
            self.pending_pattern_type_args =
                Some((count, begin.range.offset, range.end()));
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
            // The site itself keeps its outermost anchor (the head through
            // the group closer); the #2831 P2 queueing below must read the
            // tree after the move, so the arguments clone here.
            self.pending_bounded_checks.push(PendingBoundedCheck::Site {
                head: tree.head,
                args: tree.args.clone(),
                in_scope: self.type_params.clone(),
                site: tree.range,
            });
            // #2831 P2 — every nested bounded head checks as its own site:
            // upstream runs `checkNamedType` at every `NamedType`
            // including nested ones (type_arguments_verifier.dart:243-247),
            // and the outermost-only capture died at heads the snapshot
            // cannot answer (an alias outer head). Only in-file container
            // heads with written arguments queue — a raw nested head has
            // no instantiation to check and a foreign head no facts.
            self.queue_nested_bounded_sites(&tree.args);
        }
        // The #1351 capture: the open typedef RHS's group closed.
        self.bounds.alias_end_group(range);
        // The #612 capture: the header's group closed into its frame.
        self.var_end_group();
        if let Some((head, head_range)) = head {
            // Upstream anchors the NamedType arity face at the type node
            // (`named_type_resolver.dart` `_buildTypeArguments`:
            // `.at(node)`): the head name through the group's `>`, not the
            // bare `<...>` group (manual face
            // type_arguments_on_type_variable/1: `S<int>` at 1:11|6).
            let span = SourceRange {
                offset: head_range.offset,
                length: end.range.end() - head_range.offset,
            };
            self.check_prefixed_annotation_arity(head, head_range, count, span);
            return;
        }
        // The metadata annotation's own type-argument group (`@A<int>()`,
        // `@A<int>.named()`): `begin_metadata` latched `annotation_at` for
        // this group's enclosing `@`, the headless group closed unanchored
        // (the annotation's `A` row carries no callee the group abuts), and
        // the still-open `call_args_capture` holds the written arguments.
        // The pending annotation's joined spelling (`A`, `A.named`,
        // `p.A.named`, `A<int>.named`) resolves its class segment — the
        // text before the constructor selector's last `.` — and the written
        // arguments check against the container's (or the alias's own)
        // substituted bounds exactly like the constructor-reference arm
        // (`checkNamedType` → `_checkForTypeArgumentNotMatchingBounds`,
        // type_arguments_verifier.dart:243-247/:312-331; the annotation
        // face's `type_parameter_supertype_of_its_bound`/`_notMatchingBounds`
        // corpus entries `test_metadata_notMatching`/`_viaTypeAlias`).
        if head.is_none()
            && self.arg_levels.is_empty()
            && self.annotation_at.is_some()
            && let Some(annotation) = self.pending_annotations.last()
        {
            let text = self.strings.resolve(annotation.name).to_string();
            // The class segment drops the constructor selector (the last
            // `.`) and any already-written argument group (`A<int>.named`
            // resolves `A`); a prefix segment (`p.A`) resolves through its
            // prefix scope the same as the bare head.
            let class_text = match text.rsplit_once('.') {
                Some((class, _)) => class,
                None => text.as_str(),
            };
            let head_text = class_text.split('<').next().unwrap_or(class_text);
            let head_id = self.intern(head_text);
            // The capture leaves before the bounds borrow: `params` rides
            // `self.bounds` immutably, so `call_args_capture.take()`'s
            // `&mut` must complete first.
            if let Some(capture) = self.call_args_capture.take()
                && let Some(params) = self.captured_type_arg_bounds(head_id)
            {
                let (tys, spans): (Vec<Ty>, Vec<SourceRange>) =
                    capture.members.into_iter().unzip();
                let args = CallTypeArgs {
                    tys: tys.into_boxed_slice(),
                    spans: spans.into_boxed_slice(),
                };
                self.check_call_type_arg_bounds(&params, &args);
                return;
            }
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
                        self.check_creation_ref_bounds(class);
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
                None => {
                    self.check_annotation_arity(class, count, range);
                    self.check_creation_ref_bounds(class);
                }
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
        // A container head's group (`C<int,int>()` implicit unnamed creation,
        // `C<int,int> x` typed declaration): the type's own declared arity
        // counts — the base code, not the executable `_element` face. Check
        // here; the deferred path only sees the group when a send's `(`
        // follows — a type-annotation head's `;` never consumes the slot.
        if matches!(target.shape, RowShape::Bare(BareInfo::Plain { .. }) | RowShape::TypeUse(..))
            && self.resolve_local(target.name).is_none()
            && let Some((container, _)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                target.name,
            )
        {
            let declared = container.type_params;
            if declared != count {
                let name_text = self.strings.resolve(target.name).to_string();
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
        if matches!(target.shape, RowShape::Bare(_) | RowShape::Member { .. }) {
            self.deferred_args.push((base - 1, count, range));
        }
    }

    /// #3296 — the callee head an implicit creation's type-argument group
    /// applies to. `A<String>()` parses as a constructor reference, so its
    /// group arrives headless: the #610 site capture never opened, and the
    /// verdict the oracle runs at the creation's `NamedType`
    /// (`checkNamedType`, type_arguments_verifier.dart:243-247) never ran.
    /// The reference head is the last type use, and it names the group only
    /// when it abuts the `<` across trivia and this file captured it (a
    /// foreign head carries no bound facts). Three shapes stay headless:
    /// a named-constructor selector (`A.named<String>`) owns the
    /// `_constructor` face, a group outside a creation context is an
    /// invocation's explicit type arguments (the callee's own face), and
    /// the keyword spellings (`new A<String>()`, `const A<String>()`) keep
    /// their #2890 creation bounds face
    /// ([`Self::check_creation_ref_bounds`]).
    pub(crate) fn implicit_creation_head(&self, open: u32) -> Option<(StringId, SourceRange)> {
        if self.implicit_creation_depth == 0 || !self.arg_levels.is_empty() {
            return None;
        }
        let (class, ctor) = self.creation_ref?;
        if ctor.is_some() {
            return None;
        }
        let &(range, name) = self.type_uses.last()?;
        if name != class || !self.trivia_between(range.end(), open) {
            return None;
        }
        self.bounds.has_capture(class).then_some((class, range))
    }

    /// #3296 — a group-less named-type head's bounds face. Upstream runs
    /// `checkNamedType` at every `NamedType`, and a bare head answers its
    /// instantiate-to-bounds verdict ([`crate::bounds::Bounds::check_named`]'s
    /// `args.is_empty()` arm, the raw-type context row riding the report,
    /// type_arguments_verifier.dart:441-446). The #610 site capture only
    /// opens at a `<...>` group, so a head with no group reached no site;
    /// the head token opens and closes one here. Reusing the capture's own
    /// routing keeps every declaration window with its own face: inside a
    /// container header or a bound-annotation window the pair folds into
    /// that window's sink, and under a typedef RHS
    /// ([`crate::bounds::Bounds::begin_group`]'s alias arm) it is a no-op —
    /// `check_alias_rhs_sites` owns those heads. The caller delivers the
    /// head only where no `<...>` group and no such window is open.
    pub(crate) fn queue_bare_head_site(&mut self, head: StringId, range: SourceRange) {
        // An in-scope type parameter shadows any container of the same
        // name, and only a captured head carries bound facts.
        if self.type_params.contains(&head)
            || self.pending_type_params.contains(&head)
            || !self.bounds.has_capture(head)
        {
            return;
        }
        self.bounds.begin_group(Some((head, range)), true);
        let Some(mut roots) = self.bounds.end_group(range) else {
            return;
        };
        let Some(tree) = roots.pop() else {
            return;
        };
        self.pending_bounded_checks.push(PendingBoundedCheck::Site {
            head: tree.head,
            args: Vec::new(),
            in_scope: self.type_params.clone(),
            site: tree.range,
        });
    }

    /// #2831 P2 — the nested bounded heads of a captured annotation site:
    /// each in-file container argument head queues its own
    /// [`PendingBoundedCheck::Site`] (the whole subtree's span is the
    /// context row's anchor, the same face the outermost head takes),
    /// pre-order like the upstream `visitNamedType` walk. A poisoned or
    /// argument-less nested head never queues — `check_site_inner`
    /// suppresses those anyway; recursion still descends through heads
    /// the snapshot cannot answer (their own arguments may nest container
    /// heads deeper).
    fn queue_nested_bounded_sites(&mut self, args: &[crate::bounds::TypeTree]) {
        for arg in args {
            if !arg.poisoned && !arg.args.is_empty() && self.bounds.has_capture(arg.head) {
                self.pending_bounded_checks.push(PendingBoundedCheck::Site {
                    head: arg.head,
                    args: arg.args.clone(),
                    in_scope: self.type_params.clone(),
                    site: arg.range,
                });
            }
            self.queue_nested_bounded_sites(&arg.args);
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

    /// The NamedType arity face anchors at the type node, head name
    /// through the group's `>` (`named_type_resolver.dart` `.at(node)`) —
    /// manual face `type_arguments_on_type_variable/1`: `S<int>` at
    /// 1:11|6, never the bare `<int>` group at 1:12|5.
    #[test]
    fn type_arguments_on_type_variable_anchor_at_the_type_name() {
        let source = "void f<S>(S<int> a) {}";
        let report = report(source);
        let rows: Vec<(u32, u32)> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "wrong_number_of_type_arguments")
            .map(|d| (d.range.offset, d.range.length))
            .collect();
        let at = source.find("S<int>").unwrap() as u32;
        assert_eq!(rows, vec![(at, "S<int>".len() as u32)], "{rows:?}");
    }
}

/// The #2990 `?[`/`?..[` index-face pins (the operator anchor per written
/// operator, the pre-lift element face a following `?.`/`?[` decides on,
/// and the `?`-token short-circuit context), kept as a standalone module so
/// the shared `pins` rig stays untouched while its lane is in flight.
#[cfg(test)]
#[path = "ticket_2990_tests.rs"]
mod ticket_2990_tests;

/// The #3208 pins (the constructor send's call-site instantiation —
/// `new`/`const`/named-selector creations solve the class's parameters
/// from the group's own argument types, the folded `Created` row carrying
/// the instantiated face instead of the bare head's `<dynamic>`
/// residual). Registered here, not in `diagnose.rs` (other lanes' file),
/// via `#[path]`.
#[cfg(test)]
#[path = "ticket_3208_tests.rs"]
mod ticket_3208_tests;

/// The #3436 pins (the callable-typed callee's unit-kind return staging:
/// `Null Function() nf`'s `null == nf()` and `nf() is int` folds, the
/// `dynamic`/`void`/`Never` twins, and the under-report posture for
/// `Function`/`Record`/`Unknown` returns). Registered here, not in
/// `tests.rs` (other lanes' file), via `#[path]`.
#[cfg(test)]
#[path = "ticket_3436_tests.rs"]
mod ticket_3436_tests;
