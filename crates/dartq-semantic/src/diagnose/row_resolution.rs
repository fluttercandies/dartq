use super::*;

/// The send-position shape the downward-context face resolves
/// ([`Engine::send_position`]).
enum SendPosition {
    /// The index argument of `a[e]`: the `[`'s token index, and whether
    /// the access is a pure write (`a[e] = v` — the `[]=` formal's
    /// context).
    Index {
        lbracket: usize,
        write_only: bool,
    },
    /// A call argument: the enclosing `(`'s token index, the selector
    /// spelling before it, the named label (`m(x: e)`), and the
    /// positional index among the depth-0 commas.
    Argument {
        open: usize,
        selector: StringId,
        label: Option<StringId>,
        index: usize,
    },
}

/// The receiver-expression walk's recursion cap (see
/// `Engine::recv_walk_depth`): past it a hop degrades to the record's
/// classified receiver instead of recursing toward a worker-thread stack
/// overflow. ~1 MB of walk frames at four small frames per hop — safely
/// inside rayon's 2 MB worker stacks; real chains are tens of links.
const RECV_WALK_DEPTH_CAP: u32 = 1024;

/// The declaration face behind a bare local's write target (#3282) — the
/// assignment-to-final-local arm's read of the declaration header.
#[derive(Clone, Copy)]
enum FinalLocalFace {
    /// Not a `final` declaration, or one whose own row owns the write
    /// (`const` → `assignment_to_const`; `late` → the late-final row;
    /// `var`/plain → writable).
    Other,
    /// A `final` local or formal. `assigned` marks a declaration that is
    /// not definitely unassigned at a later write: an initializer, a formal
    /// parameter (bound on entry), or a for-in loop variable.
    Final { assigned: bool },
}

/// Whether the token kind after a bare name turns it into a WRITE: the
/// assignment operators (`=`, `op=`, `??=`), the increment/decrement
/// suffixes, or the for-in head's `in` (the loop assigns its target).
fn is_write_follower(kind: Option<dartq_scanner::TokenKind>) -> bool {
    use dartq_scanner::TokenKind;
    matches!(
        kind,
        Some(
            TokenKind::Eq
                | TokenKind::PlusEq
                | TokenKind::MinusEq
                | TokenKind::StarEq
                | TokenKind::SlashEq
                | TokenKind::PercentEq
                | TokenKind::TildeSlashEq
                | TokenKind::LtLtEq
                | TokenKind::GtGtEq
                | TokenKind::GtGtGtEq
                | TokenKind::AmpersandEq
                | TokenKind::BarEq
                | TokenKind::CaretEq
                | TokenKind::QuestionQuestionEq
                | TokenKind::PlusPlus
                | TokenKind::MinusMinus
                | TokenKind::Keyword(dartq_scanner::Keyword::In)
        )
    )
}

/// The `dart:async` class face the member tables lack: `Stream`'s own
/// declared instance surface (upstream Dart SDK source,
/// sdk/lib/async/stream.dart — `map`/`where`/`listen`/`first`/…; the
/// `Object` members ride the shared `Object` exemption). The extension
/// `on`-face walk chain-breaks on the unindexed declaration, so a bare
/// miss inside an extension body would report where the oracle resolves
/// through the SDK class (the xml_events `each_event` tap shape). Consulted
/// beside [`CORE_INTERFACE_MEMBERS`] / [`CORE_TYPE_MEMBERS`] by the
/// frozen-face classification.
pub(crate) const SDK_FACES_EXTRA: &[(&str, &[&str])] = &[
    (
        "Stream",
        &[
            "any",
            "asBroadcastStream",
            "asyncExpand",
            "asyncMap",
            "cast",
            "contains",
            "distinct",
            "drain",
            "elementAt",
            "every",
            "expand",
            "first",
            "firstWhere",
            "fold",
            "forEach",
            "handleError",
            "isEmpty",
            "isBroadcast",
            "join",
            "last",
            "lastWhere",
            "length",
            "listen",
            "map",
            "pipe",
            "reduce",
            "single",
            "singleWhere",
            "skip",
            "skipWhile",
            "take",
            "takeWhile",
            "timeout",
            "toList",
            "toSet",
            "transform",
            "where",
        ],
    ),
    // dart:isolate's own faces (sdk/lib/isolate/isolate.dart:849/:931) —
    // `ReceivePort implements Stream<dynamic>`, so its frozen row is the
    // whole instance face: own declarations beside the Stream's the
    // unindexed edge cannot traverse (the same rule CORE_INTERFACE_
    // MEMBERS rows follow; `SDK_SUPER_FACES` joins the Stream row).
    ("ReceivePort", &["close", "listen", "sendPort"]),
    ("SendPort", &["send"]),
    // dart:io's `Link` (sdk/lib/io/link.dart:9 — `abstract interface class
    // Link implements FileSystemEntity`): the whole instance face the
    // unindexed dart:io container cannot walk; the FileSystemEntity row's
    // members the `implements` edge would add stay the under-report
    // direction (the same shape the ReceivePort row carries).
    (
        "Link",
        &[
            "create",
            "createSync",
            "delete",
            "deleteSync",
            "exists",
            "existsSync",
            "rename",
            "renameSync",
            "resolveSymbolicLinks",
            "resolveSymbolicLinksSync",
            "target",
            "targetSync",
            "update",
            "updateSync",
        ],
    ),
];

/// The unindexed `implements` edges between SDK faces the frozen verdict
/// joins: `ReceivePort implements Stream<dynamic>` makes the Stream row's
/// members answer for a ReceivePort-anchored climb.
const SDK_SUPER_FACES: &[(&str, &[&str])] = &[("ReceivePort", &["Stream"])];

/// The frozen-face member verdict (`Engine::frozen_member_verdict`): the
/// three answers a [`Walk::ChainBroken`] member walk can still decide
/// through [`CORE_INTERFACE_MEMBERS`] / [`CORE_TYPE_MEMBERS`] —
/// `Defined` (a reachable frozen face carries the member), `Absent`
/// (every hop was decidable and none carried it — the member family
/// reports), `Unknown` (a genuinely unindexed, non-frozen declaration
/// broke the climb — the under-report direction, same as `ChainBroken`).
pub(crate) enum FrozenFace {
    Defined,
    Absent,
    Unknown,
}

/// The compound/prefix/postfix operator verdict
/// ([`Engine::adjacent_operator_defined_in`]): the operand's static type
/// defines the spelling, two applicable extensions make it ambiguous
/// (the oracle reports `ambiguous_extension_member_access` at the
/// operator, never `undefined_operator`), or nothing defines it.
enum AdjacentOp {
    Defined,
    Ambiguous(Vec<(String, String)>),
    Missing,
}

/// The static type a cascade target receiver classifies to (#2459②③): the
/// named faces answer the `..[` section's operator question directly, a
/// prefix or unresolvable receiver has no type the model can walk.
fn cascade_target_ty_of(recv: &Receiver) -> Option<Ty> {
    match recv {
        Receiver::Typed { ty, .. } => Some(*ty),
        Receiver::Created { name, .. }
        | Receiver::Container { name, .. }
        | Receiver::PrefixedContainer { name, .. }
        | Receiver::This {
            container: name, ..
        }
        | Receiver::Super {
            container: name, ..
        } => Some(Ty::named(*name)),
        Receiver::Prefix { .. } | Receiver::Unknown => None,
    }
}

impl<'a> Engine<'a> {
    /// The named argument membership test against the active callee
    /// (`type_analyzer.dart:5820`): a resolved callee whose formal table
    /// lacks the label reports `undefined_named_parameter` at the label; an
    /// unresolved callee stays silent (its own invocation code reports) and
    /// only feeds the unresolved-name table.
    pub(crate) fn check_named_argument(&mut self, name: StringId, range: SourceRange) {
        // The positional arm's live drain (upstream
        // `_visitParametersAndArguments`, element_usage_detector.dart
        // :405-412): the open level already holds every unlabeled argument
        // flushed before this label, so the index pairing runs here — the
        // arglist-close site sits in a sibling lane's file. Revisits at
        // later labels are no-ops under `check_formal_arg_annotations`'s
        // emitted-row frontier.
        self.check_positional_formal_annotations();
        // A super-constructor invocation's labels bind against the resolved
        // superclass constructor's named formals
        // (`required_parameters_verifier.dart`'s super-invocation face —
        // the callee element resolves like `check_super_constructor`, and
        // `super(name: v)` reads the unnamed constructor's table while
        // `super.named(name: v)` reads the selector's).
        if self.initializer_depth > 0 {
            if let Some((container, selector)) = self.init_super_callee() {
                // The label's declaring owner — the resolved
                // super-constructor's own spelling — feeds the deprecated-
                // parameter face (`element_usage_detector.dart`'s
                // `_visitParametersAndArguments`).
                let owner = self.super_ctor_owner(container, selector);
                // An unresolvable super-constructor (missing superclass, an
                // external container the index does not model) stays silent
                // at the labels — the unresolved-callee posture, same as the
                // invokes-stack path below; the initializer's own
                // constructor faces own that report. The names table still
                // carries the miss only when the callee resolved (the main
                // path's `!defined` shape: an unresolved callee's label is
                // not a miss).
                let (resolved, defined) = match self.super_ctor_named_formals(container, selector) {
                    // The membership test is the call-site face's own
                    // (`named_formal_for`): a `super(x: v)` label binds a
                    // `this._x` formal's public name like every other
                    // invocation (upstream keys `NamedExpression.name`
                    // against `ParameterElement.name`, which is the public
                    // spelling — `correspondingPublicName`,
                    // token_impl.dart:364).
                    Some(formals) => (true, self.named_formal_for(&formals, name).is_some()),
                    None => (false, false),
                };
                if let Some((decl_file, owner_key)) = owner {
                    self.check_formal_arg_annotations(range, name, decl_file, owner_key);
                }
                if resolved && !defined {
                    let text = self.strings.resolve(name).to_string();
                    self.emit(SemanticCode::UndefinedNamedParameter, range, &text, "");
                }
                if !defined {
                    self.record_miss(name, range, false, NameUse::NamedArgument);
                }
                return;
            }
        }
        let Some(callee) = self.invokes.last().and_then(|slot| *slot) else {
            return;
        };
        let (shape, callee_name, qualified) = match self.rows.get(callee) {
            Some(row) => (row.shape.clone(), row.name, row.qualified),
            None => return,
        };
        let (resolved, defined) = match &shape {
            // The membership test exists only where the oracle's does: a real
            // top-level function's formal table. A class callee is a
            // constructor invocation (a constructor accepting the label is
            // silent — constructors sit outside the M1 model) and a getter
            // callee is a property access; both stay unresolved, mirroring
            // the arity path's filter.
            RowShape::Bare(_) => {
                // An invoked qualified selector (`c.m(b: 1)`) reaches its
                // labels before the receiver merge: the parser fires the dot
                // event after the send, so the latched row is still the
                // selector's placeholder — the `qualified` plain row sitting
                // directly on its receiver. Classify that pair now; the
                // deferred merge needs the stack untouched.
                if matches!(shape, RowShape::Bare(BareInfo::Plain { .. })) && qualified {
                    let recv = match callee
                        .checked_sub(1)
                        .and_then(|i| self.rows.get(i))
                        .map(|below| below.shape.clone())
                    {
                        Some(RowShape::Bare(info)) => Engine::receiver_of_bare(info),
                        Some(RowShape::Receiver(recv)) => recv,
                        _ => Receiver::Unknown,
                    };
                    let formals = self.member_formals_full(&recv, callee_name);
                    // The resolved member's declaring owner keys its
                    // formal stars — the deprecated-parameter face's join.
                    if let Some(owner) = self.member_formals_owner(&recv, callee_name) {
                        self.check_formal_arg_annotations(range, name, owner.1, owner.0);
                    }
                    (
                        formals.is_some(),
                        formals.is_some_and(|f| self.named_formal_for(&f, name).is_some()),
                    )
                } else {
                    let decl = self
                        .top_level_decl(callee_name)
                        .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some());
                    let defined = decl.as_ref().is_some_and(|d| {
                        d.formals
                            .as_deref()
                            .is_some_and(|f| self.named_formal_for(f, name).is_some())
                    });
                    if defined {
                        // A top-level function keys its parameter stars
                        // under its own name, on its declaring file.
                        if let Some(file) = self.top_level_decl_file(callee_name) {
                            self.check_formal_arg_annotations(
                                range,
                                name,
                                file,
                                callee_name,
                            );
                        }
                    }
                    (decl.is_some(), defined)
                }
            }
            RowShape::Member { recv } => {
                let formals = self.member_formals_full(recv, callee_name);
                if let Some(owner) = self.member_formals_owner(recv, callee_name) {
                    self.check_formal_arg_annotations(range, name, owner.1, owner.0);
                }
                (
                    formals.is_some(),
                    formals.is_some_and(|f| self.named_formal_for(&f, name).is_some()),
                )
            }
            _ => return,
        };
        if resolved && !defined {
            let text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::UndefinedNamedParameter, range, &text, "");
        }
        if !defined {
            // The names table carries every real miss (the membership test
            // itself is live).
            self.record_miss(name, range, false, NameUse::NamedArgument);
        }
    }

    /// The positional arm of `element_usage_detector.dart`'s
    /// `_visitParametersAndArguments` (:405-412): every unlabeled argument
    /// pairs by index against the callee's positional formals
    /// (`parameters[positionalIndex++]`, the `parameter.isPositional` gate —
    /// a declared list's named formals always trail, so the open group's
    /// `formals.positional` index is the same walk). A tagged formal reports
    /// at the argument's own expression — the `checkUsage(parameter,
    /// argument)` anchor — never the callee head.
    ///
    /// The upstream walk runs once per closed argument list; this mirror
    /// drains the still-open level at each `name:` label (the group's close
    /// site sits in a sibling lane's file — the same posture the
    /// HELD-WIRING seam takes), so the same argument can visit more than
    /// once under a later label (`f(0, x: 1, y: 2)` pairs `0` at `x:` and
    /// again at `y:`). The emitted-row frontier inside
    /// [`Engine::check_formal_arg_annotations`] makes the revisit a no-op —
    /// each (argument offset, tag) reports once, matching the upstream
    /// single-pass behavior exactly.
    ///
    /// A group with no named label (`f(0)`) never reaches this arm from
    /// the label event: its wiring awaits the arglist-close seam beside
    /// `solve_call_params` (invocation.rs `close_arguments`'s
    /// `level.args`-holding point, currently owned by a sibling lane). The
    /// signature deliberately reads the live level off `arg_levels` so
    /// that seam's call is `self.check_positional_formal_annotations()`
    /// verbatim — no re-parameterization.
    pub(crate) fn check_positional_formal_annotations(&mut self) {
        let Some(level) = self.arg_levels.last() else {
            return;
        };
        // Collect the unlabeled arguments first: the pairing walks them in
        // source order and the borrow on `level` ends before the owner
        // resolution takes `&mut self` (the emit path emits).
        let positionals: Vec<SourceRange> = level
            .args
            .iter()
            .filter(|(name, _, _, _)| name.is_none())
            .map(|(_, _, _, range)| *range)
            .collect();
        let callee_row = level.callee_row;
        if positionals.is_empty() {
            return;
        }
        // Resolve the callee's `(formals, owner file, owner key)` through
        // the same shape dispatch `check_named_argument` runs — the
        // super-constructor face first (it reads the open level's callee
        // row, not `invokes`), then the row-shape walk.
        let resolved: Option<(Formals, FileId, StringId)> = if self.initializer_depth > 0
            && let Some((container, selector)) = self.init_super_callee()
        {
            let formals = self.super_ctor_named_formals(container, selector);
            let owner = self.super_ctor_owner(container, selector);
            match (formals, owner) {
                (Some(formals), Some((file, key))) => Some((formals, file, key)),
                _ => None,
            }
        } else {
            self.positional_callee_formals(callee_row)
        };
        let Some((formals, file, owner)) = resolved else {
            return;
        };
        for (index, range) in positionals.iter().enumerate() {
            let Some(formal) = formals.positional.get(index) else {
                // More unlabeled arguments than declared positionals — the
                // upstream `positionalIndex < parameters.length` bound; the
                // arity faces own that report.
                break;
            };
            self.check_formal_arg_annotations(*range, formal.name, file, owner);
        }
    }

    /// The non-super callee's formal table and declaring owner for the
    /// positional arm: the same shapes [`Engine::check_named_argument`]
    /// classifies — a qualified member send's pre-merge selector over its
    /// receiver row, an unnamed creation's container head, a named
    /// creation's selector over a class receiver, and a plain top-level
    /// callee. `callee_row` is the group's latched callee index
    /// ([`ArgLevel::callee_row`]); `invokes.last()` cannot serve inside an
    /// initializer's nested group the way it does at the label event.
    fn positional_callee_formals(
        &mut self,
        callee_row: Option<usize>,
    ) -> Option<(Formals, FileId, StringId)> {
        let callee = self
            .invokes
            .last()
            .and_then(|slot| *slot)
            .or(callee_row)?;
        let (shape, name, qualified) = match self.rows.get(callee) {
            Some(row) => (row.shape.clone(), row.name, row.qualified),
            None => return None,
        };
        match &shape {
            // `c.m(0)` / `super.m(0)` — the merged member row.
            RowShape::Member { recv } => {
                let formals = self.member_formals_full(recv, name)?.into_owned();
                let (owner, file) = self.member_formals_owner(recv, name)?;
                Some((formals, file, owner))
            }
            RowShape::Bare(BareInfo::Plain { .. }) if qualified => {
                // The pre-merge qualified selector (`c.m(`, `B.named(`): the
                // receiver row sits directly below — classify it for the
                // member walk, falling to the constructor table when the
                // receiver names a class.
                let recv = callee
                    .checked_sub(1)
                    .and_then(|index| self.rows.get(index))
                    .map_or(Receiver::Unknown, |below| {
                        match &below.shape {
                            RowShape::Bare(info) => Engine::receiver_of_bare(*info),
                            RowShape::Receiver(recv) => *recv,
                            _ => Receiver::Unknown,
                        }
                    });
                if let Some(formals) = self.member_formals_full(&recv, name) {
                    let formals = formals.into_owned();
                    let (owner, file) = self.member_formals_owner(&recv, name)?;
                    return Some((formals, file, owner));
                }
                // `B.named(`: constructors carry no MemberEntry — the
                // container's `ctor_formals`/`factory_formals` key on the
                // written selector, the owner key joins `Class.selector`.
                let (class, file) = match recv {
                    Receiver::Container {
                        name: class,
                        kind: ContainerKind::Class,
                    } => resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        class,
                    )
                    .map(|(_, file)| (class, file))?,
                    Receiver::PrefixedContainer {
                        prefix,
                        name: class,
                        kind: ContainerKind::Class,
                    } => prefixed_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        prefix,
                        class,
                    )
                    .map(|(_, file)| (class, file))?,
                    _ => return None,
                };
                let formals = self
                    .ctor_schema_formals(class, name)
                    .or_else(|| self.factory_ctor_formals(class, name))?;
                let key = self.strings.intern(&format!(
                    "{}.{}",
                    self.strings.resolve(class),
                    self.strings.resolve(name)
                ));
                Some((formals, file, key))
            }
            // `B(0)` — the unnamed creation's container head: the unnamed
            // face's `ctor_formals` key is the class's own name, and the
            // collector's `formal_annotations` owner join doubles it into
            // `C.C` (`C.<key>` with `<key>` the class name).
            RowShape::Bare(BareInfo::Container {
                name: class,
                kind: ContainerKind::Class | ContainerKind::ExtensionType,
            }) if !qualified => {
                let (decl, file) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    *class,
                )?;
                let formals = (**decl.ctor_formals.get(class)?).clone();
                let class_spelling = self.strings.resolve(*class);
                let key = self
                    .strings
                    .intern(&format!("{class_spelling}.{class_spelling}"));
                Some((formals, file, key))
            }
            // `f(0)` — a top-level function keys its formals under its own
            // name on its declaring file.
            RowShape::Bare(BareInfo::Plain { .. }) => {
                let decl = self
                    .top_level_decl(name)
                    .filter(|d| d.kind == MicroKind::TopLevelFn)?;
                let formals = decl.formals.as_deref()?.clone();
                let file = self.top_level_decl_file(name)?;
                Some((formals, file, name))
            }
            _ => None,
        }
    }

    /// The super-constructor callee an open argument group inside an
    /// initializer list binds to: `(enclosing container, selector)` —
    /// `None` = the unnamed face (`super(...)`, `super.new(...)`). The
    /// group's callee row reads back through the arg level's mark: a
    /// bare `super(` keeps its [`Receiver::Super`] row (the `invokes` link
    /// drops it — the keyword never resolves as a member), a folded
    /// `super.name(` selector row matches its [`PendingInitSuper`] entry.
    /// `None` when the open group's callee is neither shape (an ordinary
    /// member call nested in the list's expression, `x = super.m(a: 1)`).
    pub(crate) fn init_super_callee(&self) -> Option<(StringId, Option<StringId>)> {
        let level = self.arg_levels.last()?;
        // The group's callee row was latched at open (`ArgLevel.callee_row`
        // — `mark` alone cannot serve: `flush_argument` advances it past
        // each argument's own row).
        let head = self.rows.get(level.callee_row?)?;
        if let RowShape::Receiver(Receiver::Super { container, .. }) = &head.shape {
            return Some((*container, None));
        }
        // `super.name(`: the folded Value row's offset matches its
        // `PendingInitSuper.selector_offset` — only the invoked head shape
        // redirects (an `x = super.m(...)` expression callee folds the same
        // Value row but its candidate never armed `invoked`).
        self.pending_init_super
            .iter()
            .rev()
            .find(|c| c.invoked && c.head && c.selector_offset == head.range.offset)
            .map(|c| (c.container, Some(c.name)))
    }

    /// The resolved super-constructor's formal table (`ctor_formals`) for
    /// `check_named_argument`: `selector` picks the named table, `None` the
    /// unnamed face's key (the enclosing class's `superclass` spelling —
    /// `ctor_formals` keys the unnamed entry under the declared name).
    /// `None` = unresolvable superclass or a constructor the index does not
    /// model — the caller stays silent (the under-report posture; the
    /// oracle's own `undefined_constructor_in_initializer` owns that
    /// report). The table comes back owned so the caller can hand it to
    /// [`Engine::named_formal_for`] and still `emit` under `&mut self`.
    pub(crate) fn super_ctor_named_formals(
        &self,
        container: StringId,
        selector: Option<StringId>,
    ) -> Option<Formals> {
        let superclass = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass)?;
        let (super_container, _) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            superclass,
        )?;
        super_container
            .ctor_formals
            .get(&selector.unwrap_or(superclass))
            .map(|formals| (**formals).clone())
    }

    /// The declaring owner of a resolved super-constructor callee — the
    /// `(file, owner key)` pair [`Engine::check_formal_arg_annotations`]
    /// consults for the deprecated-parameter face. The owner spelling is
    /// the collector's `C.<key>` join over the superclass's `ctor_formals`
    /// lookup spelling — `Class.selector` for a named face, the doubled
    /// `Class.Class` for the unnamed one (the `ctor_formals` key is the
    /// class's own name).
    fn super_ctor_owner(
        &mut self,
        container: StringId,
        selector: Option<StringId>,
    ) -> Option<(FileId, StringId)> {
        let superclass = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass)?;
        let (_, owner_file) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            superclass,
        )?;
        let class = self.strings.resolve(superclass).to_owned();
        // The collector's `formal_annotations` owner join is `C.<key>`
        // where `<key>` is the `ctor_formals` lookup spelling — the
        // selector for a named face and the class's own name for the
        // unnamed one (`A.new`, `A`, `super(...)` all land on `C.C`).
        let key = match selector {
            Some(selector) => format!(
                "{class}.{}",
                self.strings.resolve(selector)
            ),
            None => format!("{class}.{class}"),
        };
        Some((owner_file, self.strings.intern(&key)))
    }

    /// The declaring owner of a resolved member callee — `(owner key,
    /// file)`. The member walk's `found_origin` gives the declaring
    /// container and its file; the key is the `C.m` join the collector's
    /// `formal_annotations` side-map writes. `None` on a miss, a broken
    /// chain, or a receiver the member face does not model (the
    /// under-report posture — an unresolved callee's label carries no
    /// parameter use at all).
    fn member_formals_owner(
        &mut self,
        recv: &Receiver,
        name: StringId,
    ) -> Option<(StringId, FileId)> {
        let (container, super_walk) = match *recv {
            Receiver::This { container, .. } => (container, false),
            Receiver::Super { container, .. } => (container, true),
            Receiver::Typed { type_name, .. } | Receiver::Created { name: type_name, .. } => {
                (type_name, false)
            }
            Receiver::Container {
                name: container, ..
            } => {
                let (decl, owner_file) = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    container,
                )?;
                return decl.members.get(&name).and_then(|entry| {
                    (entry.kind == MemberKind::Method).then(|| {
                        (
                            self.strings
                                .intern(&format!("{}.{}", self.strings.resolve(container), self.strings.resolve(name))),
                            owner_file,
                        )
                    })
                });
            }
            Receiver::PrefixedContainer {
                prefix,
                name: container,
                ..
            } => {
                let (decl, owner_file) = prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    container,
                )?;
                return decl.members.get(&name).and_then(|entry| {
                    (entry.kind == MemberKind::Method).then(|| {
                        (
                            self.strings
                                .intern(&format!("{}.{}", self.strings.resolve(container), self.strings.resolve(name))),
                            owner_file,
                        )
                    })
                });
            }
            // A prefixed top-level callee's owner is the function itself
            // (its declaring file) — the `f`-keyed arm.
            Receiver::Prefix { name: prefix } => {
                let dartq_resolution::Lookup::Found(NamespaceEntry::Decl(file)) = self
                    .namespaces
                    .lookup_prefixed(self.graph, self.lib, prefix, name)
                else {
                    return None;
                };
                return self
                    .index
                    .get(file)
                    .and_then(|f| f.decl(name))
                    .filter(|d| d.kind == MicroKind::TopLevelFn)
                    .map(|_| (name, file));
            }
            Receiver::Unknown => return None,
        };
        let walk = if super_walk {
            self.walk_super(container, name)
        } else {
            self.walk_members(container, name)
        };
        match walk {
            Walk::Found(entry) if entry.kind == MemberKind::Method => {
                let (decl_container, decl_file) = self.member_faces.found_origin.get()?;
                Some((
                    self.strings.intern(&format!(
                        "{}.{}",
                        self.strings.resolve(decl_container),
                        self.strings.resolve(name)
                    )),
                    decl_file,
                ))
            }
            _ => None,
        }
    }

    /// The declaring file of a resolved top-level callee — the file half of
    /// the `f`-keyed [`Engine::check_formal_arg_annotations`] owner.
    fn top_level_decl_file(&self, name: StringId) -> Option<FileId> {
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(_) => Some(self.file),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => Some(file),
            _ => None,
        }
    }

    // —— pass 2 ———————————————————————————————————————————————————————

    /// Resolves the recorded rows in source order and emits. The flow
    /// promotions (#2241) ride beside the read types: the member walk's
    /// receiver verdict re-decides against them, exactly like the staged
    /// families' teardown join.
    /// The deferred invocation face's pass-2 half (#2547): the callee's
    /// flow read type re-decides callability — a promotion that narrowed
    /// the local to a callable face (a `call` class, a function shape)
    /// drops the staged row; any other flow face (or none — the
    /// read-type fact missing) keeps the parse-time verdict.
    fn apply_deferred_invocation_faces(&mut self) {
        let deferred = std::mem::take(&mut self.deferred_invocation_faces);
        for (span, name) in deferred {
            // The flow-promoted function form: the read's flow face IS the
            // `Function` head — callable by definition
            // (function_expression_invocation_resolver.dart:67-79), even
            // though the staged type row spells `Unknown` (the shapeless
            // head has no `Ty`).
            if self.promotion_function_reads.contains(&span.offset) {
                continue;
            }
            let flow_ty = self.read_type_at(span);
            let callable = match flow_ty {
                Some(ty) => match ty {
                    Ty::Function { .. } => true,
                    Ty::Named {
                        name: head,
                        nullable,
                        ..
                    } => {
                        self.strings.resolve(head) == "Function"
                            || self.fn_var_sigs.contains_key(&name)
                            || self.alias_fn_sig(head).is_some()
                            || matches!(
                                self.resolved_member(head, self.intern("call")),
                                Some(entry) if entry.kind == MemberKind::Method
                            )
                            // The bound/extension faces the parse-time arm
                            // holds (`resolveToBound` and the extension-
                            // `call` resolve — upstream Dart SDK source,
                            // pkg/analyzer/lib/src/dart/resolver/
                            // function_expression_invocation_resolver.dart
                            // :66, :92-100): a flow read typed by an
                            // in-scope parameter answers through its bound;
                            // any head answers through an applicable
                            // extension's `call` method.
                            || (self.is_scope_param(head)
                                && self.param_bound_callable(head))
                            || self.extension_call_member(head, nullable)
                    }
                    _ => false,
                },
                None => false,
            };
            if callable {
                continue;
            }
            let code = SemanticCode::InvocationOfNonFunctionExpression;
            self.report.usage.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message("", ""),
                self.file,
                span,
                code.correction_message(""),
            ));
        }
    }

    /// The pass-2 re-bind of a promotion-latched initializer (#2539):
    /// each deferred local re-derives its initializer member's static
    /// type over the promoted receiver (the same re-decision
    /// [`Engine::rederived_receiver`] makes for the member row itself)
    /// and re-binds when the answer names a different type than the
    /// parse-time bind wrote. Runs before any row resolves, so every
    /// later face — the re-bound local's own member reads, the argument
    /// and return checks — reads the promoted accessor face. The answer
    /// rides [`Engine::receiver_static_ty`] on a synthetic row (the same
    /// single source the chained faces consult): an accessor read
    /// instantiates the declared return against the re-derived receiver,
    /// and an invoked member (`var l = c.fragments.toList()`) answers
    /// the instantiated SEND result, never the raw schema a bare
    /// `return_ty` read would leak.
    /// An object-pattern shorthand binder's declared type
    /// (`Head(:var x)`): the getter's declared return on the head — the
    /// oracle's `resolveObjectPatternPropertyGet` types the bound variable
    /// from the property. The head instantiates to bounds when it spells
    /// no arguments (`Square` over `Square<T extends MetricUnits>` reads
    /// `Square<MetricUnits>` — the iterable element's own face), and the
    /// member's return substitutes through that environment (`Unit<T>`
    /// answers `Unit<MetricUnits>`). A parametric return whose environment
    /// cannot substitute (a foreign or unresolvable face), an unresolved
    /// getter, and a non-accessor member answer `None` — the under-report
    /// direction: the binder keeps the raw unknown.
    pub(crate) fn object_pattern_binder_ty(&self, head: StringId, getter: StringId) -> Option<Ty> {
        let Walk::Found(entry) = self.walk_members(head, getter) else {
            return None;
        };
        if !matches!(entry.kind, MemberKind::Getter | MemberKind::Field) || entry.is_static {
            return None;
        }
        let result = Self::member_result_ty(&entry);
        // The bare head resolves: no free parameters, no substitution.
        let Ty::Named { args, .. } = &result else {
            return Some(result);
        };
        if *args == ArgListId::EMPTY {
            return Some(result);
        }
        // The receiver's instantiate-to-bounds face (`Square` over
        // `Square<T extends MetricUnits>`) carries the written arguments —
        // zip the head's declared parameters against them for the
        // substitution map.
        let instantiated = self.instantiated_head_ty(Ty::named(head));
        let Ty::Named {
            args: inst_args, ..
        } = &instantiated
        else {
            return None;
        };
        if *inst_args == ArgListId::EMPTY {
            return None;
        }
        // The substitution domain is the head's parameter NAMES only —
        // the same-file `container_params` snapshot is the canonical
        // producer, and a FOREIGN generic (`Circle<T extends MetricUnits>`
        // in the pattern helper's own unit) falls back to the declaring
        // unit's `type_param_names` via `foreign_container_params` — the
        // `resolve_container`-style face `instantiated_head_ty` already
        // applies for the args side (#3180). Bounds and poison bits are
        // never consulted here, so the names-only fallback suffices.
        let params: Vec<StringId> = match self.bounds.container_params(head) {
            Some(params) => params.iter().map(|(name, ..)| *name).collect(),
            None => self.foreign_container_params(head)?,
        };
        let inst_args = self.boundary.pool().get(*inst_args).to_vec();
        if params.len() != inst_args.len() {
            return None;
        }
        let env: FxHashMap<StringId, Ty> = params
            .iter()
            .zip(inst_args.iter())
            .map(|(param, ty)| (*param, *ty))
            .collect();
        let substituted = self.substitute_ty(result, &env);
        (substituted != result && substituted != Ty::UNKNOWN).then_some(substituted)
    }

    /// The shared re-bind direction gate: only a NARROWING face is a
    /// promotion (see the member arm's inline twin for the full
    /// rationale — the `!`/`as` widening binds keep their written face).
    fn promotion_narrows(&self, corrected: Ty, parse_ty: Ty) -> bool {
        match (corrected, parse_ty) {
            (
                Ty::Named {
                    name: c_head,
                    nullable: c_null,
                    ..
                },
                Ty::Named {
                    name: p_head,
                    nullable: p_null,
                    ..
                },
            ) => {
                let nullability_ok = !c_null || p_null;
                if p_head == self.intern("Object") {
                    nullability_ok && !c_null
                } else if c_head == p_head {
                    nullability_ok
                } else {
                    let non_null = |name: StringId| Ty::Named {
                        name,
                        nullable: false,
                        args: ArgListId::EMPTY,
                    };
                    !matches!(
                        self.declared_subtype_negative(&non_null(c_head), &non_null(p_head)),
                        Some(false)
                    )
                }
            }
            _ => false,
        }
    }

    fn apply_deferred_rebinds(&mut self) {
        let deferred = std::mem::take(&mut self.deferred_var_rebinds);
        for (decl_offset, member_offset, parse_ty) in deferred {
            let Some(index) = self
                .member_reads
                .binary_search_by(|read| read.offset.cmp(&member_offset))
                .ok()
            else {
                continue;
            };
            let read = self.member_reads[index];
            let recv = self.rederived_receiver(member_offset, read.recv);
            // An unlatched chain link (`ce.fragments.firstWhere(...)` over a
            // re-bound `ce`) keeps its merge-time face in the record; the
            // receiver expression re-derives through the re-bound head, so
            // its face wins when it refines the same container
            // (`List<CFrag>` over `List<Frag>`).
            let recv = match (recv, self.recv_expression_ty(read.recv_span)) {
                (
                    Receiver::Typed {
                        type_name,
                        ty: Ty::Named { name, nullable, .. },
                    },
                    Some(
                        face @ Ty::Named {
                            name: face_name,
                            nullable: face_nullable,
                            ..
                        },
                    ),
                ) if name == face_name && nullable == face_nullable => Receiver::Typed {
                    type_name,
                    ty: face,
                },
                (recv, _) => recv,
            };
            let Receiver::Typed { ty: recv_ty, .. } = recv else {
                continue;
            };
            if !matches!(recv_ty, Ty::Named { .. }) {
                continue;
            }
            // The invoked-member probe (the same byte test the chained
            // faces take): a member token directly followed by `(` is a
            // send, so the initializer's static type is the callee's
            // result face, not the accessor's declared return.
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
                shape: RowShape::Member { recv },
                intent: if invoked {
                    Intent::Invoke
                } else {
                    Intent::Read
                },
                write_op: None,
                ty: Ty::UNKNOWN,
            };
            let mut corrected = self.receiver_static_ty(&row);
            // The await flatten the parse-time bind applies (#2506): an
            // initializer inside an await extent binds the FUTURE'S
            // argument (`final db = await open();`), and the re-derived
            // accessor face owes the same flatten — a raw `Future<T>`
            // re-bind would un-flatten the local.
            if self
                .await_operand_extents
                .iter()
                .any(|&(start, end)| member_offset >= start && member_offset < end)
            {
                corrected = self.flatten(corrected);
            }
            // The promotion direction gate: only a NARROWING re-bind is a
            // promotion. The re-derived accessor's declared return can
            // WIDEN the parse-time face — a `!`-asserted initializer's
            // non-null bind, an `as`-cast bind — and the oracle keeps the
            // written face there (the assert/cast decided it), so the
            // re-bind must not replace it. `corrected` narrows `parse_ty`
            // exactly when its head's supertype chain names `parse_ty`'s
            // head and its nullability does not loosen.
            let narrows = match (corrected, parse_ty) {
                (
                    Ty::Named {
                        name: c_head,
                        nullable: c_null,
                        ..
                    },
                    Ty::Named {
                        name: p_head,
                        nullable: p_null,
                        ..
                    },
                ) => {
                    let nullability_ok = !c_null || p_null;
                    // `Object` is the universal root: every named face
                    // narrows it (the core heads' incomplete chains would
                    // answer `Missing` on the walk the resolved world
                    // cannot complete).
                    if p_head == self.intern("Object") {
                        nullability_ok && !c_null
                    } else if c_head == p_head {
                        nullability_ok
                    } else {
                        // The cross-head promotion question is subtype-ness
                        // — whether `c_head`'s supertype closure names
                        // `p_head` — never a member lookup: a subclass
                        // field's covariant override (`Impl.element`'s
                        // `Sub2` against the base getter's `Sub?`) has no
                        // member SPELLED `Sub`, so the member walk answered
                        // `Missing` and dropped every override-precision
                        // re-bind. `declared_subtype_negative`'s `None`
                        // covers subtype-or-undecidable (the conservative
                        // accept the old `ChainBroken` arm took); only a
                        // proven non-subtype rejects.
                        let non_null = |name: StringId| Ty::Named {
                            name,
                            nullable: false,
                            args: ArgListId::EMPTY,
                        };
                        !matches!(
                            self.declared_subtype_negative(&non_null(c_head), &non_null(p_head)),
                            Some(false)
                        )
                    }
                }
                _ => false,
            };
            if corrected != Ty::UNKNOWN && corrected != parse_ty && narrows {
                self.rebound_locals
                    .insert(decl_offset, (parse_ty, corrected));
            }
        }
        // The bare-identifier initializer twin (#2567): `var x = p;` where
        // `p`'s flow state carries a promotion infers the PROMOTED type
        // (the official inference reads the initializer's static type —
        // the promoted face). The parse-time bind cannot see the
        // promotion (the read-type table fills after the parse pass), so
        // the declaration stages here and the pass-2 read type re-binds
        // under the same narrowing gate.
        let deferred_bare = std::mem::take(&mut self.deferred_bare_rebinds);
        for (decl_offset, span, parse_ty) in deferred_bare {
            let Some(corrected) = self.read_type_at(span) else {
                continue;
            };
            let narrows = self.promotion_narrows(corrected, parse_ty);
            if corrected != Ty::UNKNOWN && corrected != parse_ty && narrows {
                self.rebound_locals
                    .insert(decl_offset, (parse_ty, corrected));
            }
        }
        // The implicit-call re-binds: the callee's promoted read (a pass-2
        // flow face) resolves the `call` member, and its return narrows
        // the initializer's parse-time face — `switch (x) case
        // XPathExpression():` arm's `var f = x(context)` binds
        // `XPathSequence`, never the declared `Object` arm's callee face.
        let deferred_calls = std::mem::take(&mut self.deferred_call_rebinds);
        for (decl_offset, callee_span, parse_ty) in deferred_calls {
            let Some(read) = self.read_type_at(callee_span) else {
                continue;
            };
            let Some((_, call_ret, _)) = self.implicit_call_face(read) else {
                continue;
            };
            if call_ret != Ty::UNKNOWN
                && call_ret != parse_ty
                && self.promotion_narrows(call_ret, parse_ty)
            {
                self.rebound_locals
                    .insert(decl_offset, (parse_ty, call_ret));
            }
        }
        // The deferred closure formals: the context schema recomputes over
        // the re-derived receiver face (a promotion-latched link below the
        // callee — `rebound.fragments.where((f) => f.isComplete)` — freezes
        // the stale schema entry at pass 1), and the corrected entry joins
        // the same `rebound_locals` map the var re-binds ride, so every
        // read of the formal re-derives through the re-bound face. A formal
        // pass 1 left unsolved re-derives later, after the flow-blind
        // locals' initializer pre-pass in [`Self::resolve_rows`]: its below
        // receiver is often such a local (`var n = d.name; if (n != null)
        // n.tokens.map((e) => …)`).
        let (unsolved, solved): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.deferred_closure_formals)
                .into_iter()
                .partition(|entry| entry.parse_ty == Ty::UNKNOWN);
        self.deferred_closure_formals = unsolved;
        self.apply_deferred_closure_formals(solved);
    }

    /// [`Self::apply_deferred_rebinds`]' closure-formal leg: each entry's
    /// schema recomputes over the re-derived below receiver.
    fn apply_deferred_closure_formals(&mut self, entries: Vec<DeferredClosureFormal>) {
        for entry in entries {
            let Some(face) = self.recv_expression_ty(entry.below) else {
                continue;
            };
            let Ty::Named { name: head, .. } = face else {
                continue;
            };
            let recv = Receiver::Typed {
                type_name: head,
                ty: face,
            };
            // The row keeps its member-token offset across the merge (the
            // fold tiles the length, never the start), so the offset pairs
            // the captured callee identity.
            let corrected = self.closure_context_schema(
                recv,
                entry.callee_name,
                None,
                entry.slot,
                entry.formal,
            );
            let Some(corrected) =
                corrected.filter(|ty| *ty != Ty::UNKNOWN && *ty != entry.parse_ty)
            else {
                continue;
            };
            // An unsolved pass-1 formal accepts only a decided re-derived
            // entry: a provable named type, or the `Object?` recovery of an
            // invalid element argument the receiver itself spells (`List<Und>`
            // — the face the pass-1 bind gives an unpromoted receiver). A
            // leaked type-parameter spelling or a callee-own fresh parameter
            // (`Object?` over a valid receiver) is the horizontal-inference
            // face pass 2 cannot see, and the unsolved `dynamic` stands.
            if entry.parse_ty == Ty::UNKNOWN {
                let decided = match corrected {
                    Ty::Named {
                        name,
                        nullable: false,
                        args,
                    } => self.named_type_is_type(name) && self.args_are_types(args),
                    ty if ty == self.object_question() => self
                        .receiver_first_type_arg(face)
                        .is_some_and(|arg| match arg {
                            // An undeclared head (the unresolvable import's
                            // element) — never a raw stub's leaked parameter
                            // spelling (`Iterable<V>`), which any graph unit
                            // or the core parameter table names.
                            Ty::Named { name, .. } => {
                                !self.named_type_is_type(name)
                                    && self.graph.declaring_libraries(name).is_empty()
                                    && !crate::member_face::CORE_CONTAINER_PARAMS.iter().any(
                                        |(_, params)| params.contains(&self.strings.resolve(name)),
                                    )
                            }
                            _ => false,
                        }),
                    _ => false,
                };
                if !decided {
                    continue;
                }
            }
            // The same narrowing gate the var re-binds apply: the schema
            // entry must narrow the frozen pass-1 face, never widen it. An
            // unsolved pass-1 formal (`Unknown`) takes the re-derived entry.
            let narrows = match (corrected, entry.parse_ty) {
                (_, Ty::UNKNOWN) => true,
                (
                    Ty::Named {
                        name: c_head,
                        nullable: c_null,
                        ..
                    },
                    Ty::Named {
                        name: p_head,
                        nullable: p_null,
                        ..
                    },
                ) => {
                    let nullability_ok = !c_null || p_null;
                    if p_head == self.intern("Object") {
                        nullability_ok && !c_null
                    } else if c_head == p_head {
                        nullability_ok
                    } else {
                        let non_null = |name: StringId| Ty::Named {
                            name,
                            nullable: false,
                            args: ArgListId::EMPTY,
                        };
                        // A nullable entry over a non-null face widens
                        // (the pass-1 upward solve's `T` against the
                        // re-bind's unsolved `Object?` recovery).
                        nullability_ok
                            && !matches!(
                                self.declared_subtype_negative(
                                    &non_null(c_head),
                                    &non_null(p_head)
                                ),
                                Some(false)
                            )
                    }
                }
                _ => false,
            };
            if narrows {
                self.rebound_locals
                    .insert(entry.decl_offset, (entry.parse_ty, corrected));
            }
        }
    }

    pub(crate) fn resolve_rows(&mut self, promotions: &PromotionStaging) {
        self.apply_deferred_rebinds();
        self.apply_deferred_invocation_faces();
        self.build_ur_sites();
        let rows = std::mem::take(&mut self.rows);
        // The #2459 cascade-index target latches: a first-section `..[` fold
        // binds the cascade target row itself, and its push-time type is the
        // target's static type — the oracle resolves EVERY section's index
        // face against it (`x..[0] = 1..[1] = 2` reports both brackets on
        // the target's type). Continuation sections bind the same `..[` fold
        // to the previous section's RHS leftover row (the walk's newest row
        // before the bracket), so their target question reads this table
        // instead.
        let cascade_targets: Vec<(u32, Ty)> = rows
            .iter()
            .filter_map(|row| {
                let Intent::Operator(op, bracket) = row.intent else {
                    return None;
                };
                if op != self.intern("[]") {
                    return None;
                }
                if self.cascade_index_continuation(bracket.offset) {
                    return None;
                }
                let ty = row.ty;
                (ty != Ty::UNKNOWN).then_some((row.range.offset, ty))
            })
            .collect();
        let mut deferred = std::mem::take(&mut self.deferred_miss_rows);
        deferred.sort_by_key(|row| row.range.offset);
        // The flow-blind locals' initializer faces — the pass-2 pre-pass:
        // the flow lattice cannot spell member returns, so an unannotated
        // for-in variable over a property-read iterable
        // (`for (var a in x.annotations)`) latched `Unknown` at parse; its
        // element derives HERE, once, through the semantic expression
        // faces, for every receiver face to read. Source order IS the
        // topological order (a local reads only earlier locals), and the
        // `RefCell` table stages incrementally so a later initializer's
        // receiver chain sees an earlier local's derived face.
        if !self.local_init_rows.is_empty() {
            let mut entries: Vec<(u32, (u32, bool))> = self.local_init_rows.drain().collect();
            entries.sort_by_key(|(_, (row_offset, _))| *row_offset);
            for (decl, (row_offset, for_in)) in entries {
                let Some(row) = rows.iter().find(|row| row.range.offset == row_offset) else {
                    continue;
                };
                // A member initializer's face is the accessor return — the
                // parse-time row carries `Unknown` (its receiver resolved
                // only here), so the member-read record's accessor face
                // re-derives it through the SAME receiver-expression
                // chain every pass-2 verdict reads.
                let record = match &row.shape {
                    RowShape::Member { .. } => self
                        .member_reads
                        .iter()
                        .find(|read| read.offset == row.range.offset),
                    _ => None,
                };
                let mut ty = match record {
                    Some(read) => self.chained_read_static_ty(read),
                    None => Some(self.expression_row_ty(row)),
                }
                .filter(|ty| *ty != Ty::UNKNOWN)
                .unwrap_or(Ty::UNKNOWN);
                if for_in {
                    ty = match ty {
                        Ty::Named { args, .. } => self
                            .boundary
                            .pool()
                            .get(args)
                            .first()
                            .copied()
                            .unwrap_or(Ty::Dynamic),
                        // A non-named iterable face (a raw `Iterable` or an
                        // unresolvable return) stays under-reported — no
                        // invented element.
                        _ => continue,
                    };
                }
                // The await operand's extent flattens the derived face, the
                // same re-application the parse-time bind makes (#2506).
                if !for_in
                    && ty != Ty::UNKNOWN
                    && self
                        .await_operand_extents
                        .iter()
                        .any(|&(start, end)| row.range.offset >= start && row.range.end() <= end)
                {
                    ty = self.flatten(ty);
                }
                if ty == Ty::UNKNOWN {
                    let invalid =
                        !for_in && record.is_some_and(|read| self.member_read_invalid(read));

                    if invalid {
                        self.invalid_init_locals.borrow_mut().insert(decl);
                    }
                    continue;
                }
                self.init_derived_types.borrow_mut().insert(decl, ty);
            }
        }
        // The pass-1-unsolved closure formals re-derive over the locals'
        // initializer faces just staged.
        let unsolved = std::mem::take(&mut self.deferred_closure_formals);
        self.apply_deferred_closure_formals(unsolved);
        // The fold-deferred rows interleave by offset: their emissions keep
        // the report's source order (later merges assume it).
        // The cascade streak index: every section's cascade-first recv_span
        // in one linear pass (pairwise `;` probes over adjacent sections —
        // the gaps sum to the source size). The per-row walk-back it feeds
        // was O(N²) over a 20k-section cascade (#41983).
        if !self.cascade_sections.is_empty() {
            let mut index = FxHashMap::default();
            index.reserve(self.cascade_sections.len());
            let mut first: Option<SourceRange> = None;
            let mut prev: Option<u32> = None;
            for read in &self.member_reads {
                if !self.cascade_sections.contains(&read.offset) {
                    continue;
                }
                if prev.is_some_and(|offset| self.semicolon_between(offset, read.offset)) {
                    first = None;
                }
                let span = first.unwrap_or(read.recv_span);
                first = Some(span);
                index.insert(read.offset, span);
                prev = Some(read.offset);
            }
            self.cascade_first_spans = index;
        }
        // The row loop runs with the receiver-expression memo armed: every
        // table the walk reads is frozen past this point (the rebind, init
        // and closure-formal passes above were the last writers), so each
        // span's chain answer is derived once.
        self.recv_memo_active = true;
        let mut index = 0usize;
        for row in &rows {
            // The conditional-URI face: rows sourced inside an `if (...)`
            // configuration resolve to nothing (the configuration is never
            // an expression; the parser's invalid_literal_in_configuration
            // span carries them).
            if self.conditional_uri_rows.contains(&row.range.offset) {
                continue;
            }
            while index < deferred.len() && deferred[index].range.offset <= row.range.offset {
                // The conditional-URI face gates the fold-deferred rows the
                // same way (their offsets are inside the window too).
                if !self
                    .conditional_uri_rows
                    .contains(&deferred[index].range.offset)
                {
                    self.resolve_row(promotions, &deferred[index], &cascade_targets);
                }
                index += 1;
            }
            self.resolve_row(promotions, row, &cascade_targets);
        }
        for deferred_row in &deferred[index..] {
            if !self
                .conditional_uri_rows
                .contains(&deferred[index].range.offset)
            {
                self.resolve_row(promotions, deferred_row, &cascade_targets);
            }
        }
        // The `[]` argument's void face: the index expression is the
        // operator application's argument, so a `void`-typed index uses a
        // value (`list[x]`, `list[x] = v`) exactly as any other argument
        // does. The receiver's own void face is the bracket-anchored face
        // beside this one (`_reportUnresolvedIndex`,
        // property_element_resolver.dart); this arm owns the ARGUMENT. It
        // runs over the whole resolved set (both stacks), because the
        // pass-2 walk drains `self.rows` into its own local.
        self.check_index_argument_void(&rows, &deferred);
        self.recv_memo_active = false;
        self.recv_memo.borrow_mut().clear();
    }

    /// One row's pass-2 dispatch (shared by the main stack and the
    /// fold-deferred rows).
    pub(crate) fn resolve_row(
        &mut self,
        promotions: &PromotionStaging,
        row: &Row,
        cascade_targets: &[(u32, Ty)],
    ) {
        // The #2459②③ cascade-index misfold: a CONTINUATION section's
        // `..[` fold binds the previous section's RHS leftover row
        // (`x..[0] = s..[1] = 2` folds the second bracket onto `s`, a
        // `JSObject()..['k'] = v.toJS` chain folds the next bracket onto
        // the `toJS` getter row) — the official walk binds every section's
        // index face to the CASCADE TARGET, never to that row. Answer the
        // target's own `[]`/`[]=` question at the bracket, then resolve the
        // row's pre-fold read face (the oracle still reads every leftover:
        // `s` stays a use, `toJS` resolves through its extension).
        if let Intent::Operator(op, bracket) = row.intent
            && op == self.intern("[]")
            && self.cascade_index_continuation(bracket.offset)
        {
            if let Some(target) = self.cascade_index_target_ty(bracket.offset, cascade_targets) {
                self.check_operator_expr(target, row);
            }
            let plain = Row {
                intent: Intent::Read,
                write_op: None,
                ..row.clone()
            };
            self.resolve_row(promotions, &plain, cascade_targets);
            return;
        }
        match &row.shape {
            RowShape::Bare(info) => self.resolve_bare_row(promotions, row, *info),
            RowShape::Member { recv } => {
                let recv = self.cascade_recv_receiver(row, *recv);
                let recv = self.promoted_receiver(row.range.offset, recv);
                let rebound_face = if matches!(recv, Receiver::Typed { .. })
                    && self
                        .member_reads
                        .binary_search_by(|read| read.offset.cmp(&row.range.offset))
                        .is_ok()
                {
                    self.rebound_local_receiver(row.range.offset, &recv)
                } else {
                    None
                };
                let recv = if let Some(face) = rebound_face
                    && let Ty::Named { name, .. } = face
                {
                    Receiver::Typed {
                        type_name: name,
                        ty: face,
                    }
                } else {
                    recv
                };
                let recv = self.tolerant_receiver(promotions, row, recv);
                // The compound/prefix/postfix operator applied to a MEMBER's
                // own result (#2815 R2 — `b.a.y += 0` over `A.y: int?`): the
                // application reads the member's static type unconditionally
                // and reports the shared unchecked code at the operator token
                // naming the base operator. A nullable RECEIVER keeps the
                // selector-anchored read face's own row (the member sites'
                // gate) — this face fires only when the receiver reads clean
                // and the member's result is the nullable operand.
                if let Some((op_text, op_range, kind, _)) = self.adjacent_operator(row)
                    && matches!(recv, Receiver::Typed { ty, .. } if !self.potentially_nullable(ty))
                    && let Ok(index) = self
                        .member_reads
                        .binary_search_by(|read| read.offset.cmp(&row.range.offset))
                    && let Some(result) = self.chained_read_static_ty(&self.member_reads[index])
                    && self.potentially_nullable(result)
                {
                    self.emit_unchecked_nullable(op_range, op_text, kind);
                }
                self.resolve_member_row(promotions, row, recv);
            }
            RowShape::Receiver(recv) => self.resolve_receiver_row(row, *recv),
            RowShape::TypeUse(..) => self.resolve_type_row(row),
            RowShape::Record(fields) => self.resolve_record_row(row, fields),
            RowShape::Value => {}
        }
    }

    /// The `[]` application's ARGUMENT void face (upstream
    /// `checkForUseOfVoidResult`, error_detection_helpers.dart:245-258):
    /// the index expression is the operator's argument, so its value is
    /// consumed — `list[x]`, `list[x] = v` and `list[x] += v` all report at
    /// the index expression when its static type is `void`. The receiver's
    /// void face is the bracket-anchored twin beside this one
    /// (`_reportUnresolvedIndex`, property_element_resolver.dart:181), and
    /// the two never collide: this walk only reads rows strictly INSIDE the
    /// bracket pair, the receiver row sits wholly before it.
    ///
    /// The argument is located by range, not by an operator mark: the index
    /// expression's row keeps its own intent (`Intent::Invoke` for
    /// `list[f()]`, `Intent::Operator` for `list[a + b]`), and
    /// `mark_operator` tags only `Intent::Read` rows. The same by-range
    /// fallback serves `fold_binary`'s unmarked operand. The OUTERMOST row
    /// inside the pair is the argument's whole expression (the innermost is
    /// its head: for `list[a.b]` that is the receiver `a`), so the walk
    /// keeps the greatest offset.
    fn check_index_argument_void(&mut self, rows: &[Row], deferred: &[Row]) {
        let index_op = self.intern("[]");
        let brackets: Vec<SourceRange> = rows
            .iter()
            .chain(deferred.iter())
            .filter_map(|row| match row.intent {
                Intent::Operator(op, bracket) if op == index_op => Some(bracket),
                _ => None,
            })
            .collect();
        for bracket in brackets {
            let argument = rows
                .iter()
                .chain(deferred.iter())
                .filter(|row| {
                    !matches!(row.shape, RowShape::TypeUse(..))
                        && row.range.offset > bracket.offset
                        && row.range.end() < bracket.end()
                })
                .max_by_key(|row| row.range.offset)
                .cloned();
            let Some(argument) = argument else {
                continue;
            };
            let ty = self.operand_static_ty(&argument);
            if ty != Ty::Void {
                continue;
            }
            // One report per argument: an index expression inside another
            // index expression's argument is nested, and the enclosing
            // bracket never contains it (the containment gate is strict).
            if self
                .report
                .diagnostics
                .iter()
                .any(|d| d.code.name == "use_of_void_result" && d.range == argument.range)
            {
                continue;
            }
            self.emit_use_of_void_result(ty, &argument);
        }
    }

    /// Whether a `;` separates two UTF-16 offsets — the same separator
    /// probe `merge_member`'s cascade window runs: a statement boundary
    /// proves a fresh cascade, while a `;` inside an expression's own spans
    /// (a `"a;b"` literal) is part of it.
    pub(crate) fn semicolon_between(&self, from: u32, to: u32) -> bool {
        let (start, end) = (self.byte_of(from), self.byte_of(to));
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        self.source.as_bytes()[lo..hi].contains(&b';')
    }

    /// Whether the `..[`-preceded bracket at `bracket_offset` belongs to a
    /// CONTINUATION index section. The FIRST section's `..[` fold binds the
    /// cascade target row itself (the walk's newest row before its bracket
    /// is the target), while every LATER section's fold binds the previous
    /// section's RHS leftover row — so the fold's legitimacy is the
    /// section's own position: walking back from the section operator at
    /// bracket depth 0, a preceding `..`/`?..` before any chain boundary
    /// (`;`, `,`, or the target's own opening group) marks a continuation.
    /// `false` also answers for every bracket a cascade section does not
    /// own (`..b[0]` reads the selector spelling).
    fn cascade_index_continuation(&self, bracket_offset: u32) -> bool {
        let mut b = self.byte_of(bracket_offset);
        let src = self.source.as_bytes();
        while b > 0 && src[b - 1].is_ascii_whitespace() {
            b -= 1;
        }
        if b < 2 || src[b - 1] != b'.' || src[b - 2] != b'.' {
            return false;
        }
        b -= 2; // the section's own `..`
        let mut depth = 0i32;
        while b > 0 {
            let c = src[b - 1];
            b -= 1;
            match c {
                b']' | b')' => depth += 1,
                b'[' | b'(' => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                b';' | b',' | b'{' | b'}' if depth == 0 => return false,
                b'.' if depth == 0 => {
                    if b > 0 && src[b - 1] == b'.' {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// The static type of the cascade target an index section resolves
    /// against (#2459②③): the newest selector-section read inside the
    /// `;`-free window carries the latched target receiver
    /// (`C()..foo = 1..[0] = 2` answers on `C`), else the nearest
    /// first-section target fold's push-time type (`x..[0] = 1..[1] = 2`
    /// answers on the target). Every miss keeps the section silent — the
    /// under-report posture for the shapes no pass-2 record describes.
    fn cascade_index_target_ty(
        &self,
        bracket_offset: u32,
        cascade_targets: &[(u32, Ty)],
    ) -> Option<Ty> {
        if let Some(read) = self.member_reads.iter().rev().find(|read| {
            read.offset < bracket_offset
                && self.cascade_sections.contains(&read.offset)
                && !self.semicolon_between(read.offset, bracket_offset)
        }) {
            return cascade_target_ty_of(&read.recv);
        }
        cascade_targets
            .iter()
            .rev()
            .find(|(offset, _)| {
                *offset < bracket_offset && !self.semicolon_between(*offset, bracket_offset)
            })
            .map(|(_, ty)| *ty)
    }

    /// The #2459① cascade-RHS receiver: a member merge inside a cascade
    /// section's RHS or arguments that is NOT a section selector itself
    /// (`..sourceSpan = token.span`'s `span`, `..p = original.p`'s inner
    /// read) resolves through its own receiver upstream, but the merge-time
    /// cascade latch (S4-B) substitutes the latched cascade target verbatim
    /// whenever the popped row is a leftover at section level. The
    /// substitution is detectable here: the recorded receiver equals the
    /// newest section read's receiver exactly, and the row's recorded
    /// receiver span still points at the true receiver expression — the
    /// flow read type at that span re-derives the receiver the same way the
    /// inferred-slot face (#2346①) does. Section selectors keep the latched
    /// target (the S4-B face itself), and every other shape keeps the
    /// merge-time record byte for byte.
    fn cascade_recv_receiver(&self, row: &Row, recv: Receiver) -> Receiver {
        if self.cascade_sections.contains(&row.range.offset) {
            // The section's receiver reads the latched target's PROMOTED
            // face when one stands (`if (v != null) v..m()`: the oracle
            // resolves every section against `cascade.target2`'s
            // flow-promoted type, type_analyzer.dart's `cascadeReceiver`).
            // The promotion face alone substitutes — same head, nullability
            // stripped — never a rename or a widening (a `?..` section's
            // target read stays nullable and keeps the latched face). Only
            // the FIRST section's record carries the target's own span —
            // a later section's recv_span is the previous section's RHS —
            // so the answer reads the precomputed streak index (the old
            // per-row walk-back over the cascade's sections was O(N²) on
            // a 20k-section cascade, #41983).
            let target_span = self.cascade_first_spans.get(&row.range.offset).copied();
            if let Some(span) = target_span
                && let Receiver::Typed {
                    ty:
                        Ty::Named {
                            name: static_name,
                            nullable: true,
                            ..
                        },
                    ..
                } = recv
                && let Some(Ty::Named {
                    name: face_name,
                    nullable: false,
                    ..
                }) = self.recv_expression_ty(span)
                && face_name == static_name
            {
                return Receiver::Typed {
                    type_name: static_name,
                    ty: Ty::Named {
                        name: face_name,
                        nullable: false,
                        args: ArgListId::EMPTY,
                    },
                };
            }
            // A cascade SELECTOR that recorded no receiver (`..foo()`,
            // `..foo = v` — the invocation/assignment fold left the
            // section's receiver `Unknown`) still resolves against the
            // cascade target: the oracle binds every section to
            // `cascade.target`'s static type, so the selector's miss is the
            // target's own member verdict. Inherit the prior section
            // read's recorded receiver (`C()..foo..foo()` — `..foo`'s
            // `Created`/`Typed` face) — the same latch the promoted arm
            // above reads for a nullable target.
            if matches!(recv, Receiver::Unknown) {
                // The receiver the FIRST section recorded is the cascade
                // target — a continuation section's own read latches
                // `Unknown`, so the walk back keeps the newest read that
                // still carries a receiver (the target's `Created`/`Typed`
                // face), bounded at the cascade's own statement.
                let src = self
                    .member_reads
                    .iter()
                    .rev()
                    .find(|read| {
                        read.offset < row.range.offset && read.recv != Receiver::Unknown
                    })
                    .copied();
                if let Some(read) = src
                    && !self.semicolon_between(read.offset, row.range.offset)
                {
                    return read.recv;
                }
            }
            return recv;
        }
        // The newest cascade section strictly below the row: binary-search
        // the offset-sorted records, then walk back over the interleaved
        // non-section reads (an RHS's own reads — a handful). The old
        // reverse scan over the whole vector paid O(N) per row.
        let below = self
            .member_reads
            .partition_point(|read| read.offset < row.range.offset);
        let Some(src) = self.member_reads[..below]
            .iter()
            .rev()
            .find(|read| self.cascade_sections.contains(&read.offset))
            .copied()
        else {
            return recv;
        };
        if self.semicolon_between(src.offset, row.range.offset) || src.recv != recv {
            return recv;
        }
        let Ok(index) = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&row.range.offset))
        else {
            return recv;
        };
        let read_span = self.member_reads[index].recv_span;
        // The receiver expression's own face — the flow read type (with
        // its trailing `!` applied) or the chain link below.
        match self.recv_expression_ty(read_span) {
            Some(
                face @ Ty::Named {
                    name: flow_head, ..
                },
            ) => Receiver::Typed {
                type_name: flow_head,
                ty: face,
            },
            _ => recv,
        }
    }

    /// A surviving typed-keyword receiver (`this + b`, `super[i]`): only
    /// operator intents carry checks — member merges consumed every other
    /// shape at the dot event.
    pub(crate) fn resolve_receiver_row(&mut self, row: &Row, recv: Receiver) {
        let Intent::Operator(_, op_range) = row.intent else {
            return;
        };
        let (read, write) = self.operator_spellings(row);
        match recv {
            Receiver::This { container, kind } => {
                // Inside `extension E on T`/`extension type E(T)`, `this`
                // carries the `on` type's static face — the operator
                // resolves through `T`'s member table, not `E`'s
                // (upstream `extension_member_resolver.dart`: `this[0]`
                // inside `extension on List` resolves `List.operator []`,
                // never an extension member). A class/mixin/enum's `this`
                // reads its own container unchanged.
                let resolved = match self.container_kind_of(container) {
                    Some(ContainerKind::Extension) => self
                        .extension_on_type_args(container)
                        .map_or(container, |(head, _)| head),
                    _ => container,
                };
                // The nullable `on` type's operator question (#2919): the
                // oracle's `isNullable` tail precedes the member lookup, so
                // `this[0]`/`this[0] = v`/`-this` inside `extension on A?`
                // reports the shared unchecked code at the operator before
                // the spelling's definedness is ever asked (`this?[0]` the
                // `?.` latch stands down; a `Missing` on name's
                // `InvalidType` face the helper screens). Each applied
                // spelling is a method invocation on the nullable receiver.
                let nullable = self.nullable_this_receiver(&recv)
                    && !self.null_aware_members.contains(&row.range.offset);
                // An extension applicable to the nullable `this` type
                // resolves the spelling WITHOUT dereferencing it (upstream
                // `type_property_resolver`'s `_lookupExtension` runs ahead of
                // the member consult, and the declaration's receiver type is
                // the written `on` type — `this`, exactly), so a face
                // declaring the spelling stands the row down: `this[0]`
                // inside `extension E on A? { operator [](int i) }` is E's
                // own application, never an `A?` dereference. Every applied
                // spelling must resolve that way — a spelling only the
                // instance face carries still dereferences the nullable
                // receiver.
                let this_ty = self.this_static_ty(container, kind);
                let mut extension_resolves_all = true;
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_members(resolved, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                    if nullable
                        && !matches!(
                            self.extension_application(resolved, this_ty, spelling, false, true),
                            ExtensionApp::Single
                        )
                    {
                        extension_resolves_all = false;
                    }
                }
                if nullable {
                    if !extension_resolves_all {
                        // The receiver's own dereference reports once at the
                        // operator token, under the base spelling the send
                        // applied (`this[0]` and `this[0] = v` both name `[]`
                        // — the oracle's one row per operator access).
                        let applied = read.or(write).map(|s| self.strings.resolve(s));
                        if let Some(text) = applied {
                            self.emit_unchecked_nullable(
                                op_range,
                                text,
                                UncheckedNullableKind::Method,
                            );
                        }
                    }
                    return;
                }
                if missing.is_empty() {
                    return;
                }
                let type_text = self.strings.resolve(resolved).to_string();
                for text in missing {
                    self.emit(
                        SemanticCode::UndefinedOperator,
                        row.range,
                        &text,
                        &type_text,
                    );
                }
            }
            Receiver::Super { container, kind } => {
                // `super` inside an extension/extension-type body resolves
                // against `InvalidType` upstream (`SuperContext.of != valid`
                // — binary_expression_resolver.dart:391-404) — the operator
                // lookup never runs and the dedicated context face reports.
                if matches!(kind, ContainerKind::Extension | ContainerKind::ExtensionType) {
                    return;
                }
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_super(container, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let type_text = self.strings.resolve(container).to_string();
                for text in missing {
                    // The oracle reports `undefinedSuperOperator` at the
                    // operator token (`binary_expression_resolver.dart`
                    // `.at(node.operator)`), never the `super` keyword —
                    // and operators never run the `getInherited` abstract
                    // leg, so the face here stays the single operator row.
                    self.emit(
                        SemanticCode::UndefinedSuperMember(SuperShape::Operator),
                        op_range,
                        &text,
                        &type_text,
                    );
                }
            }
            // A folded creation/extension-override receiver (`C() + 1`,
            // `E(1) * 2`): the operator resolves against the container's
            // member face — extensions report their dedicated operator row.
            Receiver::Created { name, kind, .. } => {
                let Intent::Operator(_, op_range) = row.intent else {
                    return;
                };
                // An extension may own the operator (`JSObject()..['k'] = v`
                // resolves `[]=`/`[]` through the js_interop faces over the
                // extension-type head — the same implicit application the
                // plain member tail runs, operator spellings included).
                let operator_defined = |engine: &mut Self, spelling: StringId| {
                    matches!(engine.walk_members(name, spelling), Walk::Found(_))
                        || matches!(
                            engine.extension_application(
                                name,
                                Ty::named(name),
                                spelling,
                                true,
                                false,
                            ),
                            ExtensionApp::Single | ExtensionApp::UnsafeSingle
                        )
                };
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if !operator_defined(self, spelling) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let host_text = self.strings.resolve(name).to_string();
                for text in missing {
                    let code = if kind == ContainerKind::Extension {
                        SemanticCode::UndefinedExtensionOperator
                    } else {
                        SemanticCode::UndefinedOperator
                    };
                    self.emit(code, op_range, &text, &host_text);
                }
                self.record_miss(name, op_range, true, NameUse::Read);
            }
            // A typed receiver surviving the merges (`#void[0]` — the
            // symbol literal's static face): the operator resolves as a
            // method on the receiver's static type — `needsGetterError`
            // reports `undefined_operator` at the operator token with the
            // receiver's display name (upstream Dart SDK source,
            // pkg/analyzer/lib/src/dart/resolver/
            // binary_expression_resolver.dart:486 and the postfix/prefix
            // twins). The shared operator face owns the verdict — the
            // extension-application probe, the scope-parameter bound, and
            // the `==` Object exclusion ride it.
            Receiver::Typed { ty, .. } => self.check_operator_expr(ty, row),
            // The composite receiver row (`(c ? a : b) + 1`, `(l ?? r) +
            // 1`, `a[i] + 1`, `x.f() + 1`): the re-derived static face
            // answers the operator's definedness exactly like the typed
            // arm — `recv_expression_ty` re-enters the read-fact /
            // index-result / compound chain on the row's own span. The
            // dynamic/unknown faces stay silent inside the shared check;
            // a potentially-nullable face takes the `isNullable` tail's
            // unchecked row instead (the bare operand's own gate), unless
            // an applicable extension owns the spelling — the same
            // application-first ordering the typed arm's
            // [`Self::check_operator_expr`] keeps (upstream
            // `binary_expression_resolver`'s `isNullable` tail runs on the
            // operand's resolved type, and the resolution consults the
            // applicable extensions before any dereference is read).
            Receiver::Unknown => {
                if let Some(ty) = self.recv_expression_ty(row.range) {
                    if self.potentially_nullable(ty)
                        && !self.null_aware_members.contains(&row.range.offset)
                        && !self.operator_face_extension_resolved(ty, row)
                    {
                        let Intent::Operator(op, _) = row.intent else {
                            return;
                        };
                        let op_text = self.strings.resolve(op).to_string();
                        self.emit_unchecked_nullable(
                            op_range,
                            &op_text,
                            UncheckedNullableKind::Operator,
                        );
                    } else {
                        self.check_operator_expr(ty, row);
                    }
                }
            }
            _ => {}
        }
    }

    /// The receiver re-decide a member row resolves against when the keyed
    /// [`Engine::promoted_receiver`] kept the push-time face. The flow's
    /// promoted read-type fact for an assignment-window receiver stages at
    /// the window's own shifted key (`target = target.julianday` — the
    /// staged row sits one byte left of the receiver read, while the
    /// unpromoted declared face holds the read's exact span), so the
    /// re-decide consults both spans under the same rules the keyed path
    /// applies — a named face that differs from the static type, minus the
    /// `?`-widened same-head pre-state — then falls back to the #2241
    /// promotion staging at the receiver read. The shifted window can only
    /// match rows staged at it (reads never overlap, so no other variable's
    /// row answers there); every miss keeps the keyed verdict.
    fn tolerant_receiver(
        &self,
        promotions: &PromotionStaging,
        row: &Row,
        recv: Receiver,
    ) -> Receiver {
        let Receiver::Typed {
            ty: static_ty,
            type_name,
            ..
        } = recv
        else {
            return recv;
        };
        let Ok(index) = self
            .promoted_recv_reads
            .binary_search_by(|(member, _)| member.cmp(&row.range.offset))
        else {
            return recv;
        };
        let read_span = self.promoted_recv_reads[index].1;
        // An index-result latch keys the `]` token, never a variable read:
        // the flow rows near it describe the operands, so only the
        // re-derived face (`rederived_receiver`) answers it.
        if self.index_result_recvs.contains_key(&read_span.offset) {
            return recv;
        }
        // A re-bound local's face is authoritative (#2539): the flow read
        // face still spells the parse-time bind — the staging predates the
        // pass-2 promotion re-decision — so the re-decision below would
        // clobber the promoted accessor face with the stale declared one.
        if let Some(face) = self.rebound_local_ty(row.range.offset) {
            if face == static_ty {
                return recv;
            }
            if let Ty::Named { name, .. } = face {
                return Receiver::Typed {
                    type_name: name,
                    ty: face,
                };
            }
        }
        if let Some(redecided) = self.redecided_face(static_ty, self.read_type_at(read_span)) {
            return redecided;
        }
        // The assignment window's shifted staging key (`target` reads at
        // `offset`, its promoted face stages at `offset - 1`).
        if read_span.offset > 0 {
            let shifted = SourceRange {
                offset: read_span.offset - 1,
                length: read_span.length,
            };
            if let Some(redecided) = self.redecided_face(static_ty, self.read_type_at(shifted)) {
                return redecided;
            }
        }
        // The #2241 staging: an opaque marker carries only the nullability
        // decision (the parse-time type's non-null form stands), a spelled
        // face re-decides under the same widening guard.
        let Some(promoted) = promotions.promoted(read_span) else {
            return recv;
        };
        let redecided = match promoted {
            Ty::Unknown => {
                let non_null = Self::non_null_form(static_ty);
                (non_null != static_ty).then(|| Receiver::Typed {
                    type_name,
                    ty: non_null,
                })
            }
            Ty::Named { name, .. }
                if promoted != static_ty
                    && !self.widened_face(&promoted, &static_ty)
                    && !self.flow_leaf_of(type_name, name) =>
            {
                Some(Receiver::Typed {
                    type_name: name,
                    ty: promoted,
                })
            }
            _ => None,
        };
        redecided.unwrap_or(recv)
    }

    /// The re-decide verdict of a flow read-type face: a named face that
    /// differs from the push-time static type re-decides the receiver,
    /// unless it merely `?`-widens the same head (the postfix-`!` pre-state
    /// — promotion never widens nullability).
    fn redecided_face(&self, static_ty: Ty, flow_ty: Option<Ty>) -> Option<Receiver> {
        let flow_ty = flow_ty?;
        let Ty::Named { name, .. } = flow_ty else {
            return None;
        };
        if flow_ty == static_ty || self.widened_face(&flow_ty, &static_ty) {
            return None;
        }
        if let Ty::Named {
            name: static_head, ..
        } = static_ty
            && self.flow_leaf_of(static_head, name)
        {
            return None;
        }
        Some(Receiver::Typed {
            type_name: name,
            ty: flow_ty,
        })
    }

    /// Whether `flow` is exactly `static`'s own head widened to nullable —
    /// the postfix-`!` pre-state signature, never a promotion.
    fn widened_face(&self, flow: &Ty, static_ty: &Ty) -> bool {
        matches!(
            (flow, static_ty),
            (
                Ty::Named {
                    name: flow_head,
                    nullable: true,
                    ..
                },
                Ty::Named {
                    name: static_head,
                    nullable: false,
                    ..
                },
            ) if flow_head == static_head
        )
    }

    /// One row's pass-2 dispatch (shared by the main stack and the
    /// fold-deferred rows). Bare rows carry the promotions too: their
    /// operator face re-decides against the flow-promoted type exactly
    /// like the member walk's receiver verdict.
    pub(crate) fn resolve_bare_row(
        &mut self,
        promotions: &PromotionStaging,
        row: &Row,
        info: BareInfo,
    ) {
        let name_text = self.strings.resolve(row.name);
        match info {
            // A prefix used bare: invoking a prefix is an error the oracle
            // names with an unfrozen code — record only. The bare prefix
            // reference still counts as a use of the prefix (`unused_import`
            // hasPrefixUsedWithoutName face).
            BareInfo::Container { .. } | BareInfo::Prefix { .. } => {
                if !self.self_scope_rows.contains(&row.range.offset) {
                    self.used_names.insert(row.name);
                }
                // The type-literal operator face: the operator applies to
                // the literal's static type `Type`, which declares none of
                // the user operators (`List[null]` reports
                // `undefined_operator '[]'` on `Type` — issue_45251_list;
                // upstream Dart SDK source, pkg/analyzer/lib/src/dart/
                // ast/ast.dart `TypeLiteral.staticType`). Anchored on the
                // operator's own span (the bracket pair for index shapes).
                if matches!(info, BareInfo::Container { .. })
                    && let Intent::Operator(_, op_range) = row.intent
                {
                    let (read, write) = self.operator_spellings(row);
                    for spelling in [read, write].into_iter().flatten() {
                        // The `Type` instance's operator face: the `Object`
                        // members plus the extensions of the whole import
                        // closure ([`Self::type_literal_face_resolves`]).
                        // Probing the diagnosing unit's own faces alone
                        // reported `undefined_operator` for a literal whose
                        // operator an IMPORTED extension declares.
                        if self.type_literal_face_resolves(spelling) {
                            continue;
                        }
                        let op_text = self.strings.resolve(spelling).to_string();
                        self.emit(SemanticCode::UndefinedOperator, op_range, &op_text, "Type");
                    }
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                }
            }
            BareInfo::Plain { ty } => {
                // Any resolved reference uses the name it resolved to (the
                // official `_useIdentifierElement` / `members` faces; writes
                // included — only the local-variable rows distinguish). The
                // parse-time self-reference exclusions skip their rows.
                if !self.self_scope_rows.contains(&row.range.offset) {
                    self.used_names.insert(row.name);
                }
                // The assignment-to-* family's local arm (#3282): a write to
                // a `final` local or formal reports `assignment_to_final_local`
                // (`checkFinalAlreadyAssigned`), and the for-in loop target
                // (`for (i in xs)`) is that family's write even though the
                // row model carries it as a read. The increment applications
                // (`x++`, `--x`) assign their operand the same way. The
                // local-hit gate keeps the token probes off every non-local
                // row.
                if self.local_decl_at_read.contains_key(&row.range.offset) {
                    let for_each = self.for_each_target(row.range);
                    if matches!(row.intent, Intent::Write | Intent::ReadWrite)
                        || for_each
                        || self.increment_operand(row)
                    {
                        self.report_assignment_to_final_local(row, for_each);
                    }
                }
                // The nullable implicit-`this` receiver (#2926 F2): a bare
                // send inside `extension E on A?` that the push-time latch
                // marked as resolving through the `on` face answers the
                // nullable-receiver question BEFORE its member verdict —
                // `ThisLookup` resolves the name against `this` (`A?`), and
                // `type_property_resolver`'s `isNullable` tail reports the
                // unchecked access at the name. The variant follows the
                // parent node's shape (type_property_resolver.dart:128-148):
                // an invocation, a write, or an index application lands on
                // a `MethodReferenceExpression` parent (`foo()`, `foo = 0`,
                // `foo[i]` — AssignmentExpression, PostfixExpression, and
                // IndexExpression all implement it) and spells the method
                // text; a `BinaryExpression` parent spells the operator
                // text naming the PROPERTY (`foo + 1` reports the operator
                // 'foo'); every other shape (the bare read `foo;`) spells
                // the property text. The member bookkeeping below still
                // runs — the send resolved, only the receiver question
                // precedes it.
                if self.ext_nullable_member_rows.contains(&row.range.offset) {
                    let kind = match &row.intent {
                        Intent::Operator(op, _) if *op != self.intern("[]") => {
                            UncheckedNullableKind::Operator
                        }
                        Intent::Read => UncheckedNullableKind::Property,
                        _ => UncheckedNullableKind::Method,
                    };
                    self.emit_unchecked_nullable(row.range, name_text, kind);
                }
                // The implicit-this member edge (#2618): the push-time
                // resolution published this row's target (`RefTarget::Member`
                // names the enclosing container — its walk ran at push time,
                // and the container stack is gone by pass 2), while the
                // FINAL intent — refined by the send/assignment events after
                // push time — decides the kind here. The re-walk reads the
                // pass's face memo (one hash probe); a write resolves its
                // setter side (`name=` spelling first, then a field's
                // implicit setter — a getter-only hit is the erroneous-write
                // face and resolves no element).
                if let Ok(at) = self
                    .refs
                    .binary_search_by(|published| published.range.offset.cmp(&row.range.offset))
                    && self.refs[at].range == row.range
                    && let RefTarget::Member { container } = self.refs[at].target
                {
                    let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
                    // A wildcard `_` never enters the lexical scope
                    // (upstream `WildcardScope` — the wildcard-variables
                    // experiment binds `_` nowhere): the member walk's hit
                    // is an artifact of the name-level climb, not a
                    // resolution — the bare spelling reports
                    // `undefined_identifier` (probed 3.13.7: `var a = _;`
                    // inside `m()` over a `final int _` field reports the
                    // name, `this._` still resolves). A `_` inside a
                    // constructor-initializer's RHS resolves to the field
                    // through the implicit-`this` face instead — the
                    // initializer's `_checkForImplicitThisReferenceIn
                    // Initializer` consults the member face directly, so
                    // the row keeps its member target (the
                    // `init_this_member` arm below reports it).
                    if self.is_wildcard(row.name) && !self.init_rows.contains_key(&row.range.offset)
                    {
                        self.emit(
                            SemanticCode::UndefinedIdentifier,
                            row.range,
                            name_text,
                            "",
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                    } else {
                    if write && self.init_rows.contains_key(&row.range.offset) {
                        // A bare name written inside a constructor-initializer
                        // list is the field-initializer LHS (`x = e`) — it
                        // resolves through the dedicated initializer world
                        // (`_checkForInvalidField`), never the implicit-`this`
                        // member write face below (a `final` field there is
                        // the legal initializing write, not
                        // `assignment_to_final`).
                        self.check_initializer_member_write(row, container);
                    } else if write {
                        let setter = self.setter_spelling(row.name);
                        if self.member_found(container, setter, false) {
                            let landing = self.last_landing();
                            let kind = Self::member_ref_kind(&row.intent, MemberKind::Setter);
                            self.note_member_ref(
                                row,
                                kind,
                                landing.map(|(file, origin)| (file, setter, origin)),
                            );
                        } else if let Walk::Found(entry) = self.walk_members(container, row.name)
                            && entry.kind == MemberKind::Field
                        {
                            let landing = self.last_landing();
                            let kind = Self::member_ref_kind(&row.intent, entry.kind);
                            self.note_member_ref(
                                row,
                                kind,
                                landing.map(|(file, origin)| (file, row.name, origin)),
                            );
                        }
                        // The implicit-`this` setter-miss face (#3282): a
                        // `final` field or an explicit getter of the
                        // enclosing container reached by the bare spelling.
                        self.report_setter_less_implicit_write(row, container);
                    } else if self.increment_operand(row) {
                        // The increment twin (`x++` inside the container):
                        // the operand assigns the implicit-`this` member.
                        self.report_setter_less_implicit_write(row, container);
                    } else if let Walk::Found(entry) = self.walk_members(container, row.name) {
                        let landing = self.last_landing();
                        let kind = Self::member_ref_kind(&row.intent, entry.kind);
                        self.note_member_ref(
                            row,
                            kind,
                            landing.map(|(file, origin)| (file, row.name, origin)),
                        );
                    }
                    }
                }
                // The implicit-this face (`_checkForInvalidInstanceMember
                // Access2`, upstream Dart SDK source, pkg/analyzer/lib/src/
                // generated/error_verifier.dart — `ThisContext.
                // instanceFieldDeclaration` disallows `this`): a bare READ
                // in an instance field initializer resolving to an instance
                // member of the enclosing container reports at the name.
                // Both facts snapshot at push time (the field group and the
                // container stack tear down before pass 2).
                //
                // The constructor-initializer RHS reads the same face
                // (#3400): `x = g` inside `: ...` reports `implicit_this_
                // reference_in_initializer` at `g` beside the
                // potentially-constant walk's `invalid_constant` (probed
                // 3.13.7 — `f = m` over an instance method reports both
                // codes at the name). `init_rows` carries every bare name
                // the initializer list pushed; the member question walks
                // the recorded container — a hit on a non-static member is
                // the `this` read, everything else keeps the ordinary
                // resolution (a formal, a static, a miss).
                // A name that spells a (non-wildcard) formal of the
                // enclosing constructor binds the formal, not the field —
                // `Tag2([String? name]) : name = name` reads the formal
                // and stays silent (the wildcard `this._` formal declares
                // no readable `_`, so it never suppresses the field read
                // below). The name-level union across the container's
                // constructor tables keeps the under-report direction on
                // the rare two-constructors-share-a-spelling shape.
                let init_rows_formal = |container: StringId| -> bool {
                    let Some(file) = self.index.get(self.file) else {
                        return false;
                    };
                    let Some(c) = file.container(container) else {
                        return false;
                    };
                    !self.is_wildcard(row.name)
                        && c.ctor_formals.values().any(|formals| {
                            formals
                                .positional
                                .iter()
                                .chain(formals.named.iter())
                                .any(|formal| formal.name == row.name)
                        })
                };
                let init_this_member = !matches!(row.intent, Intent::Write | Intent::ReadWrite)
                    && !self.field_init_this_rows.contains(&row.range.offset)
                    && self
                        .init_rows
                        .get(&row.range.offset)
                        .is_some_and(|container| {
                            !init_rows_formal(*container)
                                && matches!(
                                    self.walk_members(*container, row.name),
                                    Walk::Found(entry) if !entry.is_static
                                )
                        });
                if !matches!(row.intent, Intent::Write | Intent::ReadWrite)
                    && (self.field_init_this_rows.contains(&row.range.offset)
                        || init_this_member)
                {
                    self.emit(
                        SemanticCode::ImplicitThisReferenceInInitializer,
                        row.range,
                        name_text,
                        "",
                    );
                }
                // The discarded-value face (`_check` + `checkNameExpression`,
                // use_result_verifier.dart): an invocation or bare read of a
                // UseResult-carrying top-level declaration, not sealed used.
                // A local, parameter, or member shadowing the name owns the
                // reference — the callee gates that on the row's published
                // resolution, the one authority for which element the name
                // bound.
                if matches!(row.intent, Intent::Invoke | Intent::Read)
                    && !self.self_scope_rows.contains(&row.range.offset)
                {
                    self.check_top_level_unused_result(row);
                }
                // A non-write resolution of a field name is the implicit
                // getter's read (writes never spare a field) — unless the
                // statement window discarded the read (`_isReadIdentifier`'s
                // false arm, upstream Dart SDK source,
                // pkg/analyzer/lib/src/error/unused_local_elements_verifier
                // .dart:418-440: a bare compound-assignment LHS, prefix, or
                // postfix operand whose parent is the ExpressionStatement
                // joins no read set, so `_x += 1;`/`_x++;`/`-_x;` leave the
                // field unused while `??=` and member targets keep theirs).
                // Inside an extension body `this` reads the `on` type's
                // member — the owner credits the extended container, not
                // the extension (the same face `note_member_read`'s `This`
                // arm carries).
                if !matches!(row.intent, Intent::Write)
                    && !self.statement_discarded_rows.contains(&row.range.offset)
                {
                    if let Some(&pushed) = self.field_read_rows.get(&row.range.offset) {
                        self.read_names.insert(row.name);
                        let owner = self
                            .container_context()
                            .and_then(|(container, kind)| {
                                (kind == ContainerKind::Extension)
                                    .then(|| self.enclosing_extension_on_type(container))
                                    .flatten()
                            })
                            .or_else(|| self.read_owner_container(row.range.offset))
                            .or(Some(pushed));
                        if let Some(container) = owner {
                            self.member_read_owners
                                .entry(row.name)
                                .or_default()
                                .insert((self.file, container));
                        }
                    } else if let Some(on) = self
                        .container_context()
                        .filter(|&(_, kind)| kind == ContainerKind::Extension)
                        .map(|(container, _)| container)
                        .and_then(|container| self.enclosing_extension_on_type(container))
                    {
                        // The extension body's bare read of an `on`-type
                        // field never lands in `field_read_rows` (the
                        // resolution join keys only the extension's own
                        // fields), yet it is the implicit getter's read the
                        // unused-field face credits — join the extended
                        // container here (`this` inside an extension IS the
                        // `on` type).
                        if let Walk::Found(entry) = self.walk_members(on, row.name)
                            && entry.kind == MemberKind::Field
                            && !entry.is_static
                        {
                            self.read_names.insert(row.name);
                            self.member_read_owners
                                .entry(row.name)
                                .or_default()
                                .insert((self.file, on));
                        }
                    }
                }
                // The receiver faces that read the row's flow-corrected
                // static type (#2815): the direct operator application on a
                // declared-typed receiver (`c + d`, `c[i] = v`), the bare
                // invocation's callee (`f1()` over a `Function?` local), and
                // the compound/prefix/postfix applications the row model
                // stages no marker for (`x += 1`, `-x`, `x++` — read through
                // the token table's adjacency). The flow promotion's face
                // re-decides first (#2241's join, the same face the member
                // walk's receiver verdict reads): `if (sub is List) sub[i]`
                // resolves `[]` against the promoted `List`, not the
                // declared `Object?`. An opaque staged marker carries only
                // the nullability decision (#2174) — the parse-time type's
                // non-null form stands.
                //
                // The promotion-latched initializer's re-bind (#2539): an
                // implicitly typed local whose initializer's member row
                // re-derived under a promoted receiver (`final v = t.value`
                // past `t is Sub` — the base getter's declared `Object` at
                // parse time, the override's `BigInt` after the re-decision)
                // carries the corrected face only in the re-bind map. The
                // verdicts below read the same corrected static type the
                // receiver faces read (`rebound_read_face` outranks the
                // parse bind; a live promotion at the read still outranks
                // both).
                let adjacent = self.adjacent_operator(row);
                if matches!(row.intent, Intent::Operator(..) | Intent::Invoke) || adjacent.is_some()
                {
                    let ty = self.rebound_read_face(row.range).unwrap_or(ty);
                    let ty = match promotions.promoted(row.range) {
                        Some(Ty::Unknown) => Self::non_null_form(ty),
                        Some(promoted) => promoted,
                        None => ty,
                    };
                    // A trailing null-assert (`xs![index]`, `f!()`) promotes
                    // the face the same way the chained receiver walk's own
                    // `!` probe does (`null_asserted_after`).
                    let ty = self.null_asserted_after(row.range, ty);
                    // A resolved function-DECLARATION identifier carries
                    // its signature face (`f + 1` over `int f(int)` reads
                    // the tear-off `int Function(int)` —
                    // binary_expression_resolver.dart:425-478's operand
                    // static type; `f += 1` the compound twin). The
                    // `BareInfo` payload staged no face for the
                    // declaration (a bare function name is a tear-off,
                    // not a call), so the declaration-shape leg that
                    // answers the chained-receiver family answers this
                    // one too. The bare INVOCATION's callee keeps its own
                    // gate (`f()` resolves through the send path).
                    //
                    // The parenthesized-group leg is the same question for
                    // a grouped operand (`(a ?? b) + 1`): the group row's
                    // face is the inner expression's own static type.
                    let ty = if ty == Ty::UNKNOWN
                        && (matches!(row.intent, Intent::Operator(..)) || adjacent.is_some())
                    {
                        self.receiver_decl_shape(row.range)
                            .or_else(|| self.group_expression_face(row.range))
                            .unwrap_or(ty)
                    } else {
                        ty
                    };
                    if let Intent::Operator(_, op_range) = row.intent {
                        // The nullable-dereference face on the operator's own
                        // receiver (#2815 R2 — binary_expression_resolver's
                        // `isNullable` tail and the compound/index/unary
                        // twins): a potentially-nullable operand reports the
                        // shared unchecked code at the operator token and
                        // never reaches the undefined-operator family. An
                        // extension whose `on` type covers the NULLABLE
                        // spelling resolves the application (the `Single`
                        // verdict — the same suppression the member sites
                        // take); `UnsafeSingle` keeps the report.
                        if self.potentially_nullable(ty)
                            && !self.null_aware_members.contains(&row.range.offset)
                            // A `?[` index shorts the same way a `?.` member
                            // access does: the bracket's null-aware merge
                            // marks the folded value row, not this receiver
                            // row — the `?` itself is the probe.
                            && self.byte_at(self.skip_whitespace(row.range.end())) != Some(b'?')
                            && !self.operator_face_extension_resolved(ty, row)
                        {
                            let Intent::Operator(op, _) = row.intent else {
                                return;
                            };
                            let op_text = self.strings.resolve(op).to_string();
                            // The index family takes the method text with
                            // the `[]` spelling (the oracle's exact variant)
                            // anchored at the OPENING bracket — the row's
                            // bracket span covers the whole group; a binary
                            // application reports the operator text
                            // (messages.yaml's
                            // `uncheckedOperatorInvocationOfNullableValue` —
                            // the BinaryExpression arm of the node-shape
                            // dispatch).
                            let is_index = op == self.intern("[]");
                            let anchor = if is_index {
                                SourceRange {
                                    offset: op_range.offset,
                                    length: 1,
                                }
                            } else {
                                op_range
                            };
                            let kind = if is_index {
                                UncheckedNullableKind::Method
                            } else {
                                UncheckedNullableKind::Operator
                            };
                            self.emit_unchecked_nullable(anchor, &op_text, kind);
                        } else if ty == Ty::UNKNOWN && self.sdk_collection_type_literal(row.name)
                            // The type-literal receiver face (`typedef c =
                            // String; c > x`, `A < 1`): a bare name resolving
                            // to a TYPE evaluates to the type object — its
                            // static type is `Type`, which declares none of
                            // the user operators. The alias-fold equality
                            // guards the shadowed face (a local whose type
                            // folds from the same name never answers here);
                            // the SDK-collection stub arm above keeps its
                            // names-only posture.
                            || (self.named_type_is_type(row.name)
                                && ty == self.fold_alias_ty(Ty::named(row.name)))
                        {
                            let (read, write) = self.operator_spellings(row);
                            for spelling in [read, write].into_iter().flatten() {
                                // The `Type` instance's operator face, over
                                // the whole import closure's extensions —
                                // [`Self::type_literal_face_resolves`].
                                if self.type_literal_face_resolves(spelling) {
                                    continue;
                                }
                                let op_text = self.strings.resolve(spelling).to_string();
                                self.emit(
                                    SemanticCode::UndefinedOperator,
                                    op_range,
                                    &op_text,
                                    "Type",
                                );
                            }
                        } else {
                            // A bare name resolving to an enclosing
                            // container's member (`field = T` reads `a` of
                            // `class C<T extends num> { T a; m() { a += 1 } }`)
                            // carries the declaring container — the bound
                            // consult needs it to find `T`'s declared bound.
                            let member_container = self
                                .refs
                                .binary_search_by(|published| {
                                    published.range.offset.cmp(&row.range.offset)
                                })
                                .ok()
                                .filter(|&at| self.refs[at].range == row.range)
                                .and_then(|at| match self.refs[at].target {
                                    RefTarget::Member { container } => Some(container),
                                    _ => None,
                                });
                            self.check_operator_expr_in(ty, row, member_container);
                        }
                    } else if let Some((op_text, op_range, kind, compound)) = adjacent {
                        // The compound/prefix/postfix face (#2815 R2 —
                        // assignment_expression_resolver.dart:232-278, the
                        // increment twins): the application reads its operand
                        // unconditionally, so a potentially-nullable operand
                        // reports the shared unchecked code at the operator
                        // token naming the BASE operator (`x += 1` reports
                        // the method '+', `x++` the method '+').
                        if matches!(ty, Ty::Null) {
                            // The `NullType` dereference swaps the code to
                            // `invalid_use_of_null_value` at the operand
                            // (nullable_dereference_verifier.dart:47-49) —
                            // `n++`, `n += 1`, `-n` all report there.
                            self.emit(SemanticCode::InvalidUseOfNullValue, row.range, "", "");
                        } else if matches!(ty, Ty::Void) {
                            // `void` is potentially nullable upstream: the
                            // increment and unary twins report the shared
                            // unchecked code at the operator. A compound
                            // `v op= x` reports `use_of_void_result` alone
                            // (the assignment resolver's read consult never
                            // reaches the nullable tail for a void write
                            // target) — that family owns the row.
                            if !compound {
                                self.emit_unchecked_nullable(op_range, op_text, kind);
                            }
                        } else if self.potentially_nullable(ty)
                            && !self.null_aware_members.contains(&row.range.offset)
                            && !self.operator_face_extension_resolved(ty, row)
                        {
                            self.emit_unchecked_nullable(op_range, op_text, kind);
                        } else {
                            // The same member-container hint the direct
                            // operator arm derives (`a += 1` on a `T a`
                            // field consults `C`'s captured bound for `T`).
                            let member_container = self
                                .refs
                                .binary_search_by(|published| {
                                    published.range.offset.cmp(&row.range.offset)
                                })
                                .ok()
                                .filter(|&at| self.refs[at].range == row.range)
                                .and_then(|at| match self.refs[at].target {
                                    RefTarget::Member { container } => Some(container),
                                    _ => None,
                                });
                            match self.adjacent_operator_defined_in(ty, op_text, member_container) {
                                AdjacentOp::Defined => {}
                                // The ambiguous application replaces the
                                // operator miss (the oracle reports the
                                // ambiguity at the operator token, never
                                // `undefined_operator`).
                                AdjacentOp::Ambiguous(displays) => {
                                    let member = self.intern(op_text);
                                    self.emit_extension_ambiguity(member, op_range, displays);
                                }
                                AdjacentOp::Missing => {
                                    // The undefined-operator face (the
                                    // increment/compound consults —
                                    // postfix_expression_resolver.dart:159
                                    // and the assignment resolver's
                                    // operator twin): the operand's static
                                    // type answers the base operator's
                                    // definedness, a miss reports at the
                                    // operator token with the operand's
                                    // display name (dynamic/unknown
                                    // dispatch dynamically — silent).
                                    let type_text = match self.fold_alias_ty(ty) {
                                        face @ (Ty::Record { .. } | Ty::Function { .. }) => Some(
                                            face.render(&self.strings, &self.boundary.pool()),
                                        ),
                                        face @ Ty::Named { .. } => Some(self.ty_display(face)),
                                        _ => None,
                                    };
                                    if let Some(type_text) = type_text {
                                        self.emit(
                                            SemanticCode::UndefinedOperator,
                                            op_range,
                                            op_text,
                                            &type_text,
                                        );
                                    }
                                }
                            }
                        }
                    } else if row.intent == Intent::Invoke
                        && (self.local_decl_at_read.contains_key(&row.range.offset)
                            || self.read_offset_is_member(row.range.offset))
                        && self.potentially_nullable(ty)
                        && self.invoke_callee_function_shaped(ty)
                    {
                        // The bare invocation's callee gate (#2815 R2a —
                        // function_expression_invocation_resolver.dart:
                        // 53-70): a callee whose resolved type IS a function
                        // type (`Function?`, a nullable function shape, an
                        // alias folding to one, a parameter spelling through
                        // a function bound, `Never?` — question-suffixed
                        // Never, pinned by the oracle corpus) reports the
                        // shared unchecked code at the callee expression.
                        // Only variable reads fire (a local, a parameter, an
                        // implicit-this member) — a top-level function's own
                        // invocation resolves. The callee expression sits
                        // under a `FunctionExpressionInvocation`, so the
                        // oracle spells the function variant
                        // (messages.yaml's
                        // `uncheckedInvocationOfNullableValue`).
                        self.emit_unchecked_nullable(
                            row.range,
                            name_text,
                            UncheckedNullableKind::Function,
                        );
                    }
                }
            }
            BareInfo::Missing {
                this_type,
                in_exec,
                on_opaque,
                ext_invoke,
                ext_nullable_this,
            } => {
                // Mixin `on` constraints and extension `on` types are
                // invisible to the model: inside their instance members a
                // bare miss may be a constraint member — record only.
                if on_opaque {
                    self.record_miss(row.name, row.range, false, use_kind_of(&row.intent));
                    return;
                }
                // The nullable implicit `this` (`extension on C?`): the
                // `ThisLookup` arm reports the unchecked access at the name
                // first; the lexical miss below still reports for a read
                // (`undefined_identifier`), an invocation's own miss is
                // silent (the nullable `ext_invoke` host). A write that
                // resolved the extension's OWN member map (`bar = 0` for
                // `set bar` on `E`) never ran the `this` lookup — the
                // lexical `ExtensionMemberScope` answered first — so the
                // unchecked access stands down entirely; the push-time
                // write latch carries that verdict (`ext_write_target_
                // rows` without `ext_nullable_member_rows`). The variant
                // follows the parent node's shape
                // (type_property_resolver.dart:128-148): an invocation,
                // a write, or an index application lands on a
                // `MethodReferenceExpression` implementor
                // (MethodInvocation / AssignmentExpression /
                // IndexExpression / PostfixExpression / PrefixExpression)
                // and spells the method text; a `BinaryExpression`
                // operand (`gone + 1`) spells the operator text naming
                // the PROPERTY; the bare read spells the property text.
                let own_member_write = matches!(row.intent, Intent::Write | Intent::ReadWrite)
                    && self.ext_write_target_rows.contains(&row.range.offset)
                    && !self.ext_nullable_member_rows.contains(&row.range.offset);
                if ext_nullable_this
                    && !own_member_write
                    && !Self::synthetic_reference(row.range)
                    && !self.keyword_bare_reference(row.range)
                {
                    let kind = match &row.intent {
                        Intent::Invoke | Intent::Write | Intent::ReadWrite => {
                            UncheckedNullableKind::Method
                        }
                        Intent::Operator(op, _) if *op != self.intern("[]") => {
                            UncheckedNullableKind::Operator
                        }
                        Intent::Operator(..) => UncheckedNullableKind::Method,
                        _ => access_kind(&row.intent),
                    };
                    self.emit_unchecked_nullable(row.range, name_text, kind);
                }
                match row.intent {
                    Intent::Invoke => {
                        // The extension face decides first (its slot is
                        // `Some` only inside an `extension` body): a
                        // resolved `on` type's `this` hosts the miss
                        // (`undefined_method` for 'C' of `on C`, '<unknown>'
                        // of `on (int, int)`), an `InvalidType`/nullable
                        // `this` reports nothing (upstream
                        // `unpromotedThisType` — the oracle's lookup dies on
                        // the invalid extended type).
                        if let Some(host) = ext_invoke {
                            if let Some(container) = host {
                                let type_text = self.strings.resolve(container).to_string();
                                if !Self::synthetic_reference(row.range)
                                    && !self.keyword_bare_reference(row.range)
                                {
                                    self.emit(
                                        SemanticCode::UndefinedMethod,
                                        row.range,
                                        name_text,
                                        &type_text,
                                    );
                                }
                            }
                            self.record_miss(row.name, row.range, false, NameUse::Invoke);
                            return;
                        }
                        // The implicit-`this` extension application: an
                        // unqualified invocation inside an instance member
                        // may resolve through an extension over `this`'s
                        // type (`extension on M` applied inside a class
                        // implementing `M` — the same implicit application
                        // a qualified send reads; the application credits
                        // the member and import the same way). The
                        // enclosing class's OWN members claim the name
                        // first (static-extensions uniqueness — the
                        // oracle's `extension_member_resolver` only runs
                        // when the interface lookup found nothing): a
                        // `static set foo` on the class blocks every
                        // application of `foo`, leaving the row the
                        // class's own `undefined_method` face
                        // (UndefinedMethodTest
                        // test_extensionMethodHiddenByStaticSetter).
                        if let Some(container) = this_type
                            && !self.container_declares_member(container, row.name)
                            && matches!(
                                self.extension_application(
                                    container,
                                    Ty::named(container),
                                    row.name,
                                    false,
                                    false,
                                ),
                                ExtensionApp::Single | ExtensionApp::UnsafeSingle
                            )
                        {
                            // The application resolved the bare invocation
                            // (#2618): the edge targets the winning
                            // extension's member.
                            self.note_extension_ref(row);
                            self.record_miss(row.name, row.range, false, NameUse::Invoke);
                            return;
                        }
                        // UnqualifiedLookupDomain: inside a class or enum
                        // instance member the invocation miss names `this`'s
                        // type (`undefined_method`); everywhere else it is
                        // `undefined_function`.
                        if let Some(container) = this_type {
                            let type_text = self.strings.resolve(container).to_string();
                            // `_reportUndefinedMethodOrNew` is guard-free in
                            // the observed oracle (analyzer 8.1.1
                            // method_invocation_resolver.dart:424-431): a
                            // `show`-listed name of a broken import does NOT
                            // suppress `undefined_method` — only
                            // `undefined_function`/`undefined_identifier`
                            // consult `shouldIgnoreUndefined`.
                            if !Self::synthetic_reference(row.range)
                                && !self.keyword_bare_reference(row.range)
                            {
                                self.emit(
                                    SemanticCode::UndefinedMethod,
                                    row.range,
                                    name_text,
                                    &type_text,
                                );
                            }
                        } else if !Self::synthetic_reference(row.range)
                            && !self.keyword_bare_reference(row.range)
                            && !self.guard.ignore(None, row.name)
                        {
                            self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        }
                        self.record_miss(row.name, row.range, false, NameUse::Invoke);
                    }
                    Intent::Read if row.name == self.intern("await") && in_exec => {
                        // `await` in a sync body is never a lookup miss: the
                        // frozen code names the missing async marker. The
                        // oracle's arm fires before the guard is consulted
                        // (simple_identifier_resolver.dart 8.1.1:217-221 —
                        // `undefined_identifier_await` reports unconditionally,
                        // the `shouldIgnoreUndefinedIdentifier` check rides
                        // the `else if` below it).
                        self.emit(SemanticCode::UndefinedIdentifierAwait, row.range, "", "");
                        self.record_miss(row.name, row.range, false, NameUse::Read);
                    }
                    Intent::Read if self.method_type_arguments_after(row.range) => {
                        // A generic instantiation head (`foo<int>` — the
                        // oracle's `ReadSyntax.typeInstantiation`): the
                        // unqualified lookup fails its `this` face like the
                        // invocation arm — `undefined_method` naming `this`'s
                        // type where one exists (an extension body answers
                        // its `on` type), `undefined_identifier` where none
                        // does (lookup_failure_reporter.dart's
                        // `UnqualifiedLookupDomain._readFailure`
                        // typeInstantiation arm). An extension whose `on`
                        // head is `InvalidType` reports nothing — the
                        // `ext_invoke` slot's `None` host, mirroring the
                        // invocation arm's silent posture.
                        if let Some(host) = ext_invoke {
                            if let Some(container) = host {
                                let type_text = self.strings.resolve(container).to_string();
                                if !Self::synthetic_reference(row.range)
                                    && !self.keyword_bare_reference(row.range)
                                {
                                    self.emit(
                                        SemanticCode::UndefinedMethod,
                                        row.range,
                                        name_text,
                                        &type_text,
                                    );
                                }
                            }
                            self.record_miss(row.name, row.range, false, NameUse::Read);
                            return;
                        }
                        if let Some(container) = this_type {
                            let type_text = self.strings.resolve(container).to_string();
                            if !Self::synthetic_reference(row.range)
                                && !self.keyword_bare_reference(row.range)
                            {
                                self.emit(
                                    SemanticCode::UndefinedMethod,
                                    row.range,
                                    name_text,
                                    &type_text,
                                );
                            }
                        } else if !Self::synthetic_reference(row.range)
                            && !self.keyword_bare_reference(row.range)
                            && !self.guard.ignore(None, row.name)
                        {
                            self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                        }
                        self.record_miss(row.name, row.range, false, NameUse::Read);
                    }
                    Intent::Read | Intent::Write | Intent::ReadWrite => {
                        // A write to a defined getter-only name is under-reported
                        // (the oracle's setter code needs a receiver type); a
                        // fully missing name reports. A write pushed inside a
                        // constructor-initializer list is the field-initializer
                        // LHS — the oracle codes it through the dedicated
                        // initializer face (`_checkForInvalidField`,
                        // error_verifier.dart: `initializer_for_non_existent_
                        // field` / `field_initialized_outside_declaring_
                        // class`), never `undefined_identifier`.
                        let is_write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
                        let initializer_container = is_write
                            .then(|| self.init_rows.get(&row.range.offset))
                            .flatten()
                            .copied();
                        if let Some(container) = initializer_container {
                            // `_checkForInvalidField` reads the ENCLOSING
                            // class's own field map (`enclosingInstanceElement
                            // .getField` — superclass fields never initialize
                            // here): a miss or a non-field member (method,
                            // getter/setter) codes the whole initializer. The
                            // static-field arm (`initializer_for_static_
                            // field`) has no code-table variant yet — that hit
                            // stays silent rather than mis-code.
                            let own_field = resolve_container(
                                self.graph,
                                self.namespaces,
                                self.index,
                                self.lib,
                                self.file,
                                container,
                            )
                            .and_then(|(decl, _)| decl.members.get(&row.name))
                            .is_some_and(|entry| {
                                entry.kind == MemberKind::Field && !entry.is_static
                            });
                            if !own_field {
                                // `.at(initializer)` — the whole
                                // `ConstructorFieldInitializer` node, never
                                // the bare field name.
                                let range = self.initializer_node_span(row.range);
                                self.emit(
                                    SemanticCode::InitializerForNonExistentField,
                                    range,
                                    name_text,
                                    "",
                                );
                            }
                        } else if !Self::synthetic_reference(row.range)
                            && !self.keyword_bare_reference(row.range)
                            && !self.guard.ignore(None, row.name)
                            && (initializer_container.is_some()
                                || !self.write_target_rescues(&row, this_type))
                        {
                            self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                        }
                        let use_kind = match row.intent {
                            Intent::Write | Intent::ReadWrite => NameUse::Write,
                            _ => NameUse::Read,
                        };
                        self.record_miss(row.name, row.range, false, use_kind);
                    }
                    Intent::Operator(..) => {
                        // The undefined operand reports itself (`missing + 1`'s
                        // undefined_identifier at the head — the
                        // simple-identifier face the operator's own resolution
                        // rides): the same guarded emission the Read arm
                        // carries (the write-initializer clause is a
                        // write-only posture no operator head takes). The
                        // operator face never cascades a second row — the
                        // invalid operand's dynamic face swallows the
                        // operator lookup (probed: `missing + 1` reports the
                        // identifier alone, `missing[0]` likewise).
                        // A setter-only name does not rescue the operand
                        // read — the operator consults the getter namespace
                        // upstream (a `name=` hit is recovery, never the
                        // requested element, lexical_lookup.dart:22-35), so
                        // the `bare_write_target_exists` second chance does
                        // not apply here the way it does to `Intent::Write`.
                        if !Self::synthetic_reference(row.range)
                            && !self.keyword_bare_reference(row.range)
                            && !self.guard.ignore(None, row.name)
                        {
                            self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                        }
                        self.record_miss(row.name, row.range, false, NameUse::Read);
                    }
                }
            }
        }
    }

    /// The operator token adjacent to a non-operator read/write row (#2815
    /// R2): the suffix face — a compound-assignment (`+=`, `<<=`, …) or
    /// postfix `++`/`--` token directly after the row's span — or the
    /// prefix face — a `++`/`--`/`~`/`-` token directly before it, unary
    /// by the token ahead of the operator (an operand-ending token makes
    /// `-` the binary minus whose RIGHT operand this row is; `~` has no
    /// binary spelling, `~/` is its own token). The row model stages no
    /// marker for these applications, so the pass reads the token table
    /// — the same stream `mark_assignment`'s own candidate rule walks.
    /// `None` for every other adjacency (a plain `=` among them: a plain
    /// assignment reads nothing unconditionally; `??=` guards its write).
    /// The returned triple is the BASE operator's spelling (`+=` reports
    /// the method '+'), the operator token's span, and the variant pick
    /// (every shape in this family takes the method variant —
    /// messages.yaml's `uncheckedMethodInvocationOfNullableValue`, the
    /// compound/unary faces' oracle text).
    fn adjacent_operator(
        &self,
        row: &Row,
    ) -> Option<(&'static str, SourceRange, UncheckedNullableKind, bool)> {
        let tokens = self.tokens.as_slice();
        let span = |token: &dartq_scanner::Token| SourceRange {
            offset: token.utf16_offset,
            // Operator lexemes are ASCII: the UTF-16 length is the byte
            // length.
            length: token.byte_len,
        };
        let at = tokens.partition_point(|t| t.utf16_offset < row.range.end());
        let suffix = tokens.get(at).and_then(|token| {
            let (text, kind, compound) = match token.kind {
                dartq_scanner::TokenKind::PlusPlus => ("+", UncheckedNullableKind::Method, false),
                dartq_scanner::TokenKind::PlusEq => ("+", UncheckedNullableKind::Method, true),
                dartq_scanner::TokenKind::MinusMinus => {
                    ("-", UncheckedNullableKind::Method, false)
                }
                dartq_scanner::TokenKind::MinusEq => ("-", UncheckedNullableKind::Method, true),
                dartq_scanner::TokenKind::StarEq => ("*", UncheckedNullableKind::Method, true),
                dartq_scanner::TokenKind::SlashEq => ("/", UncheckedNullableKind::Method, true),
                dartq_scanner::TokenKind::PercentEq => ("%", UncheckedNullableKind::Method, true),
                dartq_scanner::TokenKind::TildeSlashEq => {
                    ("~/", UncheckedNullableKind::Method, true)
                }
                dartq_scanner::TokenKind::LtLtEq => ("<<", UncheckedNullableKind::Method, true),
                dartq_scanner::TokenKind::GtGtEq => (">>", UncheckedNullableKind::Method, true),
                dartq_scanner::TokenKind::GtGtGtEq => {
                    (">>>", UncheckedNullableKind::Method, true)
                }
                dartq_scanner::TokenKind::AmpersandEq => {
                    ("&", UncheckedNullableKind::Method, true)
                }
                dartq_scanner::TokenKind::BarEq => ("|", UncheckedNullableKind::Method, true),
                dartq_scanner::TokenKind::CaretEq => ("^", UncheckedNullableKind::Method, true),
                _ => return None,
            };
            Some((text, span(token), kind, compound))
        });
        if suffix.is_some() {
            return suffix;
        }
        let before = tokens.partition_point(|t| t.utf16_offset < row.range.offset);
        let token = before.checked_sub(1).and_then(|index| tokens.get(index))?;
        let unary = match token.kind {
            dartq_scanner::TokenKind::PlusPlus => true,
            dartq_scanner::TokenKind::MinusMinus => true,
            dartq_scanner::TokenKind::Tilde => true,
            dartq_scanner::TokenKind::Minus => before
                .checked_sub(2)
                .and_then(|index| tokens.get(index))
                .is_none_or(|prev| !Self::operand_ending_kind(prev.kind)),
            _ => false,
        };
        if !unary {
            return None;
        }
        // The oracle's operator-name spellings (messages.yaml): unary minus
        // is `unary-` (never the binary `-`), the increments report their
        // base operator.
        let text = match token.kind {
            dartq_scanner::TokenKind::PlusPlus => "+",
            dartq_scanner::TokenKind::Minus => "unary-",
            dartq_scanner::TokenKind::Tilde => "~",
            _ => "-",
        };
        Some((text, span(token), UncheckedNullableKind::Method, false))
    }

    /// Whether the token kind can END an operand (the unary-minus
    /// discriminator: `a - x` makes `x` the binary's right operand; `-x`
    /// makes it the negation's receiver). Interpolated-set-literal and
    /// postfix edges (`}` ahead of `-`, an `++` before it) are not
    /// distinguishable at token level — the block/statement readings win,
    /// the documented divergence for those recovery shapes.
    fn operand_ending_kind(kind: dartq_scanner::TokenKind) -> bool {
        use dartq_scanner::TokenKind as K;
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
                | K::Keyword(dartq_scanner::Keyword::This)
                | K::Keyword(dartq_scanner::Keyword::Super)
                | K::Keyword(dartq_scanner::Keyword::True)
                | K::Keyword(dartq_scanner::Keyword::False)
                | K::Keyword(dartq_scanner::Keyword::Null)
        )
    }

    /// Whether a scope parameter's bound declares the operator spelling —
    /// upstream resolves a type-parameter receiver through
    /// `resolveToBound(leftType)` before the property consult
    /// (binary_expression_resolver.dart:461), so `T extends num` resolves
    /// `+` on `num`'s face. The captured windows survive the pass-2 frame
    /// teardown ([`Self::param_bound_of`]); a class-generic parameter
    /// resolves through its DECLARING container's captured bounds —
    /// `hint` carries the receiver/member's owning container when the
    /// verdict site knows it (a member read's walk container, the `refs`
    /// publication's `Member` target for a bare field row); `None` keeps
    /// the scope-window probes alone.
    fn bound_defines_operator_in(
        &self,
        head: StringId,
        spelling: StringId,
        hint: Option<StringId>,
    ) -> bool {
        let bound = self
            .param_bound(head)
            .or_else(|| self.param_bound_of(head))
            .or_else(|| {
                let container = hint?;
                self.bounds
                    .container_params(container)?
                    .into_iter()
                    .find(|(param, ..)| *param == head)
                    .and_then(|(_, tree, _)| tree)
                    .map(|tree| self.tree_ty(&tree))
            });
        let Some(bound) = bound else {
            return false;
        };
        let Ty::Named { name, .. } = self.fold_alias_ty(bound) else {
            return false;
        };
        matches!(self.walk_members(name, spelling), Walk::Found(_))
    }

    /// Whether the compound/prefix/postfix spelling resolves on the operand
    /// face: the container walk, the bound, or an extension-declared
    /// operator (upstream resolves the increment and compound twins through
    /// the same property consult — `extension E on A { A operator+(int) }`
    /// keeps `a++` and `a += 1` silent). The message spelling probes its
    /// method spelling (`unary-` reports as such but answers on `-`). A
    /// dynamic/void/unknown face dispatches dynamically and stays silent; a
    /// record or function operand consults the shape-extension faces the
    /// same `_lookupExtension` run answers (an extension over `(int,
    /// String)` or `Function` defines the spelling on its shape).
    /// [`Self::adjacent_operator_defined_in`] with the declaring-container
    /// hint for the scope-parameter bound consult — see
    /// [`Self::bound_defines_operator_in`].
    fn adjacent_operator_defined_in(
        &mut self,
        ty: Ty,
        op_text: &str,
        hint: Option<StringId>,
    ) -> AdjacentOp {
        let probe = self.intern(if op_text == "unary-" { "-" } else { op_text });
        if probe == self.intern("==") {
            return AdjacentOp::Defined;
        }
        match self.fold_alias_ty(ty) {
            Ty::Named { name, .. } => {
                if matches!(self.walk_members(name, probe), Walk::Found(_))
                    || self.bound_defines_operator_in(name, probe, hint)
                {
                    return AdjacentOp::Defined;
                }
                match self.extension_application(name, ty, probe, false, false) {
                    // The ambiguous application replaces the operator miss
                    // (the oracle reports the ambiguity at the operator
                    // token, never `undefined_operator` —
                    // `a += 0` over two `operator +` extensions).
                    ExtensionApp::Ambiguous(displays) => AdjacentOp::Ambiguous(displays),
                    ExtensionApp::Single | ExtensionApp::UnsafeSingle => AdjacentOp::Defined,
                    ExtensionApp::None => AdjacentOp::Missing,
                }
            }
            Ty::Record { .. } | Ty::Function { .. } => {
                if self.record_extension_face(ty, probe).is_some() {
                    AdjacentOp::Defined
                } else {
                    AdjacentOp::Missing
                }
            }
            _ => AdjacentOp::Defined,
        }
    }

    /// Whether an extension application resolves the operator face a row
    /// spells against the NULLABLE receiver `ty` (the #2815 R2 gate's
    /// suppression): only the `Single` verdict — a face whose `on` type
    /// covers the nullable spelling — resolves; `UnsafeSingle` (applicable
    /// through the non-null projection alone) keeps the unchecked report,
    /// the same split the member sites encode.
    fn operator_face_extension_resolved(&mut self, ty: Ty, row: &Row) -> bool {
        let (read, write) = self.operator_spellings(row);
        let Ty::Named { name, .. } = ty else {
            return false;
        };
        [read, write].into_iter().flatten().any(|spelling| {
            spelling == self.intern("==")
                || matches!(
                    self.extension_application(name, ty, spelling, false, true),
                    ExtensionApp::Single
                )
        })
    }

    /// Whether a bare invocation's callee type is function-shaped (the
    /// #2815 R2a gate — upstream `resolveToBound(receiverType) is
    /// FunctionTypeImpl`): a nullable function shape (directly or through
    /// a type-alias fold), the bare `Function` head spelled with `?`, a
    /// `?`-less parameter whose bound walks to one of those, and `Never?`
    /// — the question-suffixed Never the oracle corpus pins to the
    /// function face (`Never? x; x()` reports the function text).
    fn invoke_callee_function_shaped(&self, ty: Ty) -> bool {
        let function_name = self.intern("Function");
        let never_name = self.intern("Never");
        let mut ty = Some(self.fold_alias_ty(ty));
        for _ in 0..4 {
            match ty {
                Some(Ty::Function { .. }) => return true,
                Some(Ty::Named {
                    name,
                    nullable: true,
                    ..
                }) => {
                    if name == function_name || name == never_name {
                        return true;
                    }
                    // A nullable parameter spelling walks its bound the
                    // same way the non-null arm does: `T?` where
                    // `T extends Function` is a nullable function face
                    // (`x.first()` over `List<T?>` invokes it —
                    // function_expression_invocation_resolver.dart's
                    // `isNullableFunctionType` reads through the bound).
                    match self.param_bound_of(name) {
                        Some(bound) => {
                            ty = Some(self.fold_alias_ty(Self::nullable_form(bound)));
                        }
                        None => return false,
                    }
                }
                Some(Ty::Named {
                    name,
                    nullable: false,
                    ..
                }) => match self.param_bound_of(name) {
                    Some(bound) => ty = Some(self.fold_alias_ty(bound)),
                    None => return false,
                },
                _ => return false,
            }
        }
        false
    }

    /// Emits a type-position row from its push-time verdict snapshot.
    pub(crate) fn resolve_type_row(&mut self, row: &Row) {
        let RowShape::TypeUse(verdict, suppressed) = row.shape else {
            return;
        };
        let name_text = self.strings.resolve(row.name);
        match verdict {
            TypeVerdict::PrefixHead => {
                // The declaration-head keyword skip rides this face too: a
                // prefix head can never spell `dynamic`/`var`/`final`/`const`/
                // `void` (no legal parse writes one — `dynamic` answers the
                // top type before any prefix question upstream), so a row
                // carrying that spelling is parser-recovery noise and the
                // oracle reports nothing (probe: dio dio_mixin.dart:419
                // `if (T != dynamic &&` — the recovered `dynamic` reported
                // `undefined_identifier` here while `dart analyze` stays
                // silent; the value-position gate does not reach a TypeUse
                // row).
                if !Self::synthetic_reference(row.range)
                    && !self.keyword_bare_reference(row.range)
                    && !self.guard.ignore(None, row.name)
                {
                    self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                }
                self.record_miss(row.name, row.range, false, NameUse::Read);
            }
            TypeVerdict::Prefixed { prefix, local } => {
                // `p.dynamic` — a prefix whose own import edge exports the
                // builtin (`import 'dart:core' as core show dynamic;`):
                // upstream resolves the builtin top type through the
                // prefix's core namespace ([`Self::prefix_exports_dynamic`]),
                // and the export scope never carries the synthetic element,
                // so the push-time verdict reads a prefixed miss. Reporting
                // here named a class that does not exist.
                let builtin_top = row.name == self.intern("dynamic")
                    && self.prefix_exports_dynamic(prefix);
                // The local-prefix face: a prefix head that names a local
                // or parameter element is not an import prefix — the
                // oracle's prefixed lookup fails on the head and asks no
                // question of the continuation, so the miss stays silent
                // (upstream Dart SDK source, pkg/analyzer/lib/src/dart/
                // resolver/named_type_resolver.dart — the importPrefix
                // gate leaves the NamedType unresolved with no second
                // row). #1444: an extension-type clause position stays
                // silent too — the official clause-parent guard defers
                // the face.
                if !local
                    && !builtin_top
                    && !self.guard.ignore(Some(prefix), row.name)
                    && !Self::take_extension_clause_miss(
                        &mut self.extension_clause_miss,
                        row.range.offset,
                    )
                {
                    // `_getErrorRange`'s default (named_type_resolver.dart
                    // :731-747): a written prefix opens the range at its own
                    // token, the simple name's end closes it — `math
                    // .Unresolved` reports over the whole written name.
                    let range = self.qualified_name_range(row.range);
                    self.emit(SemanticCode::UndefinedClass, range, name_text, "");
                }
                self.record_miss(row.name, row.range, true, NameUse::Type);
            }
            // The plain path's non-type face: the name resolved to a
            // non-type element (`reportNullOrNonTypeElement`'s tail,
            // named_type_resolver.dart:794-806). A type-argument position
            // replaces it with `non_type_as_type_argument` (the same
            // context chain the miss rows ride); the `is`/`as` suppression
            // does not apply — the oracle reports the resolver face there
            // too (the pinned `x isn't a type` on a parameter inside an
            // `is` operand).
            TypeVerdict::NonType => {
                if Self::take_context_miss(&mut self.type_arg_miss_rows, row.range.offset) {
                    self.emit_corrected(SemanticCode::NonTypeAsTypeArgument, row.range, name_text);
                // The clause-parent suppression covers resolved non-types
                // too (named_type_resolver.dart:772-778 returns before the
                // `element != null` notAType arm): a clause entry that names
                // a variable or function reports the clause code instead
                // (scope_analyzer.dart:1195-1228 — the element exists but is
                // no ClassElement/MixinElement, so the clause switch
                // decides).
                } else if let Some(tag) =
                    Self::take_clause_miss(&mut self.clause_miss, row.range.offset)
                {
                    self.emit_clause_miss(tag, row.range, name_text);
                } else if let Some((anchor_file, anchor_range)) =
                    Self::take_decl_anchor(&mut self.non_type_anchors, row.range.offset)
                {
                    // The declaration row rides when the resolved element's
                    // fragment carries a location
                    // (named_type_resolver.dart:794-806).
                    self.emit_with_context(
                        SemanticCode::NotAType,
                        row.range,
                        name_text,
                        "",
                        vec![ContextMessage {
                            message: format!("The declaration of '{name_text}' is here.").into(),
                            file: anchor_file,
                            range: anchor_range,
                        }],
                    );
                } else {
                    self.emit(SemanticCode::NotAType, row.range, name_text, "");
                }
            }
            TypeVerdict::RedirectTarget => {
                // `redirect_to_non_class` is the oracle's answer here and it
                // is outside the frozen gate — record the miss, emit nothing.
                self.record_miss(row.name, row.range, false, NameUse::Type);
            }
            TypeVerdict::Missing => {
                // A GFT's own type-parameter spellings (`Function<T>(T x)`,
                // `… Function<T>(void Function(T), T)`): the names declare
                // function-type parameters bound across the signature —
                // inside the `<…>` group itself or in any formal type — so
                // they are never undefined classes (the parser's GFT scan
                // emits no type-variable events; this gate owns the
                // verdict by harvesting the enclosing group's names).
                if let Some((lo, hi)) = self.gft_param_group_covering(row.range.offset) {
                    let name_bytes = self.strings.resolve(row.name).as_bytes();
                    let mut window = (lo..hi).filter_map(|at| self.byte_at(at));
                    let mut word: Vec<u8> = Vec::new();
                    let mut declared = false;
                    while let Some(b) = window.next() {
                        match b {
                            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'$' => word.push(b),
                            _ => {
                                if !word.is_empty() {
                                    if word.as_slice() == name_bytes {
                                        declared = true;
                                        break;
                                    }
                                    word.clear();
                                }
                            }
                        }
                    }
                    if !declared && !word.is_empty() && word.as_slice() == name_bytes {
                        declared = true;
                    }
                    if declared {
                        self.record_miss(row.name, row.range, false, NameUse::Type);
                        return;
                    }
                }
                if !suppressed && !self.guard.ignore(None, row.name) {
                    // A declaration parse keeps `await` in the type position:
                    // the unresolved simple name carries the await code, not
                    // `undefined_class` (named_type_resolver.dart:814-818 —
                    // `importPrefix == null && lexeme == 'await'` fires
                    // `undefinedIdentifierAwait` at the NamedType and
                    // returns before the `undefinedClass` fallthrough; the
                    // message names `await` itself, no arguments).
                    if row.name == self.intern("await") {
                        self.emit(SemanticCode::UndefinedIdentifierAwait, row.range, "", "");
                    }
                    // #1314 FAM13: the context chain replaces the
                    // `undefined_class` fallthrough — the type-argument
                    // face first, then the creation head
                    // (named_type_resolver.dart:760-770; both replacements
                    // pinned by the oracle: only the context row lands at
                    // the name).
                    else if Self::take_context_miss(
                        &mut self.type_arg_miss_rows,
                        row.range.offset,
                    ) {
                        self.emit_corrected(
                            SemanticCode::NonTypeAsTypeArgument,
                            row.range,
                            name_text,
                        );
                    } else if let Some(explicit) =
                        Self::take_keyed_miss(&mut self.creation_miss_rows, row.range.offset)
                    {
                        // An explicit `new`/`const` creation head reports
                        // `creation_with_non_type` on every miss — the
                        // `ConstructorInvocation` parent routes both the
                        // unresolved name and the resolved-non-class shape
                        // (upstream Dart SDK source, named_type_resolver.dart
                        // :285-289/:313-330). An implicit `C(...)` head keeps
                        // the split: a resolved non-class reports
                        // `creation_with_non_type`, an unresolved name
                        // `undefined_class`.
                        if explicit
                            || matches!(self.resolve_bare(row.name).0, BareInfo::Plain { .. })
                        {
                            self.emit_corrected(
                                SemanticCode::CreationWithNonType,
                                row.range,
                                name_text,
                            );
                        } else {
                            self.emit_corrected(SemanticCode::UndefinedClass, row.range, name_text);
                        }
                    }
                    // The catch clause's `on`-type row: the F-P recovered
                    // shapes code `non_type_in_catch_clause`, never the
                    // resolver's `undefined_class` (the parser's gate
                    // tripwires pin the routing; nonTypeInCatchClause,
                    // messages.yaml:13748-13753).
                    else if Self::take_context_miss(
                        &mut self.catch_on_miss_rows,
                        row.range.offset,
                    ) {
                        self.emit_corrected(
                            SemanticCode::NonTypeInCatchClause,
                            row.range,
                            name_text,
                        );
                    }
                    // #1314 FAM14: the clause faces replace the fallthrough
                    // (scope_analyzer.dart `_resolveType`'s clause switch);
                    // the `on` constraint is additive — the oracle keeps
                    // the resolver's `undefined_class` beside it.
                    else if let Some(tag) =
                        Self::take_clause_miss(&mut self.clause_miss, row.range.offset)
                    {
                        self.emit_clause_miss(tag, row.range, name_text);
                    }
                    // #1444: an extension-type header clause never reports
                    // `undefined_class` from name resolution — the official
                    // clause-parent guard defers (see the ledger note), so
                    // the hit consumes silently.
                    else if Self::take_extension_clause_miss(
                        &mut self.extension_clause_miss,
                        row.range.offset,
                    ) {
                    } else {
                        self.emit(SemanticCode::UndefinedClass, row.range, name_text, "");
                    }
                }
                self.record_miss(row.name, row.range, false, NameUse::Type);
            }
        }
    }

    /// Consumes one recorded context-miss offset (#1314 FAM13): the rows
    /// ascend with parse order, so a binary hit removes exactly the matching
    /// reference.
    pub(crate) fn take_context_miss(rows: &mut Vec<u32>, offset: u32) -> bool {
        match rows.binary_search(&offset) {
            Ok(index) => {
                rows.remove(index);
                true
            }
            Err(_) => false,
        }
    }

    /// Consumes one recorded offset out of a keyed miss table (the
    /// creation-head explicit flag — the same ascending binary hit as
    /// [`Self::take_context_miss`], returning the carried tag).
    pub(crate) fn take_keyed_miss(rows: &mut Vec<(u32, bool)>, offset: u32) -> Option<bool> {
        match rows.binary_search_by_key(&offset, |entry| entry.0) {
            Ok(index) => Some(rows.remove(index).1),
            Err(_) => None,
        }
    }

    /// Consumes one recorded non-type declaration anchor (the `not_a_type`
    /// context face): the same ascending binary hit as
    /// [`Self::take_keyed_miss`], returning the carried (file, span) pair.
    pub(crate) fn take_decl_anchor(
        rows: &mut Vec<(u32, (FileId, SourceRange))>,
        offset: u32,
    ) -> Option<(FileId, SourceRange)> {
        match rows.binary_search_by_key(&offset, |entry| entry.0) {
            Ok(index) => Some(rows.remove(index).1),
            Err(_) => None,
        }
    }

    /// Consumes one recorded clause-entry offset (#1314 FAM14): the same
    /// ascending binary hit, returning the clause face.
    pub(crate) fn take_clause_miss(
        rows: &mut Vec<(u32, ClauseTag)>,
        offset: u32,
    ) -> Option<ClauseTag> {
        match rows.binary_search_by_key(&offset, |entry| entry.0) {
            Ok(index) => Some(rows.remove(index).1),
            Err(_) => None,
        }
    }

    /// Inserts one clause-entry miss keeping the ledger's ascending order
    /// (the binary consumption above requires it): out-of-order clause
    /// recovery pushes an extends head whose offset precedes already-fed
    /// `implements`/`with` entries.
    pub(crate) fn push_clause_miss(&mut self, offset: u32, tag: ClauseTag) {
        let at = self
            .clause_miss
            .partition_point(|(existing, _)| *existing < offset);
        self.clause_miss.insert(at, (offset, tag));
    }

    /// Emits a clause-entry miss through its ledger tag
    /// (scope_analyzer.dart `_resolveType`'s clause switch): the `on`
    /// constraint is additive — the oracle keeps the resolver's
    /// `undefined_class` beside `mixin_super_class_constraint_non_interface`.
    pub(crate) fn emit_clause_miss(&mut self, tag: ClauseTag, range: SourceRange, name_text: &str) {
        match tag {
            ClauseTag::With => {
                self.emit(SemanticCode::MixinOfNonClass, range, "", "");
            }
            // The two clause faces carrying oracle correctionMessages
            // (codes.rs `correction_message`): the emit helper pushes no
            // correction, so these route through the correction-carrying
            // emit.
            ClauseTag::Implements => {
                let code = SemanticCode::ImplementsNonClass;
                let message = code.message("", "");
                self.emit_message_with_correction(code, range, message);
            }
            ClauseTag::Extends => {
                let code = SemanticCode::ExtendsNonClass;
                let message = code.message("", "");
                self.emit_message_with_correction(code, range, message);
            }
            ClauseTag::AliasSuper => {
                self.emit(SemanticCode::MixinWithNonClassSuperclass, range, "", "");
            }
            ClauseTag::OnConstraint => {
                self.emit(
                    SemanticCode::MixinSuperClassConstraintNonInterface,
                    range,
                    "",
                    "",
                );
                self.emit(SemanticCode::UndefinedClass, range, name_text, "");
            }
        }
    }

    /// Consumes one recorded extension-type clause entry (#1444): the same
    /// ascending binary hit; a hit is silent.
    pub(crate) fn take_extension_clause_miss(rows: &mut Vec<u32>, offset: u32) -> bool {
        match rows.binary_search(&offset) {
            Ok(index) => {
                rows.remove(index);
                true
            }
            Err(_) => false,
        }
    }

    /// The synthetic-token reference skip: a row whose token span is empty
    /// is a recovery-inserted identifier — the oracle's resolver walks real
    /// identifier tokens only and reports nothing for one (upstream Dart
    /// SDK source, pkg/analyzer/lib/src/dart/element/element.dart — a
    /// synthetic identifier token carries no element to resolve against).
    pub(crate) fn synthetic_reference(range: SourceRange) -> bool {
        range.length == 0
    }

    /// The keyword-token bare-reference skip: the statement lane leaves a
    /// The declaration-start keyword gate: a bare row whose source text is
    /// one of `var`/`final`/`const`/`dynamic`/`void` stands where the oracle
    /// parsed a declaration head, not an expression identifier — its
    /// recovery reports no `undefined_identifier` for the standing token
    /// (probe: `var;`/`final;`/`const;`/`dynamic;`/`void;` emit nothing
    /// upstream). Every other keyword consumed as a name DOES resolve —
    /// `ensureIdentifier` returns `isIdentifier` spellings directly
    /// (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/parser/
    /// identifier_context_impl.dart:397) and `isKeywordOrIdentifier`
    /// fallbacks are consumed with `expectedIdentifierButGotKeyword`
    /// (:435-440), so `assert`, `class`, `in`, `as`, `when`, `late`,
    /// `static`, `covariant` all report `undefined_identifier` upstream.
    /// Member faces keep this check off: a built-in identifier is a legal
    /// member name (`obj.late`).
    pub(crate) fn keyword_bare_reference(&self, range: SourceRange) -> bool {
        let start = self.byte_of(range.offset);
        let end = self.byte_of(range.offset + range.length);
        self.source
            .get(start..end)
            .is_some_and(|text| matches!(text, "var" | "final" | "const" | "dynamic" | "void"))
    }

    /// The static type of one receiver EXPRESSION: the flow read-type face
    /// first (the #2346① leg — a promoted local's read answers at its own
    /// span), then the chained-member face — a receiver that is itself a
    /// merged member read (`e.supertype.m`'s `e.supertype`, `a.b.c`'s
    /// `a.b`) owns the accessor's declared return even when the merge froze
    /// the leaf's receiver `Unknown` because the predecessor hop resolved
    /// on the declared (unpromoted) receiver type. The merge records the
    /// receiver's span as the receiver ROW's own range — for a chained
    /// member node that is the node's member token — so the chain leg
    /// matches the node's staged read record by its token's end and
    /// re-derives its receiver through this same face
    /// ([`Self::receiver_static_ty`] then answers getters, fields, and
    /// invocation returns alike). Each backward hop matches a strictly
    /// earlier token, so the walk descends the chain monotonically and
    /// terminates.
    pub(crate) fn recv_expression_ty(&self, read_span: SourceRange) -> Option<Ty> {
        // The pass-2 memo (`recv_memo`'s contract): consult and populate
        // only while the resolve loop holds the tables the answer reads
        // frozen. The chain walk re-derives a receiver chain once per
        // node, so a cascade's N sections cost N hops total, not N²/2.
        if self.recv_memo_active
            && let Some(hit) = self.recv_memo.borrow().get(&read_span.offset)
        {
            return *hit;
        }
        let ty = self
            .recv_expression_face(read_span)
            .map(|ty| self.null_asserted_after(read_span, ty));
        if self.recv_memo_active {
            self.recv_memo.borrow_mut().insert(read_span.offset, ty);
        }
        ty
    }

    /// The CALL RESULT of a receiver span a send already consumed. A
    /// completed send's callee span denotes the call's VALUE, never the
    /// callee's own type: upstream types a `FunctionExpressionInvocation`
    /// / `MethodInvocation` node by the invoked signature's return (the
    /// receiver-type computation in
    /// `method_invocation_resolver.dart` reads the receiver node's
    /// `staticType`, which for `f(x)` is `staticType.returnType`), so
    /// `f(x).m` resolves `m` on `f`'s RETURN type. Reading the callee's
    /// own face there modelled the receiver as its `FunctionType` and ran
    /// the member verdict on the function face instead, reporting
    /// `undefined_method`/`undefined_getter` against a `Function`
    /// receiver the oracle never resolves against.
    ///
    /// [`Engine::call_ends`] keys exactly the callee offsets of completed
    /// sends, so the gate is exact: a non-invoked read of the same span
    /// (a tear-off `f.gone`, a bare operand) keeps the callee's own face,
    /// and a callable face whose result the walk cannot decide (a generic
    /// signature, a callable class) keeps it too — the under-report
    /// posture the result faces already take.
    fn invoked_receiver_face(&self, read_span: SourceRange, face: Ty) -> Ty {
        if self
            .call_ends
            .binary_search_by_key(&read_span.offset, |(offset, _)| *offset)
            .is_err()
        {
            return face;
        }
        self.callable_result_of(face).unwrap_or(face)
    }

    /// [`Self::recv_expression_ty`] before the trailing null-assert: the
    /// flow read face, then the chained-member face.
    fn recv_expression_face(&self, read_span: SourceRange) -> Option<Ty> {
        // The value legs answer the receiver EXPRESSION's own face; a span
        // a completed send already consumed denotes the call's VALUE, so
        // the callee's own face is swapped for it
        // ([`Self::invoked_receiver_face`]).
        if let Some(face) = self.recv_value_face(read_span) {
            return Some(self.invoked_receiver_face(read_span, face));
        }
        // The chain leg needs no swap: an invoked node's answer IS the
        // send's result already (`a.b()`'s node is the send, never the
        // tear-off), and swapping it again would unwrap the result type.
        // It reads the staged member-read records only: a chain node's own
        // record sits at its member token, whose span END equals the
        // receiver span the child merge recorded (`e.supertype.m`'s leaf
        // names `supertype`'s end) — so the walk matches the node by its
        // member token's end and descends through each record's
        // `recv_span`. Offsets are UTF-16 units, so the token end is the
        // name's UTF-16 length past its offset.
        let prev = self.chain_node_read(read_span, false)?;
        self.chained_read_static_ty(prev)
    }

    /// [`Self::recv_expression_face`]'s value legs: the re-bound local's
    /// read, the flow read face, the flow-blind initializer face, the
    /// index-result fold, the compound join, and the parenthesized group.
    fn recv_value_face(&self, read_span: SourceRange) -> Option<Ty> {
        // The re-bound local's corrected face outranks the raw flow read:
        // the flow layer's declared slot staged the parse-time bind
        // (#2140) and pass 2's re-bind (#2539) is the only holder of the
        // promoted accessor face — without this layer a `var` chain
        // (`var a = f.element; var b = a.m`) re-derives `b`'s initializer
        // against `a`'s stale parse face and the whole chain rides the
        // base declaration. The helper's own gate keeps a genuinely
        // promoted read (a flow face past the parse bind) on its flow
        // face.
        if let Some(ty) = self.rebound_read_face(read_span) {
            return Some(ty);
        }
        // The flow row is the PROMOTED face when one stands; a flow-blind
        // local's row carries `Unknown` (the lattice cannot spell the
        // initializer's member return), and that must not short-circuit
        // the derived leg below — every caller that pattern-matches a face
        // treats `Some(Unknown)` exactly as `None`.
        if let Some(ty) = self.read_type_at(read_span).filter(|ty| *ty != Ty::UNKNOWN) {
            return Some(ty);
        }
        // The flow-blind local's pass-2 initializer face: the for-in
        // variable over a property-read iterable (the flow lattice cannot
        // spell the member return), answered by the `resolve_rows`
        // pre-pass over the initializer row. The NON-NULL form: the face
        // feeds member RESOLUTION (the head), while the nullability
        // verdicts stay with the flow — the promotion machinery only
        // knows flow-staged types, so a nullable derived face would
        // report `unchecked_use_of_nullable_value` rows the oracle's
        // `!= null`/`is` promotions silence (and a guard-less read the
        // oracle DOES report was already suppressed as Unknown — the
        // under-report direction stands either way).
        if let Some(&decl) = self.local_decl_at_read.get(&read_span.offset)
            && let Some(ty) = self.init_derived_types.borrow().get(&decl).copied()
        {
            return Some(Self::non_null_form(ty));
        }
        // The index-result leg: a folded `recv[i]` result row re-types
        // through its receiver expression's re-derived face — the fold's
        // push-time result read the unpromoted receiver. Each hop names a
        // strictly earlier span (the receiver precedes its `]`), so the
        // walk terminates; the depth cap keeps a flat 20k-bracket chain
        // from exhausting the worker stack (past it the answer degrades
        // to the outer fallbacks).
        if let Some(&(indexed, null_aware)) = self.index_result_recvs.get(&read_span.offset)
            && indexed.offset < read_span.offset
            && self.recv_walk_depth.get() < RECV_WALK_DEPTH_CAP
        {
            self.recv_walk_depth.set(self.recv_walk_depth.get() + 1);
            let recv_ty = self.recv_expression_ty(indexed);
            self.recv_walk_depth.set(self.recv_walk_depth.get() - 1);
            let recv_ty = recv_ty?;
            return match self.index_result_ty(recv_ty) {
                Ty::Unknown => None,
                ty if null_aware => Some(Self::nullable_form(ty)),
                ty => Some(ty),
            };
        }
        // The compound receiver face (`(c ? a : b).m`, `(a ?? b).m` — the
        // resolver's `static_type` join, resolver.dart:2426 →
        // static_type_analyzer.dart:74-92): no staged fact covers the
        // composite span, so the token-level `??`/`?:` split re-derives
        // each arm's face through this same chain and joins them
        // name-level. The walk keys strictly on spans ENDING inside the
        // receiver (the member token's own end marks the chain's node);
        // a nested composite re-enters through the recursion under the
        // same depth cap the chain walk observes.
        if self.recv_walk_depth.get() < RECV_WALK_DEPTH_CAP {
            self.recv_walk_depth.set(self.recv_walk_depth.get() + 1);
            let compound = self.compound_expression_face(read_span);
            self.recv_walk_depth.set(self.recv_walk_depth.get() - 1);
            if compound.is_some() {
                return compound;
            }
        }
        // The parenthesized-group face (`(a ?? b).m`): the group row's own
        // range is its closing paren alone, so the composite's type is not
        // on record anywhere — the group's static type is its inner
        // expression's own, re-derived through this same walk. The leg
        // carries its own depth guard.
        if let Some(group) = self.group_expression_face(read_span) {
            return Some(group);
        }
        None
    }

    /// The name-level join of a `??`/`?:` receiver's arm faces — the
    /// upstream `least_upper_bound` the resolver stamps on the composite's
    /// `staticType` (binary_expression_resolver.dart:69-109 for `??`,
    /// conditional_expression_resolver for `?:`). Arms re-derive through
    /// [`Self::recv_expression_ty`] so promoted reads and chained sends
    /// type the same way the walk above types them. `None` when the span
    /// holds no top-level `??`/`?:` (the caller falls to the chain walk)
    /// or no arm types (the member verdict keeps the under-report).
    fn compound_expression_face(&self, read_span: SourceRange) -> Option<Ty> {
        let tokens = self.tokens.as_slice();
        let mut first = tokens.partition_point(|t| t.utf16_offset < read_span.offset);
        let mut last = tokens.partition_point(|t| t.utf16_offset < read_span.end());
        // Strip enclosing parens: `(e)` ends at the `)` — the group is the
        // expression.
        while last > first + 1
            && tokens[first].kind == dartq_scanner::TokenKind::OpenParen
            && self.tokens.matching_end(first) == Some(last - 1)
        {
            first += 1;
            last -= 1;
        }
        if last <= first + 1 {
            return None;
        }
        // Depth-0 separator scan: bracket groups jump their matching close
        // (`matching_end` also covers a `<` the table paired). `?:` is the
        // LOWEST-precedence operator — when a top-level `?` pairs a `:` it
        // owns the split (`a ?? b ? c : d` = `(a ?? b) ? c : d`, never
        // `a ?? (b ? c : d)`), so the conditional outranks the leftmost
        // `??`. A `?` that never pairs (`T?`'s type suffix, `?[`'s index
        // probe) falls back to the NEXT `:` regardless: the split
        // mis-reads the condition side, but the two value arms still
        // come out intact (`x is int? ? a : b` joins `a`/`b` — the
        // upstream join's operands).
        let mut question = None;
        let mut if_null = None;
        let mut at = first;
        while at < last && (question.is_none() || if_null.is_none()) {
            match tokens[at].kind {
                dartq_scanner::TokenKind::OpenCurly
                | dartq_scanner::TokenKind::StringInterpolationExpression
                | dartq_scanner::TokenKind::OpenParen
                | dartq_scanner::TokenKind::OpenSquare
                | dartq_scanner::TokenKind::Lt => {
                    if let Some(close) = self.tokens.matching_end(at) {
                        if close < last {
                            at = close + 1;
                            continue;
                        }
                    }
                    at += 1;
                }
                dartq_scanner::TokenKind::QuestionQuestion => {
                    if_null.get_or_insert(at);
                    at += 1;
                }
                dartq_scanner::TokenKind::Question => {
                    question.get_or_insert(at);
                    at += 1;
                }
                _ => at += 1,
            }
        }
        if let Some(at) = question {
            // The matching `:` is the one returning conditional depth to
            // zero — a nested `a ? b ? x : y : c` keeps the inner arm
            // whole for the recursion.
            let mut colon = None;
            let mut depth = 1u32;
            let mut first_colon = None;
            let mut scan = at + 1;
            while scan < last {
                match tokens[scan].kind {
                    dartq_scanner::TokenKind::OpenCurly
                    | dartq_scanner::TokenKind::StringInterpolationExpression
                    | dartq_scanner::TokenKind::OpenParen
                    | dartq_scanner::TokenKind::OpenSquare
                    | dartq_scanner::TokenKind::Lt => {
                        if let Some(close) = self.tokens.matching_end(scan) {
                            if close < last {
                                scan = close + 1;
                                continue;
                            }
                        }
                        scan += 1;
                    }
                    dartq_scanner::TokenKind::Question => {
                        depth += 1;
                        scan += 1;
                    }
                    dartq_scanner::TokenKind::Colon => {
                        if first_colon.is_none() {
                            first_colon = Some(scan);
                        }
                        depth -= 1;
                        if depth == 0 {
                            colon = Some(scan);
                            break;
                        }
                        scan += 1;
                    }
                    _ => scan += 1,
                }
            }
            if let Some(colon) = colon.or(first_colon) {
                let then = self.arm_span(&tokens[at + 1..colon], at + 1);
                let other = self.arm_span(&tokens[colon + 1..last], colon + 1);
                return self.join_branch_ty(
                    then.and_then(|span| self.recv_expression_ty(span)),
                    other.and_then(|span| self.recv_expression_ty(span)),
                );
            }
        }
        if let Some(at) = if_null {
            let left = self.arm_span(&tokens[first..at], first);
            let right = self.arm_span(&tokens[at + 1..last], at + 1);
            // `LUB(NonNull(lhs), rhs)` — the official `??` result types
            // the left operand's non-null form (`resolveIfNull`,
            // binary_expression_resolver.dart:69-109): the expression is
            // non-null exactly when both the unwrapped left and the
            // right are.
            return self.join_branch_ty(
                left.and_then(|span| self.recv_expression_ty(span))
                    .map(Self::non_null_form),
                right.and_then(|span| self.recv_expression_ty(span)),
            );
        }
        None
    }

    /// The static type of a PARENTHESIZED receiver group
    /// (`(a ?? b).m`, `(x ?? y) + 1`): the group's own row carries no
    /// face — the `??` fold leaves the enclosing group row's range as its
    /// closing paren alone, so nothing stages the composite's type and the
    /// receiver resolves `Unknown`. `ParenthesizedExpression.staticType`
    /// is the inner expression's own type, so the leg re-enters
    /// [`Self::recv_expression_ty`] on the inner extent: the compound face
    /// then joins the `??`/`?:` arms exactly as it does for a receiver
    /// spanning the composite, and a chained inner read (`(a.b).c`) types
    /// through the chain walk.
    ///
    /// The leg keys on the degenerate shape — one `CloseParen` token — and
    /// walks back to its matched `(`, so a span that already covers its
    /// own extent is left to the legs above. The walk visits the group's
    /// own tokens (groups nest, never overlap) and the recursion rides the
    /// same depth cap the chain and compound legs observe, so the face
    /// stays bounded.
    fn group_expression_face(&self, read_span: SourceRange) -> Option<Ty> {
        let tokens = self.tokens.as_slice();
        let first = tokens.partition_point(|t| t.utf16_offset < read_span.offset);
        let close = tokens.partition_point(|t| t.utf16_offset < read_span.end());
        if close != first + 1 || tokens[first].kind != dartq_scanner::TokenKind::CloseParen {
            return None;
        }
        let close = first;
        // The matched `(`: a depth walk over the three unambiguous bracket
        // families (`<`/`>` pair through the table but the raw kind is the
        // relational operator too, so they stay uncounted).
        let mut depth = 0usize;
        let mut open = None;
        for at in (0..close).rev() {
            match tokens[at].kind {
                dartq_scanner::TokenKind::CloseParen
                | dartq_scanner::TokenKind::CloseSquare
                | dartq_scanner::TokenKind::CloseCurly => depth += 1,
                dartq_scanner::TokenKind::OpenParen
                | dartq_scanner::TokenKind::OpenSquare
                | dartq_scanner::TokenKind::OpenCurly => match depth.checked_sub(1) {
                    Some(next) => depth = next,
                    None => {
                        open = Some(at);
                        break;
                    }
                },
                _ => {}
            }
        }
        let open = open?;
        if open + 1 >= close || self.tokens.matching_end(open) != Some(close) {
            return None;
        }
        let begin = tokens[open + 1].utf16_offset;
        let inner = SourceRange {
            offset: begin,
            length: tokens[close].utf16_offset.saturating_sub(begin),
        };
        if self.recv_walk_depth.get() >= RECV_WALK_DEPTH_CAP {
            return None;
        }
        self.recv_walk_depth.set(self.recv_walk_depth.get() + 1);
        let ty = self.recv_expression_ty(inner);
        self.recv_walk_depth.set(self.recv_walk_depth.get() - 1);
        ty.filter(|ty| *ty != Ty::UNKNOWN)
    }

    /// One arm's expression span for the [`Self::recv_expression_ty`]
    /// re-derivation (`lo..hi` are the tokens' table indices). A single
    /// token IS the read row — its own range is the staged facts' key. A
    /// composite arm anchors at the LAST token: the chain walk keys
    /// `member_read_ends` by the member token's end (and
    /// `index_result_recvs` by the `]`), so the tail token's range
    /// re-enters that chain without the FIRST token's read facts
    /// (`local_decl_at_read`, `rebound_read_face`) answering for the whole
    /// expression. A fully parenthesized arm — the nested `(...)`
    /// composite — keeps the whole extent so the separator scan
    /// re-enters. Leading/trailing `?`/`:` tokens a mis-paired
    /// conditional split leaves behind (`x is int? ? a : b`'s type
    /// suffix) trim first.
    fn arm_span(
        &self,
        tokens: &[dartq_scanner::Token],
        lo: usize,
    ) -> Option<SourceRange> {
        let mut front = 0usize;
        let mut len = tokens.len();
        while len > front
            && matches!(
                tokens[front].kind,
                dartq_scanner::TokenKind::Question | dartq_scanner::TokenKind::Colon
            )
        {
            front += 1;
        }
        while len > front
            && matches!(
                tokens[len - 1].kind,
                dartq_scanner::TokenKind::Question | dartq_scanner::TokenKind::Colon
            )
        {
            len -= 1;
        }
        if len <= front {
            return None;
        }
        let (first, last) = (&tokens[front], &tokens[len - 1]);
        let token_end = |token: &dartq_scanner::Token| {
            dartq_source::byte_offset_to_utf16(
                self.source,
                (token.byte_offset + token.byte_len) as usize,
            )
        };
        let offset = if len - front > 1 {
            // A pure `(...)` arm keeps the group's extent (the nested
            // composite re-enters the separator scan); every other
            // composite anchors at the tail token — the chain walk's
            // `member_read_ends`/`index_result_recvs` keys and the read
            // facts' span keys all hang there, never at the head token.
            let pure_group = first.kind == dartq_scanner::TokenKind::OpenParen
                && self.tokens.matching_end(lo + front) == Some(lo + len - 1);
            if pure_group {
                first.utf16_offset
            } else {
                last.utf16_offset
            }
        } else {
            first.utf16_offset
        };
        let end = token_end(last);
        Some(SourceRange {
            offset,
            length: end.saturating_sub(offset),
        })
    }

    /// The name-level `LUB(a, b)` a compound receiver's member verdict
    /// reads: subtype-related arms collapse to the wider side
    /// (`LUB(int, num) = num`), `Null` joins to the nullable partner
    /// (`LUB(Null, T) = T?`), `dynamic`/`void`/`Unknown` absorb the join,
    /// and an unordered pair keeps the LEFT arm — the under-report
    /// direction the member face prefers over a wrongly-named container
    /// (the real hierarchy's `LUB(int, double) = num` climb is a resolved
    /// back-end question the name level does not answer).
    fn join_branch_ty(&self, a: Option<Ty>, b: Option<Ty>) -> Option<Ty> {
        // An arm that types nothing is the oracle's InvalidType operand —
        // `LUB(InvalidType, X) = X` (least_upper_bound.dart:55): the join
        // lands on the spelled side, never sinks both.
        let (a, b) = match (a, b) {
            (None, other) | (other, None) => return other,
            (Some(a), Some(b)) => (a, b),
        };
        match (a, b) {
            (Ty::Unknown, _) => Some(b),
            (_, Ty::Unknown) => Some(a),
            (Ty::Never, _) => Some(b),
            (_, Ty::Never) => Some(a),
            (Ty::Dynamic, _) | (Ty::Void, _) => Some(a),
            (_, Ty::Dynamic) | (_, Ty::Void) => Some(b),
            (Ty::Null, other) | (other, Ty::Null) => Some(match other {
                Ty::Named {
                    name, args, ..
                } => Ty::Named {
                    name,
                    nullable: true,
                    args,
                },
                Ty::Record { shape, .. } => Ty::Record {
                    shape,
                    nullable: true,
                },
                Ty::Function { shape, .. } => Ty::Function {
                    shape,
                    nullable: true,
                },
                other => other,
            }),
            _ if a == b => Some(a),
            _ => {
                if self.assignable(a, b, false) == Some(true) {
                    Some(b)
                } else if self.assignable(b, a, false) == Some(true) {
                    Some(a)
                } else {
                    // The unordered pair joins at the root (`LUB(C1<int>,
                    // C1<String>) = Object` — the upstream interface-LUB's
                    // terminal climb, least_upper_bound.dart's common-
                    // supertypes walk): two unrelated class heads share
                    // `Object`, and a nullable arm joins to `Object?`. A
                    // shape/record/function arm has no shared head the
                    // name level can spell — the left arm stands (the
                    // under-report direction).
                    match (a, b) {
                        (
                            Ty::Named {
                                nullable: a_nullable,
                                ..
                            },
                            Ty::Named {
                                nullable: b_nullable,
                                ..
                            },
                        ) => {
                            let mut ty = Ty::named(self.intern("Object"));
                            if a_nullable || b_nullable {
                                ty = Self::nullable_form(ty);
                            }
                            Some(ty)
                        }
                        _ => Some(a),
                    }
                }
            }
        }
    }

    /// The staged member-read record whose member token ENDS the span — the
    /// chain node `read_span` names (`skip_bang`: past a trailing `!`).
    /// O(1) through [`Engine::member_read_ends`]; the old reverse scan
    /// cost O(N) per hop, quadratic over a whole chain walk.
    pub(crate) fn chain_node_read(
        &self,
        read_span: SourceRange,
        skip_bang: bool,
    ) -> Option<&MemberRead> {
        let mut end = read_span.offset + read_span.length;
        let src = self.source.as_bytes();
        while skip_bang && end > read_span.offset && src.get(end as usize - 1) == Some(&b'!') {
            end -= 1;
        }
        let read = self
            .member_read_ends
            .get(&end)
            .map(|&index| &self.member_reads[index as usize])?;
        (read.offset >= read_span.offset).then_some(read)
    }

    /// The `Receiver::Unknown` arm's declaration-shape leg: the bare read's
    /// own resolution (the `refs` publication — the push-time decision
    /// survives every fold) names the declaration whose STATIC type the
    /// receiver carries, and the flow/derived faces above cannot spell the
    /// shapes that name no container. A method read is a tear-off — its
    /// static type is the member's signature (`MethodElement.type`,
    /// upstream `method_invocation_resolver.dart`'s receiver-type
    /// computation), a bare top-level function the same, a local function
    /// its own declaration. `None` keeps the under-report: a plain local's
    /// own `var` face either arrived through the `BareInfo` payload (the
    /// push-time `closure_init_ty` re-derivation) or stays unsolvable here
    /// (the flow layer's declared slot staged `Unknown` and nothing else
    /// recorded the literal's type).
    fn receiver_decl_shape(&self, read_span: SourceRange) -> Option<Ty> {
        let reference = self
            .refs
            .iter()
            .find(|reference| reference.range.offset == read_span.offset)?;
        match reference.target {
            RefTarget::Member { container } => {
                match self.walk_members(container, reference.name) {
                    Walk::Found(entry) => self.member_entry_shape_ty(&entry),
                    _ => None,
                }
            }
            RefTarget::Decl { .. } => {
                let decl = self.top_level_decl(reference.name)?;
                if !matches!(decl.kind, MicroKind::TopLevelFn) {
                    return None;
                }
                let formals = decl.formals.as_deref()?;
                let sig = FnVarSig {
                    params: Box::default(),
                    required_positional: u16::try_from(
                        formals.positional.iter().filter(|f| f.required).count(),
                    )
                    .unwrap_or(u16::MAX),
                    positional: formals
                        .positional
                        .iter()
                        .map(|f| f.ty)
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                    named: formals
                        .named
                        .iter()
                        .map(|f| (f.name, f.ty, f.required))
                        .collect::<Vec<_>>()
                        .into_boxed_slice(),
                    return_ty: decl.return_ty,
                };
                Some(self.fn_sig_ty(&sig))
            }
            RefTarget::Local => {
                // The declaring local's name-token range keys
                // `local_decl_at_read` — the non-moving twin (`local_hits`
                // stack indices go stale with `close_frame_locals`'
                // truncate; the roster itself is gone by pass 2). The
                // name's own utf16 width lands the walk past the
                // declarator: a local function's `(` (its written
                // signature) or a `var`-inferred declarator's `=` (the
                // initializer face `closure_init_ty` reads).
                let decl_offset = *self.local_decl_at_read.get(&read_span.offset)?;
                let name_len = self.strings.resolve(reference.name).len() as u32;
                let bytes = self.source.as_bytes();
                let mut at = self.byte_of(decl_offset + name_len);
                while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
                    at += 1;
                }
                match bytes.get(at) {
                    // A local function: `ret name(` — the group's written
                    // formals ride `closure_init_ty`'s positional walk; the
                    // return column is the annotation ending at the name
                    // (absent → `dynamic`, the oracle's inferred face). A
                    // `(` immediately after the name token IS the
                    // function-declaration shape — a plain variable's
                    // declarator continues with `=`/`,`/`;`, never `(`.
                    Some(b'(') => {
                        let sig_ty = self.closure_init_ty(bytes, at);
                        let Ty::Function { shape, .. } = sig_ty else {
                            return None;
                        };
                        let (type_params, positional, named) = {
                            let pool = self.boundary.pool();
                            let shape = pool.function_shape(shape);
                            (
                                shape.type_params,
                                shape.positional.to_vec(),
                                shape.named.to_vec(),
                            )
                        };
                        // The written return annotation is the last
                        // type-use ending before the name — only
                        // whitespace separates it from the declarator
                        // (`int f(`: `int`'s row ends a space ahead of
                        // `f`). Its absence keeps the face
                        // `closure_init_ty` derived (the arrow body's own
                        // expression type, `dynamic` for a block — the
                        // oracle's uninferred-return posture).
                        let decl_byte = self.byte_of(decl_offset);
                        let Some(ret) = self
                            .type_uses
                            .iter()
                            .filter(|(range, _)| range.end() <= decl_offset)
                            .filter(|(range, _)| {
                                self.source.as_bytes()
                                    [self.byte_of(range.end())..decl_byte]
                                    .iter()
                                    .all(u8::is_ascii_whitespace)
                            })
                            .last()
                            .map(|(_, name)| Ty::named(*name))
                        else {
                            return Some(sig_ty);
                        };
                        Some(self.boundary.intern_function_ty(
                            type_params,
                            ret,
                            &positional,
                            &named,
                            false,
                        ))
                    }
                    Some(b'=') => {
                        let mut at = at + 1;
                        while bytes.get(at).is_some_and(|b| b.is_ascii_whitespace()) {
                            at += 1;
                        }
                        (bytes.get(at) == Some(&b'('))
                            .then(|| self.closure_init_ty(bytes, at))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The member entry's tear-off face as an interned type — the pass-2
    /// twin of [`Self::member_entry_tear_off_ty`]'s conversion (a method's
    /// signature as a function type, a getter its return, a field its
    /// declared type), kept local so the declaration-shape leg answers the
    /// receiver's static type without reopening the operand face.
    fn member_entry_shape_ty(&self, entry: &MemberEntry) -> Option<Ty> {
        match entry.kind {
            MemberKind::Method => {
                let formals = entry.formals.as_deref()?;
                let named: Vec<(StringId, Ty)> = formals
                    .named
                    .iter()
                    .map(|formal| (formal.name, self.fold_alias_ty(formal.ty)))
                    .collect();
                let positional: Vec<Ty> = formals
                    .positional
                    .iter()
                    .map(|formal| self.fold_alias_ty(formal.ty))
                    .collect();
                Some(self.boundary.intern_function_ty(
                    0,
                    self.fold_alias_ty(entry.return_ty),
                    &positional,
                    &named,
                    false,
                ))
            }
            MemberKind::Getter if entry.return_ty != Ty::Unknown => {
                Some(self.fold_alias_ty(entry.return_ty))
            }
            MemberKind::Field if entry.var_ty != Ty::Unknown => {
                Some(self.fold_alias_ty(entry.var_ty))
            }
            _ => None,
        }
    }

    /// A member token's UTF-16 length (the token-end arithmetic
    /// [`Engine::chain_node_read`] and the staged-join key share).
    pub(crate) fn member_name_units(&self, name: StringId) -> u32 {
        let name = self.strings.resolve(name);
        if name.is_ascii() {
            name.len() as u32
        } else {
            name.chars().map(char::len_utf16).sum::<usize>() as u32
        }
    }

    /// Whether a member read types as the oracle's `InvalidType`: its
    /// receiver's face is a head that resolves to no declaration, or the
    /// receiver is itself an invalid chain node or an invalid-initialized
    /// local (`type_property_resolver`'s invalid-bounded short-circuit
    /// types every read on an invalid receiver invalid). Each hop descends
    /// to a strictly earlier record, so the walk terminates.
    pub(crate) fn member_read_invalid(&self, read: &MemberRead) -> bool {
        if let Some(&decl) = self.local_decl_at_read.get(&read.recv_span.offset)
            && self.invalid_init_locals.borrow().contains(&decl)
        {
            return true;
        }
        let recv_ty = self
            .recv_expression_ty(read.recv_span)
            .or(match &read.recv {
                Receiver::Typed { ty, .. } => Some(*ty),
                _ => None,
            });
        match recv_ty {
            Some(Ty::Dynamic) => false,
            Some(ty @ Ty::Named { .. }) => self.is_invalid_ty(ty),
            _ => match self.chain_node_read(read.recv_span, true) {
                Some(prev) if prev.offset < read.offset => self.member_read_invalid(prev),
                _ => false,
            },
        }
    }

    /// The re-derived static type of one chained member node, keyed by its
    /// staged read record: the receiver re-reads through
    /// [`Self::recv_expression_ty`] — the flow face wins because the
    /// record's merge-time receiver carries the declared, unpromoted type
    /// — with the record's classified receiver as the flow-blind
    /// fallback, and the accessor face answers over the re-typed
    /// receiver — [`Self::receiver_static_ty`] on a synthetic row whose
    /// receiver carries the re-derived type, so the
    /// getter/field/tear-off/invocation distinctions stay in one place.
    fn chained_read_static_ty(&self, read: &MemberRead) -> Option<Ty> {
        // The re-derived face wins: the record's merge-time receiver carries
        // the UNPROMOTED static type (the promotion re-decision #1465 keeps
        // the declared face when the flow answers only in pass 2), so the
        // flow read at the receiver's own span — the same face the node's
        // own member verdict runs on — re-types it first. The record's
        // classified receiver remains the fallback for the shapes the flow
        // layer cannot spell (`this`, a created receiver).
        // The hop re-derives only when the receiver starts strictly before
        // the member token — the documented "strictly earlier span"
        // descent, now enforced rather than assumed. Recovery-staged
        // records can name their own member token as the receiver (`x.(…)`'s
        // degenerate `recv_span`, the anonymous-method cascade folds), and
        // without the guard the hop cycles on the same span forever
        // (issue 29943 / the anonymous-method cascades' stack overflow);
        // the classified receiver answers instead. The depth cap bounds
        // legitimately deep chains before they exhaust a worker thread's
        // stack — past the cap the record's classified receiver answers.
        let recv_ty = if read.recv_span.offset < read.offset
            && self.recv_walk_depth.get() < RECV_WALK_DEPTH_CAP
        {
            self.recv_walk_depth.set(self.recv_walk_depth.get() + 1);
            let ty = self.recv_expression_ty(read.recv_span);
            self.recv_walk_depth.set(self.recv_walk_depth.get() - 1);
            ty
        } else {
            None
        }
        .or(match &read.recv {
            Receiver::Typed { ty, .. } => Some(*ty),
            Receiver::This {
                container, kind, ..
            } => Some(self.this_static_ty(*container, *kind)),
            Receiver::Created {
                name,
                kind: ContainerKind::Class,
                ..
            } => Some(Ty::named(*name)),
            _ => None,
        })?;
        let Ty::Named { name, .. } = recv_ty else {
            return None;
        };
        let row = Row {
            name: read.name,
            range: SourceRange {
                offset: read.offset,
                length: 0,
            },
            qualified: true,
            shape: RowShape::Member {
                recv: Receiver::Typed {
                    type_name: name,
                    ty: recv_ty,
                },
            },
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        };
        let mut ty = self.receiver_static_ty(&row);
        if ty == Ty::UNKNOWN
            && let Some(ext) = self.extension_accessor_ty(recv_ty, read.name, false)
        {
            ty = ext;
        }
        // An invoked callee's chain node is the SEND, not the read: a
        // member token followed by `(` (the same byte probe the
        // redirect-edge face takes) makes the node's value the
        // implicit-`call` dispatch, so the send's static type is the
        // `call` method's return — never the accessor's own declared
        // type (`entry.key(context)` reads `XPathSequence` through
        // `XPathExpression.call`, not the `key` field's own class).
        let units = self.member_name_units(read.name);
        let invoked = self.byte_at(read.offset + units) == Some(b'(');
        // An applied extension METHOD's send types as its instantiated
        // return (`l.elementAtOrNull(0)` reads `Tk?`), no implicit call.
        if ty == Ty::UNKNOWN
            && invoked
            && let Some(ret) = self.extension_accessor_ty(recv_ty, read.name, true)
        {
            return Some(ret);
        }
        let ty = if invoked && !self.is_never_ty(ty) {
            match self.implicit_call_face(ty) {
                Some((_, call_ret, _)) => call_ret,
                None => ty,
            }
        } else {
            ty
        };
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// Marks a read-shaped member access under its resolved receiver
    /// handle (the `unused_field` element-identity face). An unresolved
    /// receiver marks nothing — the name-level `read_names` still feeds
    /// the use face.
    pub(crate) fn note_member_read(&mut self, name: StringId, recv: &Receiver) {
        let handle = match *recv {
            // `this`/`super` inside an extension body resolves against the
            // `on` type's instance face — `this`'s static type IS the
            // extended type, so the read credits the `on` container's
            // member, not the extension's. A `super` access is a plain
            // container (extensions declare no `super` member face). A
            // named-container `this` keeps its own container.
            Receiver::This { container, kind } if kind == ContainerKind::Extension => {
                match self.enclosing_extension_on_type(container) {
                    Some(on) => (self.file, on),
                    // The on-name never resolved — no honest owner.
                    None => return,
                }
            }
            Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                (self.file, container)
            }
            // `C.f` — a static-member read through the container's own name
            // (a `static const` field of a private class rides this face):
            // the receiver IS the owning container, no instance to type.
            Receiver::Container {
                name: container, ..
            } => (self.file, container),
            Receiver::Typed { type_name, .. } => {
                match resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    type_name,
                ) {
                    Some((_, owner)) => (owner, type_name),
                    None => return,
                }
            }
            _ => return,
        };
        self.member_read_owners
            .entry(name)
            .or_default()
            .insert(handle);
    }

    /// The read's owning container: the published push-time bare
    /// resolution (`RefTarget::Member` names the enclosing container — the
    /// frame stack is gone by pass 2), else the live container context.
    pub(crate) fn read_owner_container(&self, offset: u32) -> Option<StringId> {
        if let Some(found) = self.refs.iter().rev().find(|r| r.range.offset == offset)
            && let RefTarget::Member { container } = found.target
        {
            return Some(container);
        }
        self.container_context().map(|(container, _)| container)
    }

    /// Whether a read resolved against the container or any of its
    /// supertypes (`Sub`'s read of an inherited `_f` spares the base
    /// field; a sibling class's same-spelled field never does). The join
    /// walks each recorded receiver's supertype closure toward the
    /// declaring container — a `Sub`-typed receiver reading `Base._f`
    /// reaches `Base` through `Sub`'s own ancestry. Walking the
    /// declaration's supertypes instead would credit a base's read of its
    /// own member to a same-spelled subtype field (the inverse relation).
    pub(crate) fn member_was_read(&self, container: StringId, name: &StringId) -> bool {
        let Some(receivers) = self.member_read_owners.get(name) else {
            return false;
        };
        let decl = (self.file, container);
        if receivers.contains(&decl) {
            return true;
        }
        let alias_hop =
            |name: StringId, file: FileId, lib: LibraryId| self.alias_clause_hop(name, file, lib);
        let hierarchy = FoldHierarchy {
            graph: self.graph,
            namespaces: self.namespaces,
            index: self.index,
            file: self.file,
            lib: self.lib,
            class_modifiers: self.class_modifiers,
            memo: RefCell::default(),
            in_progress: RefCell::default(),
            alias_head: Some(&alias_hop),
        };
        receivers
            .iter()
            .any(|receiver| *receiver == decl || hierarchy.closure(*receiver).contains(&decl))
    }

    pub(crate) fn resolve_member_row(
        &mut self,
        promotions: &PromotionStaging,
        row: &Row,
        recv: Receiver,
    ) {
        if matches!(recv, Receiver::Unknown)
            && matches!(row.intent, Intent::Read | Intent::ReadWrite)
        {
            self.unresolved_member_rows
                .push((row.range.offset, row.name));
        }
        // Any member access uses the accessed name (the official `members`
        // set; unresolved reads land in `unresolvedReadMembers`, which the
        // field rule credits the same). Read-shaped accesses also join the
        // field-read index — writes never spare a field. A row that named
        // the enclosing executable itself never marks (the official
        // `_enclosingExec` exclusion).
        let self_named = self.self_scope_rows.contains(&row.range.offset);
        if !self_named {
            self.used_names.insert(row.name);
        }
        // A read intent credits the field's implicit getter (the official
        // `members` face); an `Intent::Operator` is the dot/index-merged
        // read the `[]` fold rewrites from `Read` (`x._m[k]`, `C.f[i]`) —
        // the operand still reads the member, so it credits the same.
        // `ReadWrite` is the compound `??=`/`+=` face (a read before the
        // write). A bare `Intent::Write` never reads the getter.
        let reads_getter = matches!(row.intent, Intent::Read | Intent::ReadWrite)
            || matches!(row.intent, Intent::Operator(..));
        if !self_named && reads_getter {
            self.read_names.insert(row.name);
            self.note_member_read(row.name, &recv);
        }
        let name_text = self.strings.resolve(row.name);
        // The synthetic selector skip: a zero-length member name is a
        // recovery-inserted identifier (`x.;`) — the oracle's resolver
        // skips the synthetic token, so no member face reports for it and
        // the miss only lands in the byproduct table.
        if Self::synthetic_reference(row.range) {
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        // The invalid-type container (scanner.dart's `scanner
        // .configuration` over a `fasta.Scanner` the failed
        // package:_fe_analyzer_shared import left): the oracle's
        // `InvalidType` receiver makes every member ask vacuous — the
        // walk's undefined trio never reports, only the type-position miss
        // beside it does. A container whose spelling resolves nowhere in
        // this scope (and is no compiler-known head) is exactly that
        // face.
        let container_face = match &recv {
            Receiver::Typed { type_name, .. } => Some(*type_name),
            Receiver::Container { name, .. } => Some(*name),
            Receiver::This { container, .. } => Some(*container),
            _ => None,
        };
        if let Some(container) = container_face
            && !self.is_scope_param(container)
            && self.param_bound(container).is_none()
            // The pass-2 bound window (`param_bound_of`): the scope/param
            // tables the two probes above read are torn down by the member
            // walk, so a `void f<T>(T t)` receiver spelled `T` reads as an
            // invalid container here. A name any captured bound window
            // declares IS a scope parameter — keep the member face (its own
            // bound walk decides), never the invalid-container bail.
            && self.param_bound_of(container).is_none()
            && !matches!(
                self.strings.resolve(container),
                "Symbol" | "Version" | "VersionConstraint"
            )
            && !crate::member_face::CORE_TYPE_MEMBERS
                .iter()
                .chain(crate::member_face::CORE_INTERFACE_MEMBERS.iter())
                .any(|(head, _)| *head == self.strings.resolve(container))
            && self.is_invalid_ty(Ty::named(container))
            // A potentially-nullable receiver keeps the member face
            // (#2815 R1): the scope-parameter and bound probes this bail
            // reads are parse-time tables — empty at pass 2 — so a `?`-
            // less parameter spelling or a nullable alias head would read
            // as an invalid container here. The nullable verdict's own
            // bound walk (the post-walk captures) decides; its false arm
            // keeps the InvalidType posture unchanged.
            && !self.potentially_nullable(match &recv {
                Receiver::Typed { ty, .. } => *ty,
                _ => Ty::UNKNOWN,
            })
        {
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        match recv {
            Receiver::Unknown => {
                // The type-literal receiver face runs BEFORE the
                // invalid-receiver one: a head that SPELLS a type —
                // `dynamic`, `Never`, or an ungrounded type alias —
                // denotes the `Type` INSTANCE even when the receiver
                // face never resolved, and that verdict owns the row
                // ([`Self::check_type_literal_receiver`] claims it; the
                // alias's INVOKE twin is the invocation walk's
                // `report_alias_selector_face`, which already emitted).
                // Without this order `Fn.new()` over
                // `typedef Fn = void Function()` paired the correct
                // `Type` row with this arm's `<unknown>` twin; probe
                // 3.13.7: `dynamic.new()` and `Never.new()` also name
                // `Type`, while `Undefined.new()` — no type literal —
                // keeps `<unknown>` below.
                if self.check_type_literal_receiver(row) {
                    return;
                }
                // The `new` selector face (`prefix.C.new` after the prefix
                // failed, cascade `..new()` over an unknown target): the
                // official property lookup answers `needsGetterError`
                // unconditionally for the name 'new' — before the
                // dynamic/invalid-receiver silence
                // (type_property_resolver.dart:78-83) — so the read reports
                // `undefined_getter`, the invocation `undefined_method`,
                // against the InvalidType the failed receiver left.
                if row.name == self.intern("new") {
                    let code = match row.intent {
                        Intent::Invoke => SemanticCode::UndefinedMethod,
                        Intent::Write | Intent::ReadWrite => SemanticCode::UndefinedSetter,
                        _ => SemanticCode::UndefinedGetter,
                    };
                    // The member family on an invalid receiver is
                    // guard-free like every member face (the oracle's
                    // `_isIgnored` consults `shouldIgnoreUndefined` only on
                    // the unqualified/prefixed *name* domains — an
                    // instance-member miss never reaches it). The display
                    // splits by intent: the invocation face names the
                    // invocation resolver's `<unknown>` receiverClassName
                    // (method_invocation_resolver.dart — only an
                    // `InterfaceType`/`FunctionType` receiver escapes the
                    // default), the read/write faces name the `InvalidType`
                    // the failed lookup left (probe: `Undefined.new()`
                    // reports `<unknown>`, `var x = Undefined.new` reports
                    // `InvalidType`).
                    let type_text = if row.intent == Intent::Invoke {
                        "<unknown>"
                    } else {
                        "InvalidType"
                    };
                    self.emit(code, row.range, name_text, type_text);
                    self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                    return;
                }
                // The cascade-section face (`f()..g()`): the official walk
                // answers the section send itself, so the discarded value
                // reports at the selector when the name carries the face —
                // with the target type unknown to the model, the section
                // resolves by scope (the top-level site). Other unknown
                // receivers keep the under-report (`dyn.foo()` dispatches
                // dynamically, no element to check).
                if !self_named
                    && matches!(row.intent, Intent::Invoke | Intent::Read)
                    && self.cascade_sections.contains(&row.range.offset)
                {
                    self.check_top_level_unused_result(row);
                }
                // The inferred-slot receiver face (#2346①): a receiver
                // whose declared slot the name-level model never bound —
                // the for-in loop variable (`for (var e in xs)`'s `e`
                // carries the iterable's element type, upstream's
                // `_inferForEachVariable`), a pattern destructure the
                // flat-record bind declined — still owns a static type:
                // the flow layer's read-type row at the receiver's own
                // span (the same face the promoted-receiver re-decision
                // #1465 reads). A named flow face runs the member verdict
                // on its container's instance face, the flow type's own
                // nullability riding the receiver verdict — a nullable
                // face emits the shared unchecked-access code at the
                // found/miss gates (`type_property_resolver`'s
                // `isNullable` tail precedes every member resolution), a
                // non-null face the `undefined_*` trio. Every other face
                // — a dynamic dispatch, an unresolvable receiver, a
                // diverging repeat read — keeps the under-report.
                if !self_named
                    && let Some(read_span) = self
                        .member_reads
                        .binary_search_by(|read| read.offset.cmp(&row.range.offset))
                        .ok()
                        .map(|index| self.member_reads[index].recv_span)
                {
                    // The type-literal head (`dynamic`/`Never` spelled as
                    // the receiver, an ungrounded alias): its answer is the
                    // `Type` instance's face, and the flow faces below
                    // never see a type token's read — the check runs
                    // before them.
                    if self.check_type_literal_receiver(row) {
                        return;
                    }
                    match self
                        .rebound_local_receiver(row.range.offset, &recv)
                        .or_else(|| self.recv_expression_ty(read_span))
                        // A COMPLETED SEND's span is the call's RESULT
                        // expression, never a function reference: the
                        // `refs` entry at that offset is the bare CALLEE
                        // identifier, and
                        // [`Self::receiver_decl_shape`] answers its
                        // tear-off face. `rawDyn(1).m` would then read
                        // `rawDyn`'s function type as the receiver and
                        // report the miss against a `Function` receiver,
                        // where the oracle types the *result* (an
                        // unannotated or `dynamic` callee returns
                        // `Unknown`/`dynamic` and the access is silent).
                        // `call_ends` records one span per completed send
                        // (callee offset -> closing `)`), the same offset
                        // key [`Self::invocation_span`] reads, so the gate
                        // is exact and covers all three declaration arms
                        // (`Decl`/`Member`/`Local`). The send's own result
                        // face is answered by [`Self::recv_expression_ty`]'s
                        // value legs above; a declaration shape is the
                        // callee's SIGNATURE, so a completed send has no
                        // declaration-shape face at all (an untyped or
                        // parameter-headed return would otherwise leak a
                        // `Function`/unresolved head into the receiver).
                        .or_else(|| {
                            if self
                                .call_ends
                                .binary_search_by_key(&read_span.offset, |(offset, _)| *offset)
                                .is_ok()
                            {
                                return None;
                            }
                            self.receiver_decl_shape(read_span)
                        })
                    {
                        Some(
                            face @ Ty::Named {
                                name: flow_head, ..
                            },
                        ) => {
                            self.check_instance_member(
                                promotions,
                                row,
                                Receiver::Typed {
                                    type_name: flow_head,
                                    ty: face,
                                },
                            );
                            return;
                        }
                        // A shape face has no container head for the
                        // instance-member walk: the member verdict runs
                        // upstream's non-interface arm whole — `call` and a
                        // record's own fields resolve first, the extension
                        // application then the `Object` interface, a clean
                        // miss reporting the access-intent trio
                        // (`type_property_resolver.dart:202-234`). A
                        // NULLABLE face dereferences unconditionally first
                        // (#2815 R1c — a `?`-suffixed shape reports the
                        // shared unchecked code at the selector; an
                        // `Object` member stays exempt).
                        Some(face @ (Ty::Record { .. } | Ty::Function { .. })) => {
                            // The type-literal head (the `dynamic` keyword or
                            // an ungrounded alias spelling) denotes the
                            // `Type` instance, never a value of the shape.
                            if self.check_type_literal_receiver(row) {
                                return;
                            }
                            let receiver_nullable = match face {
                                Ty::Record { nullable, .. }
                                | Ty::Function { nullable, .. } => {
                                    nullable
                                        && !self
                                            .null_aware_members
                                            .contains(&row.range.offset)
                                }
                                _ => false,
                            };
                            let display = self.render_ty(face);
                            self.check_shape_receiver(
                                row,
                                face,
                                &display,
                                receiver_nullable,
                            );
                            return;
                        }
                        _ => {}
                    }
                }
                // The record-receiver extension face: a receiver whose
                // static type is a RECORD (a `(BoxedT,)?` sentinel, a
                // record typedef's instance) names no container — the flow
                // lattice cannot spell records either, so the parse-time
                // side table (`record_recv_tys`) is the only holder of the
                // face. The shape application credits the declaring
                // extension's member (the resolution itself stays silent,
                // matching the oracle's resolved access). A NULLABLE record
                // receiver dereferences unconditionally instead (#2815 R1c
                // — `(int, String)? r; r.foo(0)` reports the shared
                // unchecked code at the selector: a non-nullable `on (int,
                // String)` face never applies to the nullable spelling),
                // an `Object` member excepted (the nullable branch's first
                // lookup hits it).
                if !self_named
                    && matches!(
                        row.intent,
                        Intent::Invoke | Intent::Read | Intent::Write | Intent::ReadWrite
                    )
                    && let Some(&(_, record)) = self
                        .record_recv_tys
                        .iter()
                        .find(|(offset, _)| *offset == row.range.offset)
                {
                    // The side-table shape face (records AND function
                    // types, invocation.rs:857-867) runs the same member
                    // verdict as the flow-derived one — a miss reports the
                    // access-intent trio, never the silent credit alone.
                    let receiver_nullable = match record {
                        Ty::Record { nullable, .. } | Ty::Function { nullable, .. } => {
                            nullable && !self.null_aware_members.contains(&row.range.offset)
                        }
                        _ => false,
                    };
                    // The type-literal head (the `dynamic` keyword or an
                    // ungrounded alias spelling) denotes the `Type`
                    // instance, never a value of the shape.
                    if self.check_type_literal_receiver(row) {
                        return;
                    }
                    let display = self.render_ty(record);
                    self.check_shape_receiver(row, record, &display, receiver_nullable);
                    return;
                }
            }
            Receiver::Prefix { name: prefix } => {
                // `lazy.loadLibrary()` is the deferred-load runtime face
                // the oracle synthesizes on a `deferred as` prefix — it
                // joins no declared table, so the prefixed hit probe
                // misses it by construction. Skip it here (upstream
                // `DeferredLoad` synthesis: every deferred prefix carries
                // a `loadLibrary` function returning `Future<void>`).
                if row.name == self.intern("loadLibrary")
                    && row.intent == Intent::Invoke
                    && self
                        .graph
                        .import_edges(self.lib)
                        .iter()
                        .any(|edge| edge.deferred && edge.prefix == Some(prefix))
                {
                    return;
                }
                if self.prefixed_hit(prefix, row.name) || self.guard.ignore(Some(prefix), row.name)
                {
                    return;
                }
                match row.intent {
                    // Verified: a prefixed *invocation* miss is
                    // `undefined_function`; the property path reports
                    // `undefined_prefixed_name` (PrefixedLookupDomain).
                    Intent::Invoke => {
                        self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        self.record_miss(row.name, row.range, true, NameUse::Invoke);
                    }
                    Intent::Read | Intent::Write | Intent::ReadWrite => {
                        let type_text = self.strings.resolve(prefix).to_string();
                        self.emit(
                            SemanticCode::UndefinedPrefixedName,
                            row.range,
                            name_text,
                            &type_text,
                        );
                        let use_kind = match row.intent {
                            Intent::Write | Intent::ReadWrite => NameUse::Write,
                            _ => NameUse::Read,
                        };
                        self.record_miss(row.name, row.range, true, use_kind);
                    }
                    Intent::Operator(..) => {
                        self.record_miss(row.name, row.range, true, NameUse::Read);
                    }
                }
            }
            Receiver::Container { name, kind } => {
                // A cascade section's receiver is the target's VALUE: for a
                // type-literal target (`C..foo()`) that value is the `Type`
                // instance, so the section resolves an INSTANCE lookup on
                // `Type` — the container's own statics never answer
                // (`C..bar()` over C's own `static void bar()` reports
                // `undefined_method` naming `Type`, probed 3.13.7; reads
                // name it `undefined_getter`, writes `undefined_setter`),
                // while the `Object` face (`C..toString()`) resolves and
                // stays silent. The static-container face never runs for a
                // section.
                if self.cascade_sections.contains(&row.range.offset) {
                    // Only a bare type-literal target (`C..foo()`) carries
                    // the `Type` instance as its VALUE. A member or send
                    // target (`C.new..foo()`, `Future.value(x)..foo()`,
                    // `C.staticField..foo()`) denotes THAT expression's
                    // value ([`Self::cascade_target_is_bare_type_literal`]):
                    // a selector continuation's face is the member's own —
                    // the constructor/method tear-off reads `Function`
                    // (`C.new..gone()` reports `undefined_method` naming
                    // `Function`, probed 3.13.7), the static field or
                    // getter reads its declared type (`C.sg..gone()` names
                    // `int`) — and a `)`/`]`-ended target keeps the arm's
                    // under-report (its send/index fold owns the face
                    // elsewhere).
                    if self.cascade_target_is_bare_type_literal(row.range) {
                        let spelling = if matches!(row.intent, Intent::Write | Intent::ReadWrite) {
                            self.setter_spelling(row.name)
                        } else {
                            row.name
                        };
                        if !self.type_literal_face_resolves(spelling) {
                            let name_text = self.strings.resolve(row.name);
                            match row.intent {
                                Intent::Invoke => self.emit(
                                    SemanticCode::UndefinedMethod,
                                    row.range,
                                    name_text,
                                    "Type",
                                ),
                                Intent::Read => self.emit(
                                    SemanticCode::UndefinedGetter,
                                    row.range,
                                    name_text,
                                    "Type",
                                ),
                                Intent::Write | Intent::ReadWrite => self.emit(
                                    SemanticCode::UndefinedSetter,
                                    row.range,
                                    name_text,
                                    "Type",
                                ),
                                Intent::Operator(..) => {}
                            }
                        }
                    } else if let Some((face, head)) =
                        self.cascade_selector_target_ty(row.range, name)
                    {
                        self.check_instance_member(
                            promotions,
                            row,
                            Receiver::Typed {
                                type_name: head,
                                ty: face,
                            },
                        );
                        return;
                    }
                    self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                    return;
                }
                // An instantiated type literal's method send (`C<T>.sm<T>()`)
                // is a `MethodInvocation` on the `Type` instance, never the
                // container's static face — see
                // [`Self::check_instantiated_literal_member`].
                if self.check_instantiated_literal_member(row) {
                    return;
                }
                self.check_container_member(row, name, kind);
            }
            // The prefix-resolved container (`io.Directory.gone`): the
            // member verdict runs on the foreign container's static face —
            // the bare-scope `resolve_container` cannot reach it (the name
            // sits behind the prefix table), so the check re-resolves
            // through [`prefixed_container`] directly.
            Receiver::PrefixedContainer { prefix, name, kind } => {
                if let Some((resolved, owner)) = prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    name,
                ) {
                    self.container_member_verdict(row, resolved, owner, name, kind);
                }
            }
            // A folded creation: class-shaped results resolve on the
            // instance face (superclass walk, `Object` members); an
            // extension override resolves against the extension's members —
            // its `on` type's `Object` face is the under-report.
            Receiver::Created { name, kind, .. } => {
                if kind == ContainerKind::Extension {
                    if self.members.object_members.contains(&row.name)
                        || row.name == self.intern("==")
                    {
                        return;
                    }
                    self.check_container_member(row, name, kind);
                } else {
                    self.check_instance_member(
                        promotions,
                        row,
                        Receiver::Typed {
                            type_name: name,
                            ty: Ty::UNKNOWN,
                        },
                    );
                }
            }
            Receiver::This { .. } | Receiver::Typed { .. } => {
                self.check_instance_member(promotions, row, recv);
            }
            Receiver::Super { container, kind } => {
                self.check_super_member(row, container, kind);
            }
        }
    }

    pub(crate) fn prefixed_hit(&self, prefix: StringId, name: StringId) -> bool {
        if !matches!(
            self.namespaces
                .lookup_prefixed(self.graph, self.lib, prefix, name),
            dartq_resolution::Lookup::NotFound
        ) {
            return true;
        }
        // `p.dynamic`: the spelled name is the `dynamic` keyword, which the
        // export scope never carries (dart:core's `DynamicElementImpl` is
        // synthetic — `seed_implicit_core` declares only the named faces),
        // so the scoped probe always misses. Upstream resolves it through
        // the prefixed core namespace the same way `myCore.dynamic` does
        // (`named_type_resolver.dart:85-121`'s prefix path): a prefix whose
        // own import edge targets `dart:core` and whose combinators admit
        // the name resolves `dynamic` (a `hide dynamic` clause or a prefix
        // bound to a non-core library does not).
        name == self.intern("dynamic") && self.prefix_exports_dynamic(prefix)
    }

    /// Whether `prefix` owns an import edge whose combinators admit the
    /// `dynamic` name into the dart:core synthetic face — the
    /// `p.dynamic`/`myCore.dynamic` prefixed-type resolution the scoped
    /// probe cannot answer (the name never registers in any export scope).
    /// A `show dynamic` clause or a `hide` omission both count, exactly the
    /// oracle's `PrefixScope` filtering over the same name.
    fn prefix_exports_dynamic(&self, prefix: StringId) -> bool {
        let dynamic = self.intern("dynamic");
        self.graph
            .import_edges(self.lib)
            .iter()
            .filter(|edge| edge.prefix == Some(prefix))
            .filter(|edge| edge.combinators.allows(dynamic))
            .any(|edge| {
                edge.target.is_some_and(|target| {
                    self.graph.uri(target) == "dart:core"
                        || self.graph.exported(target, dynamic).is_some()
                })
            })
    }

    /// The whole written qualified name's span (`p.C`): the prefix token's
    /// start through the simple name's end — `_getErrorRange`'s default
    /// range (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
    /// named_type_resolver.dart:731-747). The prefix token opens it when
    /// one is written, `node.name.end` closes it regardless of type
    /// arguments; a recovery shape without the `.` keeps the name's own
    /// span.
    fn qualified_name_range(&self, name_range: SourceRange) -> SourceRange {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < name_range.offset);
        if at >= 2 && tokens[at - 1].kind == dartq_scanner::TokenKind::Period {
            let prefix = &tokens[at - 2];
            return SourceRange {
                offset: prefix.utf16_offset,
                length: name_range.end() - prefix.utf16_offset,
            };
        }
        name_range
    }

    /// Static/enum/extension access on a container name (`C.m`, `E.x`,
    /// `Ext.g`) — the oracle's `StaticLookupDomain`
    /// (`lookup_failure_reporter.dart`): the interface face reports
    /// `undefined_method` (the type-literal shape shares the registry name
    /// and message), `undefined_getter`, `undefined_enum_constant` on enum
    /// reads, and `undefined_setter` on writes; the extension face reports
    /// its dedicated trio. Operator intents have no static-face shape the
    /// M1 model can name — record only. External containers suppress.
    pub(crate) fn check_container_member(
        &mut self,
        row: &Row,
        container: StringId,
        kind: ContainerKind,
    ) {
        let Some((resolved, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        ) else {
            return;
        };
        self.container_member_verdict(row, resolved, owner, container, kind);
    }

    /// [`Engine::check_container_member`] over an already-resolved
    /// container — the resolved-prefix arm (`io.Directory.gone`) holds its
    /// `prefixed_container` hit, which the bare-scope `resolve_container`
    /// above cannot reproduce (the name lives behind the prefix, not in
    /// the diagnosing library's scope). `owner` is the resolved
    /// container's declaring file (the reference edge target, #2618).
    pub(crate) fn container_member_verdict(
        &mut self,
        row: &Row,
        resolved: &Container,
        owner: FileId,
        container: StringId,
        kind: ContainerKind,
    ) {
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);

        // A named-CONSTRUCTOR reference on the class head (`$Result.error`
        // — the implicit creation `C.ctor(...)`, a redirecting factory
        // included) is not a static member access: the constructor-
        // reference dispatch owns the verdict, and the getter/method trio
        // never fires (the use credits the constructor's name the same
        // `check_enum_constructor_selector` credit carries).
        if !write
            && matches!(
                resolved.kind,
                ContainerKind::Class | ContainerKind::ExtensionType
            )
            && resolved.constructors.contains(&row.name)
        {
            self.used_names.insert(row.name);
            self.record_miss(row.name, row.range, true, NameUse::Read);
            return;
        }
        // Write intent resolves through the SETTER spelling plus a
        // non-final field's implicit setter — a getter under the base
        // name never satisfies a write (`Ext(item).g = x` on a
        // getter-only `g` reports `undefined_extension_setter`, the
        // same verdict the class-face `undefined_setter` shares).
        let defined = if write {
            resolved
                .members
                .contains_key(&self.setter_spelling(row.name))
                || resolved
                    .members
                    .get(&row.name)
                    .is_some_and(|entry| matches!(entry.kind, MemberKind::Field) && !entry.is_final)
        } else {
            resolved.members.contains_key(&row.name)
        };
        // The row's `kind` is the receiver's push-time spelling — the
        // micro tag conflates `extension` and `extension type` into
        // `ContainerKind::Extension`. The verdict keys on the RESOLVED
        // container's kind instead (upstream `StaticLookupDomain`'s element
        // switch, analyzer source `lookup_failure_reporter.dart`:437-463 —
        // `ExtensionElementImpl` alone takes the extension trio; an
        // `ExtensionTypeElement` is an `InterfaceElement` and reports the
        // class-family `undefined_method`/`_getter`/`_setter` "for the
        // type").
        let resolved_kind = resolved.kind;
        // An enum's synthesized STATIC `values` getter joins the
        // interface face without an entry in `members` — a static read
        // through the container's own name resolves the accessor
        // upstream (`EnumElementImpl`'s synthesized static members; the
        // `index`/`name` synth twins are instance members and keep the
        // miss at this face).
        if !defined
            && resolved_kind == ContainerKind::Enum
            && row.name == self.members.enum_synth_members[0]
        {
            return;
        }
        if !defined
            && (resolved.constructors.contains(&row.name)
                || (row.name == self.intern("new")
                    && resolved_kind != ContainerKind::Extension
                    && resolved.has_unnamed_constructor))
        {
            // A named-constructor selector (`C.named`) resolves through the
            // creation face — the constructor is the class's own
            // declaration, never an undefined member. The `.new` spelling
            // answers the unnamed constructor the same way
            // (ast_rewrite.dart's `name.lexeme == 'new'` →
            // `unnamedConstructor`); extensions have no constructors — an
            // extension TYPE does (its primary and named forms), so the
            // `Extension` exclusion is the resolved kind's, not the row's.
            self.check_member_unused_result(row, container, &Receiver::Unknown, true);
            return;
        }
        // The assignment-to-* family (#3282): the static write's setter
        // lookup missed while the plain name declares a getter-backed face —
        // a `final` static field (`assignment_to_final`) or an explicit
        // getter (`assignment_to_final_no_setter`, naming the DECLARING
        // container the resolved face projects from). The row is claimed
        // here, so the `undefined_setter` tail below never sees it.
        if write
            && !defined
            && let Some(entry) = resolved.members.get(&row.name)
        {
            let (kind, is_final, name_range) = (entry.kind, entry.is_final, entry.name_range);
            if self.report_setter_less_write(row, kind, is_final, name_range, Some((owner, container)))
            {
                return;
            }
        }
        // The increment twin on a static face (`C.x++`), #3282: the operand
        // assigns the member its plain spelling resolves.
        if !write
            && self.increment_operand(row)
            && let Some(entry) = resolved.members.get(&row.name)
            && (entry.kind == MemberKind::Getter
                || (entry.kind == MemberKind::Field && entry.is_final))
        {
            let (kind, is_final, name_range) = (entry.kind, entry.is_final, entry.name_range);
            self.report_setter_less_write(row, kind, is_final, name_range, Some((owner, container)));
        }
        if defined {
            // The compound-write read face: a write satisfied through the
            // SETTER spelling alone still reports the getter half's miss —
            // the oracle resolves the read first (`C.foo += 1` over a
            // `static set foo` is `undefined_getter` naming the container,
            // UndefinedGetterTest test_compoundAssignment_hasSetter_static;
            // the miss arm below shares this per-face split).
            if row.intent == Intent::ReadWrite && !resolved.members.contains_key(&row.name) {
                let name_text = self.strings.resolve(row.name);
                let type_text = self.strings.resolve(container).to_string();
                let read_code = if resolved_kind == ContainerKind::Extension {
                    SemanticCode::UndefinedExtensionGetter
                } else if resolved_kind == ContainerKind::Enum {
                    SemanticCode::UndefinedEnumConstant
                } else {
                    SemanticCode::UndefinedGetter
                };
                self.emit(read_code, row.range, name_text, &type_text);
            }
            // The static/extension face resolved (#2618): the edge targets
            // the container's own declaration — statics resolve on the
            // declaring container only, and the write's satisfying arm
            // (setter or non-final field) picks the lookup spelling.
            let setter = self.setter_spelling(row.name);
            let spelling = if write && resolved.members.contains_key(&setter) {
                setter
            } else {
                row.name
            };
            let entry_kind = resolved
                .members
                .get(&spelling)
                .map_or(MemberKind::Method, |entry| entry.kind);
            let ref_kind = Self::member_ref_kind(&row.intent, entry_kind);
            self.note_member_ref(row, ref_kind, Some((owner, spelling, container)));
            self.check_member_unused_result(
                row,
                container,
                &Receiver::Container {
                    name: container,
                    kind,
                },
                false,
            );
            return;
        }
        let name_text = self.strings.resolve(row.name);
        let type_text = self.strings.resolve(container).to_string();
        // A compound write checks both faces (the oracle resolves the read
        // first): the read-face text, then the setter text.
        let read_face_missing = !resolved.members.contains_key(&row.name);
        let (invoke_code, read_code, write_code) = match resolved_kind {
            ContainerKind::Extension => (
                SemanticCode::UndefinedExtensionMethod,
                SemanticCode::UndefinedExtensionGetter,
                SemanticCode::UndefinedExtensionSetter,
            ),
            _ => (
                SemanticCode::UndefinedMethod,
                if resolved_kind == ContainerKind::Enum {
                    SemanticCode::UndefinedEnumConstant
                } else {
                    SemanticCode::UndefinedGetter
                },
                SemanticCode::UndefinedSetter,
            ),
        };
        match row.intent {
            Intent::Invoke => self.emit(invoke_code, row.range, name_text, &type_text),
            Intent::Read => self.emit(read_code, row.range, name_text, &type_text),
            Intent::Write => self.emit(write_code, row.range, name_text, &type_text),
            Intent::ReadWrite => {
                if read_face_missing {
                    self.emit(read_code, row.range, name_text, &type_text);
                }
                self.emit(write_code, row.range, name_text, &type_text);
            }
            Intent::Operator(..) => {}
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// Whether the read at `offset` names a MEMBER access — a merged
    /// `receiver.member` read ([`Engine::member_reads`]) or a bare read
    /// published as [`RefTarget::Member`] (an implicit-this field). The
    /// promotion-eligibility guard reads it (#2815 R3): the flow layer
    /// resolves by name and can stage a same-named variable's promotion
    /// row at a selector span, but a member access's nullability verdict
    /// reads its own accessor's promotion alone (the promotable-field
    /// substitution decides eligibility; upstream reads the condition's
    /// resolved static type through the accessor element, never a shadow
    /// variable — `bool_expression_verifier.dart:44-73`'s member gate).
    fn read_offset_is_member(&self, offset: u32) -> bool {
        if self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&offset))
            .is_ok()
        {
            return true;
        }
        self.refs
            .binary_search_by(|published| published.range.offset.cmp(&offset))
            .is_ok_and(|at| {
                self.refs[at].range.offset == offset
                    && matches!(self.refs[at].target, RefTarget::Member { .. })
            })
    }

    /// The `Type` instance's member face: `Object`'s members plus every
    /// member an ACCESSIBLE extension declares on a top-shaped `on` type.
    ///
    /// A type-literal receiver's static type IS `Type` (upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/ast/ast.dart
    /// `TypeLiteral.staticType`), and the oracle resolves the member with
    /// the ordinary property/invocation machinery — `Type`'s own interface
    /// first, then every applicable extension of the whole import closure
    /// (`extension_member_resolver.dart` `_getApplicable`). Reading
    /// `Object`'s members alone reported `undefined_*` naming `Type` for
    /// accesses an IMPORTED `extension E on Object?` resolves (the
    /// `syntax_helper.dart` `expectStaticType` family), and for operators
    /// the same gap reported `undefined_operator`.
    ///
    /// Only a face whose `on` head the `Type` receiver reaches suppresses:
    /// `Object`/`Object?`, `dynamic`, `Type` itself, and an unbounded own
    /// parameter (`on T`). A concrete `on` type declares the member for ITS
    /// receivers and leaves the literal's miss standing.
    fn type_literal_face_resolves(&mut self, member: StringId) -> bool {
        if member == self.intern("==") || self.members.object_members.contains(&member) {
            return true;
        }
        self.accessible_extension_faces();
        let boundary = &self.boundary;
        let object = self.intern("Object");
        let type_head = self.intern("Type");
        let applies = self
            .accessible_extensions
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .any(|(face, face_file)| {
                // A `static` extension member never answers an instance
                // access (`extension_member_resolver` instantiates the
                // extension and looks the member up in the instance face).
                if !face
                    .members
                    .get(&member)
                    .is_some_and(|entry| !entry.is_static)
                {
                    return false;
                }
                match boundary.ty(*face_file, face.on_ty) {
                    // `on dynamic`/`on O` with an unmodelled `O`: the
                    // under-report direction.
                    Ty::Dynamic | Ty::Unknown => true,
                    Ty::Named { name, .. } => {
                        name == object
                            || name == type_head
                            || face
                                .type_param_names
                                .iter()
                                .position(|&param| param == name)
                                .is_some_and(|index| {
                                    matches!(
                                        face.type_param_bounds.get(index).copied(),
                                        None | Some(Ty::Dynamic) | Some(Ty::Unknown)
                                    )
                                })
                    }
                    _ => true,
                }
            });
        // The application probe leaves its reference latch behind
        // ([`Engine::extension_application`]'s contract); this face answers
        // only whether the member resolves, so the latch must not leak to
        // the next real application.
        self.members.ext_ref.set(None);
        applies
    }

    /// The interface a type alias names (`typedef A = C;` → `C`) when the
    /// right-hand side is a NAMED type this compilation grounds.
    ///
    /// Upstream resolves an interface-typed alias head exactly like the
    /// class it names — `_resolveTargetInterfaceElement`
    /// (pkg/analyzer/lib/src/dart/resolver/property_element_resolver.dart:
    /// 507-517) and `_resolveReceiverTypeLiteral`
    /// (method_invocation_resolver.dart:168-179) both hand the ALIASED
    /// element to the static face — so such a head is never the `Type`
    /// literal. A function/record right-hand side has no interface and
    /// keeps the literal face; a named one this pass cannot ground (a
    /// foreign alias whose private target is not in the diagnosing scope)
    /// answers `None` and keeps the under-report.
    fn alias_interface_head(&self, alias: StringId) -> Option<(&'a Container, FileId, StringId)> {
        let (_, tree) = self.bounds.alias_named_shape(alias)?;
        let head = tree.head;
        let (container, owner) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            head,
        )?;
        Some((container, owner, head))
    }

    /// Whether a cascade section's target is the BARE type literal itself —
    /// the only target shape whose value is the `Type` instance.
    ///
    /// `C..foo()` writes `C`, `..`, `foo`: the target's last token is an
    /// identifier with no member continuation before it. `C.new..foo()`,
    /// `C.staticField..foo()`, and `Future.value(x)..foo()` all continue
    /// past a `.` (or end in `)`/`]`), so the section's receiver is THAT
    /// expression's value, never the literal — the Type-instance verdict
    /// does not apply (`Future.value(C())..expectStaticType(…)` resolves
    /// the extension member on the `Future` instance).
    fn cascade_target_is_bare_type_literal(&self, selector: SourceRange) -> bool {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < selector.offset);
        // `tokens[at - 1]` is the section's `..`, `tokens[at - 2]` the
        // target's last token (comments are trivia, never tokens).
        if at < 2
            || !matches!(
                tokens[at - 1].kind,
                dartq_scanner::TokenKind::PeriodPeriod
                    | dartq_scanner::TokenKind::QuestionPeriodPeriod
            )
            || tokens[at - 2].kind != dartq_scanner::TokenKind::Identifier
        {
            return false;
        }
        !(at >= 3
            && matches!(
                tokens[at - 3].kind,
                dartq_scanner::TokenKind::Period | dartq_scanner::TokenKind::QuestionPeriod
            ))
    }

    /// The cascade target's VALUE face when the target continues a selector
    /// past a `.`/`?.` (`C.new..foo()`, `C.named..foo()`, `C.sm..foo()`,
    /// `C.sg..foo()`): upstream `CascadeExpression.target` is the whole
    /// `C.sel` expression, whose `staticType` is the selector's own — a
    /// constructor or static-method tear-off types `FunctionType` (the
    /// section's member verdict runs on the `Function` face: `call` and the
    /// `Object` members resolve, a miss names `Function`), and a static
    /// field/getter types its declared type. `None` for every other
    /// continuation (an instance member — its own access face reports; a
    /// miss on the head — the selector's own row reports; a `)`/`]`-ended
    /// target — its send/index fold's face lives on a different row).
    ///
    /// Returns `(face, head)`: `face` is the receiver's static type,
    /// `head` the container spelling the member verdict keys on (`Function`
    /// for the tear-off face — `type_property_resolver`'s
    /// `isDartCoreFunction` arm resolves `call` and reports the trio
    /// against `Function` itself).
    fn cascade_selector_target_ty(
        &self,
        selector: SourceRange,
        container: StringId,
    ) -> Option<(Ty, StringId)> {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < selector.offset);
        // `tokens[at - 1]` is the section's `..`, `tokens[at - 2]` the
        // target's last token, `tokens[at - 3]` the selector `.`/`?.` that
        // makes the target non-bare (a `)`/`]` end or a bare head never
        // reaches here — the call sites gate the same pair).
        if at < 3
            || !matches!(
                tokens[at - 1].kind,
                dartq_scanner::TokenKind::PeriodPeriod
                    | dartq_scanner::TokenKind::QuestionPeriodPeriod
            )
            || tokens[at - 2].kind != dartq_scanner::TokenKind::Identifier
            || !matches!(
                tokens[at - 3].kind,
                dartq_scanner::TokenKind::Period | dartq_scanner::TokenKind::QuestionPeriod
            )
        {
            return None;
        }
        let member = self.intern(&self.source[tokens[at - 2].byte_range()]);
        let (found, owner) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )?;
        let function = self.intern("Function");
        // A constructor tear-off (`C.new`, `C.named`, `E.new` — the
        // enum's generative miss tears off `Function` too, the oracle
        // reporting the section's verdict beside the constructor's own
        // row): the selector names a constructor when the member table
        // does not carry it first (a static member sharing the spelling
        // keeps its own face).
        if !found.members.contains_key(&member)
            && (found.constructors.contains(&member)
                || (member == self.intern("new") && found.has_unnamed_constructor))
        {
            return Some((Ty::named(function), function));
        }
        let entry = found.members.get(&member)?;
        if !entry.is_static {
            // `C.im..foo()` — the instance member's own access face
            // (`static_access_to_instance_member` at the selector) owns
            // the report; the section's receiver is Invalid upstream.
            return None;
        }
        let entry = self.boundary.member(owner, container, member, entry);
        match entry.kind {
            MemberKind::Method => Some((Ty::named(function), function)),
            MemberKind::Field | MemberKind::Getter => {
                let declared = Self::member_result_ty(&entry);
                let head = match declared {
                    Ty::Named { name, .. } => name,
                    _ => function,
                };
                (declared != Ty::UNKNOWN).then_some((declared, head))
            }
            MemberKind::Setter => None,
        }
    }

    /// The type-literal receiver face: a receiver head that spells a TYPE —
    /// the `dynamic` keyword, `Never`, or an ungrounded type alias (a
    /// function or record target; upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/ast/ast.dart `TypeLiteral.staticType`) —
    /// denotes the `Type` INSTANCE, so the member lookup is an instance
    /// lookup on `Type`: the alias's (or dynamic's) own statics never
    /// answer, the `Object` face resolves, and a clean miss reports the
    /// access-intent trio naming `Type` (`Cb.foo` over `typedef Cb = void
    /// Function()` reports "The getter 'foo' isn't defined for the type
    /// 'Type'." — the cascade sections' twin, `Receiver::Container`'s gate
    /// above). A receiver head spelling a variable, field, or getter (a
    /// VALUE of function/record type — `cb.gone` over a `void Function()`
    /// formal) keeps the shape face's own display. The alias INVOKE twin
    /// (`Cb.bar()`) is reported by the invocation walk's
    /// `report_alias_selector_face`; here it is only claimed, so the shape
    /// dispatch stays quiet. True when the row was answered here.
    fn check_type_literal_receiver(&mut self, row: &Row) -> bool {
        let span = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&row.range.offset))
            .ok()
            .map(|index| self.member_reads[index].recv_span);
        let Some(span) = span else {
            return false;
        };
        let start = self.byte_of(span.offset);
        let end = self.byte_of(span.offset + span.length);
        let text = self.source.get(start..end).unwrap_or("");
        let head = self.intern(text);
        let is_alias = matches!(
            self.top_level_decl(head).as_deref(),
            Some(decl) if decl.kind == MicroKind::TypeAlias
        );
        if is_alias && let Some((container, owner, target)) = self.alias_interface_head(head) {
            // An interface-typed alias head denotes the class it aliases,
            // never the `Type` instance ([`Self::alias_interface_head`]):
            // the aliased container's STATIC face owns the verdict, so
            // `A.sm()` resolves `C`'s statics and a clean miss names `C`.
            let kind = container.kind;
            self.container_member_verdict(row, container, owner, target, kind);
            return true;
        }
        if !(is_alias || text == "dynamic" || text == "Never") {
            return false;
        }
        let spelling = if matches!(row.intent, Intent::Write | Intent::ReadWrite) {
            self.setter_spelling(row.name)
        } else {
            row.name
        };
        if !self.type_literal_face_resolves(spelling) {
            let name_text = self.strings.resolve(row.name);
            match row.intent {
                // The alias invoke (`Cb.bar()`) is the invocation walk's
                // own face (`report_alias_selector_face`, its no-callee
                // branch), which already names `Type` at the selector:
                // this face only quiets the shape dispatch there (which
                // would otherwise read the head as a `Function` VALUE).
                // The keyword heads have no invocation-side twin — that
                // face is alias-gated — so their invoke reports here.
                Intent::Invoke if is_alias => {}
                Intent::Invoke => self.emit(
                    SemanticCode::UndefinedMethod,
                    row.range,
                    name_text,
                    "Type",
                ),
                Intent::Read => self.emit(
                    SemanticCode::UndefinedGetter,
                    row.range,
                    name_text,
                    "Type",
                ),
                Intent::Write | Intent::ReadWrite => self.emit(
                    SemanticCode::UndefinedSetter,
                    row.range,
                    name_text,
                    "Type",
                ),
                Intent::Operator(..) => {}
            }
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
        true
    }

    /// An instantiated type-literal receiver used as a member target
    /// (`C<T>.sel<T>()`, `Alias<T>.sel<T>()`). The selector's own type
    /// arguments keep the send a `MethodInvocation` — without them the
    /// spelling is the CREATION face (`C<T>.sel()` is a named-constructor
    /// call, `C<T>.sel` a constructor reference: the
    /// `new_with_undefined_constructor` / `class_instantiation_access_to_member`
    /// families, which this face must not claim). Upstream
    /// `_resolveReceiverTypeLiteral` (pkg/analyzer/lib/src/dart/resolver/
    /// method_invocation_resolver.dart:1212-1268) then resolves `sel`
    /// against the `Type` INSTANCE: the class's (or alias's) own statics
    /// never answer, only the `Object` face resolves, and a clean miss
    /// reports `undefined_method` naming `Type` (probe 3.13.7:
    /// `A<int>.sm<num>()` over a `static void sm<Y>()` reports "The method
    /// 'sm' isn't defined for the type 'Type'."; the alias-of-class twin
    /// `T<int>.sm<num>()` the same). True when the row was answered here.
    fn check_instantiated_literal_member(&mut self, row: &Row) -> bool {
        if row.intent != Intent::Invoke || !self.method_type_arguments_after(row.range) {
            return false;
        }
        if !self.receiver_closes_type_arguments(row.range) {
            return false;
        }
        if !self.type_literal_face_resolves(row.name) {
            let name_text = self.strings.resolve(row.name);
            self.emit(SemanticCode::UndefinedMethod, row.range, name_text, "Type");
        }
        self.record_miss(row.name, row.range, true, NameUse::Invoke);
        true
    }

    /// Whether the selector is followed (across trivia) by its own `<` —
    /// the written method type arguments that make the send a
    /// `MethodInvocation` rather than a creation.
    fn method_type_arguments_after(&self, selector: SourceRange) -> bool {
        let at = self.skip_trivia_forward(self.byte_of(selector.end()));
        self.source.as_bytes().get(at) == Some(&b'<')
    }

    /// `at` advanced past whitespace and comments (the forward twin of the
    /// backward walk in [`Self::receiver_closes_type_arguments`]).
    fn skip_trivia_forward(&self, mut at: usize) -> usize {
        let bytes = self.source.as_bytes();
        loop {
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            if bytes.get(at) != Some(&b'/') {
                return at;
            }
            match bytes.get(at + 1) {
                Some(b'/') => {
                    while at < bytes.len() && bytes[at] != b'\n' {
                        at += 1;
                    }
                }
                Some(b'*') => {
                    let Some(close) = self.source.get(at + 2..).and_then(|s| s.find("*/")) else {
                        return bytes.len();
                    };
                    at += 2 + close + 2;
                }
                _ => return at,
            }
        }
    }

    /// Whether the receiver expression's `.` follows (across trivia) the
    /// type-argument group's `>` — the instantiated type-literal spelling
    /// (`C<T>.sel`, `Alias<T>.sel`). A value receiver ends in `)`/`]` or a
    /// name (`f<T>().sel`, `xs[0].sel`, `c.sel`), so its send keeps the
    /// shape face.
    fn receiver_closes_type_arguments(&self, selector: SourceRange) -> bool {
        let bytes = self.source.as_bytes();
        let mut at = self.byte_of(selector.offset);
        // Back over trivia to the receiver's `.` (block comments both
        // between the `.` and the selector, and between the `>` and the
        // `.`).
        loop {
            while at > 0 && bytes[at - 1].is_ascii_whitespace() {
                at -= 1;
            }
            if at >= 2 && &bytes[at - 2..at] == b"*/" {
                let Some(open) = self.source.get(..at - 2).and_then(|s| s.rfind("/*")) else {
                    return false;
                };
                at = open;
                continue;
            }
            break;
        }
        if at == 0 || bytes[at - 1] != b'.' {
            return false;
        }
        at -= 1;
        loop {
            while at > 0 && bytes[at - 1].is_ascii_whitespace() {
                at -= 1;
            }
            if at >= 2 && &bytes[at - 2..at] == b"*/" {
                let Some(open) = self.source.get(..at - 2).and_then(|s| s.rfind("/*")) else {
                    return false;
                };
                at = open;
                continue;
            }
            break;
        }
        at > 0 && bytes[at - 1] == b'>'
    }

    /// Instance access through a declaration-typed receiver (`this.m`, `c.m`
    /// on a `C c`). A clean walk that misses reports the member family
    /// (`undefined_method`/`_getter`/`_setter` by access intent); a broken
    /// chain, the extension pool, enum synthesized members, and `Object`
    /// members suppress. Operator intents are the member-result shape
    /// (`c.j + 1`, `c.j[i]`): the member itself is the operator's receiver —
    /// a found member hands the operator check to its result type, a clean
    /// miss reports the getter read (the operator's invalid type never
    /// cascades). Mixin `on` constraints and extension `on` types are
    /// invisible to the model, so bodies of those containers record only.
    pub(crate) fn check_instance_member(
        &mut self,
        promotions: &PromotionStaging,
        row: &Row,
        recv: Receiver,
    ) {
        // The never-receiver junction (`receiverType is NeverTypeImpl`,
        // method_invocation_resolver.dart:644-667, the property resolver's
        // twin guards): the member lookup never runs on a `Never` receiver —
        // the staged receiver row (`receiver_of_type_never`) and the send's
        // dead tail own the face, and `_resolveReceiverWithoutTarget`
        // answers nothing (`f().foo()` reports the receiver pair alone).
        // A receiver head that SPELLS `Never` is the type literal, not a
        // `Never`-typed value — its answer is the `Type` instance's face
        // below (probe 3.13.7: `Never.gone;` reports "The getter 'gone'
        // isn't defined for the type 'Type'.").
        if matches!(recv, Receiver::Typed { ty, .. } if self.is_never_ty(ty)) {
            self.check_type_literal_receiver(row);
            return;
        }
        let (container, this_kind) = match recv {
            Receiver::This { container, kind } => (container, Some(kind)),
            Receiver::Typed { type_name, .. } => (type_name, None),
            _ => return,
        };
        // A receiver whose class came from the closure-formal inference
        // (the solved callee-own parameter — the type is OUR approximation
        // of the oracle's): the member walk on it under-resolves the
        // extension/import members the oracle resolves through, so every
        // face against it stays silent — the undefined rows the walk would
        // emit (and the operator faces beside them) are under-report, not
        // evidence.
        if self.inferred_formal_containers.contains(&container) {
            // The oracle resolves this access (the posture's premise), so
            // the spelling stays a resolved use — the declaration it names
            // must not flip to `unused_element` because OUR walk sat out.
            // Instance-member sites read the spelling credit; extension
            // members read the per-face triple the application credit
            // keys — every same-file face declaring the spelling takes it
            // (the self-use arm's granularity; a suppressed access never
            // disambiguates siblings).
            self.inferred_recv_used_members.insert(row.name);
            if let Some(elements) = self.index.get(self.file) {
                let face_credits: Vec<u32> = elements
                    .extension_faces()
                    .iter()
                    .enumerate()
                    .filter(|(_, face)| face.members.contains_key(&row.name))
                    .filter_map(|(index, _)| u32::try_from(index).ok())
                    .collect();
                for index in face_credits {
                    self.extension_member_used
                        .insert((self.file, index, row.name));
                }
            }
            return;
        }
        // A typedef-to-shape receiver (`previous._value?.copyWith(..)`
        // over `_DataRecord<ValueT>`, `fn.bar` over `typedef F = void
        // Function()`): the alias folds to the record/function shape the
        // member verdict answers — no container exists to walk, and the
        // shape arm owns the access (the field/`call` resolves, the
        // extension application, the `Object` interface, then the
        // access-intent trio). A NULLABLE folded shape dereferences
        // unconditionally instead (#2815 R1c — `typedef R = (int, String)?;`
        // `r.foo(0)` reports the shared unchecked code at the selector
        // before any extension consult: a non-nullable `on` face never
        // applies to the nullable spelling, the same split the member
        // sites' `receiver_nullable` gate encodes).
        if let Receiver::Typed { ty, .. } = recv
            && matches!(
                row.intent,
                Intent::Invoke | Intent::Read | Intent::Write | Intent::ReadWrite
            )
            && matches!(
                self.fold_alias_ty(ty),
                Ty::Record { .. } | Ty::Function { .. }
            )
        {
            // A receiver head that spells the type (an ungrounded alias —
            // `Cb.foo` over `typedef Cb = void Function()`) is the type
            // literal's `Type` instance, never a value of the shape.
            if self.check_type_literal_receiver(row) {
                return;
            }
            let folded = self.fold_alias_ty(ty);
            let mut receiver_nullable = match folded {
                Ty::Record { nullable, .. } | Ty::Function { nullable, .. } => {
                    nullable && !self.null_aware_members.contains(&row.range.offset)
                }
                _ => false,
            };
            // The promoted-variable stand-down (`fn != null` in the guard,
            // `fn!`'s assert): a promoted receiver read stands non-null —
            // the same face the general arm below re-decides through
            // `promoted_recv_reads`.
            if receiver_nullable
                && let Some(read_span) = self
                    .promoted_recv_reads
                    .binary_search_by(|(member, _)| member.cmp(&row.range.offset))
                    .ok()
                    .map(|index| self.promoted_recv_reads[index].1)
                && !self.read_offset_is_member(read_span.offset)
                && let Some(promoted) = promotions.promoted(read_span)
                && (promoted == Ty::Unknown || promoted.is_strictly_non_nullable())
            {
                receiver_nullable = false;
            }
            let display = self.render_ty(ty);
            self.check_shape_receiver(row, folded, &display, receiver_nullable);
            return;
        }
        // The extension-application faces read the receiver's nullability
        // (applicability and the unchecked-access companion both split on
        // it). A `?.`-merged access unwraps the receiver first — `x?.m`
        // accesses `m` on the non-null `C`, so a non-nullable `on C` face
        // still applies (the oracle's `applicableTo` runs against the
        // promoted non-null static type, not `C?`).
        let mut receiver_nullable = match &recv {
            Receiver::Typed { ty, .. } => {
                self.potentially_nullable(*ty)
                    && !self.null_aware_members.contains(&row.range.offset)
            }
            // The `this` of an `extension E on A?` body: `this`'s static
            // type IS the extended type (`A?`), so the member verdicts
            // below ask the same nullable-receiver question a typed
            // receiver asks — `this.foo` reports the unchecked access at
            // the selector (`type_property_resolver.dart`'s `isNullable`
            // tail precedes the member resolution), `this?.foo` already
            // unwrapped the receiver (the `?.` latch stands the flag
            // down). The helper carries the `InvalidType`-`this` exclusion
            // (`on Missing?` contributes no honest type — silent).
            Receiver::This { .. } => {
                self.nullable_this_receiver(&recv)
                    && !self.null_aware_members.contains(&row.range.offset)
            }
            _ => false,
        };
        // A `?`-less `FutureOr<A>` over a potentially-nullable `A` answers
        // the miss face only: the extension lookups still see the written
        // non-nullable spelling (an `on FutureOr<V>` face binds `V` to the
        // nullable argument and applies), so the flag joins at the final
        // unchecked-vs-undefined split alone.
        let future_or_nullable = match &recv {
            Receiver::Typed { ty, .. } => {
                !receiver_nullable
                    && self.future_or_potentially_nullable(*ty)
                    && !self.null_aware_members.contains(&row.range.offset)
            }
            _ => false,
        };
        // `Object`'s own members read on a nullable receiver need no null
        // check: `Null` declares them too, so the oracle resolves
        // `x.hashCode` / `x.toString()` / `x.runtimeType` /
        // `x.noSuchMethod(...)` on an `Object?` receiver through the
        // nullable type's interface and reports nothing
        // (`type_property_resolver.dart`'s nullable-receiver branch looks
        // the name up on `Null` before reporting).
        if receiver_nullable && self.members.object_members.contains(&row.name) {
            receiver_nullable = false;
        }
        // The promoted-property face (#1409, pass-2 leg): a member access
        // whose receiver read stood promoted (`_r != null` in the guard)
        // reads a non-null receiver — the promotable-field substitution
        // decides eligibility exactly like the teardown join's staged
        // checks (`isPropertyPromotable`, field_promotability.dart:70-76).
        // The receiver read's span is the `promoted_recv_reads` entry keyed
        // on this member's offset.
        if receiver_nullable
            && let Some(read_span) = self
                .promoted_recv_reads
                .binary_search_by(|(member, _)| member.cmp(&row.range.offset))
                .ok()
                .map(|index| self.promoted_recv_reads[index].1)
            && let Some(ty) = match &recv {
                Receiver::Typed { ty, .. } => Some(*ty),
                _ => None,
            }
            && self
                .promotable_field_substitution(&self.prop_promotion_rows, read_span, ty)
                .is_some()
        {
            receiver_nullable = false;
        }
        // The promoted-variable face (#2241): a bare-read receiver whose
        // flow promotion stands — `if (files != null) files.length` on the
        // `var`-inferred local whose promoted head is the flow lattice's
        // opaque marker (a signature-less function head shares the shape,
        // #2174). `filter_staged`'s general arm rewrites the staged
        // `Ty::Unknown` to the parse-time type's non-null form; the
        // receiver verdict reads the same staging: an opaque head, or a
        // promotion to a strictly non-null face, reads a non-null receiver
        // — a `Null` or nullable promoted head (the `== null` branch)
        // keeps the verdict.
        // A MEMBER access's receiver read never reads this face (#2815
        // R3): the flow layer resolves by name and can stage a variable
        // promotion row at a selector span (a getter, a public or non-
        // final field, an enum-prefixed target — none promotable), but
        // the oracle reads the accessor's own promotion only
        // (`isPropertyPromotable`, field_promotability.dart:70-76) — the
        // promotable-field arm above is the one legitimate stand-down.
        if receiver_nullable
            && let Some(read_span) = self
                .promoted_recv_reads
                .binary_search_by(|(member, _)| member.cmp(&row.range.offset))
                .ok()
                .map(|index| self.promoted_recv_reads[index].1)
            && !self.read_offset_is_member(read_span.offset)
            && let Some(promoted) = promotions.promoted(read_span)
            && (promoted == Ty::Unknown || promoted.is_strictly_non_nullable())
        {
            receiver_nullable = false;
        }
        // F-C: a selector pushed inside a constructor-initializer list
        // resolves through the dedicated initializer world — the field name
        // against the field set (`_checkForInvalidField`), the invocation
        // against the class's constructor table
        // (`RedirectingConstructorInvocation`; a missing target reports
        // nothing) — and the member trio never fires there. A write miss
        // (the name absent, or resolving to an accessor rather than a
        // field) codes `initializer_for_non_existent_field`; read and
        // invocation intents stay silent (the under-report posture the
        // redirect face shares).
        if self.init_rows.contains_key(&row.range.offset) {
            if matches!(row.intent, Intent::Write | Intent::ReadWrite) {
                self.check_initializer_member_write(row, container);
            }
            return;
        }
        if let Intent::Operator(..) = row.intent {
            // The `Object` face the walk cannot reach (its chain ends at an
            // unindexed root): `hashCode`/`toString`/`==`/`noSuchMethod`/
            // `runtimeType` are defined on every class-shaped receiver, so
            // the plain member arm's walk below never fires on them
            // (`member_defined`'s exemption). The operator-merged row owes
            // the same guard — `inner.hashCode * 5` is a defined getter read
            // followed by a check of `*` on the getter's result, never an
            // `undefined_getter`.
            if row.name == self.intern("==") || self.members.object_members.contains(&row.name) {
                return;
            }
            // The `on`-face consult the plain tail runs (`this`'s static
            // type inside a mixin or extension body is the constraint /
            // extended type): the operator-merged row owes it too —
            // `this.offset + 1` inside `extension E on C` resolves `offset`
            // through C's face, never through the extension's member table.
            // A resolvable member suppresses; an undecidable face (a frozen
            // `Unknown` — the on-name the diagnosing scope cannot name is
            // the oracle's `InvalidType` extendedType) silences the miss
            // the same; only a decidable `Absent` reports below, naming the
            // `on` type.
            let mut report_on = container;
            if let Some(kind) = self.container_kind_of(container)
                && matches!(kind, ContainerKind::Mixin | ContainerKind::Extension)
            {
                if !matches!(self.on_face_walk(container, kind, row.name), Walk::Missing) {
                    // The `on`-face hit on a nullable `this` (`this.offset`
                    // inside `extension on A?`) resolves the member through
                    // the nullable extended type — the oracle's `isNullable`
                    // tail reports the unchecked access at the selector
                    // before the member lookup ever runs.
                    if receiver_nullable {
                        let name_text = self.strings.resolve(row.name);
                        self.emit_unchecked_nullable(
                            row.range,
                            name_text,
                            access_kind(&row.intent),
                        );
                    }
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                    return;
                }
                if kind == ContainerKind::Extension && matches!(recv, Receiver::This { .. }) {
                    match self.enclosing_extension_on_type(container) {
                        Some(on) => {
                            if !matches!(
                                self.frozen_member_verdict(on, row.name),
                                FrozenFace::Absent
                            ) {
                                self.record_miss(row.name, row.range, true, NameUse::Read);
                                return;
                            }
                            report_on = on;
                        }
                        None => {
                            self.record_miss(row.name, row.range, true, NameUse::Read);
                            return;
                        }
                    }
                }
            }
            match self.walk_members(container, row.name) {
                Walk::Found(entry) => {
                    // The nullable-dereference face the plain member arm
                    // fires (`member_defined`'s found arm): the oracle's
                    // `isNullable` tail precedes every member resolution, so
                    // a found member read through a nullable receiver is
                    // `unchecked_use_of_nullable_value` at the selector even
                    // as the operator's own face resolves on the getter's
                    // result (`a.offset - b.offset` over `a: Node?` reports
                    // the unchecked row, never an undefined getter).
                    if receiver_nullable {
                        let name_text = self.strings.resolve(row.name);
                        self.emit_unchecked_nullable(
                            row.range,
                            name_text,
                            access_kind(&row.intent),
                        );
                    }
                    // The operator-merged read resolved (#2618): the member
                    // read face carries the edge.
                    let landing = self.last_landing();
                    let kind = Self::member_ref_kind(&row.intent, entry.kind);
                    self.note_member_ref(
                        row,
                        kind,
                        landing.map(|(file, origin)| (file, row.name, origin)),
                    );
                    let ty = Self::member_result_ty(&entry);
                    self.check_operator_expr_in(ty, row, Some(container));
                    return;
                }
                // The frozen interface faces decide a break at the root or
                // a deeper hop the same as the plain member arm below: a
                // carried member suppresses (its result type is beyond the
                // name-level model — `check_operator_expr` reads `Unknown`),
                // a decidable miss reports the getter read — or the shared
                // unchecked-access code on a merely-nullable receiver — and
                // a genuinely unindexed declaration keeps the under-report.
                _ if {
                    // An applicable extension accessor resolves the
                    // operator-merged read too (`ports.toDart[0]` through
                    // `JSArrayToList.toDart`): its result types the operator.
                    let receiver_ty = match recv {
                        Receiver::Typed { ty, .. } => ty,
                        Receiver::This { container, kind } => self.this_static_ty(container, kind),
                        _ => Ty::UNKNOWN,
                    };
                    let applied = !matches!(
                        self.extension_application(
                            container,
                            receiver_ty,
                            row.name,
                            false,
                            receiver_nullable
                        ),
                        ExtensionApp::None
                    );

                    applied
                } =>
                {
                    // The application resolved the operator-merged read
                    // (#2618) when it was single-faced — the latch holds
                    // the target only then (an ambiguous application
                    // resolves nothing and latches nothing).
                    self.note_extension_ref(row);
                    return;
                }
                Walk::ChainBroken => {
                    match self.frozen_member_verdict(report_on, row.name) {
                        FrozenFace::Absent => {
                            let name_text = self.strings.resolve(row.name);
                            if receiver_nullable {
                                self.emit_unchecked_nullable(
                                    row.range,
                                    name_text,
                                    access_kind(&row.intent),
                                );
                            } else {
                                let type_text = self.strings.resolve(report_on).to_string();
                                self.emit(
                                    SemanticCode::UndefinedGetter,
                                    row.range,
                                    name_text,
                                    &type_text,
                                );
                            }
                            self.record_miss(row.name, row.range, true, NameUse::Read);
                        }
                        FrozenFace::Defined | FrozenFace::Unknown => {}
                    }
                    return;
                }
                Walk::Missing => {
                    let name_text = self.strings.resolve(row.name);
                    if receiver_nullable {
                        self.emit_unchecked_nullable(
                            row.range,
                            name_text,
                            access_kind(&row.intent),
                        );
                    } else {
                        let type_text = self.strings.resolve(report_on).to_string();
                        self.emit(
                            SemanticCode::UndefinedGetter,
                            row.range,
                            name_text,
                            &type_text,
                        );
                    }
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                    return;
                }
            }
        }
        // The extension-application face reads the receiver's full static
        // type: `a.b().c`'s chained `Typed.ty` carries the declared return
        // type's argument list (`Map<K, FieldItem>`), which the `on`
        // clause's argument gate compares positionally. `this` reads its
        // container's instance type (an extension body's `this` is the
        // extended type). A bare head skips the gate either way.
        let receiver_ty = match recv {
            Receiver::Typed { ty, .. } => ty,
            Receiver::This { container, kind } => self.this_static_ty(container, kind),
            _ => Ty::UNKNOWN,
        };
        // An extension `this` whose `on` type is a record or function type
        // has a fully DECIDABLE member face — the record's field getters
        // (or the function type's `call`), then nothing — so a miss reports
        // exactly like the oracle: `undefined_method`/`-getter`/`-setter`
        // against the extended type's display ('<unknown>' for a record,
        // 'Function' for `on void Function()`). `member_defined` cannot see
        // this (a non-named receiver reads as the suppress-everything
        // face), so the record/function `this` is answered here. A nullable
        // `this` keeps the under-report (the oracle's
        // `unchecked_use_of_nullable_value` face).
        if this_kind == Some(ContainerKind::Extension) {
            let record_hit = matches!(receiver_ty, Ty::Record { .. })
                && self
                    .record_field_ty(receiver_ty, row.name, receiver_nullable)
                    .is_some();
            let function_hit = matches!(
                receiver_ty,
                Ty::Function {
                    nullable: false,
                    ..
                }
            ) && row.name == self.intern("call");
            if record_hit || function_hit {
                return;
            }
            let (decidable, display) = match receiver_ty {
                Ty::Record {
                    nullable: false, ..
                } => (true, "<unknown>"),
                Ty::Function {
                    nullable: false, ..
                } => (true, "Function"),
                _ => (false, ""),
            };
            if decidable {
                let name_text = self.strings.resolve(row.name);
                match row.intent {
                    Intent::Invoke => {
                        self.emit(SemanticCode::UndefinedMethod, row.range, name_text, display);
                    }
                    Intent::Read => {
                        self.emit(SemanticCode::UndefinedGetter, row.range, name_text, display);
                    }
                    Intent::Write | Intent::ReadWrite => {
                        self.emit(SemanticCode::UndefinedSetter, row.range, name_text, display);
                    }
                    Intent::Operator(..) => {}
                }
                self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                return;
            }
        }
        // `call` on a bare `Function` receiver (`f.call`, `f.call()` over
        // `Function f`): the interface lookup answers nothing for the name
        // — the core `Function` type carries no `call` member — and the
        // oracle's `isDartCoreFunction` arm resolves it to the receiver's
        // own signature before the `undefined_*` verdict ever runs
        // (`type_property_resolver.dart:194-198`). A merely-nullable
        // receiver reaches `call` only through the extension/`Object`
        // path, where the name declares nothing — the `isNullable` tail's
        // unchecked access keeps the miss.
        if !receiver_nullable
            && row.name == self.intern("call")
            && matches!(
                self.fold_alias_ty(receiver_ty),
                Ty::Named { name, .. } if name == self.intern("Function")
            )
        {
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        // A type-PARAMETER receiver (`t.m()` over `void f<T extends Bound>
        // (T t)`): upstream's `resolveToBound` reads the member through the
        // parameter's bound (the bound's own interface plus the extensions
        // its head admits), but every miss face names the PARAMETER: the
        // invocation resolver's `receiverClassName` escapes both its
        // `InterfaceType` and `FunctionType` arms on a `TypeParameterType`
        // (`<unknown>` — method_invocation_resolver.dart:1190-1199), the
        // getter/setter faces spell the parameter itself (probed 3.13.7).
        // The scope window survives in the post-walk `param_bound_of`
        // captures; a bound the model cannot walk keeps the under-report
        // (the oracle's `InvalidType` bound owns no honest verdict). The
        // same convention `param_bound_potentially_nullable` keeps decides
        // the receiver's nullability — a spelling any captured window
        // declares IS a parameter, even where a same-named declared type
        // exists (the name-level answer upstream's shadowing rules give).
        // Known under-report: an extension `on Object` applied to a bounded
        // parameter reads `Single` here (the bound reaches `Object` by
        // name), while the oracle's `applicableTo` instantiates the
        // parameter to its bound and reports the shared unchecked code —
        // the bound's own-name walk cannot express the instantiation.
        // The arm fires only when the receiver's static type IS the
        // parameter spelling — a lambda/formal whose declared type is `T`
        // but whose inferred face already resolved to a concrete head
        // (`(t) => t.name` under `mapOrNull<R>(R Function(T) mapper)`
        // over a `Prefix?` receiver types `t` `Prefix`, never `T`) keeps
        // the normal `member_defined` walk; gating on `container` alone
        // would misread the param's bound for that concrete face.
        if matches!(recv, Receiver::Typed { .. })
            && !matches!(row.intent, Intent::Operator(..))
            && matches!(
                self.fold_alias_ty(receiver_ty),
                Ty::Named { name, .. } if name == container
            )
            && let Some(bound) = self.param_bound_of(container)
        {
            // A parameter bound to another parameter (`T extends S`)
            // resolves on the LAST hop's bound — the same chase
            // `param_bound_potentially_nullable` runs, capped so a cyclic
            // capture falls out on the last seen bound.
            let mut bound = bound;
            for _ in 0..4 {
                let Ty::Named {
                    name: hop,
                    nullable: false,
                    args: ArgListId::EMPTY,
                    ..
                } = self.fold_alias_ty(bound)
                else {
                    break;
                };
                let Some(next) = self.param_bound_of(hop) else {
                    break;
                };
                bound = next;
            }
            match self.fold_alias_ty(bound) {
                // A shape bound answers its own members first (`call` on a
                // function bound, the record field getters), then the
                // extension consult the shape's own faces admit — but every
                // miss face names the PARAMETER: `<unknown>` on the
                // invocation face even over a `Function` bound (probed
                // 3.13.7: `t.gone()` over `T extends Bound Function()`
                // names `<unknown>`, never `Function` — the
                // `TypeParameterType` receiver escapes the invocation
                // resolver's `receiverClassName` arms).
                shape @ (Ty::Record { .. } | Ty::Function { .. }) => {
                    let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
                    let field_read = matches!(shape, Ty::Record { .. })
                        && self
                            .record_field_ty(
                                shape,
                                row.name,
                                self.null_aware_members.contains(&row.range.offset),
                            )
                            .is_some();
                    // Records are immutable upstream: a field hit answers
                    // the READ face only — a write to a declared field
                    // reports `undefined_setter` on the parameter.
                    let read_resolved = field_read
                        || (matches!(shape, Ty::Function { .. })
                            && row.name == self.intern("call"))
                        || self.members.object_members.contains(&row.name)
                        || row.name == self.intern("==")
                        || self.record_extension_face(shape, row.name).is_some();
                    let setter = self.setter_spelling(row.name);
                    let write_resolved = write
                        && self
                            .record_extension_face(shape, setter)
                            .is_some();
                    // A write resolves only through the `name=` extension
                    // spelling; its READ half reports unless the field or
                    // an `Object` member answered it (`check_shape_receiver`'s
                    // same split — a compound miss reports getter then
                    // setter, a field hit still reports the setter alone).
                    let (getter_miss, setter_miss) = match row.intent {
                        Intent::Write => (false, !write_resolved),
                        Intent::ReadWrite => (!read_resolved, !write_resolved),
                        Intent::Read => (!read_resolved, false),
                        Intent::Invoke => (!read_resolved, false),
                        Intent::Operator(..) => (false, false),
                    };
                    if getter_miss || setter_miss {
                        let name_text = self.strings.resolve(row.name);
                        if receiver_nullable {
                            self.emit_unchecked_nullable(
                                row.range,
                                name_text,
                                access_kind(&row.intent),
                            );
                        } else {
                            let param_text =
                                self.strings.resolve(container).to_string();
                            match row.intent {
                                Intent::Invoke => {
                                    self.emit(
                                        SemanticCode::UndefinedMethod,
                                        row.range,
                                        name_text,
                                        "<unknown>",
                                    );
                                }
                                _ => {
                                    if getter_miss {
                                        self.emit(
                                            SemanticCode::UndefinedGetter,
                                            row.range,
                                            name_text,
                                            &param_text,
                                        );
                                    }
                                    if setter_miss {
                                        self.emit(
                                            SemanticCode::UndefinedSetter,
                                            row.range,
                                            name_text,
                                            &param_text,
                                        );
                                    }
                                }
                            }
                        }
                    }
                    self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                    return;
                }
                Ty::Named {
                    name: bound_head, ..
                } => {
                    // `call` on a `Function`-bound parameter is the
                    // `isDartCoreFunction` arm's silent resolve (probed
                    // 3.13.7: `t.call`/`t.call()` over `T extends Function`
                    // report nothing; `T extends Bound Function()` the
                    // same).
                    if !receiver_nullable
                        && row.name == self.intern("call")
                        && bound_head == self.intern("Function")
                    {
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                    // Every interface member upstream resolves through the
                    // `Object` face on ANY bound — `t.hashCode`/`t ==
                    // other`/`t.toString()` stay silent even over the
                    // implicit `Object?` (the `Null` face's member map
                    // answers them).
                    if self.member_defined(bound_head, bound, row, false, receiver_nullable) {
                        return;
                    }
                    // A bound spelling a core interface/type-table head
                    // answers its frozen members the same way
                    // `check_core_type_member` does — the rig's stub
                    // classes declare nothing, so `num.abs`, `int.sign`,
                    // `Function.call` resolve off the static face
                    // (`class num {}` walks no members; the table carries
                    // the names-only SDK answer upstream's bound walk
                    // finds). `write` reads the setter spelling's
                    // membership the same as the getter's — the frozen
                    // face declares no setter, so a miss stays a miss.
                    let bound_text = self.strings.resolve(bound_head);
                    let member_text = self.strings.resolve(row.name);
                    let setter_id = self.setter_spelling(row.name);
                    let setter_text = self.strings.resolve(setter_id);
                    let bound_core_member = crate::member_face::CORE_INTERFACE_MEMBERS
                        .iter()
                        .chain(crate::member_face::CORE_TYPE_MEMBERS.iter())
                        .any(|(head, members)| {
                            *head == bound_text
                                && (members.contains(&member_text)
                                    || (matches!(row.intent, Intent::Write | Intent::ReadWrite)
                                        && members.contains(&setter_text)))
                        })
                        || (bound_text == "Function" && member_text == "call");
                    if bound_core_member {
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                    // A bound name the model cannot walk (unresolvable,
                    // or another parameter spelling): the miss stays
                    // under-report rather than inventing a verdict.
                    let bound_walkable = matches!(self.strings.resolve(bound_head), "Object")
                        || resolve_container(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            bound_head,
                        )
                        .is_some()
                        || crate::member_face::CORE_INTERFACE_MEMBERS
                            .iter()
                            .chain(crate::member_face::CORE_TYPE_MEMBERS.iter())
                            .any(|(head, _)| *head == self.strings.resolve(bound_head));
                    if !bound_walkable {
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                    // A receiver riding a nullable-own-param face's own
                    // parameter (`extension<T> on T?`'s `T`): the oracle
                    // instantiates the parameter at the application site
                    // and resolves the access against the instantiation —
                    // the same suppression `check_core_type_member`'s
                    // undecidable arm encodes, kept here so the bound
                    // miss does not over-report on a receiver an armed
                    // nullable extension already owns.
                    let own_param_armed = receiver_nullable
                        && self.accessible_extension_faces().iter().any(|(face, _)| {
                            face.on_type_nullable
                                && face.on_type == Some(container)
                                && face.type_param_names.contains(&container)
                        });
                    let name_text = self.strings.resolve(row.name);
                    if receiver_nullable && !own_param_armed {
                        self.emit_unchecked_nullable(
                            row.range,
                            name_text,
                            access_kind(&row.intent),
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                    if own_param_armed {
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                    let param_text = self.strings.resolve(container).to_string();
                    match row.intent {
                        Intent::Invoke => {
                            self.emit(
                                SemanticCode::UndefinedMethod,
                                row.range,
                                name_text,
                                "<unknown>",
                            );
                        }
                        Intent::Read => {
                            self.emit(
                                SemanticCode::UndefinedGetter,
                                row.range,
                                name_text,
                                &param_text,
                            );
                        }
                        Intent::Write | Intent::ReadWrite => {
                            // The oracle resolves the read face first — a
                            // compound write reports both faces in that
                            // order (`t.z += 1` reports getter then
                            // setter, probe 3.13.7).
                            if row.intent == Intent::ReadWrite {
                                self.emit(
                                    SemanticCode::UndefinedGetter,
                                    row.range,
                                    name_text,
                                    &param_text,
                                );
                            }
                            self.emit(
                                SemanticCode::UndefinedSetter,
                                row.range,
                                name_text,
                                &param_text,
                            );
                        }
                        Intent::Operator(..) => {}
                    }
                    self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                    return;
                }
                // A `dynamic`/`Null`/`void`/unresolvable bound owns no
                // member verdict — the under-report posture the
                // `InvalidType` face keeps.
                _ => {
                    self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                    return;
                }
            }
        }
        // The `?`-less `FutureOr<A>` receiver's extension consult reads its
        // arg-nullability too: `extension on Object` (or `on Future`)
        // applies to `FutureOr<int?>` only through the non-null projection
        // — the oracle's unsafe application earns the shared unchecked
        // code at the selector, the same verdict the miss tail below
        // answers for a clean miss (`x.om()` vs `y.om()` over
        // `FutureOr<int>`, #3457). Other receivers never carry the flag.
        if self.member_defined(
            container,
            receiver_ty,
            row,
            false,
            receiver_nullable || future_or_nullable,
        ) {
            // The compound-write read face (the oracle resolves the getter
            // before the setter): a compound write satisfied through the
            // SETTER spelling (`name=`) still reports the getter half's own
            // miss — `c.foo += 1` over a setter-only `C` is
            // `undefined_getter` naming `C` (the class setter's claim also
            // blocks the extension-getter application,
            // UndefinedGetterTest test_compoundAssignment_hasSetter_instance
            // and test_extension_instance_extendedHasSetter_extensionHasGetter).
            // A satisfied READ half (a getter or field under the plain
            // spelling) stays silent.
            if row.intent == Intent::ReadWrite
                && !receiver_nullable
                && matches!(self.walk_members(container, row.name), Walk::Missing)
                && matches!(
                    self.frozen_member_verdict(container, row.name),
                    FrozenFace::Absent
                )
                && !self.members.object_members.contains(&row.name)
                && matches!(
                    self.extension_application(
                        container,
                        receiver_ty,
                        row.name,
                        false,
                        receiver_nullable,
                    ),
                    ExtensionApp::None
                )
            {
                let name_text = self.strings.resolve(row.name);
                let type_text =
                    self.getter_display(&recv, container, receiver_ty, row.range.offset);
                self.emit(SemanticCode::UndefinedGetter, row.range, name_text, &type_text);
                self.record_miss(row.name, row.range, true, NameUse::Read);
                return;
            }
            // No implicit-this face here: every caller hands a QUALIFIED
            // member (`x.m`, `C().m`, `this.m`), and the oracle runs
            // `_checkForInvalidInstanceMemberAccess2` from the unqualified
            // nodes only (`visitUnqualifiedNameExpression`,
            // `UnqualifiedFunctionInvocation`, the unqualified assignment
            // targets) — the bare-row face owns it; an explicit `this` in
            // an initializer is `invalid_reference_to_this` at the keyword.
            self.check_member_unused_result(
                row,
                container,
                &Receiver::Typed {
                    type_name: container,
                    ty: Ty::UNKNOWN,
                },
                false,
            );
            return;
        }
        if matches!(self.walk_members(container, row.name), Walk::ChainBroken) {
            // A chain broken at the root type (`int x; x.noSuchGetter`) is
            // the external-primitive face: the core member table decides.
            // A broken superclass hop is decidable through the frozen
            // interface faces (`Comparable`, `Match`, `Stream`, …) the
            // element index cannot carry — `face_member_frozen`'s same
            // consult, three-valued so a genuinely unindexed declaration
            // keeps the under-report. A write resolves its own pair of
            // lookups exactly like `member_defined`'s write arm: the plain
            // spelling satisfies the write through a field/getter hit (the
            // assignment-to-* family owns that face), and only a miss on
            // BOTH spellings reports `undefined_setter` — so the getter
            // verdict consults first and the `name=` spelling breaks the
            // tie (`Match`'s face carries no setters; `IOSink`'s and
            // `StreamController`'s do). A compound write's read face is the
            // getter verdict's own answer.
            let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
            let getter_verdict = self.frozen_member_verdict(container, row.name);
            let setter_verdict = write.then(|| {
                let setter = self.setter_spelling(row.name);
                self.frozen_member_verdict(container, setter)
            });
            // The write-side defined answer joins the getter face's: a
            // `name=` member satisfies the write like a field hit does.
            let defined = matches!(getter_verdict, FrozenFace::Defined)
                || matches!(setter_verdict, Some(FrozenFace::Defined));
            let getter_absent = matches!(getter_verdict, FrozenFace::Absent);
            let setter_absent =
                matches!(setter_verdict, Some(FrozenFace::Absent)) || !write && getter_absent;
            let undecidable = matches!(getter_verdict, FrozenFace::Unknown)
                || matches!(setter_verdict, Some(FrozenFace::Unknown));
            if defined {
                // The member resolved on the frozen face — the found-arm's
                // faces (`member_defined`'s nullable-deref gate plus the
                // unused-result companion) apply exactly as a walkable
                // hit's. A compound write judges its two faces separately:
                // `m` absent under a defined `m=` still reports the getter
                // read (the oracle resolves it first), and `m` defined
                // under an absent `m=` reports the setter write.
                if row.intent == Intent::ReadWrite {
                    let name_text = self.strings.resolve(row.name);
                    // A typed receiver's miss names its static type under
                    // the oracle's `getDisplayString` render — the
                    // instantiate-to-bounds fill (`Map` → `Map<dynamic,
                    // dynamic>`) or the written arguments as spelled. A
                    // `this`-family receiver's instance face is the bare
                    // head the model honestly carries (`C`, whose `C<T>`
                    // display is the type-parameter list no face spells).
                    let type_text = match recv {
                        Receiver::Typed {
                            ty: ty @ Ty::Named { .. },
                            ..
                        } => self.ty_display(ty),
                        _ => self.strings.resolve(container).to_string(),
                    };
                    if getter_absent {
                        let getter_text =
                            self.getter_display(&recv, container, receiver_ty, row.range.offset);
                        self.emit(
                            SemanticCode::UndefinedGetter,
                            row.range,
                            name_text,
                            &getter_text,
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                    if matches!(setter_verdict, Some(FrozenFace::Absent)) {
                        self.emit(
                            SemanticCode::UndefinedSetter,
                            row.range,
                            name_text,
                            &type_text,
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                }
                if receiver_nullable {
                    let name_text = self.strings.resolve(row.name);
                    self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
                }
                self.check_member_unused_result(
                    row,
                    container,
                    &Receiver::Typed {
                        type_name: container,
                        ty: Ty::UNKNOWN,
                    },
                    false,
                );
                return;
            }
            if undecidable {
                self.check_core_type_member(container, row, receiver_nullable);
                return;
            }
            // `member_defined` already ran the extension pool and the
            // `Object`-member/`==` arms. A miss through a merely-nullable
            // receiver is the oracle's unchecked-access row (`type_property_
            // resolver`'s `isNullable` tail never reaches `undefined_*` —
            // the shared `unchecked_use_of_nullable_value` code stands for
            // every variant); a non-nullable miss reports the access-intent
            // trio like a walkable clean miss, per face: the read face
            // answers the getter verdict, the write face the pair.
            let name_text = self.strings.resolve(row.name);
            if receiver_nullable {
                self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
                self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                return;
            }
            let type_text = match recv {
                Receiver::Typed {
                    ty: ty @ Ty::Named { .. },
                    ..
                } => self.ty_display(ty),
                _ => self.strings.resolve(container).to_string(),
            };
            // The invocation face names the receiver's bare element display —
            // never the written instantiation or nullability (`List<int> l;
            // l.removeMiddle()` names `List`, `A<int>? a; a?.foo()` names `A`;
            // the oracle 3.13.7 renders the method face through the element's
            // display name while the getter/setter faces keep the full
            // `getDisplayString` render).
            let method_text = self.strings.resolve(container).to_string();
            match row.intent {
                Intent::Invoke => {
                    self.emit(
                        SemanticCode::UndefinedMethod,
                        row.range,
                        name_text,
                        &method_text,
                    );
                }
                Intent::Read => {
                    let getter_text =
                        self.getter_display(&recv, container, receiver_ty, row.range.offset);
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &getter_text,
                    );
                }
                Intent::Write | Intent::ReadWrite => {
                    // The oracle resolves the getter before the setter — a
                    // compound write reports its absent read face first.
                    if row.intent == Intent::ReadWrite && getter_absent {
                        let getter_text =
                            self.getter_display(&recv, container, receiver_ty, row.range.offset);
                        self.emit(
                            SemanticCode::UndefinedGetter,
                            row.range,
                            name_text,
                            &getter_text,
                        );
                    }
                    if setter_absent {
                        self.emit(
                            SemanticCode::UndefinedSetter,
                            row.range,
                            name_text,
                            &type_text,
                        );
                    }
                }
                Intent::Operator(..) => {}
            }
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        // An SDK-stub anchor's clean MISS suppresses when the frozen rows
        // define the member — dart:isolate's names-only `ReceivePort` (an
        // indexed empty container with an unindexed `implements Stream`
        // clause) walks to `Missing`, and `first`/`sendPort` are defined
        // by the stub faces (`SDK_FACES_EXTRA`/`SDK_SUPER_FACES`). Every
        // other verdict keeps the miss's own reporting path — the
        // stub-face ABSENT answer must flow through the unchanged arm
        // (the message's type face and the rig pins depend on it).
        else if matches!(self.walk_members(container, row.name), Walk::Missing)
            && matches!(
                self.frozen_member_verdict(container, row.name),
                FrozenFace::Defined
            )
        {
            self.check_member_unused_result(
                row,
                container,
                &Receiver::Typed {
                    type_name: container,
                    ty: Ty::UNKNOWN,
                },
                false,
            );
            return;
        }
        // A receiver head that arrived through a foreign member signature
        // (never spelled in this scope) takes its kind from the declaring
        // user library the member walk re-anchored on — the walk already
        // decided the clean miss there. SDK heads keep the external-root
        // face below (their element model is a stub).
        let foreign_kind = || {
            // The receiver link must itself be typed: a head read off an
            // Unknown-receiver send (`f.m()` over an untyped closure
            // formal) is the name-level fallback's guess, not a resolved
            // signature.
            let link_typed = self
                .member_reads
                .binary_search_by(|read| read.offset.cmp(&row.range.offset))
                .ok()
                .and_then(|index| self.chain_node_read(self.member_reads[index].recv_span, true))
                .is_none_or(|prev| !matches!(prev.recv, Receiver::Unknown));
            if !link_typed {
                return None;
            }
            let (file, lib) = self.unique_declared_elsewhere(container, self.lib)?;
            if self.graph.uri(lib).starts_with("dart:") {
                return None;
            }
            self.index.get(file)?.container(container).map(|c| c.kind)
        };
        let Some(kind) = self.container_kind_of(container).or_else(foreign_kind) else {
            // The external-root face: an unindexable receiver type (the
            // core primitives in the CLI world, an undeclared name in the
            // bare unit) decides through the core member table — the
            // dead-edge walk lands here as a clean `Missing` now, never as
            // the broken-chain arm above. The `?`-less `FutureOr<A>`
            // receiver's arg-nullability joins the same unchecked question
            // the miss tail answers (#3457): an extension `on Object`
            // (or `on Future`) applies to `FutureOr<int?>` only through the
            // non-null projection, so `x.om()` earns the oracle's unsafe
            // application row beside `y.om()`'s silent resolve over
            // `FutureOr<int>`.
            self.check_core_type_member(
                container,
                row,
                receiver_nullable || future_or_nullable,
            );
            return;
        };
        if kind == ContainerKind::Enum && self.members.enum_synth_members.contains(&row.name) {
            // The synthesized `index`/`values`/`names` getters are defined
            // instance members; a qualified read of one never reports the
            // initializer face (the bare `index` read is the bare row's).
            return;
        }
        // The `on`-face union (#2186 S2): `walk_members` never climbs a
        // mixin's `on` constraints or an extension's extended type, but
        // those members ride the receiver's interface (`this`'s static
        // type inside an extension IS the `on` type). A carried member
        // resolves the access; a face the model cannot walk keeps the
        // record-only path (the constraint may carry the member). A
        // decidable miss reports like the oracle's `InstanceLookupDomain` —
        // the miss names `this`'s static type: the mixin itself for a mixin
        // body, the `on` type for an extension body. An unresolved on-name
        // is the oracle's `InvalidType` extendedType — its member misses
        // stay silent. A declared-type receiver's decidable miss keeps
        // naming the written static type (the class-family tail below owns
        // the shared emission).
        let mut report_on = container;
        if matches!(kind, ContainerKind::Mixin | ContainerKind::Extension) {
            if !matches!(self.on_face_walk(container, kind, row.name), Walk::Missing) {
                // The `on`-face hit on a nullable `this` (`this.foo` inside
                // `extension on A?`) resolved the member through the
                // nullable extended type — the oracle's `isNullable` tail
                // reports the unchecked access at the selector before the
                // member lookup ever runs.
                if receiver_nullable {
                    let name_text = self.strings.resolve(row.name);
                    self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
                }
                self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                return;
            }
            if kind == ContainerKind::Extension && matches!(recv, Receiver::This { .. }) {
                match self.enclosing_extension_on_type(container) {
                    Some(on) => {
                        // The `on` face the model cannot walk (a bare SDK
                        // type like `Stream<T>`) still answers by its
                        // frozen member table: a hit resolves the access —
                        // the extension applies to a receiver whose
                        // interface carries the member. A miss names the
                        // `on` type in the report below (the climb anchors
                        // at the on-name itself). A face the verdict
                        // cannot decide keeps the record-only posture: the
                        // on-name itself is the oracle's unresolvable
                        // extended type (`InvalidType`), whose member
                        // misses stay silent.
                        match self.frozen_member_verdict(on, row.name) {
                            FrozenFace::Defined | FrozenFace::Unknown => {
                                self.record_miss(
                                    row.name,
                                    row.range,
                                    true,
                                    use_kind_of(&row.intent),
                                );
                                return;
                            }
                            FrozenFace::Absent => report_on = on,
                        }
                    }
                    // The on-name never resolved — nothing honest names the
                    // report, so the record-only posture holds here too.
                    None => {
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                }
            }
        }
        let name_text = self.strings.resolve(row.name);
        // A decidable miss through a merely-nullable receiver is the
        // oracle's unchecked-access row — `type_property_resolver`'s
        // `isNullable` tail reports the shared
        // `unchecked_use_of_nullable_value` code at the selector and never
        // reaches the `undefined_*` family, whatever the member face the
        // model decided the miss on.
        if receiver_nullable || future_or_nullable {
            self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        // The typed receiver's miss names its static type under the
        // oracle's `getDisplayString` render — the promoted head fills its
        // instantiate-to-bounds arguments (`Map` → `Map<dynamic,
        // dynamic>`), a written-argument face keeps them (`G<int>`). Every
        // `this` stays the bare head — the extension `on` type's arguments
        // are the extension's own type parameters, never a face the user
        // spelled (`this.nope()` inside `extension Tap<T> on Stream<T>`
        // names `Stream`).
        let type_text = match (recv, receiver_ty) {
            (Receiver::Typed { .. }, ty @ Ty::Named { .. }) => self.ty_display(ty),
            _ => self.strings.resolve(report_on).to_string(),
        };
        // The invocation face names the receiver's bare element display —
        // never the written instantiation or nullability (`A<int> a;
        // a.foo()` names `A`, `V<String> v; v.fooInt()` names `V`; the
        // oracle 3.13.7 renders the method face through the element's
        // display name while the getter/setter faces keep the full
        // `getDisplayString` render).
        let method_text = self.strings.resolve(report_on).to_string();
        // The getter face keeps the full display but strips the nullability
        // on a null-aware access (the shorted non-null face — the
        // `getter_display` doc's p6 probe); the setter face keeps the
        // plain render (its own family owns that text).
        let getter_text =
            self.getter_display(&recv, report_on, receiver_ty, row.range.offset);
        match row.intent {
            Intent::Invoke => self.emit(
                SemanticCode::UndefinedMethod,
                row.range,
                name_text,
                &method_text,
            ),
            Intent::Write | Intent::ReadWrite => {
                // A compound write checks the read face first (the oracle
                // resolves the getter before the setter).
                if row.intent == Intent::ReadWrite
                    && matches!(self.walk_members(container, row.name), Walk::Missing)
                {
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &getter_text,
                    );
                }
                self.emit(
                    SemanticCode::UndefinedSetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Read => {
                self.emit(
                    SemanticCode::UndefinedGetter,
                    row.range,
                    name_text,
                    &getter_text,
                );
            }
            Intent::Operator(..) => unreachable!("operator intents returned above"),
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// `super.member` — the oracle's `SuperLookupDomain`: one analyzerCode
    /// (`undefined_super_member`) with a per-access-shape message, reported
    /// against the enclosing container's name, plus the
    /// `abstract_super_member_reference` verdict a super-interface
    /// signature earns when no concrete dispatch target exists. Operator
    /// intents follow the member-result shape (the direct `super + b` form
    /// lives on the receiver rows in [`Engine::resolve_receiver_row`],
    /// which reports the operator shape). A broken super chain (mixins:
    /// `on` constraints are not edges) suppresses.
    ///
    /// Inside an `extension`/`extension type` body `super` is not a valid
    /// reference at all — upstream resolves it against `InvalidType`
    /// (`SuperContext.of != valid`, method_invocation_resolver.dart:947-955
    /// / property_element_resolver.dart:2014-2016), so the member lookup
    /// never runs and this face stays silent; the dedicated
    /// `super_in_extension*` context rows own the report.
    pub(crate) fn check_super_member(
        &mut self,
        row: &Row,
        container: StringId,
        kind: ContainerKind,
    ) {
        if matches!(kind, ContainerKind::Extension | ContainerKind::ExtensionType) {
            return;
        }
        if let Intent::Operator(..) = row.intent {
            match self.walk_super(container, row.name) {
                Walk::Found(entry) => {
                    // The operator-merged super read resolved (#2618).
                    let landing = self.last_landing();
                    let kind = Self::member_ref_kind(&row.intent, entry.kind);
                    self.note_member_ref(
                        row,
                        kind,
                        landing.map(|(file, origin)| (file, row.name, origin)),
                    );
                    let ty = Self::member_result_ty(&entry);
                    self.check_operator_expr_in(ty, row, Some(container));
                }
                // The frozen faces decide a break on an unindexed SDK
                // supertype (`class C extends Match { f() { super.m + 1 } }`)
                // — carried member suppresses, decidable miss reports the
                // getter shape, genuinely unknown hops stay silent.
                Walk::ChainBroken => {
                    if matches!(
                        self.frozen_super_verdict(container, row.name),
                        FrozenFace::Absent
                    ) {
                        let name_text = self.strings.resolve(row.name);
                        let type_text = self.strings.resolve(container).to_string();
                        self.emit(
                            SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                            row.range,
                            name_text,
                            &type_text,
                        );
                        self.record_miss(row.name, row.range, true, NameUse::Read);
                    }
                }
                Walk::Missing => {
                    let name_text = self.strings.resolve(row.name);
                    let type_text = self.strings.resolve(container).to_string();
                    self.emit(
                        SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                        row.range,
                        name_text,
                        &type_text,
                    );
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                }
            }
            return;
        }
        // Extensions never apply to `super` accesses; the nullability
        // flag is inert on this path.
        if self.member_defined(container, Ty::UNKNOWN, row, true, false) {
            self.check_member_unused_result(
                row,
                container,
                &Receiver::Super { container, kind },
                false,
            );
            return;
        }
        if matches!(self.walk_super(container, row.name), Walk::ChainBroken) {
            // The broken-hop consult: the frozen SDK interface faces answer
            // a break that the element index cannot carry (`class C extends
            // Match` reaching `super.m`). A write resolves its pair of
            // lookups like `member_defined`'s `name=` arm — the plain
            // spelling satisfies the write through a field/getter hit, and
            // only a miss on BOTH spellings reports the setter shape; a
            // compound write's read face is the getter verdict's own
            // answer. A genuinely unindexed face stays silent.
            let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
            let getter_verdict = self.frozen_super_verdict(container, row.name);
            let setter_verdict = write.then(|| {
                let setter = self.setter_spelling(row.name);
                self.frozen_super_verdict(container, setter)
            });
            let defined = matches!(getter_verdict, FrozenFace::Defined)
                || matches!(setter_verdict, Some(FrozenFace::Defined));
            let getter_absent = matches!(getter_verdict, FrozenFace::Absent);
            let setter_absent =
                matches!(setter_verdict, Some(FrozenFace::Absent)) || !write && getter_absent;
            let undecidable = matches!(getter_verdict, FrozenFace::Unknown)
                || matches!(setter_verdict, Some(FrozenFace::Unknown));
            let name_text = self.strings.resolve(row.name);
            let type_text = self.strings.resolve(container).to_string();
            if defined {
                // The compound write judges its faces separately: `m`
                // absent under a defined `m=` still reports the getter
                // shape, `m` defined under an absent `m=` the setter's.
                if row.intent == Intent::ReadWrite {
                    if getter_absent {
                        self.emit(
                            SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                            row.range,
                            name_text,
                            &type_text,
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                    if matches!(setter_verdict, Some(FrozenFace::Absent)) {
                        self.emit(
                            SemanticCode::UndefinedSuperMember(SuperShape::Setter),
                            row.range,
                            name_text,
                            &type_text,
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                }
                self.check_member_unused_result(
                    row,
                    container,
                    &Receiver::Super { container, kind },
                    false,
                );
                return;
            }
            if undecidable {
                return;
            }
            let shape = match row.intent {
                Intent::Invoke => SuperShape::Method,
                Intent::Write | Intent::ReadWrite => SuperShape::Setter,
                Intent::Read => SuperShape::Getter,
                Intent::Operator(..) => unreachable!("operator intents returned above"),
            };
            // A compound write checks the read face first (the oracle
            // resolves the getter before the setter); its absent faces
            // report in that order.
            if row.intent == Intent::ReadWrite && getter_absent {
                self.emit(
                    SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            if !write || setter_absent {
                self.emit(
                    SemanticCode::UndefinedSuperMember(shape),
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        let name_text = self.strings.resolve(row.name).to_string();
        let type_text = self.strings.resolve(container).to_string();
        // The dispatch walk missed cleanly — the oracle retries through the
        // inherited interface (`getInherited`,
        // property_element_resolver.dart:2035-2058 / 2093-2116): a
        // super-interface still carrying the name (an `implements`
        // signature or an abstract member on the chain) reports
        // `abstract_super_member_reference` with the found member's own
        // kind word; a clean interface miss reports the
        // `undefined_super_member` shape of the access. A compound write's
        // read and write faces judge independently — the read spelling
        // first, then the `name=` setter (upstream runs the read leg's
        // `getMember`/`getInherited` pair before the write leg's
        // `lookUpSetter` pair).
        let report = |engine: &mut Self, spelling: StringId, miss_shape: SuperShape| {
            match engine.walk_super_interface(container, spelling) {
                Walk::Found(entry) => {
                    let shape = match entry.kind {
                        MemberKind::Getter | MemberKind::Field => SuperShape::Getter,
                        MemberKind::Setter => SuperShape::Setter,
                        MemberKind::Method => SuperShape::Method,
                    };
                    engine.emit(
                        SemanticCode::AbstractSuperMemberReference(shape),
                        row.range,
                        &name_text,
                        "",
                    );
                }
                Walk::Missing | Walk::ChainBroken => {
                    engine.emit(
                        SemanticCode::UndefinedSuperMember(miss_shape),
                        row.range,
                        &name_text,
                        &type_text,
                    );
                }
            }
        };
        match row.intent {
            Intent::Invoke => report(self, row.name, SuperShape::Method),
            Intent::Read => report(self, row.name, SuperShape::Getter),
            Intent::Write => {
                let setter = self.setter_spelling(row.name);
                report(self, setter, SuperShape::Setter);
            }
            Intent::ReadWrite => {
                // The read face reports first (the oracle resolves the
                // getter before the setter).
                report(self, row.name, SuperShape::Getter);
                let setter = self.setter_spelling(row.name);
                report(self, setter, SuperShape::Setter);
            }
            Intent::Operator(..) => unreachable!("operator intents returned above"),
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The frozen-face member verdict behind a [`Walk::ChainBroken`]
    /// receiver: climbs the receiver type's supertype graph exactly like
    /// [`Engine::face_member_frozen`] (the `@override` face's consult —
    /// `extends` linear, `with` mixins in reverse application order,
    /// `implements`/`on` faces through the pending queue), but three-valued.
    /// Each `resolve_container` failure classifies by the same split the
    /// member walk applies: a frozen-face spelling
    /// ([`CORE_INTERFACE_MEMBERS`] / [`CORE_TYPE_MEMBERS`], aliased through
    /// [`COLLECTION_INTERFACE_ALIASES`]) answers its member set outright; a
    /// dead edge ([`Engine::supertype_hop_dead`]'s `true` arm — an
    /// undefined, ambiguous, or non-class supertype, the oracle's
    /// `InvalidType` that contributes nothing) keeps the climb decidable;
    /// a resolved-but-unindexed declaration the tables do not know leaves
    /// the chain genuinely broken (`Unknown`, the under-report direction —
    /// its members could carry the name). `Defined` needs the write
    /// spelling joined by its setter form upstream: call sites pass the
    /// single spelling this access resolves through.
    /// Whether `name` is a frozen SDK face the member tables carry (the
    /// `dart:core`/`dart:async`/`dart:io` interfaces plus the
    /// `dart:collection` bases that re-export them, and the `Stream` row
    /// the tables lack) — its members are known exactly, so a spelling it
    /// lacks contributes nothing.
    pub(crate) fn frozen_face_text(name: &str) -> bool {
        CORE_INTERFACE_MEMBERS
            .iter()
            .chain(CORE_TYPE_MEMBERS.iter())
            .chain(SDK_FACES_EXTRA.iter())
            .any(|(container, _)| name == *container)
            || COLLECTION_INTERFACE_ALIASES
                .iter()
                .any(|(alias, _)| name == *alias)
    }

    pub(crate) fn frozen_member_verdict(&self, face: StringId, spelling: StringId) -> FrozenFace {
        self.frozen_faces_verdict([(face, self.file)].into_iter(), spelling, true)
    }

    /// The super-face twin of [`Self::frozen_member_verdict`]: `super.m`'s
    /// lookup starts above the enclosing container's own members, so the
    /// climb seeds with the declaration's supertype edges — its superclass,
    /// `with` mixins, `implements` faces and `on` constraints (the same
    /// seed [`Engine::face_member_any`] reads). An unindexed container
    /// itself breaks the chain: `Unknown`.
    pub(crate) fn frozen_super_verdict(
        &self,
        container: StringId,
        spelling: StringId,
    ) -> FrozenFace {
        let Some(decl) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
        else {
            return FrozenFace::Unknown;
        };
        let seeds = decl
            .superclass
            .into_iter()
            .chain(decl.with_clause.iter().copied())
            .chain(decl.implements_clause.iter().copied())
            .chain(decl.on_clause.iter().copied())
            .map(|face| (face, self.file));
        self.frozen_faces_verdict(seeds, spelling, false)
    }

    /// Whether a `?`-less `FutureOr<A>` receiver is potentially nullable
    /// (type_system.dart `_isNonNullable`: `FutureOr<A>` answers `A`'s
    /// verdict): `A` written with `?`, a top/`Null` argument, an
    /// InvalidType spelling the file failed to resolve, or a nested
    /// `FutureOr` of one. A type-parameter argument stays non-nullable here
    /// (its bound is not modeled — the under-report direction).
    pub(crate) fn future_or_potentially_nullable(&self, ty: Ty) -> bool {
        let future_or = self.intern("FutureOr");
        let mut ty = ty;
        for _ in 0..8 {
            let Ty::Named {
                name,
                nullable: false,
                args,
            } = ty
            else {
                return false;
            };
            if name != future_or {
                return false;
            }
            let Some(&arg) = self.boundary.pool().get(args).first() else {
                return false;
            };
            match arg {
                Ty::Dynamic | Ty::Void | Ty::Null => return true,
                Ty::Named { nullable: true, .. } => return true,
                Ty::Named { name, .. } if self.invalid_type_names.contains(&name) => return true,
                // The argument's own unresolved spelling interns as
                // `Unknown` (the InvalidType slot): nullable exactly when the
                // unit carries an InvalidType spelling; any other `Unknown`
                // is unmodeled and keeps the under-report.
                Ty::Unknown => return !self.invalid_type_names.is_empty(),
                Ty::Named { .. } => ty = arg,
                _ => return false,
            }
        }
        false
    }

    /// The one graph-wide declaration of container `name` outside
    /// `scope_lib`, when exactly one library declares it — the anchor face
    /// for a receiver type the diagnosing library's scope does not carry
    /// (`XmlNodeList` reached unimported through a member's declared type).
    /// Any non-unique answer stays `None`: the climb must not guess.
    pub(crate) fn unique_declared_elsewhere(
        &self,
        name: StringId,
        scope_lib: LibraryId,
    ) -> Option<(FileId, LibraryId)> {
        // The oracle's `InvalidType` posture first: a type the diagnosing
        // file itself spells and cannot resolve is `undefined_class` there
        // and the receiver's static type is Invalid — its member misses
        // stay silent even when another library declares the same name
        // (the corpus's `Context context` from a broken package import).
        // Only a receiver type that arrived through a resolved member
        // signature — never spelled in this file — re-anchors the climb.
        if self
            .report
            .names
            .iter()
            .any(|r| r.name == name && r.use_kind == NameUse::Type && !r.qualified)
            // The miss rows record at their own resolve step, which may
            // run after the member probe: the push-time `Missing`/`NonType`
            // verdict is the same InvalidType spelling (`MessageEvent
            // event` under a broken `package:web` import while `dart:html`
            // elsewhere declares `MessageEvent`). The set latches before
            // the resolve pass; a probe during the parse reads the live
            // rows.
            || self.invalid_type_names.contains(&name)
            || self.rows.iter().any(|r| {
                r.name == name
                    && !r.qualified
                    && matches!(r.shape, RowShape::TypeUse(TypeVerdict::Missing, _))
            })
        {
            return None;
        }
        // A name the diagnosing library pulls through a broken import's
        // `show` list is the same InvalidType face (`import 'missing.dart'
        // show Response;` — the use sites never record a miss, the guard
        // suppresses them).
        if scope_lib == self.lib && self.guard.ignore(None, name) {
            return None;
        }
        let libs = self.graph.declaring_libraries(name);
        if libs.len() != 1 || libs[0] == scope_lib {
            return None;
        }
        let lib = libs[0];
        let file = self.graph.unit_files(lib).iter().copied().find(|&f| {
            self.index
                .get(f)
                .is_some_and(|unit| unit.container(name).is_some())
        })?;
        Some((file, lib))
    }

    /// The shared climb behind [`Self::frozen_member_verdict`] and
    /// [`Self::frozen_super_verdict`] (see there for the three-valued
    /// contract and the hop classification). `anchor` holds when the first
    /// seed IS the receiver type's own spelling rather than a supertype
    /// edge: an unresolvable anchor that no frozen face knows leaves the
    /// receiver statically `InvalidType`, which answers every member face
    /// silently — `Unknown`, never `Absent` (a genuinely undeclared
    /// receiver type must not emit `undefined_*`). An anchor the scope
    /// cannot resolve but exactly one graph library declares re-anchors
    /// the climb there ([`Self::unique_declared_elsewhere`]).
    fn frozen_faces_verdict(
        &self,
        seeds: impl Iterator<Item = (StringId, FileId)>,
        spelling: StringId,
        anchor: bool,
    ) -> FrozenFace {
        let spelling_text = self.strings.resolve(spelling);
        let frozen_face = Self::frozen_face_text;
        let frozen_hit = |name: &str| {
            let aliases: &[&str] = COLLECTION_INTERFACE_ALIASES
                .iter()
                .chain(SDK_SUPER_FACES.iter())
                .find(|(alias, _)| name == *alias)
                .map_or(&[][..], |(_, aliases)| aliases);
            CORE_INTERFACE_MEMBERS
                .iter()
                .chain(CORE_TYPE_MEMBERS.iter())
                .chain(SDK_FACES_EXTRA.iter())
                .any(|(container, members)| {
                    (name == *container || aliases.contains(container))
                        && members.contains(&spelling_text)
                })
        };
        let mut seen = FxHashSet::default();
        let mut undecidable = false;
        // Set when the current `name` hop is a fallback re-attempt (the
        // seen-check must not divert it back to the pending queue).
        let mut retried = false;
        // The `implements`/`on` faces a resolved hop leaves unclimbed — the
        // linear superclass chain handles `extends`+`with`; when it
        // dead-ends, the interface faces keep the climb alive in
        // declaration order (mirrors `face_member_frozen`'s pending queue).
        let mut pending: VecDeque<(StringId, FileId)> = seeds.collect();
        let Some((first, first_owner)) = pending.pop_front() else {
            return FrozenFace::Unknown;
        };
        let mut name = first;
        let mut owner = first_owner;
        let mut scope_lib = self.lib;
        let mut at_anchor = anchor;
        'climb: loop {
            if !retried && !seen.insert(name) {
                let Some((next, next_owner)) = pending.pop_front() else {
                    break;
                };
                name = next;
                owner = next_owner;
                // A queued face is never the receiver type itself.
                at_anchor = false;
                continue;
            }
            retried = false;
            let Some((found, found_owner)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            ) else {
                // The unindexed hop answers by its frozen member face: a
                // hit defines the member, a known face without it
                // contributes nothing (the frozen rows fold the whole
                // instance face, inherited members included), a dead edge
                // contributes nothing, and anything else breaks the climb's
                // decidability — its members could carry the name. The
                // anchor itself is the receiver's whole static type: not
                // frozen-known means `InvalidType`, which never reports.
                let text = self.strings.resolve(name);
                if frozen_hit(text) {
                    return FrozenFace::Defined;
                }
                if !frozen_face(text) {
                    if at_anchor
                        && let Some((declared_file, declared_lib)) =
                            self.unique_declared_elsewhere(name, scope_lib)
                    {
                        // Re-anchor at the unique declaring library: its
                        // namespace resolves the supertype edges the
                        // diagnosing scope never saw. The same spelling
                        // cannot resolve here again (`seen` holds it), so
                        // the retry bypasses the queue check once.
                        scope_lib = declared_lib;
                        owner = declared_file;
                        at_anchor = false;
                        retried = true;
                        continue 'climb;
                    }
                    if at_anchor || !self.supertype_hop_dead(name, scope_lib, owner) {
                        undecidable = true;
                    }
                }
                at_anchor = false;
                let Some((next, next_owner)) = pending.pop_front() else {
                    break;
                };
                name = next;
                owner = next_owner;
                continue;
            };
            at_anchor = false;
            if let Some(entry) = found.members.get(&spelling)
                && !entry.is_static
            {
                return FrozenFace::Defined;
            }
            // A RESOLVED dart: stub container (the SDK projection's
            // names-only units — an empty member map, no modelled
            // clauses) is the frozen rows' face too: the unindexed arm
            // above answers the unresolvable spelling, this one the
            // indexed-but-empty stub (dart:isolate's `ReceivePort`,
            // whose `first` rides `implements Stream`). Membership in a
            // `dart:` library is the whole test: a stub file registers
            // as its node's unit (`library_of_file` answers it), and a
            // unit-less file (the bare rig's single file, a local
            // declaration the diagnosing file itself carries) is never
            // an SDK face — its member face is exactly what the index
            // resolved above, so a local `class Iterable {}` keeps
            // reporting a missing `first`.
            if self
                .graph
                .library_of_file(found_owner)
                .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"))
                && frozen_hit(self.strings.resolve(name))
            {
                return FrozenFace::Defined;
            }
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    continue;
                }
                match resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    owner_lib,
                    owner,
                    *mixin,
                ) {
                    Some((mixin_container, _))
                        if mixin_container
                            .members
                            .get(&spelling)
                            .is_some_and(|entry| !entry.is_static) =>
                    {
                        return FrozenFace::Defined;
                    }
                    None => {
                        let text = self.strings.resolve(*mixin);
                        if frozen_hit(text) {
                            return FrozenFace::Defined;
                        }
                        if !frozen_face(text) && !self.supertype_hop_dead(*mixin, owner_lib, owner)
                        {
                            undecidable = true;
                        }
                    }
                    _ => {}
                }
            }
            for super_face in found.implements_clause.iter().chain(found.on_clause.iter()) {
                pending.push_back((*super_face, found_owner));
            }
            match found.superclass {
                Some(superclass) => {
                    scope_lib = owner_lib;
                    name = superclass;
                    owner = found_owner;
                }
                None => {
                    let Some((next, next_owner)) = pending.pop_front() else {
                        break;
                    };
                    name = next;
                    owner = next_owner;
                    scope_lib = owner_lib;
                }
            }
        }
        if undecidable {
            FrozenFace::Unknown
        } else {
            FrozenFace::Absent
        }
    }

    /// The external-primitive member face (`int x; x.noSuchGetter`): the
    /// receiver type is a dart:core primitive when the name is one of the
    /// [`CORE_TYPE_MEMBERS`] keys and resolves to no indexed container.
    /// Extension members still apply implicitly, so the pool gates first; a
    /// table hit is defined, a miss reports the access-intent trio.
    pub(crate) fn check_core_type_member(
        &mut self,
        container: StringId,
        row: &Row,
        receiver_nullable: bool,
    ) {
        let container_text = self.strings.resolve(container);
        // `Symbol` rides the same face: every symbol literal is
        // `Symbol`-typed (`#void.foo;` resolves `.foo` against it —
        // symbols.dart) and the class declares none of the user selectors.
        const SYMBOL_MEMBERS: [&str; 0] = [];
        let symbol_face = container_text == "Symbol";
        // The `Function` interface's own instance surface: `call` is the
        // one selector the core `Function` type resolves (the
        // `isDartCoreFunction` arm, type_property_resolver.dart:194-198 —
        // every `Object` member joins through the arm below). A miss on the
        // head is DECIDABLE — the oracle reports the access-intent trio
        // against `Function`, so the face rides the same table the
        // primitives do rather than the undecidable under-report.
        const FUNCTION_MEMBERS: [&str; 1] = ["call"];
        let function_face = container_text == "Function";
        let core = CORE_TYPE_MEMBERS
            .iter()
            .find(|(name, _)| container_text == *name)
            .map(|(name, members)| (*name, *members));
        let Some((_, members)) = core
            .or_else(|| symbol_face.then_some(("", &SYMBOL_MEMBERS[..])))
            .or_else(|| function_face.then_some(("", &FUNCTION_MEMBERS[..])))
        else {
            // Not a known primitive — an UNDECIDABLE receiver head (`Never`
            // through the names-only stub, a `?`-less type-parameter
            // spelling): the potentially-nullable verdict alone still
            // reports (#2815 R1 — `type_property_resolver`'s nullable branch
            // never reaches the member question), the nullability predicate
            // itself deciding parameter-hood. An applicable extension face
            // resolves first (the nullable branch's own order — only a
            // face whose `on` spelling covers the nullable receiver answers
            // `Single` and suppresses; `UnsafeSingle` keeps the report, the
            // same split the member sites encode); a merely-unresolvable
            // name answers `false` in the predicate (the oracle's
            // InvalidType posture) and keeps the suppression.
            let app = self.extension_application(
                container,
                Ty::named(container),
                row.name,
                matches!(row.intent, Intent::Write | Intent::ReadWrite),
                receiver_nullable,
            );
            // An UNNAMED extension's application face rides its own member
            // map (the pool above carries named declarations only): a hit
            // resolves the access the same `Single` way.
            let unnamed_face_resolves = self
                .unnamed_extension_members()
                .is_some_and(|members| members.contains_key(&row.name));
            // A receiver riding a nullable-own-param face's own parameter
            // (`extension<T> on T?`'s `T`): the member-gated consult above
            // answers on the ACCESSED member, so a face the receiver's own
            // type is armed by never shows up for a member it does not
            // declare. The oracle instantiates the parameter at the
            // application site and resolves the access against the
            // instantiation — a question the name level cannot carry — so
            // the armed face suppresses on the type itself (the same
            // type-level verdict `extension_application`'s own-param arm
            // encodes, read here without the member gate).
            let own_param_armed = receiver_nullable
                && self.accessible_extension_faces().iter().any(|(face, _)| {
                    face.on_type_nullable
                        && face.on_type == Some(container)
                        && face.type_param_names.contains(&container)
                });
            if receiver_nullable
                && !self.members.object_members.contains(&row.name)
                && !unnamed_face_resolves
                && !matches!(app, ExtensionApp::Single)
                && !own_param_armed
            {
                let name_text = self.strings.resolve(row.name);
                self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
                self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            }
            return;
        };
        if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .is_some()
        {
            return; // locally declared: the regular walk already decided
        }
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        // The core-type face knows the receiver's spelling only — the
        // consult reads the UNKNOWN-typed spelling and the caller's
        // nullability verdict. A non-nullable `on` face applied to a
        // nullable receiver is the oracle's unsafe application: the member
        // resolves but the dereference reports the shared unchecked code
        // at the selector (`applicableTo`'s non-nullable `on` never covers
        // a nullable `FutureOr<A>` receiver — the #3457 `x.om()` face).
        match self.extension_application(
            container,
            Ty::UNKNOWN,
            row.name,
            write,
            receiver_nullable,
        ) {
            ExtensionApp::None => {}
            ExtensionApp::Single => return,
            ExtensionApp::UnsafeSingle => {
                if receiver_nullable {
                    let name_text = self.strings.resolve(row.name);
                    self.emit_unchecked_nullable(
                        row.range,
                        name_text,
                        access_kind(&row.intent),
                    );
                    self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                }
                return;
            }
            ExtensionApp::Ambiguous(displays) => {
                self.emit_extension_ambiguity(row.name, row.range, displays);
                return;
            }
        }
        let name_text = self.strings.resolve(row.name);
        // A recovery-spliced synthetic selector never reports on the Symbol
        // face (the oracle's `#foo.void` keeps the keyword error only).
        if symbol_face && name_text.is_empty() {
            return;
        }
        if members.contains(&name_text)
            || (write && members.contains(&self.strings.resolve(self.setter_spelling(row.name))))
        {
            // The nullable-dereference face (nullable_dereference_verifier
            // .dart report — the same emission the walkable-model found-arm
            // makes): a member defined on the core type read through a
            // merely-nullable receiver is the unchecked-access row at the
            // selector. The oracle's Object-member reads (`s.hashCode`,
            // `s.toString()`, `s == other`) stay silent — the member face
            // answers without a dereference question — so the
            // `==`/Object arm below keeps its under-report.
            if receiver_nullable && !self.members.object_members.contains(&row.name) {
                self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
            }
            return;
        }
        if row.name == self.intern("==") || self.members.object_members.contains(&row.name) {
            return;
        }
        // A table miss through a merely-nullable receiver is the oracle's
        // unchecked-access row — the `isNullable` tail reports the shared
        // `unchecked_use_of_nullable_value` code and never reaches the
        // `undefined_*` family.
        if receiver_nullable {
            self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        let type_text = container_text.to_string();
        match row.intent {
            Intent::Invoke => self.emit(
                SemanticCode::UndefinedMethod,
                row.range,
                name_text,
                &type_text,
            ),
            Intent::Write | Intent::ReadWrite => {
                if row.intent == Intent::ReadWrite {
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                }
                self.emit(
                    SemanticCode::UndefinedSetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Read => self.emit(
                SemanticCode::UndefinedGetter,
                row.range,
                name_text,
                &type_text,
            ),
            Intent::Operator(..) => {}
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The member verdict on a shape-typed receiver — a FUNCTION type
    /// (`f.bar` over `var f = () => 0`, a method tear-off `m.bar`, a
    /// `int Function()` formal, a typedef-to-function head) or a RECORD
    /// type (`r.foo` over `var r = (1, "a")`) — upstream's non-interface
    /// arms of the property resolver (`type_property_resolver.dart:202-234`,
    /// `property_element_resolver.dart:540-544`): `call` resolves a
    /// function receiver's own signature, a record's `fieldByName` resolves
    /// its `$n`/named slots, the extension pool answers next, and the
    /// `Object` interface closes the lookup. A clean miss reports the
    /// access-intent trio — `undefined_getter`/`_setter` name the
    /// receiver's own type display (`display`, the `targetType` argument of
    /// property_element_resolver.dart:619-625 — the alias spelling when the
    /// receiver spelled one, the shape otherwise), `undefined_method` the
    /// invocation resolver's `receiverClassName` for the shape
    /// (`method_invocation_resolver.dart:1190-1199`: `Function` for a
    /// `FunctionType`, `<unknown>` for a record). A merely-nullable
    /// receiver never reaches the `undefined_*` family — the `isNullable`
    /// branch resolves `Object` members and nullable-`on` extensions
    /// silently, then reports the shared unchecked code
    /// (`type_property_resolver.dart:96-148`).
    pub(crate) fn check_shape_receiver(
        &mut self,
        row: &Row,
        shape_ty: Ty,
        display: &str,
        receiver_nullable: bool,
    ) {
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        let record_field = !receiver_nullable
            && matches!(shape_ty, Ty::Record { .. })
            && self
                .record_field_ty(
                    shape_ty,
                    row.name,
                    self.null_aware_members.contains(&row.range.offset),
                )
                .is_some();
        // A record field resolves the READ face only — records are
        // immutable upstream (`property_element_resolver`'s record arm
        // consults the field getters; no setter element exists), so a
        // write to an existing field reports `undefined_setter` on the
        // record (`r.bar = v`), and a compound read-write the same
        // (`r.bar += 1` — the getter resolved, the setter's face alone
        // reports). `receiver_nullable` stays ahead: the `isNullable`
        // tail's unchecked row precedes the `undefined_*` family.
        if write && record_field {
            let name_text = self.strings.resolve(row.name);
            self.emit(SemanticCode::UndefinedSetter, row.range, name_text, display);
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        // A compound write's READ face issues its own verdict: the write's
        // `name=` consult never answers it, and the record arm's flags are
        // independent (`ResolutionResult.needsGetterError`/`needsSetterError`,
        // type_property_resolver.dart:218-225 — a field miss sets both, a
        // field hit sets the getter flag false and leaves the setter flag
        // standing). So a record WITHOUT the field still reports
        // `undefined_getter` on the record display when an extension setter
        // alone resolves the write (`r.foo += 1` over
        // `extension E on ({int bar}) { set foo(int v) {} }` — the oracle
        // reports the getter miss alone). A read answered by the record
        // field above, an extension getter or an `Object` member stays
        // silent, and the full-miss tail below reports the pair itself.
        if matches!(row.intent, Intent::ReadWrite)
            && !receiver_nullable
            && matches!(shape_ty, Ty::Record { .. })
            && !record_field
            && !self.members.object_members.contains(&row.name)
            && row.name != self.intern("==")
            && self.record_extension_face(shape_ty, row.name).is_none()
        {
            let setter = self.setter_spelling(row.name);
            if self.record_extension_face(shape_ty, setter).is_some() {
                let name_text = self.strings.resolve(row.name);
                self.emit(SemanticCode::UndefinedGetter, row.range, name_text, display);
            }
        }
        // `call` resolves a non-nullable function receiver's own signature
        // (`callFunctionType`, type_property_resolver.dart:202-209) — a
        // nullable receiver reaches it only through the extension/`Object`
        // path, where `call` declares nothing. A record's own fields
        // resolve the access whole (`fieldByName`,
        // type_property_resolver.dart:218-225). A write resolves through
        // the `name=` extension spelling like every member face (the
        // oracle's `writeElement` lookup).
        let resolved_spelling = if (!receiver_nullable
            && matches!(shape_ty, Ty::Function { .. })
            && row.name == self.intern("call"))
            // A record field resolves the READ face only — records are
            // immutable upstream (`property_element_resolver`'s record arm
            // consults the field getters; no setter element exists), so
            // `r.bar = v` reports `undefined_setter` on the record even
            // though `bar` is a declared field.
            || !write
                && matches!(shape_ty, Ty::Record { .. })
                && self
                    .record_field_ty(
                        shape_ty,
                        row.name,
                        self.null_aware_members.contains(&row.range.offset),
                    )
                    .is_some()
            || self.members.object_members.contains(&row.name)
            || row.name == self.intern("==")
        {
            None
        } else if self
            .record_extension_face(self.unwrapped_shape_ty(shape_ty, row), row.name)
            .is_some()
        {
            Some(row.name)
        } else {
            let setter = self.setter_spelling(row.name);
            if write
                && self
                    .record_extension_face(self.unwrapped_shape_ty(shape_ty, row), setter)
                    .is_some()
            {
                Some(setter)
            } else {
                // `receiverClassName` for the invocation face: `Function`
                // for a `FunctionType` receiver, `<unknown>` for a record
                // (method_invocation_resolver.dart:1190-1199 — only an
                // `InterfaceTypeImpl` or a `FunctionType` escapes the
                // default).
                let method_face = if matches!(shape_ty, Ty::Function { .. }) {
                    "Function"
                } else {
                    "<unknown>"
                };
                self.shape_receiver_miss(row, display, method_face, receiver_nullable);
                return;
            }
        };
        if let Some(spelling) = resolved_spelling {
            self.credit_record_extension_use(shape_ty, spelling, row.range.offset);
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// `shape_ty` with the `?` flag stripped when the access `?.`-unwrapped
    /// its receiver — a `f?.bar` member row's `null_aware_members` latch
    /// marks the unwrap, so the extension consult reads the non-null shape
    /// (upstream's `applicableTo` runs against the promoted non-null static
    /// type, never `T?` — the same unwrap
    /// [`Self::credit_record_extension_use`] applies to its credit face).
    fn unwrapped_shape_ty(&self, shape_ty: Ty, row: &Row) -> Ty {
        if !self.null_aware_members.contains(&row.range.offset) {
            return shape_ty;
        }
        match shape_ty {
            Ty::Record { shape, .. } => Ty::Record {
                shape,
                nullable: false,
            },
            Ty::Function { shape, .. } => Ty::Function {
                shape,
                nullable: false,
            },
            other => other,
        }
    }

    /// [`Self::check_shape_receiver`]'s miss tail: a merely-nullable
    /// receiver reports the shared unchecked code (`isNullable`'s tail
    /// precedes the `undefined_*` family); a non-nullable one reports the
    /// access-intent trio.
    fn shape_receiver_miss(
        &mut self,
        row: &Row,
        display: &str,
        method_face: &str,
        receiver_nullable: bool,
    ) {
        let name_text = self.strings.resolve(row.name);
        if receiver_nullable {
            self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        match row.intent {
            Intent::Invoke => {
                self.emit(
                    SemanticCode::UndefinedMethod,
                    row.range,
                    name_text,
                    method_face,
                );
            }
            Intent::Read => {
                self.emit(SemanticCode::UndefinedGetter, row.range, name_text, display);
            }
            Intent::Write => {
                self.emit(SemanticCode::UndefinedSetter, row.range, name_text, display);
            }
            Intent::ReadWrite => {
                // The oracle resolves the read before the write — a
                // compound access reports both faces in that order.
                self.emit(SemanticCode::UndefinedGetter, row.range, name_text, display);
                self.emit(SemanticCode::UndefinedSetter, row.range, name_text, display);
            }
            // The operator application on a shape receiver is a member
            // miss the same property consult answers (`f < g`, `(1,'x')
            // + r`): the `==`/`call`/own-field/extension spellings
            // resolved above, so a clean miss reports `undefined_operator`
            // at the operator token naming the receiver's own shape
            // display (binary_expression_resolver.dart's
            // `instanceReceiverType` — the composite type, never the
            // method's `receiverClassName`).
            Intent::Operator(_, op_range) => {
                self.emit(SemanticCode::UndefinedOperator, op_range, name_text, display);
            }
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The read and write operator spellings an access needs (`[]` / `[]=`
    /// for index rows by their write form; the plain spelling for binary
    /// operators); `(None, None)` for non-operator rows.
    pub(crate) fn operator_spellings(&self, row: &Row) -> (Option<StringId>, Option<StringId>) {
        let Intent::Operator(op, _) = &row.intent else {
            return (None, None);
        };
        if *op == self.intern("[]") {
            match row.write_op {
                Some(OpWrite::Assign) => (None, Some(self.intern("[]="))),
                Some(OpWrite::Compound) => (Some(*op), Some(self.intern("[]="))),
                None => (Some(*op), None),
            }
        } else {
            (Some(*op), None)
        }
    }

    /// Whether a bare-miss row's write side rescues it from
    /// `undefined_identifier`. Only a pure `Intent::Write` (`name = v`)
    /// consults the setter namespace upstream (`preferSetter`): a `name=`
    /// hit resolves the write silently. Every other intent — a bare read,
    /// a compound `name++`/`name += v`, a prefix `++name` — still reads the
    /// GETTER namespace, where a setter-only name is recovery, never the
    /// requested element (lexical_lookup.dart:22-35), so it reports. The
    /// write side the pass cannot re-walk (#2926 F2's extension-body
    /// `foo = 0`) rides the push-time `ext_write_target_rows` latch under
    /// the same gate.
    fn write_target_rescues(&mut self, row: &Row, this_type: Option<StringId>) -> bool {
        row.intent == Intent::Write
            && (self.ext_write_target_rows.contains(&row.range.offset)
                || self.bare_write_target_exists(row.name, this_type))
    }

    /// The `name=` lookup spelling of a write target. Cached: the `format!`
    /// plus intern allocation repeats for the same write targets across a
    /// unit, and pass-time member checks ask on every write.
    pub(crate) fn setter_spelling(&mut self, name: StringId) -> StringId {
        self.members.setter_spelling(self.strings, name)
    }

    /// A bare write's second-chance setter resolution (`m = v` where `m`
    /// exists only as a setter): the push-time tentative-read lookup sees
    /// plain-name declarations — getters, fields, top-levels — and never
    /// the `name=` spelling, because the assignment upgrades the row's
    /// intent after the push. The oracle's write resolution consults the
    /// setter namespace (`preferSetter`), so a top-level or implicit-this
    /// setter defines the write silently. A getter-only name stays
    /// under-reported (the oracle's `undefined_setter` needs a receiver
    /// type this face cannot supply).
    ///
    /// The implicit-`this` arm's write FACE — the setter's value formal the
    /// assignment check compares the source against — is
    /// [`Self::bare_write_formal_ty`]'s answer; the two read the same
    /// `name=` hit under the same accessibility gate, so a write this
    /// predicate leaves reporting never acquires an assignability verdict.
    pub(crate) fn bare_write_target_exists(
        &mut self,
        name: StringId,
        this_type: Option<StringId>,
    ) -> bool {
        let setter = self.setter_spelling(name);
        // The scoped lookup runs live (top-level setters resolve here); the
        // implicit-this arm rides the push-time snapshot (`this_type`) —
        // the frame stack is gone by pass 2.
        if !matches!(self.resolve_bare(setter).0, BareInfo::Missing { .. }) {
            return true;
        }
        this_type.is_some_and(|container| {
            resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                container,
            )
            .is_some_and(|(decl, owner)| {
                decl.members.contains_key(&setter)
                    && self.member_accessible_in(name, owner)
            })
        })
    }

    /// The write face of a bare write the getter-side lookup MISSED: the
    /// implicit-`this` `name=` setter's value formal. A setter-only member
    /// (`set only(T v)` with no getter) is exactly that shape — the
    /// push-time tentative read consults the getter namespace and finds
    /// nothing, so the row carries `BareInfo::Missing` and no declared face,
    /// while the oracle's write resolution resolves the `name=` element and
    /// checks the source against ITS type (`AssignmentExpressionImpl
    /// .writeType` — upstream Dart SDK source, pkg/analyzer/lib/src/dart/
    /// resolver/assignment_expression_resolver.dart:54/:83/:361; the write
    /// element is the setter, whose type IS the value formal).
    ///
    /// The verdict is [`Self::bare_write_target_exists`]'s implicit-`this`
    /// arm verbatim — the same own-container `name=` hit under the same
    /// accessibility gate — so a write that rescue leaves reporting
    /// (`undefined_identifier`) answers no write face here either: the
    /// assignability check stays silent rather than inventing a type over a
    /// reference the model deemed unresolved. `Unknown` for a getter-only
    /// name (the setter element does not exist), a foreign-private setter,
    /// a broken chain, and every off-instance context.
    pub(crate) fn bare_write_formal_ty(
        &mut self,
        name: StringId,
        this_type: Option<StringId>,
    ) -> Ty {
        let Some(container) = this_type else {
            return Ty::UNKNOWN;
        };
        let setter = self.setter_spelling(name);
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .filter(|(_, owner)| self.member_accessible_in(name, *owner))
        .and_then(|(decl, _)| decl.members.get(&setter))
        .and_then(|entry| entry.formals.as_deref())
        .and_then(|formals| formals.positional.first())
        .map_or(Ty::UNKNOWN, |formal| self.fold_alias_ty(formal.ty))
    }

    /// The implicit-`this` member's accessibility gate (upstream
    /// `element.isAccessibleIn(_definingLibrary)` —
    /// property_element_resolver.dart:463-465): a `_`-prefixed member
    /// declared in another library is invisible to this unit's `this`
    /// lookup even when the name resolves on the face — it answers
    /// `undefined_identifier`, never the member. `owner` is the file the
    /// member was found on; a same-file or same-library hit is always
    /// accessible, and a public member is accessible from anywhere.
    fn member_accessible_in(&self, name: StringId, owner: FileId) -> bool {
        !self.is_private_name(name)
            || self.graph.library_of_file(owner) == Some(self.lib)
    }

    /// A member's result type as far as M1 knows it: the declared field type
    /// or getter return type; methods (function-typed tear-offs) and setters
    /// stay [`Ty::Unknown`] — every check on them suppresses.
    pub(crate) fn member_result_ty(entry: &MemberEntry) -> Ty {
        match entry.kind {
            MemberKind::Field => entry.var_ty,
            MemberKind::Getter => entry.return_ty,
            MemberKind::Method | MemberKind::Setter => Ty::UNKNOWN,
        }
    }

    /// The row's type read as the enclosing EXPRESSION's static type: a
    /// `?.` short-circuit makes the whole chain node nullable — `a?.b`
    /// types `B?` and `a?.b.c` types `C?` (upstream Dart SDK source,
    /// pkg/analyzer's `ShortCircuiting` resolution: the `?` joins the
    /// result of every node the operator guards). The row keeps the
    /// member's own declared face — the continuation's receiver check reads
    /// that face (`a?.b.c` resolves `c` on `b`'s written `B`/`B?`, never
    /// the short-circuit's `B?`) — so the `?` re-arms only where the row is
    /// consumed as a result (an assignment's right side, a `return`, an
    /// argument, a condition operand). The member-offset latch
    /// ([`Self::null_aware_members`]) marks every chain node the `?.`
    /// guards: the `?.` member itself plus the plain continuations its
    /// merge short-circuits (`merge_member` propagates the latch while the
    /// receiver it folded reads non-null — a nullable member result keeps
    /// the continuation's own unchecked-dereference verdict). An
    /// operator-merged row keeps its operator result — and a row inside a
    /// statically-`bool` composite keeps the composite's `bool`: `x?.m ==
    /// y` types `bool`, never `bool?` — so the upgrade leaves both alone.
    pub(crate) fn expression_row_ty(&self, row: &Row) -> Ty {
        let ty = if row.intent == Intent::Invoke {
            // An invocation expression's static type is the send's RESULT,
            // never the callee face the row carries: a function-typed
            // callee's signature return (`f(x)` on `int Function(int)`
            // types `int`), a resolved callee's declared return
            // (`typedef F<T> = T Function(); F<int> mk();` reads `F<int>`,
            // the alias face the oracle's `recordStaticType` records),
            // and a callable value's `.call` dispatch. The resolved
            // callee's return wins over the stored face — an alias-typed
            // callee stores its EXPANDED signature on the row, which the
            // callable-result read would then apply a second time
            // (`void Function() g(); g()` stored `void Function()` and
            // misread `void` — the `use_of_void_result` double-apply,
            // #3402). The context face and the callable face are the
            // fallback order: a generic send whose declared return stays
            // free answers the context its argument position supplies
            // (#3064's `h(a.remainder(f()))` numeric ladder /
            // `a[g()]++` index formal — resolver.dart:3378's
            // `analyzeExpression(node.index, indexContextType)`),
            // and an unresolved callee whose row carries a function type
            // still answers its signature's return.
            let send = self.callee_return_ty(row);
            let send = if send != Ty::UNKNOWN {
                send
            } else if let Some(result) = self.invoked_callee_result(row) {
                result
            } else if let Some(context) = self.context_invoke_ty(row) {
                context
            } else {
                // A folded creation whose row carries nothing: the
                // expression's static type is the instantiated head
                // (`A<int>()` is `A<int>`) — the written arguments
                // reattach from the creation row (#2873) so the
                // assignment faces decide and DISPLAY the instantiation
                // (`A value of type 'A<int, dynamic>' ...`), never the
                // bare fold-sentinel head. An inferred creation has no
                // written entry and keeps the row's own face.
                match &row.shape {
                    RowShape::Receiver(Receiver::Created { name, kind, .. })
                        if *kind != ContainerKind::Extension =>
                    {
                        self.creation_row_ty(*name, false, row.range.offset)
                            .unwrap_or(row.ty)
                    }
                    _ => row.ty,
                }
            };
            // A stored send type stands where it already IS the resolved
            // answer: the `!` rewrite (`f()!` promoted the row to the
            // operand's non-null form — the only face that non-nulls a
            // send row) and a creation fold's solved instance. Everything
            // else re-decides above.
            let asserted = self
                .invocation_span(row.range)
                .map(|span| self.null_asserted_after(span, send))
                .unwrap_or(send);
            if send == Ty::UNKNOWN
                || row.ty == asserted
                || (row.ty != Ty::UNKNOWN && row.ty == Self::non_null_form(send))
            {
                row.ty
            } else {
                asserted
            }
        } else {
            match &row.shape {
                RowShape::Receiver(Receiver::Created { name, kind, .. })
                    if *kind != ContainerKind::Extension =>
                {
                    self.creation_row_ty(*name, false, row.range.offset)
                        .unwrap_or(row.ty)
                }
                _ => row.ty,
            }
        };
        if self.null_aware_members.contains(&row.range.offset)
            && !matches!(row.intent, Intent::Operator(..))
            && !self.bool_decided_covers(row)
        {
            Self::nullable_form(ty)
        } else {
            ty
        }
    }

    // —— the generic send's downward context (#3064) ————————————————————
    //
    // Upstream resolves every send under a context type
    // (`analyzeExpression(node, contextType)`): an argument's schema is
    // its formal's type, and the `+`/`-`/`*`/`%`/`remainder`/`clamp`
    // faces refine it (`refineNumericInvocationContext`,
    // type_system.dart:1881-1990) over the receiver's static type —
    // `h(a.remainder(f()))` with `h(int x)` hands `f<T>` the context
    // `int`, so `T` binds `int` and the invocation's static type is
    // `int`. The row model reads the same answer off the token table:
    // the position shapes are syntactic, and the numeric ladder needs
    // only the receiver's face and the enclosing formal.

    /// The send-position shape of the send `row`: an index argument
    /// (`a[e]`), or a call argument with its enclosing open paren,
    /// selector spelling, named label, and positional index. `None` for
    /// every other neighborhood (a statement tail, a binary operand, a
    /// collection element — the shapes whose context this face does not
    /// resolve).
    fn send_position(&self, row: &Row) -> Option<SendPosition> {
        let tokens = self.tokens.as_slice();
        let open = tokens.partition_point(|token| token.utf16_offset < row.range.end());
        if tokens.get(open)?.kind != dartq_scanner::TokenKind::OpenParen {
            return None;
        }
        let close = self.tokens.matching_end(open)?;
        match tokens.get(close + 1)?.kind {
            // `a[ f() ]` — the `]` closes the bracket the send sits in.
            dartq_scanner::TokenKind::CloseSquare => {
                // The matching `[`: backward from the send's own `(`,
                // skipping the nested groups its arguments opened.
                let mut depth = 0usize;
                let mut j = open;
                let lbracket = loop {
                    if j == 0 || open - j > 128 {
                        return None;
                    }
                    j -= 1;
                    match tokens[j].kind {
                        dartq_scanner::TokenKind::CloseParen
                        | dartq_scanner::TokenKind::CloseSquare
                        | dartq_scanner::TokenKind::CloseCurly => depth += 1,
                        dartq_scanner::TokenKind::OpenSquare => {
                            if depth == 0 {
                                if self.tokens.matching_end(j) == Some(close + 1) {
                                    break j;
                                }
                                return None;
                            }
                            depth -= 1;
                        }
                        dartq_scanner::TokenKind::OpenParen | dartq_scanner::TokenKind::OpenCurly => {
                            if depth == 0 {
                                return None;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                };
                // The receiver-expression gate: a token that can end an
                // operand stands before the `[` (`a[f()]`, `m[k][f()]`);
                // a type-argument list's `>` (`<int>[f()]`) or an
                // assignment `=` marks a collection literal instead.
                if lbracket == 0 || !Self::operand_ending_kind(tokens[lbracket - 1].kind) {
                    return None;
                }
                // The pure-write face: `a[e] = v` resolves the index under
                // `[]=`'s first formal (`resolveForWrite`'s
                // `hasRead: false` — property_element_resolver.dart:1108's
                // `writeElement?.firstParameterType`); the read and the
                // compound twins use `[]`'s (`hasRead: true`).
                let write_only = tokens
                    .get(close + 2)
                    .is_some_and(|token| token.kind == dartq_scanner::TokenKind::Eq);
                Some(SendPosition::Index {
                    lbracket,
                    write_only,
                })
            }
            // `p(f(), …)` / `p(…, f())` — an argument of the enclosing call.
            dartq_scanner::TokenKind::Comma | dartq_scanner::TokenKind::CloseParen => {
                let mut depth = 0usize;
                let mut j = open;
                let parent_open = loop {
                    if j == 0 || open - j > 128 {
                        return None;
                    }
                    j -= 1;
                    match tokens[j].kind {
                        dartq_scanner::TokenKind::CloseParen
                        | dartq_scanner::TokenKind::CloseSquare
                        | dartq_scanner::TokenKind::CloseCurly => depth += 1,
                        dartq_scanner::TokenKind::OpenParen => {
                            if depth == 0 {
                                break j;
                            }
                            depth -= 1;
                        }
                        // A collection literal between the send and the
                        // paren (`h([f()])`) — the send is no argument.
                        dartq_scanner::TokenKind::OpenSquare | dartq_scanner::TokenKind::OpenCurly => {
                            if depth == 0 {
                                return None;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                };
                // The selector must be an identifier — control-flow
                // parens (`if (f())`) and parenthesized groups spell
                // keywords or operators here.
                let sel = tokens.get(parent_open.checked_sub(1)?)?;
                if sel.kind != dartq_scanner::TokenKind::Identifier {
                    return None;
                }
                let selector = self.intern(&self.source[sel.byte_range()]);
                // The named-label face: `m(x: f())` reads the named
                // formal the label spells. A member send's row anchors at
                // its member token, but the argument — and the label —
                // begin at the receiver (`m(x: a.b.f())` labels at `a`).
                // The label must open the argument — `(` or a depth-0
                // comma stands before it — so a conditional expression's
                // `:` (`f(a ? b : f())`) never masquerades as one.
                let head = self.expression_start(row);
                let at = tokens.partition_point(|token| token.utf16_offset < head);
                let label = (at >= 3
                    && tokens[at - 1].kind == dartq_scanner::TokenKind::Colon
                    && tokens[at - 2].kind == dartq_scanner::TokenKind::Identifier
                    && matches!(
                        tokens[at - 3].kind,
                        dartq_scanner::TokenKind::OpenParen | dartq_scanner::TokenKind::Comma
                    ))
                    .then(|| self.intern(&self.source[tokens[at - 2].byte_range()]));
                // The positional index: the depth-0 commas between the
                // enclosing `(` and the send's own head. The backward scan
                // above broke at depth 0, so the counter restarts there.
                let mut index = 0usize;
                for token in &tokens[parent_open + 1..at] {
                    match token.kind {
                        dartq_scanner::TokenKind::OpenParen
                        | dartq_scanner::TokenKind::OpenSquare
                        | dartq_scanner::TokenKind::OpenCurly => depth += 1,
                        dartq_scanner::TokenKind::CloseParen
                        | dartq_scanner::TokenKind::CloseSquare
                        | dartq_scanner::TokenKind::CloseCurly => depth -= 1,
                        dartq_scanner::TokenKind::Comma if depth == 0 => index += 1,
                        _ => {}
                    }
                }
                Some(SendPosition::Argument {
                    open: parent_open,
                    selector,
                    label,
                    index,
                })
            }
            _ => None,
        }
    }

    /// The context type the send at `row` resolves under: an index
    /// argument reads its `[]`/`[]=` formal, a call argument reads the
    /// enclosing formal refined by the numeric context ladder
    /// ([`Self::refine_numeric_context`]). `None` keeps the
    /// unknown-suppress posture — a statement tail, a binary operand
    /// (the operator-param context needs the enclosing assignment's own
    /// context to decide `int` vs `num`, a climb the pinned faces do not
    /// need and the unknown answer matches today's suppression), or an
    /// unresolvable position.
    fn invoke_downward_context(&self, row: &Row) -> Option<Ty> {
        match self.send_position(row)? {
            SendPosition::Index {
                lbracket,
                write_only,
            } => self.index_formal_context(lbracket, write_only),
            SendPosition::Argument {
                open, selector, ..
            } => {
                // The enclosing send's callee row: the last row spelling
                // the selector that closed before its argument list —
                // the same resolution the row stack already landed.
                let open_offset = self.tokens.as_slice()[open].utf16_offset;
                let at = self
                    .rows
                    .iter()
                    .rposition(|probe| probe.name == selector && probe.range.end() <= open_offset)?;
                let parent = &self.rows[at];
                // A cascade section resolves without the enclosing context
                // (upstream `test_remainder_int_context_cascaded` infers
                // `num` where the plain call infers `int` — the section's
                // own resolution never sees the outer argument's formal).
                let cascade = self.cascade_selector_before(parent.range.offset);
                let context = if cascade {
                    None
                } else {
                    self.send_position_context(parent)
                };
                // The ladder reads the enclosing send's receiver face and
                // its selector spelling. At the argument flush the parent
                // send has NOT merged yet (the dot reduction follows the
                // send's own close), so the receiver is the row below the
                // parent's callee row — a member parent only (a bare
                // callee has no receiver).
                let recv_ty = if parent.qualified {
                    match self.rows.get(at.checked_sub(1)?) {
                        Some(recv_row) if recv_row.range.end() <= parent.range.offset => {
                            self.receiver_static_ty(recv_row)
                        }
                        _ => Ty::UNKNOWN,
                    }
                } else {
                    Ty::UNKNOWN
                };
                let name = self.strings.resolve(parent.name);
                self.refine_numeric_context(&name, recv_ty, context)
            }
        }
    }

    /// The context at a send's OWN position — the plain formal its
    /// argument sits against (the upstream contextType stack pushes the
    /// written formal, unrefined; only the faces *inside* the send get
    /// the numeric ladder). One climb, no deeper.
    fn send_position_context(&self, callee: &Row) -> Option<Ty> {
        match self.send_position(callee)? {
            SendPosition::Index {
                lbracket,
                write_only,
            } => self.index_formal_context(lbracket, write_only),
            SendPosition::Argument {
                open,
                selector,
                label,
                index,
            } => {
                let open_offset = self.tokens.as_slice()[open].utf16_offset;
                let at = self
                    .rows
                    .iter()
                    .rposition(|probe| probe.name == selector && probe.range.end() <= open_offset)?;
                let formals = self.send_formals(&self.rows[at])?;
                let formal = match label {
                    Some(label) => formals.named_formal(label)?.ty,
                    None => formals.positional.get(index)?.ty,
                };
                (formal != Ty::UNKNOWN).then_some(formal)
            }
        }
    }

    /// The first index formal of the receiver whose `[` opens at
    /// `lbracket`: `[]`'s first formal for a read/compound access, the
    /// `[]=` spelling for a pure write (`a[e] = v`) — with the write
    /// spelling as the read fallback when the receiver declares no `[]`
    /// (a write-only index). Unresolved receivers and missing formals
    /// keep `None`.
    fn index_formal_context(&self, lbracket: usize, write_only: bool) -> Option<Ty> {
        let tokens = self.tokens.as_slice();
        let bracket_offset = tokens[lbracket].utf16_offset;
        // The receiver row: the closest row wholly before the `[` — the
        // same geometry `handle_indexed_expression`'s fold reads (the
        // `[]` operator mark lands AFTER the staging ask, so the shape —
        // not the intent — decides here).
        let at = self
            .rows
            .iter()
            .rposition(|probe| probe.range.end() <= bracket_offset)?;
        let recv = match &self.rows[at].shape {
            RowShape::Member { recv } => *recv,
            RowShape::Bare(info) => Engine::receiver_of_bare(*info),
            _ => return None,
        };
        let read = self.intern("[]");
        let write = self.intern("[]=");
        let spellings: &[StringId] = if write_only { &[write] } else { &[read, write] };
        for &spelling in spellings {
            let ty = self
                .member_formals_full(&recv, spelling)
                .as_deref()
                .and_then(|formals| formals.positional.first())
                .map(|formal| formal.ty)
                .filter(|ty| *ty != Ty::UNKNOWN);
            if let Some(ty) = ty {
                return Some(ty);
            }
        }
        None
    }

    /// The formal table of the send `row`'s callee: a member send walks
    /// its receiver's face ([`Self::member_formals_full`]), a bare send
    /// reads the top-level declaration.
    fn send_formals(&self, row: &Row) -> Option<Formals> {
        match &row.shape {
            RowShape::Member { recv } => self
                .member_formals_full(recv, row.name)
                .map(|formals| formals.into_owned()),
            RowShape::Bare(_) => self
                .top_level_decl(row.name)
                .and_then(|decl| decl.formals.as_deref().cloned()),
            _ => None,
        }
    }

    /// Whether the selector at `offset` follows a cascade operator
    /// (`a..m(...)`, `a?..m(...)`).
    fn cascade_selector_before(&self, offset: u32) -> bool {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < offset);
        at > 0
            && matches!(
                tokens[at - 1].kind,
                dartq_scanner::TokenKind::PeriodPeriod
                    | dartq_scanner::TokenKind::QuestionPeriodPeriod
            )
    }

    /// The expression's own start offset for the token-position reads: a
    /// member send's row anchors at its member token, while the
    /// expression — the argument's label and comma geometry — begins at
    /// the receiver.
    fn expression_start(&self, row: &Row) -> u32 {
        if row.qualified {
            let range = row.range;
            return self
                .property_access_span(SourceRange {
                    offset: range.offset,
                    length: range.end() - range.offset,
                })
                .map_or(row.range.offset, |span| span.offset);
        }
        row.range.offset
    }

    /// `refineNumericInvocationContext`'s ladder
    /// (type_system.dart:1881-1990): on a `+`/`-`/`*`/`%`/`remainder`/
    /// `clamp` face whose receiver `T` reads `num?`-bounded, the context
    /// is `int` when `C` admits `int` but not `num` and `T <: int?`,
    /// `double` when `C` admits `double` but not `num` and not
    /// `T <: double?`, `num` otherwise. `C = None` reads as the
    /// inference top (`UnknownInferredType` admits everything — both
    /// refinements fail and the answer is `num`, the upstream
    /// no-context posture). A receiver outside the `num` family or any
    /// other selector keeps `None` — no context resolves, the send
    /// stays untyped.
    fn refine_numeric_context(&self, name: &str, t: Ty, c: Option<Ty>) -> Option<Ty> {
        if !matches!(name, "+" | "-" | "*" | "%" | "remainder" | "clamp") {
            return None;
        }
        let (int, double, num) = (
            self.members.primitive_ids[0],
            self.members.primitive_ids[1],
            self.members.primitive_ids[2],
        );
        let named_q = |head| Ty::Named {
            name: head,
            nullable: true,
            args: ArgListId::EMPTY,
        };
        if !self.name_subtype(t, named_q(num)) {
            return None;
        }
        // `C` admits `head`: the empty context is the inference top —
        // every type is below it.
        let admits = |head| c.is_none_or(|c| self.name_subtype(Ty::named(head), c));
        if admits(int) && !admits(num) && self.name_subtype(t, named_q(int)) {
            return Some(Ty::named(int));
        }
        if admits(double) && !admits(num) && !self.name_subtype(t, named_q(double)) {
            return Some(Ty::named(double));
        }
        Some(Ty::named(num))
    }

    /// The identity-generic send's context answer: `T f<T>()` — the
    /// declared return IS the single free inference parameter — binds
    /// `T` to the position's concrete context. Every other shape (a
    /// composite return spelling a parameter, a non-generic callee, a
    /// context the ladder declines) keeps `None`, and `dynamic` never
    /// infers (the closure schema's own rule).
    fn context_invoke_ty(&self, row: &Row) -> Option<Ty> {
        let declared = match &row.shape {
            RowShape::Member { recv } => {
                let container = match recv {
                    Receiver::Typed { type_name, .. } => *type_name,
                    Receiver::This { container, .. }
                    | Receiver::Super { container, .. }
                    | Receiver::Created { name: container, .. }
                    | Receiver::Container { name: container, .. } => *container,
                    _ => return None,
                };
                let entry = self.resolved_member(container, row.name)?;
                (entry.kind == MemberKind::Method).then(|| entry.return_ty)?
            }
            RowShape::Bare(_) => self.top_level_decl(row.name)?.return_ty,
            _ => return None,
        };
        let Ty::Named {
            name: param,
            args: ArgListId::EMPTY,
            nullable: false,
        } = self.fold_alias_ty(declared)
        else {
            return None;
        };
        let recv = match &row.shape {
            RowShape::Member { recv } => *recv,
            RowShape::Bare(info) => Engine::receiver_of_bare(*info),
            _ => return None,
        };
        let params = self.closure_callee_params(&recv, row.name)?;
        let bound = params.0.iter().find(|(name, _)| *name == param)?.1;
        let context = self.invoke_downward_context(row)?;
        if !matches!(context, Ty::Named { .. }) {
            return None;
        }
        // An unsatisfiable binding falls back upstream (`T extends num`
        // under a `String` context answers the bound, never the
        // context) — the unknown stand-in is the same under-report.
        if let Some(bound) = bound && !self.name_subtype(context, self.fold_alias_ty(bound)) {
            return None;
        }
        Some(context)
    }

    /// The member-read row's static type for the `var`-inference window —
    /// the follow-up the window's own faces answer `Unknown` on: the
    /// official `staticType` of a property access is the resolved member's
    /// declared type, so a getter read merged over a typed receiver
    /// answers its declared return (the accessor face
    /// [`Self::receiver_static_ty`] already carries), and an instance miss
    /// consults the applied extension face ([`Self::extension_read_ty`]).
    /// The extension getter's return is the one shape the merge-time
    /// field face can never answer (the merged row's `ty` keeps `Unknown`
    /// there), and every `var` declarator the read initializes stays
    /// untyped without it, so an extension use reached through the local
    /// never credits (`element.ffiClass` over `var element =
    /// elementAnnotation?.element` — the ffi_verifier census shape). The
    /// `?.` short-circuit's result nullability re-arms on the answer
    /// (`var a = c?.id` keeps `a` nullable). Every shape neither arm
    /// answers stays [`Ty::UNKNOWN`] — the under-report direction; a
    /// method tear-off is the window's own face, not this one.
    pub(crate) fn initializer_read_ty(&mut self, row: &Row) -> Ty {
        if row.intent != Intent::Read {
            return Ty::UNKNOWN;
        }
        let declared = match self.receiver_static_ty(row) {
            Ty::UNKNOWN => None,
            ty => Some(ty),
        };
        let Some(ty) = declared.or_else(|| self.extension_read_ty(row)) else {
            return Ty::UNKNOWN;
        };
        if self.null_aware_members.contains(&row.range.offset) && !self.bool_decided_covers(row) {
            return Self::nullable_form(ty);
        }
        ty
    }

    /// The extension arm of [`Self::initializer_read_ty`]: a member read
    /// whose instance walk missed but whose accessor resolves through an
    /// applied extension face answers the face's declared return. The
    /// application runs the resolve-side pipeline
    /// ([`Self::extension_application`]) — applicability, specificity, and
    /// the element credit the access owes — so the verdict must be a
    /// resolved application (`Single`/`UnsafeSingle`), and the attributed
    /// face is the pool's UNIQUE declarer of the spelling: a second
    /// declarer is the ambiguous or specificity-ordered application this
    /// face cannot attribute (the oracle's ambiguous access resolves no
    /// member either). `None` keeps the under-report: an unclassified
    /// receiver, a nullable-`on`-less verdict, and non-accessor members.
    fn extension_read_ty(&mut self, row: &Row) -> Option<Ty> {
        let RowShape::Member { recv } = &row.shape else {
            return None;
        };
        let (head, receiver_ty, receiver_nullable) = match recv {
            Receiver::Typed { type_name, ty, .. } => {
                (*type_name, *ty, self.potentially_nullable(*ty))
            }
            Receiver::This { container, kind } => {
                let ty = self.this_static_ty(*container, *kind);
                let Ty::Named { name, .. } = ty else {
                    return None;
                };
                (name, ty, self.potentially_nullable(ty))
            }
            Receiver::Created {
                name,
                kind: ContainerKind::Class,
                ..
            } => (*name, Ty::named(*name), false),
            _ => return None,
        };
        // A `?.` access unwraps the receiver before the member read (the
        // same verdict `check_instance_member` feeds the application).
        let receiver_nullable =
            receiver_nullable && !self.null_aware_members.contains(&row.range.offset);
        self.accessible_extension_faces();
        let faces = self.accessible_extensions.as_deref().unwrap_or(&[]);
        let declarers: Vec<(&ExtensionFace, FileId)> = faces
            .iter()
            .filter(|(face, _)| face.members.contains_key(&row.name))
            .copied()
            .collect();
        if declarers.is_empty() {
            return None;
        }
        match self.extension_application(head, receiver_ty, row.name, false, receiver_nullable) {
            ExtensionApp::Single | ExtensionApp::UnsafeSingle => {}
            _ => return None,
        }
        // Several accessible extensions can declare the spelling over one
        // head (the analyzer corpus's paired `withOriginDeclaration`
        // getters: a concrete `on List<ConstructorFragmentImpl>` beside a
        // generic `on List<T extends …>`); the oracle resolves by
        // applicability, and for the read's static type every declarer
        // that CAN apply to this receiver instantiates to the same answer
        // — the concrete face names the receiver's own argument, the
        // generic one binds its parameter to it. A face whose `on`
        // arguments cannot match (a different concrete element) never
        // applies; disagreement among the viable faces stays the
        // under-report (`None`).
        self.extension_accessor_ty(receiver_ty, row.name, false)
    }

    /// The declared accessor face an accessible extension gives `member`
    /// on `receiver_ty`, over the already-filled extension pool (`None`
    /// before the fill): every viable declarer must instantiate to the
    /// same answer.
    pub(crate) fn extension_accessor_ty(
        &self,
        receiver_ty: Ty,
        member: StringId,
        invoked: bool,
    ) -> Option<Ty> {
        let faces = self.accessible_extensions.as_deref()?;
        let mut answer: Option<Ty> = None;
        for &(face, face_file) in faces {
            let Some(entry) = face.members.get(&member) else {
                continue;
            };
            let declared = match entry.kind {
                MemberKind::Getter => entry.return_ty,
                MemberKind::Field => entry.var_ty,
                // A send's chain node (`l.elementAtOrNull(0)`) types as the
                // method's return; a method tear-off is a function value
                // this face cannot spell.
                MemberKind::Method if invoked => entry.return_ty,
                _ => continue,
            };
            // The face's types are foreign-file payloads (argument lists
            // interned in the extension's own pool) — project through the
            // boundary before any argument read, the same cross-pool
            // discipline everywhere else.
            let on_ty = self.boundary.ty(face_file, face.on_ty);
            // `_getApplicable` instantiates the extended type against the
            // receiver's supertype closure: a `List<Tk>` receiver meets
            // `on Iterable<T>` as `Iterable<Tk>` (`asInstanceOf`). Only a
            // generic `on` type takes the view — the argument projection is
            // what the binding needs; a non-generic face keeps the
            // head-equality gate the use-credit faces agree with.
            let viewed = match (on_ty, receiver_ty) {
                (
                    Ty::Named {
                        name: on_head,
                        args: on_args,
                        ..
                    },
                    Ty::Named {
                        name: recv_head,
                        nullable,
                        ..
                    },
                ) if on_head != recv_head && on_args != ArgListId::EMPTY => {
                    let Some(env) = self.ops.and_then(|ops| {
                        ops.instance_env(receiver_ty, on_head, &self.boundary.args)
                    }) else {
                        continue;
                    };
                    let args: Vec<Ty> = env.into_iter().map(|(_, ty)| ty).collect();
                    self.boundary
                        .args
                        .borrow_mut()
                        .named(on_head, nullable, &args)
                }
                _ => receiver_ty,
            };
            if !self.extension_on_applies(on_ty, viewed, face, face_file) {
                continue;
            }
            let ty = self.boundary.ty(face_file, declared);
            // A generic extension's declared return still spells its own
            // type parameters (`Iterable<T>` under `extension E<T> on
            // List<T>`); the oracle instantiates the extended type against
            // the receiver before answering, so the `on` type's
            // bare-parameter arguments pair with the receiver's arguments
            // positionally (`List<X>` binds `T := X`) and the declared
            // return substitutes through — a `T?` leaf keeps its `?`.
            let map: FxHashMap<StringId, Ty> = match (on_ty, viewed) {
                (
                    Ty::Named { args: on_args, .. },
                    Ty::Named {
                        args: recv_args, ..
                    },
                ) if on_args != ArgListId::EMPTY
                    && recv_args != ArgListId::EMPTY
                    && !face.type_param_names.is_empty() =>
                {
                    let on_list = self.boundary.pool().get(on_args).to_vec();
                    let recv_list = self.boundary.pool().get(recv_args).to_vec();
                    on_list
                        .into_iter()
                        .zip(recv_list)
                        .filter_map(|(on_arg, recv_arg)| {
                            let Ty::Named {
                                name,
                                nullable,
                                args: ArgListId::EMPTY,
                            } = on_arg
                            else {
                                return None;
                            };
                            // `on Iterable<T?>` against `Iterable<X?>` binds
                            // `T := X` (the `?` belongs to the pattern).
                            let bound = if nullable {
                                Self::non_null_form(recv_arg)
                            } else {
                                recv_arg
                            };
                            face.type_param_names
                                .contains(&name)
                                .then_some((name, bound))
                        })
                        .collect()
                }
                // `extension E<T> on T` (the bare own-parameter `on` head):
                // the parameter binds the receiver whole, so a declared
                // return spelling `T` instantiates to the receiver's type
                // — the same `T := receiverType` the viability gate above
                // answered.
                (
                    Ty::Named {
                        name: on_head,
                        args: ArgListId::EMPTY,
                        ..
                    },
                    _,
                ) if face.type_param_names.contains(&on_head) => {
                    let mut map = FxHashMap::default();
                    map.insert(on_head, viewed);
                    map
                }
                _ => FxHashMap::default(),
            };
            let ty = self.fold_alias_ty(self.substitute_ty(ty, &map));
            // A parameter still spelled in the answer (an unbound face
            // parameter, the method's own `R`) is free: the oracle infers
            // it from the call site, which this face does not model.
            let mut free = false;
            self.boundary.pool().visit_named(ty, &mut |named| {
                if let Ty::Named { name, .. } = named
                    && (face.type_param_names.contains(&name)
                        || entry.type_param_names.contains(&name))
                {
                    free = true;
                }
            });
            if free {
                return None;
            }
            match answer {
                None => answer = Some(ty),
                Some(kept) if kept == ty => {}
                Some(_) => return None,
            }
        }
        answer
    }

    /// Whether an extension's `on` type can apply to `receiver`: heads
    /// equal, and each `on` argument either equals the receiver's argument
    /// or is one of the extension's own bare type parameters (instantiable
    /// against whatever the receiver carries). The read face's per-face
    /// viability filter — a face whose concrete arguments differ from the
    /// receiver's can never be the oracle's resolution, whatever the
    /// specificity ordering among the survivors.
    ///
    /// The `on` clause spelling the extension's OWN bare parameter
    /// (`extension E<T> on T`, `on T?`) takes the `applicableTo` arm
    /// instead: the parameter binds to the receiver wholesale
    /// (`constrainArgument` answers `T := receiverType`, upstream Dart SDK
    /// source, `pkg/analyzer/lib/src/dart/resolver/
    /// applicable_extensions.dart:205-281`), so applicability is the
    /// parameter's declared bound admitting the receiver — an absent or
    /// `dynamic` bound is the top. A receiver already rendered
    /// non-nullable by the member face still satisfies the nullable
    /// spelling (`on T?` over `T <: bound` core), so the verdict asks the
    /// bound's non-null core against the receiver's.
    fn extension_on_applies(
        &self,
        on_ty: Ty,
        receiver: Ty,
        face: &ExtensionFace,
        face_file: FileId,
    ) -> bool {
        if let Ty::Named {
            name: on_head,
            args: ArgListId::EMPTY,
            ..
        } = on_ty
            && let Some(index) = face.type_param_names.iter().position(|&n| n == on_head)
        {
            return match face.type_param_bounds.get(index).copied() {
                None | Some(Ty::Dynamic) | Some(Ty::Unknown) => true,
                Some(bound) => {
                    let bound = Self::non_null_form(self.boundary.ty(face_file, bound));
                    self.resolved_subtype(Self::non_null_form(receiver), bound)
                }
            };
        }
        let (
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
        ) = (on_ty, receiver)
        else {
            return false;
        };
        if on_head != recv_head || (on_args == ArgListId::EMPTY) != (recv_args == ArgListId::EMPTY)
        {
            return false;
        }
        if on_args == ArgListId::EMPTY {
            return true;
        }
        let on_list = self.boundary.pool().get(on_args).to_vec();
        let recv_list = self.boundary.pool().get(recv_args).to_vec();
        on_list.len() == recv_list.len()
            && on_list
                .iter()
                .zip(recv_list.iter())
                .all(|(on_arg, recv_arg)| {
                    on_arg == recv_arg
                        || matches!(on_arg, Ty::Named {
                        name,
                        args: ArgListId::EMPTY,
                        ..
                    } if face.type_param_names.contains(&name))
                })
    }

    /// The operator application on a known receiver type (`c + d`, `c[i]`,
    /// `c.j + 1` against the member result): each read/write spelling that
    /// the type's container genuinely lacks reports `undefined_operator` at
    /// the operator's anchor span — unless an applicable extension declares
    /// the spelling (the implicit-application face resolves user operators
    /// too: `x[0]` over an extension `operator []`, upstream Dart SDK
    /// source, `pkg/analyzer/lib/src/dart/resolver/extension_member_resolver.dart`,
    /// `getExtension` + the `[]`-operator lookup). Each index spelling is its
    /// own lookup key — a read never satisfies through `[]=`, a compound
    /// write needs both. Unresolved or external types (`Unknown`, broken
    /// chains) suppress — an invalid receiver type never cascades.
    pub(crate) fn check_operator_expr(&mut self, ty: Ty, row: &Row) {
        self.check_operator_expr_in(ty, row, None);
    }

    /// [`Self::check_operator_expr`] with the receiver's declaring container
    /// as the type-parameter bound hint (`Some(container)` when a member
    /// walk produced `ty` — `c.a + 1` over `class C<T> { T a }` resolves
    /// `+` through `T`'s bound on `C`).
    pub(crate) fn check_operator_expr_in(
        &mut self,
        ty: Ty,
        row: &Row,
        param_hint: Option<StringId>,
    ) {
        let Intent::Operator(_, op_range) = row.intent else {
            return;
        };
        let (read, write) = self.operator_spellings(row);
        let Ty::Named { name, .. } = ty else {
            match ty {
                Ty::Record { .. } | Ty::Function { .. } => {
                    // A record or function face declares no user operator —
                    // every applied spelling but `==` (`Object`'s face)
                    // reports, the type display riding the full render
                    // (`(String,) + 0`, a generic function tear-off's
                    // `f < g`; upstream `instanceReceiverType` answers the
                    // composite type and the property consult misses) —
                    // unless a shape extension declares the spelling
                    // (type_property_resolver.dart:227's `_lookupExtension`
                    // runs for shape receivers too: `(1, 'x') + r` resolves
                    // through `extension on (int, String)`, a function
                    // operand through `extension on Function`).
                    let type_text = ty.render(&self.strings, &self.boundary.pool());
                    for spelling in [read, write].into_iter().flatten() {
                        if spelling == self.intern("==")
                            || self.record_extension_face(ty, spelling).is_some()
                        {
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
                // A `Null`-typed operand dereferences unconditionally and
                // the verifier's `NullType` arm swaps the reported code to
                // `invalid_use_of_null_value` at the receiver expression —
                // one row per application, anchored on the operand
                // (nullable_dereference_verifier.dart:47-49, reached through
                // the isNullable tail: `Null` is potentially nullable).
                Ty::Null => {
                    self.emit(SemanticCode::InvalidUseOfNullValue, row.range, "", "");
                }
                // A `void` operand is potentially nullable upstream
                // (`isPotentiallyNullable`'s VoidType arm → the isNullable
                // tail): a binary application reports the shared unchecked
                // code at the operator token. The `[]`/`[]=` index
                // spellings report `use_of_void_result` upstream
                // (property_element_resolver.dart:183-186 precedes the
                // member consult) — that family owns the index row, so the
                // bracket stays silent here.
                Ty::Void => {
                    if [read, write].into_iter().flatten().any(|spelling| {
                        spelling != self.intern("[]") && spelling != self.intern("[]=")
                    }) {
                        let Intent::Operator(op, _) = row.intent else {
                            return;
                        };
                        let op_text = self.strings.resolve(op).to_string();
                        self.emit_unchecked_nullable(
                            op_range,
                            &op_text,
                            UncheckedNullableKind::Operator,
                        );
                    }
                }
                // `dynamic`/`InvalidType` (`Ty::Unknown`)/`Never` receivers
                // never reach `undefined_operator` upstream: the
                // `isDynamicBounded`/`isInvalidBounded` short-circuit
                // answers with `needsGetterError` cleared
                // (type_property_resolver.dart:88-94), and the `NeverType`
                // arm clears the same flags (:211-216) — the dynamic
                // dispatch and the never-receiver family own those rows.
                _ => {}
            }
            return;
        };
        // An unindexable receiver still answers through the frozen faces:
        // `Match`/`Stream`-typed operands chain-break the walk at the root,
        // but the interface table decides a user operator's absence exactly
        // (`x + 1` on `Match` — `undefined_operator`); a genuinely
        // unindexed declaration keeps the under-report.
        let indexed = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .is_some();
        let mut missing: Vec<String> = Vec::new();
        for spelling in [read, write].into_iter().flatten() {
            let mut walk_missing = if indexed {
                matches!(self.walk_members(name, spelling), Walk::Missing)
            } else {
                matches!(
                    self.frozen_member_verdict(name, spelling),
                    FrozenFace::Absent
                )
            };
            // A scope-parameter receiver answers on its bound (the
            // upstream `resolveToBound` consult) — `T extends num`
            // resolves `+` through `num`'s face.
            if walk_missing && self.bound_defines_operator_in(name, spelling, param_hint) {
                walk_missing = false;
            }
            if walk_missing {
                match self.extension_application(name, ty, spelling, false, false) {
                    ExtensionApp::None => {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                    ExtensionApp::Single | ExtensionApp::UnsafeSingle => {}
                    ExtensionApp::Ambiguous(displays) => {
                        // The ambiguity verdict replaces the member family
                        // (the oracle reports the ambiguous application, not
                        // the operator miss) — anchored at the operator.
                        self.emit_extension_ambiguity(spelling, op_range, displays);
                    }
                }
            }
        }
        if missing.is_empty() {
            return;
        }
        // The receiver's display is the oracle's `getDisplayString` face —
        // instantiate-to-bounds fills a raw head's arguments
        // (`Map` → `Map<dynamic, dynamic>`), the written ones render as
        // spelled (`is G<int>` → `G<int>`). `ty_display` is the shared
        // render.
        let type_text = self.ty_display(ty);
        for text in missing {
            self.emit(SemanticCode::UndefinedOperator, op_range, &text, &type_text);
        }
    }

    /// Whether an instance field's declaration initializer is open: the
    /// variable-initializer bracket (`in_initializer`) inside a member
    /// field group at the group's own frame depth (a closure body inside
    /// the initializer is not the initializer), with the group non-static
    /// — the instance context `_checkForImplicitThisReferenceInInitializer`
    /// arms on.
    pub(crate) fn in_instance_field_initializer(&self) -> bool {
        // A `late` instance field's initializer is lazy — `this` is
        // in scope there (upstream `SuperContext.fieldDeclaration`'s
        // late arm falls through to the enclosing class), so the
        // `_checkForImplicitThisReferenceInInitializer` face arms only
        // on a non-late instance group.
        self.in_initializer
            && self.field_group_open
            && self.init_frame_depth == self.field_group_depth
            && !self.open_member_static
            && !self.group_late
    }

    /// Runs the walk(s) an access needs and applies the shared suppressions
    /// (broken chain, extension application, `Object` members). A write also
    /// accepts the setter spelling (`name=`): a same-named getter, field, or
    /// method suppresses with the oracle's assignment-to-* codes, a setter
    /// defines the write.
    /// The reference-edge kind of a member row's final intent (#2618): the
    /// send/assignment events finalize the intent before pass 2, so the
    /// mapping runs at the verdict sites where both facts stand. A method
    /// read as a value is a tear-off and mints the index's tear-off-shaped
    /// `Invoked` face; an operator-shaped access (`a.m + 1`, `a.m[i]`)
    /// reads the member's getter.
    fn member_ref_kind(intent: &Intent, entry_kind: MemberKind) -> MemberRefKind {
        match intent {
            Intent::Invoke => MemberRefKind::Invoked,
            Intent::Read | Intent::Operator(..) => {
                if entry_kind == MemberKind::Method {
                    MemberRefKind::Invoked
                } else {
                    MemberRefKind::Read
                }
            }
            Intent::Write => MemberRefKind::Written,
            Intent::ReadWrite => MemberRefKind::ReadWritten,
        }
    }

    /// Records one resolved member-access reference edge (#2618): the use
    /// row's span against the declaration the resolution landed on —
    /// `(declaring file, lookup spelling, declaring container)`. Rows
    /// land beside the bare-reference publications; the engine folds them
    /// into the library reference surface the query/rename faces serve.
    fn note_member_ref(
        &mut self,
        row: &Row,
        kind: MemberRefKind,
        target: Option<(FileId, StringId, StringId)>,
    ) {
        if let Some((target_file, target_name, target_container)) = target {
            self.report.member_refs.push(MemberRef {
                range: row.range,
                kind,
                target_file,
                target_name,
                target_container,
            });
        }
    }

    /// The walk-landing facts of a `Found` outcome (#2618): the declaring
    /// file and container, read as one snapshot before any later probe
    /// can move the face memo's origin cell.
    fn last_landing(&self) -> Option<(FileId, StringId)> {
        Some((
            self.member_faces.last_found_file()?,
            self.member_faces.last_origin()?,
        ))
    }

    /// Records the edge for the extension application the last
    /// [`Engine::extension_application`] call resolved (#2618): the latch
    /// holds the winning face's target only at a single-face conclusion —
    /// `Ambiguous` resolves no member and latches nothing.
    fn note_extension_ref(&mut self, row: &Row) {
        let target = self.members.ext_ref.take();
        let kind = target.map_or(MemberRefKind::Read, |(_, _, _, entry)| {
            Self::member_ref_kind(&row.intent, entry)
        });
        let target = target.map(|(file, spelling, face, _)| (file, spelling, face));
        self.note_member_ref(row, kind, target);
    }

    /// The read-face display: the receiver's full `getDisplayString` render
    /// (type arguments kept — `C<int>` stays `C<int>`), with the NULLABILITY
    /// stripped when the access is null-aware (`b?.gone` over `B<int>? b`
    /// names `B<int>` — the oracle resolves the member on the shorted
    /// non-null face and renders it without the `?`, probe 3.13.7:
    /// "The getter 'gone' isn't defined for the type 'B<int>'.").
    fn getter_display(
        &self,
        recv: &Receiver,
        container: StringId,
        receiver_ty: Ty,
        row_offset: u32,
    ) -> String {
        let base = match recv {
            Receiver::Typed {
                ty: ty @ Ty::Named { .. },
                ..
            } => self.ty_display(*ty),
            _ => self.strings.resolve(container).to_string(),
        };
        if self.null_aware_members.contains(&row_offset)
            && matches!(receiver_ty, Ty::Named { nullable: true, .. })
        {
            base.trim_end_matches('?').to_string()
        } else {
            base
        }
    }

    /// The constructor-initializer write face (`_checkForInvalidField`,
    /// upstream Dart SDK source, `pkg/analyzer/lib/src/generated/
    /// error_verifier.dart`): a `name = e` initializer resolves the name
    /// against the enclosing class's own field set — a real field is the
    /// legal initializing write (its `final` is the point of the
    /// declaration, never `assignment_to_final`), while a miss or an
    /// accessor spelling codes `initializer_for_non_existent_field` at the
    /// whole initializer node. Called wherever a write pushed inside the
    /// initializer list reaches the member arm: `this.x` through
    /// [`Self::check_instance_member`]'s `init_rows` gate, a bare `x`
    /// through [`Self::resolve_bare_row`]'s implicit-`this` arm.
    fn check_initializer_member_write(&mut self, row: &Row, container: StringId) {
        let reports = match self.walk_members(container, row.name) {
            Walk::Missing => true,
            Walk::Found(entry) => {
                // The initializer's field write resolved (#2618) — the edge
                // lands whatever the validity face says.
                let landing = self.last_landing();
                let kind = Self::member_ref_kind(&row.intent, entry.kind);
                self.note_member_ref(
                    row,
                    kind,
                    landing.map(|(file, origin)| (file, row.name, origin)),
                );
                entry.kind != MemberKind::Field
            }
            Walk::ChainBroken => false,
        };
        if reports {
            let name_text = self.strings.resolve(row.name);
            // `.at(initializer)` — the whole node span (`x = 'foo'`,
            // `this.x = 0`), never the bare field name.
            let range = self.initializer_node_span(row.range);
            self.emit(
                SemanticCode::InitializerForNonExistentField,
                range,
                name_text,
                "",
            );
        }
    }

    /// The assignment-to-* family's member-side verdict (#3282): the write's
    /// `name=` lookup missed and the plain spelling resolved `kind` at
    /// `landing`. Upstream's `AssignmentVerifier.verify` takes the recovery
    /// element instead of a member miss (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/error/assignment_verifier.dart:63-87): a getter
    /// whose variable is a field-or-variable declaration reports
    /// `assignment_to_final` (`'{0}' can't be used as a setter because it's
    /// final.`), while an accessor-origin getter — an explicit `get`
    /// declaration, `isOriginGetterSetter` true — reports
    /// `assignment_to_final_no_setter` naming the DECLARING container
    /// (`variable.enclosingElement.displayName`). Both anchor at the
    /// written name (`at(node)`). `false` for a face this family does not
    /// own (a method: `assignment_to_method`'s; a `const` field:
    /// `assignment_to_const`'s), leaving the row's usual path untouched.
    fn report_setter_less_write(
        &mut self,
        row: &Row,
        kind: MemberKind,
        is_final: bool,
        name_range: SourceRange,
        landing: Option<(FileId, StringId)>,
    ) -> bool {
        let name_text = self.strings.resolve(row.name);
        match kind {
            MemberKind::Field if is_final => {
                // The model's `is_final` folds `const` (the assignment-to-
                // const row) and `late final` (whose uninitialized form
                // still declares a `name=` setter) into one flag. A
                // same-file declaration answers both from its own header;
                // a foreign one keeps the model convention (a `final`
                // field has no setter — the same reading the override
                // faces apply). Either carve-out still CLAIMS the row: the
                // write resolved a getter-backed face, so the member-miss
                // tail's `undefined_setter` would be a false row — the
                // sibling family's own code stays the honest omission.
                if landing.is_some_and(|(file, _)| file == self.file)
                    && !self.field_declares_no_setter(name_range)
                {
                    return true;
                }
                self.emit(SemanticCode::AssignmentToFinal, row.range, name_text, "");
                true
            }
            MemberKind::Getter => {
                let class_text = landing.map_or_else(String::new, |(_, origin)| {
                    self.strings.resolve(origin).to_string()
                });
                self.emit(
                    SemanticCode::AssignmentToFinalNoSetter,
                    row.range,
                    name_text,
                    &class_text,
                );
                true
            }
            _ => false,
        }
    }

    /// Whether the field declaration whose name token sits at `name_range`
    /// declares no setter — a plain `final` field. The two `is_final`
    /// carve-outs are read from the declaration's own header: a `const`
    /// member owns `assignment_to_const`, and a `late final` declarator
    /// without an initializer still induces a `name=` setter (upstream
    /// `definesSetter`: `!is_final || (isLate && isFinal && !hasInit)` —
    /// the collector's own induced-setter rule). A declaration outside this
    /// file, or one whose captured span is missing, keeps the model's
    /// `final`-implies-no-setter reading.
    fn field_declares_no_setter(&self, name_range: SourceRange) -> bool {
        if name_range == MemberEntry::MISSING_NAME_RANGE {
            return true;
        }
        let tokens = self.tokens.as_slice();
        let Some(at) = self.token_index_at(name_range.offset) else {
            return true;
        };
        let mut late = false;
        // The group header (`[abstract] [external] [static] [late]
        // [final|const|var] Type`) precedes the FIRST declarator; a
        // trailing declarator (`final int a, b;`) walks back over its
        // comma and siblings to the same head. The walk stops at the
        // group's own boundaries.
        let mut cursor = at;
        while let Some(prev) = cursor.checked_sub(1) {
            if at - prev > 32 {
                break;
            }
            match tokens[prev].kind {
                dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Late) => late = true,
                dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Const) => return false,
                dartq_scanner::TokenKind::Semicolon
                | dartq_scanner::TokenKind::OpenCurly
                | dartq_scanner::TokenKind::CloseCurly
                | dartq_scanner::TokenKind::OpenParen
                | dartq_scanner::TokenKind::Function => break,
                _ => {}
            }
            cursor = prev;
        }
        if !late {
            return true;
        }
        // `late final int x;` declares the setter; an initializer
        // (`late final int x = 0;`) does not.
        matches!(
            tokens.get(at + 1).map(|token| token.kind),
            Some(dartq_scanner::TokenKind::Eq)
        )
    }

    /// The bare-write twin of [`Self::report_setter_less_write`] over an
    /// implicit-`this` member face: the write resolves the setter spelling
    /// first (`bare_write_target_exists`'s rule), and a miss there falls
    /// back to the plain name — a `final` field or an explicit getter the
    /// assignment-to-* family owns. Called from the bare row's
    /// `RefTarget::Member` arm, where the pass-2 walk re-reads the face.
    fn report_setter_less_implicit_write(&mut self, row: &Row, container: StringId) {
        let setter = self.setter_spelling(row.name);
        if self.member_found(container, setter, false) {
            return;
        }
        let Walk::Found(entry) = self.walk_members(container, row.name) else {
            return;
        };
        let landing = self.last_landing();
        let (kind, is_final, name_range) = (entry.kind, entry.is_final, entry.name_range);
        self.report_setter_less_write(row, kind, is_final, name_range, landing);
    }

    /// The increment/decrement twin of
    /// [`Self::report_setter_less_implicit_write`]: `x++`/`--x` assigns its
    /// operand, so a `final` field or an explicit getter the operand
    /// resolves reports through the same family. The read face's own
    /// bookkeeping has already run; this only answers the setter question.
    fn report_increment_target(&mut self, row: &Row, container: StringId, super_walk: bool) {
        let setter = self.setter_spelling(row.name);
        if self.member_found(container, setter, super_walk) {
            return;
        }
        let walk = if super_walk {
            self.walk_super(container, row.name)
        } else {
            self.walk_members(container, row.name)
        };
        let Walk::Found(entry) = walk else {
            return;
        };
        let landing = self.last_landing();
        let (kind, is_final, name_range) = (entry.kind, entry.is_final, entry.name_range);
        self.report_setter_less_write(row, kind, is_final, name_range, landing);
    }

    /// The byte offset's token index, `None` when no token starts there.
    fn token_index_at(&self, utf16: u32) -> Option<usize> {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < utf16);
        tokens
            .get(at)
            .is_some_and(|token| token.utf16_offset == utf16)
            .then_some(at)
    }

    /// Whether the bare row at `range` is a for-in loop's existing-variable
    /// target (`for (i in xs)` — the identifier directly before the head's
    /// `in`). That shape is a WRITE per iteration upstream
    /// (`ForEachPartsWithIdentifier`), and its final-local verdict reports
    /// unconditionally (`isForEachIdentifier`, `checkFinalAlreadyAssigned`
    /// :395-425), so the row model's read intent never decides it.
    fn for_each_target(&self, range: SourceRange) -> bool {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < range.end());
        matches!(
            tokens.get(at).map(|token| token.kind),
            Some(dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::In))
        )
    }

    /// Whether the row at `range` is an increment/decrement operand
    /// (`x++`, `--x`): those applications assign their operand — upstream
    /// routes them through the same finality consults
    /// (`increment_or_decrement_resolver.dart`'s compound-write resolve),
    /// while the row model leaves the operand a plain read. A member row
    /// spans only the selector, so the PREFIX spelling is read from the
    /// receiver's own start (`++c.x` puts the token before `c`).
    fn increment_operand(&self, row: &Row) -> bool {
        let tokens = self.tokens.as_slice();
        let incdec = |kind: Option<dartq_scanner::TokenKind>| {
            matches!(
                kind,
                Some(
                    dartq_scanner::TokenKind::PlusPlus | dartq_scanner::TokenKind::MinusMinus
                )
            )
        };
        let after = tokens.partition_point(|token| token.utf16_offset < row.range.end());
        if incdec(tokens.get(after).map(|token| token.kind)) {
            return true;
        }
        let start = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&row.range.offset))
            .ok()
            .map(|index| self.member_reads[index].recv_span)
            .filter(|span| span.length > 0)
            .map_or(row.range.offset, |span| span.offset);
        let before = tokens.partition_point(|token| token.utf16_offset < start);
        incdec(
            before
                .checked_sub(1)
                .and_then(|index| tokens.get(index))
                .map(|token| token.kind),
        )
    }

    /// The assignment-to-final-local family (#3282): a write (or for-in
    /// target) whose declaration is a `final` local or formal that is not
    /// definitely unassigned at the write. The declaration facts come from
    /// [`Self::final_local_face`]; the verdict mirrors
    /// `AssignmentExpressionShared.checkFinalAlreadyAssigned` (upstream Dart
    /// SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver
    /// .dart:395-425): `late` locals and the `const` rows own their own
    /// codes, and a `final int x;` declaration's first write is the legal
    /// one (reporting starts once the variable can be assigned).
    fn report_assignment_to_final_local(&mut self, row: &Row, for_each: bool) {
        let Some(&decl) = self.local_decl_at_read.get(&row.range.offset) else {
            return;
        };
        // The declaration token itself (a declared loop variable's binder,
        // a pattern binder's own name) is not a write to an existing local.
        if decl == row.range.offset {
            return;
        }
        let FinalLocalFace::Final { assigned } = self.final_local_face(decl) else {
            return;
        };
        if !assigned
            && !for_each
            && !self.has_prior_write(row.name, decl, row.range.offset)
        {
            return;
        }
        let name_text = self.strings.resolve(row.name);
        self.emit(
            SemanticCode::AssignmentToFinalLocal,
            row.range,
            name_text,
            "",
        );
    }

    /// The declaration face behind a bare local's name token (#3282): the
    /// header keywords (`final`/`const`/`late`/`var`), the `this.`/`super.`
    /// filed-formal receiver, and the two facts that make the FIRST write
    /// legal — an initializer, or a formal parameter (bound on entry) — plus
    /// the for-in loop variable the `in` token introduces.
    fn final_local_face(&self, decl: u32) -> FinalLocalFace {
        let tokens = self.tokens.as_slice();
        let Some(at) = self.token_index_at(decl) else {
            return FinalLocalFace::Other;
        };
        // A field/super formal (`this.x`, `super.a`) is a final local
        // regardless of any written keyword (the initializing-formal
        // scope's rule, language spec 10.6.1).
        if let Some(receiver) = at.checked_sub(2)
            && tokens.get(at - 1).is_some_and(|t| t.kind == dartq_scanner::TokenKind::Period)
            && matches!(
                tokens[receiver].kind,
                dartq_scanner::TokenKind::Keyword(
                    dartq_scanner::Keyword::This | dartq_scanner::Keyword::Super
                )
            )
        {
            return FinalLocalFace::Final { assigned: true };
        }
        let mut is_final = false;
        let mut formal = false;
        let mut cursor = at;
        while let Some(prev) = cursor.checked_sub(1) {
            if at - prev > 32 {
                break;
            }
            let token = &tokens[prev];
            match token.kind {
                dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Final) => {
                    is_final = true;
                }
                // `const` owns `assignment_to_const`; `late` (with or
                // without an initializer) owns the late-final row; `var`
                // is not final at all.
                dartq_scanner::TokenKind::Keyword(
                    dartq_scanner::Keyword::Const
                    | dartq_scanner::Keyword::Late
                    | dartq_scanner::Keyword::Var,
                ) => return FinalLocalFace::Other,
                dartq_scanner::TokenKind::OpenParen => {
                    // A `(` after a name/type head opens a formal list; the
                    // parameter is bound on entry, so a later write is not
                    // its first assignment.
                    formal = matches!(
                        tokens.get(prev.wrapping_sub(1)).map(|t| t.kind),
                        Some(
                            dartq_scanner::TokenKind::Identifier
                                | dartq_scanner::TokenKind::CloseParen
                                | dartq_scanner::TokenKind::CloseSquare
                                | dartq_scanner::TokenKind::Question
                        )
                    );
                    break;
                }
                dartq_scanner::TokenKind::Semicolon
                | dartq_scanner::TokenKind::OpenCurly
                | dartq_scanner::TokenKind::CloseCurly
                | dartq_scanner::TokenKind::Function
                | dartq_scanner::TokenKind::Eq
                | dartq_scanner::TokenKind::Comma => break,
                _ => {}
            }
            cursor = prev;
        }
        if !is_final {
            return FinalLocalFace::Other;
        }
        // A parameter is always assigned; a declarator's `=` (or a pattern
        // group's trailing `=`) initializes it; a for-in loop variable is
        // bound by the loop.
        let mut assigned = formal;
        let mut probe = at + 1;
        let mut steps = 0;
        while let Some(token) = tokens.get(probe) {
            steps += 1;
            if steps > 8 {
                break;
            }
            match token.kind {
                dartq_scanner::TokenKind::Eq
                | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::In) => {
                    assigned = true;
                    break;
                }
                dartq_scanner::TokenKind::Comma
                | dartq_scanner::TokenKind::CloseParen
                | dartq_scanner::TokenKind::CloseSquare
                | dartq_scanner::TokenKind::CloseCurly => probe += 1,
                _ => break,
            }
        }
        FinalLocalFace::Final { assigned }
    }

    /// Whether the same spelling took a write between the declaration at
    /// `decl` and the row at `write` — the model's stand-in for the flow
    /// layer's "not definitely unassigned" (`isDefinitelyUnassigned` is
    /// false once ANY path assigns). A token counts when it is an
    /// assignment target (`x =`, `x +=`, `x++`, `++x`) or a for-in head
    /// (`x in`).
    fn has_prior_write(&self, name: StringId, decl: u32, write: u32) -> bool {
        let tokens = self.tokens.as_slice();
        let lexeme = self.strings.resolve(name);
        let start = tokens.partition_point(|token| token.utf16_offset <= decl);
        let end = tokens.partition_point(|token| token.utf16_offset < write);
        (start..end).any(|at| {
            tokens[at].kind == dartq_scanner::TokenKind::Identifier
                && self.tokens.lexeme(self.source, at) == lexeme
                && (is_write_follower(tokens.get(at + 1).map(|t| t.kind))
                    || matches!(
                        tokens.get(at.wrapping_sub(1)).map(|t| t.kind),
                        Some(
                            dartq_scanner::TokenKind::PlusPlus
                                | dartq_scanner::TokenKind::MinusMinus
                        )
                    ))
        })
    }

    pub(crate) fn member_defined(
        &mut self,
        container: StringId,
        receiver_ty: Ty,
        row: &Row,
        super_walk: bool,
        receiver_nullable: bool,
    ) -> bool {
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);

        // The write target resolves the SETTER side
        // (`assignment_expression_resolver.dart` `_resolveSimpleAssignment`:
        // `writeElement` is the `name=` lookup): a setter or a field
        // satisfies the write cleanly, while a same-named getter or method
        // hit still leaves `undefined_setter` silent — the oracle routes it
        // to the assignment-to-* family (`assignment_to_final_no_setter` on
        // a getter, `assignment_to_method` on a method). The two
        // finality rows this pass owns report below
        // ([`Self::report_setter_less_write`]); the method/type rows stay
        // with their own families. `undefined_setter` fires only when
        // neither spelling declares anything.
        let found = if write {
            let named = if super_walk {
                self.walk_super(container, row.name)
            } else {
                self.walk_members(container, row.name)
            };
            // The write's edge target (#2618) resolves its SETTER side —
            // the `name=` spelling first (the oracle's `writeElement`),
            // then a non-final field's implicit setter. The plain walk's
            // landing snapshot reads before the setter probe can move the
            // face memo's origin cell.
            let named_hit = if let Walk::Found(entry) = &named {
                Some((
                    self.last_landing(),
                    entry.kind,
                    entry.is_final,
                    entry.name_range,
                ))
            } else {
                None
            };
            let setter = self.setter_spelling(row.name);
            let setter_hit = self.member_found(container, setter, super_walk);
            if setter_hit {
                let landing = self.last_landing();
                let kind = Self::member_ref_kind(&row.intent, MemberKind::Setter);
                self.note_member_ref(
                    row,
                    kind,
                    landing.map(|(file, origin)| (file, setter, origin)),
                );
            } else if let Some((Some((file, origin)), MemberKind::Field, _, _)) = named_hit {
                let kind = Self::member_ref_kind(&row.intent, MemberKind::Field);
                self.note_member_ref(row, kind, Some((file, row.name, origin)));
            }
            // The assignment-to-* family (#3282): the setter lookup missed
            // while the plain name resolved a getter-backed face — a `final`
            // field (no induced setter) or an explicit getter (no setter at
            // all). The oracle's `AssignmentVerifier.verify` reports there
            // instead of a member miss, so the row is claimed here and the
            // caller's miss tail never sees it.
            if !setter_hit
                && let Some((landing, kind, is_final, name_range)) = named_hit
                && self.report_setter_less_write(row, kind, is_final, name_range, landing)
            {
                return true;
            }
            matches!(named, Walk::Found(_)) || setter_hit
        } else {
            let walk = if super_walk {
                self.walk_super(container, row.name)
            } else {
                self.walk_members(container, row.name)
            };
            if let Walk::Found(entry) = &walk {
                let landing = self.last_landing();
                let kind = Self::member_ref_kind(&row.intent, entry.kind);
                self.note_member_ref(
                    row,
                    kind,
                    landing.map(|(file, origin)| (file, row.name, origin)),
                );
                // The best-practices access face (#2777): the
                // `_checkForOtherInvalidAccess` template leg over the
                // resolved declaration.
                self.check_visible_for_template_member(row, entry, landing);
                // The member-callee's own invocation (#2919): a read shaped
                // `x.m()` resolves the accessor `m`, then applies `()` to
                // its result — a nullable function-shaped result reports
                // the shared unchecked code's function variant at the
                // callee (`function_expression_invocation_resolver.dart`'s
                // nullable-function arm — `x.first()` over `List<T?>` where
                // `T extends Function` invokes `T?`). A method entry is the
                // method send itself; only the accessor kinds carry a
                // callable result.
                if matches!(row.intent, Intent::Invoke)
                    && matches!(entry.kind, MemberKind::Field | MemberKind::Getter)
                    && !self.null_aware_members.contains(&row.range.offset)
                {
                    let result = self.receiver_static_ty(row);
                    if self.invoke_callee_function_shaped(result) {
                        self.emit_unchecked_nullable(
                            row.range,
                            "",
                            UncheckedNullableKind::Function,
                        );
                    }
                }
            }
            matches!(walk, Walk::Found(_))
        };
        // The increment/decrement operand face (#3282): `x++`/`--x` assigns
        // the member its plain spelling resolves, so a `final` field or an
        // explicit getter reports through the assignment-to-* family exactly
        // as a `=` write does. The read face's bookkeeping above already
        // ran; this only answers the setter question.
        if !write && self.increment_operand(row) {
            self.report_increment_target(row, container, super_walk);
        }
        if found {
            // The nullable-dereference face (nullable_dereference_verifier
            // .dart report): an interface member reached through a
            // merely-nullable receiver reads a possibly-null receiver —
            // `unchecked_use_of_nullable_value` at the selector. A member
            // applied through a nullable `on` extension face never
            // dereferences (the applicability test already accepted the
            // nullable type), and a `?.` access unwrapped the receiver —
            // both stay silent here. The nullable-`on` extension wins over
            // the instance member even when the class DECLARES the name:
            // the receiver's nullable type resolves the access through the
            // applicable extension, never the unsafe instance dereference
            // (`extension E on C? { m }` keeps `C? x; x.m()` silent beside
            // `C`'s own instance `m` — the oracle's applicable-extension
            // consult runs against the nullable type first).
            if receiver_nullable {
                let extension_wins = matches!(
                    self.extension_application(container, receiver_ty, row.name, write, true),
                    ExtensionApp::Single
                );
                if !extension_wins {
                    let name_text = self.strings.resolve(row.name);
                    self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
                } else {
                    // The nullable-`on` extension resolved the access: the
                    // member's result keeps the extension's own nullability
                    // (the staged nullable-condition face stands down).
                    self.extension_nullable_resolved.insert(row.range.offset);
                }
            }
            return true;
        }
        if row.name == self.intern("==") || self.members.object_members.contains(&row.name) {
            return true;
        }
        if super_walk {
            // Extensions do not apply to `super` accesses; the conservative
            // any-face gate keeps the under-report direction for the
            // unresolvable-receiver shapes.
            return self
                .index
                .get(self.file)
                .is_some_and(|f| f.has_extension_member(row.name));
        }
        match self.extension_application(container, receiver_ty, row.name, write, receiver_nullable)
        {
            ExtensionApp::None => false,
            ExtensionApp::Single => {
                // The nullable-`on` face resolved a nullable receiver: the
                // access is safe and the member's result keeps the
                // extension's own nullability — the staged
                // nullable-condition face stands down.
                if receiver_nullable {
                    self.extension_nullable_resolved.insert(row.range.offset);
                }
                // The application resolved the member (#2618): the edge
                // targets the winning extension's declaration.
                self.note_extension_ref(row);
                true
            }
            // The member resolved through a non-nullable `on` face against a
            // nullable receiver (`x.m` on `C?` where `extension E on C`
            // declares `m`): the access resolves the member but reads a
            // possibly-null receiver — the oracle's unsafe-access row
            // (`unchecked_use_of_nullable_value` at the selector), never
            // the `undefined_*` face. A `?.` access already read the
            // receiver non-null, so `receiver_nullable` is false there.
            ExtensionApp::UnsafeSingle if receiver_nullable => {
                let name_text = self.strings.resolve(row.name);
                self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
                self.note_extension_ref(row);
                true
            }
            ExtensionApp::UnsafeSingle => {
                self.note_extension_ref(row);
                true
            }
            ExtensionApp::Ambiguous(displays) => {
                self.emit_extension_ambiguity(row.name, row.range, displays);
                // The verdict leaves the property unresolved for the
                // null-safety walk: a nullable receiver reports the
                // unchecked-access face beside it (the oracle's paired rows
                // — both codes at the same access, the property variant of
                // the shared code).
                if receiver_nullable {
                    let name_text = self.strings.resolve(row.name);
                    self.emit_unchecked_nullable(row.range, name_text, access_kind(&row.intent));
                }
                true
            }
        }
    }

    /// Whether the container's OWN declaration spells the name — the bare
    /// member spelling or its setter twin (`static set foo` claims `foo`
    /// for every invocation intent). Only the container's own map counts:
    /// an inherited claimant resolves the row before it ever reaches the
    /// miss faces (the lexical-shadow gate for the implicit-`this`
    /// application — upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/extension_member_resolver.dart
    /// runs only when the interface lookup found nothing).
    pub(crate) fn container_declares_member(
        &mut self,
        container: StringId,
        name: StringId,
    ) -> bool {
        let setter = self.setter_spelling(name);
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .is_some_and(|(decl, _)| {
            decl.members.contains_key(&name) || decl.members.contains_key(&setter)
        })
    }

    /// The implicit-application outcome for `member` against a receiver
    /// whose resolved container is `receiver`
    /// (`extension_member_resolver.dart:85-128` `findExtension` +
    /// `applicable_extensions.dart:157-171` `applicableTo`): the same-file
    /// faces whose `on` type spells the receiver's name and that declare the
    /// member under its base spelling — or, when `accept_setter` holds, the
    /// member under its base or setter spelling (write targets only; a read
    /// never resolves through a setter, so `x.foo` with a face declaring
    /// only `foo=` still reports). A face with an absent or unresolvable
    /// `on` type never matches a resolved receiver — its members are
    /// suppressed from application (the oracle's `extendedType` fails to
    /// resolve; an inapplicable extension never gates an `undefined_*`).
    ///
    /// Two or more applicable candidates are never separable at the M1 name
    /// level: `_isMoreSpecific` (`extension_member_resolver.dart:469-513`)
    /// decides on SDK-ness and subtype relations between the instantiated
    /// `on` types, and same-named `on` types answer equal — so the access is
    /// ambiguous by the oracle's definition. The `Ambiguous` payload carries
    /// the candidates' display pairs (the error path only; `None`/`Single`
    /// allocate nothing).
    pub(crate) fn extension_application(
        &mut self,
        receiver: StringId,
        receiver_ty: Ty,
        member: StringId,
        accept_setter: bool,
        receiver_nullable: bool,
    ) -> ExtensionApp {
        // The reference-edge latch (#2618): cleared per call, set only at a
        // single-face conclusion — the verdict sites read it as the
        // application's resolved target.
        self.members.ext_ref.set(None);
        let setter = self.setter_spelling(member);
        // Applicable faces in declaration order, bucketed by specificity:
        // against a non-nullable receiver a non-nullable `on` type
        // dominates its nullable-`on` twins (`_isMoreSpecific`). A nullable
        // receiver does NOT remove the member — it resolves through the
        // non-nullable `on` face too, but the access is unsafe
        // (`unchecked_use_of_nullable_value`): the `UnsafeSingle` verdict
        // carries that distinction. Equal on-types answer equal — two
        // non-nullable faces stay ambiguous.
        // The face cache borrows `self` mutably (the lazy fill); every
        // immutable lookup it needs is hoisted ahead so the borrow region
        // only spans the walk below.
        let world = self.name_world();
        let object = self.intern("Object");
        let iterable = self.intern("Iterable");
        let strings = self.strings;
        // The lazy fill needs `&mut self`; its result slice then lives as a
        // shared borrow so the argument gate can reach the boundary pool.
        self.accessible_extension_faces();
        let boundary = &self.boundary;
        let faces = self.accessible_extensions.as_deref().unwrap_or(&[]);
        // Applicability is the receiver's subtype relation to the `on`
        // type (`extension_member_resolver.dart` `_getApplicable`: the
        // receiver is assignable to the instantiated extended type), so an
        // extension on `I<T>` applies to `J extends I<T>` — the declared
        // supertype lattice, not name equality.
        // The name-level subtype walk plus the faces it cannot see: every
        // type reaches `Object`, and the unindexed SDK collection roots.
        // `List`/`Set` reach `Iterable` by fiat; the `dart:collection` bases
        // reach every interface their alias entry declares (`HashMap <: Map`,
        // `HashSet <: Set`, `ListQueue <: Queue` — the stub declarations the
        // declared-lattice walk cannot see).
        let reaches = |sub: StringId, sup: StringId| {
            sub == sup
                || sup == object
                || world.named_sub(sub, sup)
                || (sup == iterable && matches!(strings.resolve(sub), "List" | "Set"))
                || crate::member_face::COLLECTION_INTERFACE_ALIASES
                    .iter()
                    .any(|(alias, roots)| {
                        *alias == strings.resolve(sub) && roots.contains(&strings.resolve(sup))
                    })
        };
        // A head the index resolves to a user-declared container carries a
        // complete written-clause model, so the name-level walk over it is
        // exact and a miss is a real miss — an `extension type LookupName`
        // with no `implements` reaches nothing. An SDK builtin head
        // (`String`, `int` — no container anywhere) or an excerpt-declared
        // one answers nothing here: its real supertypes (`String implements
        // Comparable<String>`) are not modeled, so the gates below keep
        // the face (the under-report direction). An owner the graph never
        // registered (the single-file rigs) is in-file, hence user.
        let user_container = |head: StringId| -> bool {
            resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                head,
            )
            .is_some_and(|(_, owner)| {
                self.graph
                    .library_of_file(owner)
                    .is_none_or(|lib| !self.graph.uri(lib).starts_with("dart:"))
            })
        };
        // A head the diagnosing file spells but cannot resolve as a type
        // (the push-time `Missing`/`NonType` verdict) is the oracle's
        // InvalidType even when another library's same-named declaration
        // makes the name-level world know it (`JSPromise<Response>` under a
        // broken `package:web` import beside an unrelated `Response`).
        let invalid_head = |head: StringId| -> bool { self.invalid_type_names.contains(&head) };
        // The instantiated argument's verdict against a face parameter's
        // declared bound (`applicableTo`'s `tryChooseFinalTypes` bound
        // check): `arg <: bound`. An in-scope type parameter answers
        // through its own declared bound — the oracle's
        // `TypeParameterType` subtypes a bound exactly when its own bound
        // does (`subtype.dart:306-317`), so `T extends Enum` satisfies
        // `Enum` while the unbounded `T` reads `Object?` and a concrete
        // bound rejects it (the #2497 `values.byName` row over
        // `List<T>`/`Iterable<T>`). A bound written in terms of a
        // parameter (`T extends S`) chains through `S`'s bound; the hop
        // count caps the recovered cycle (`T extends T`). A shape the
        // name level cannot order keeps the face (the under-report
        // direction).
        let satisfies_bound = |receiver_arg: Ty,
                               bound: Ty,
                               bindings: &[(dartq_types::TypeParamId, dartq_types::TypeId)]|
         -> bool {
            let mut arg = receiver_arg;
            let mut hops = 0u8;
            while let Ty::Named {
                name,
                nullable,
                args: ArgListId::EMPTY,
            } = arg
            {
                // The scope walk has finished by the time the row runs
                // (`frames`/`type_params` are already torn down), so the
                // in-scope test can't see `T`. Its bound survives only in
                // the per-executable/container captures — `param_bound_of`
                // answers the written bound (or the implicit `Object?`)
                // for a name a capture actually declares, `None` for a
                // concrete head (`String`/`Color`), which stays put.
                let Some(declared) = self.param_bound_of(name) else {
                    break;
                };
                // The argument's own `?` survives the substitution: a
                // `T?` instantiation never satisfies a non-nullable
                // bound even when `T`'s declared bound does.
                arg = if nullable {
                    match declared {
                        Ty::Named { name, args, .. } => Ty::Named {
                            name,
                            nullable: true,
                            args,
                        },
                        other => other,
                    }
                } else {
                    declared
                };
                hops += 1;
                if hops >= 8 {
                    break;
                }
            }
            // #2508: both sides concrete — the snapshot's structural
            // subtype answers the bound check (`arg <: bound[T := arg]`,
            // `tryChooseFinalTypes`' instantiation before the subtype
            // test) where the name-level match below can only
            // pattern-match heads. The face-parameter bindings substitute
            // first, so a recursive bound (`T extends Comparable<T>`)
            // reads `Comparable<arg>` exactly like the oracle's inference
            // result. A `None` — an unmodelled arg or bound, or a bound
            // that still mentions a parameter after the substitution —
            // keeps the name-level arms verbatim (the under-report
            // direction). The parameter-arg chain above already resolved
            // every in-scope parameter through `param_bound_of`; a name
            // that still maps to a declared parameter (`type_params` is
            // name-keyed — a container-level parameter no exec window
            // declares) is NOT a concrete pair, so the strict arm
            // declines it the same way.
            if let Some(ops) = self.ops {
                let arg_id = ops.map_ty_strict(arg, &self.boundary.args.borrow());
                let bound_id = ops
                    .map_ty_strict(bound, &self.boundary.args.borrow())
                    .map(|bound_id| ops.substitute(bound_id, bindings));
                if let (Some(arg_id), Some(bound_id)) = (arg_id, bound_id)
                    && !ops.mentions_param(arg_id)
                    && !ops.mentions_param(bound_id)
                    && ops.subtype_ids(arg_id, bound_id)
                {
                    // A structural subtype path is a proof; a structural
                    // `false` is not — the snapshot's hierarchy carries only
                    // the edges the captures fed (an SDK `num` without its
                    // `Comparable<num>` face), so the negative falls to
                    // the name-level arms below.
                    return true;
                }
            }
            match (arg, bound) {
                // `dynamic <: Object` and `dynamic <: Object?` — the only
                // tops a `dynamic` instantiation satisfies; any concrete
                // bound rejects it.
                (Ty::Dynamic, Ty::Named { name, args, .. }) => {
                    args == ArgListId::EMPTY && name == object
                }
                (
                    Ty::Named {
                        name: arg_head,
                        nullable: arg_nullable,
                        args: arg_args,
                    },
                    Ty::Named {
                        name: bound_head,
                        nullable: bound_nullable,
                        args: bound_args,
                    },
                ) => {
                    // A bound carrying its own arguments (the recursive
                    // `T extends Comparable<T>`) decides at the head level
                    // when the bare argument head is a user container —
                    // `extension type LookupName` never satisfies
                    // `Comparable<T>` (the oracle's `applicableTo` fails
                    // the inference and the face never applies), while a
                    // user `implements Comparable<Self>` passes. A nested
                    // argument or an unmodeled argument head keeps the
                    // face (under-report).
                    if arg_args == ArgListId::EMPTY && bound_args != ArgListId::EMPTY {
                        return (!arg_nullable || bound_nullable)
                            && (!user_container(arg_head)
                                || arg_head == bound_head
                                || reaches(arg_head, bound_head));
                    }
                    if arg_args != ArgListId::EMPTY || bound_args != ArgListId::EMPTY {
                        return true;
                    }
                    // A bare argument head the name level cannot order — a
                    // scope parameter the torn-down window lost (`List<T>`
                    // over the class's own `T extends Enum`), or an
                    // undefined spelling (the oracle's InvalidType
                    // subtypes every bound, `subtype.dart`'s invalid arm)
                    // — keeps the face (the under-report posture); only a
                    // head some container or the known world declares is
                    // ordered into the subtype test.
                    (!arg_nullable || bound_nullable)
                        && (arg_head == bound_head
                            || reaches(arg_head, bound_head)
                            || (!user_container(arg_head)
                                && (!world.is_known_head(arg_head) || invalid_head(arg_head))))
                }
                // A shape the name level cannot order (an unknown
                // instantiation, a record or function bound) keeps the
                // face.
                _ => true,
            }
        };
        // A nullable `on` argument position (`on Iterable<T?>`) binds the
        // parameter to the receiver argument's non-nullable core — the
        // inferrer matches `Iterable<L?>` against `Iterable<T?>` as
        // `T := L` — so the bound check reads the core, never the
        // receiver's own `?` (`NullableIterableExtensions<T extends
        // Object>.nonNulls` applies to every `Iterable<X?>`, the face's
        // whole purpose).
        let bind_through_nullable = |on_nullable: bool, receiver_arg: Ty| -> Ty {
            match receiver_arg {
                Ty::Named {
                    name,
                    nullable: true,
                    args,
                } if on_nullable => Ty::Named {
                    name,
                    nullable: false,
                    args,
                },
                other => other,
            }
        };
        // The `on` clause's argument list gates beside the head
        // (`applicable_extensions.dart:205-281` `applicableTo`: the oracle
        // infers the extended type's fresh parameters against the receiver
        // — `on Map<K, FieldItem>` never applies to a
        // `Map<K, MethodItem>` receiver, while an argument spelling the
        // extension's own type parameter instantiates and always passes).
        // The name-level read compares concrete arguments only when the
        // two sides share the head — a subtype hop's argument projection
        // stays unmodeled (the under-report direction), and a side that
        // never interned its list (`Unknown`, a bare head) skips the gate.
        let arg_applicable = |face: &ExtensionFace, face_file: FileId| {
            // The face's `on` type interns in its declaring file's pool —
            // a foreign face's argument list projects across the boundary
            // before the diagnosing pool reads it (same-file faces pass
            // through untouched).
            let on_ty = boundary.ty(face_file, face.on_ty);
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
            let receiver_args = boundary.pool().get(receiver_args).to_vec();
            let on_args = boundary.pool().get(on_args).to_vec();
            // A raw-head receiver (`Map x`, `List xs`, `JSArray a`)
            // instantiates each of ITS OWN declared parameters to that
            // parameter's bound (`instantiateToBounds`: `List<dynamic>`,
            // `JSArray<JSAny?>`) — the receiver DECLARATION's bounds
            // (`Container::type_param_bounds`, projected across its file
            // boundary), never the face's: the face's parameter bound is
            // the question's right-hand side. An unbounded or unmodeled
            // parameter reads `dynamic`, so a bare `Map` against `on Map<K,
            // FieldItem>` fails `dynamic <: FieldItem` and a bare `List`
            // against `T extends AccessorFragment` fails its bound, exactly
            // as the oracle's `applicableTo` rejects both (probed 3.13.1:
            // `List xs; xs.withOrigin` over two bounded `on List<T>` faces
            // is `undefined_getter`, never ambiguous), while a bare
            // `JSArray` reads `JSArray<JSAny?>` and `JSArrayToList<T extends
            // JSAny?>` applies.
            let receiver_args = if receiver_args.is_empty() && !on_args.is_empty() {
                let declared: Vec<Ty> = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    receiver_head,
                )
                .map(|(decl, owner)| {
                    decl.type_param_bounds
                        .iter()
                        .map(|&bound| boundary.ty(owner, bound))
                        .collect()
                })
                .unwrap_or_default();
                (0..on_args.len())
                    .map(|index| declared.get(index).copied().unwrap_or(Ty::Dynamic))
                    .collect()
            } else {
                receiver_args
            };
            // #2508: the face-parameter binding table the strict bound
            // check substitutes — `applicableTo` binds each `on`
            // parameter to the receiver's argument at the same position
            // (the identity projection this whole `arg_applicable` arm
            // already assumes), then asks `arg <: bound[T:=arg]`. A
            // receiver argument that maps nothing structural leaves its
            // slot out, and a bound leaf over that parameter keeps the
            // `TypeParameter` shape — the strict arm declines it (the
            // under-report direction).
            let strict_bindings: Vec<(dartq_types::TypeParamId, dartq_types::TypeId)> =
                match self.ops {
                    Some(ops) if receiver_args.len() == on_args.len() => {
                        let pool = boundary.args.borrow();
                        receiver_args
                            .iter()
                            .zip(on_args.iter())
                            .filter_map(|(&receiver_arg, &on_arg)| {
                                let Ty::Named {
                                    name: param_name,
                                    nullable: on_nullable,
                                    args: param_args,
                                } = on_arg
                                else {
                                    return None;
                                };
                                if param_args != ArgListId::EMPTY {
                                    return None;
                                }
                                let param = ops.param_id(param_name, face_file)?;
                                let bound_arg = bind_through_nullable(on_nullable, receiver_arg);
                                Some((param, ops.map_ty_strict(bound_arg, &pool)?))
                            })
                            .collect()
                    }
                    _ => Vec::new(),
                };
            if receiver_head != on_head {
                // The subtype hop's argument projection is unmodeled; the
                // one decision still decidable across it is the applied
                // face's parameter bounds. `applicableTo` instantiates
                // each `on` parameter to the receiver's corresponding
                // argument — the identity mapping every SDK collection
                // edge writes (`List<E> implements Iterable<E>`) — then
                // fails the inference when the instantiation violates the
                // parameter's declared bound (`Iterable<String>` never
                // applies `EnumByName<T extends Enum>`; an unbounded `T`
                // receiver fails the same way, #2497). A concrete `on`
                // argument gates the receiver's argument covariantly under
                // the same identity projection, decidable only when the
                // receiver argument head is a user container
                // (`on Iterable<BaseName>` never applies to
                // `Set<LookupName>`). Positions whose receiver argument is
                // unmodeled, nested, or raw-defaulted, or whose `on`
                // argument is parameterized or arity-mismatched, stay
                // unmodeled (the under-report direction).
                if receiver_args.len() != on_args.len() {
                    return true;
                }
                return receiver_args
                    .iter()
                    .zip(on_args.iter())
                    .all(|(&receiver_arg, &on_arg)| {
                        let Ty::Named {
                            name: arg_name,
                            nullable: on_nullable,
                            args,
                        } = on_arg
                        else {
                            return true;
                        };
                        if args != ArgListId::EMPTY {
                            return true;
                        }
                        let Some(param_index) =
                            face.type_param_names.iter().position(|&n| n == arg_name)
                        else {
                            let Ty::Named {
                                name: receiver_arg_head,
                                args: receiver_arg_args,
                                ..
                            } = receiver_arg
                            else {
                                return true;
                            };
                            return receiver_arg_args != ArgListId::EMPTY
                                || !user_container(receiver_arg_head)
                                || reaches(receiver_arg_head, arg_name);
                        };
                        let bound = face
                            .type_param_bounds
                            .get(param_index)
                            .copied()
                            .unwrap_or(Ty::Dynamic);
                        let bound = boundary.ty(face_file, bound);
                        if matches!(bound, Ty::Dynamic) {
                            return true;
                        }
                        satisfies_bound(
                            bind_through_nullable(on_nullable, receiver_arg),
                            bound,
                            &strict_bindings,
                        )
                    });
            }
            if receiver_args.len() != on_args.len() {
                return true;
            }
            receiver_args
                .iter()
                .zip(on_args.iter())
                .all(|(&receiver_arg, &on_arg)| {
                    // An `on` argument spelling one of the extension's own
                    // type parameters instantiates against the receiver —
                    // the inferrer binds it, then checks the instantiated
                    // bound: an argument the bound rejects fails the
                    // inference and the face never applies
                    // (`applicable_extensions.dart` `applicableTo`'s
                    // `tryChooseFinalTypes`), the
                    // `_ListOfPropertyAccessorFragment<T extends
                    // PropertyAccessorFragmentImpl> on List<T>` shape. A
                    // raw receiver instantiates to the `dynamic` row
                    // below, which every concrete bound rejects — the
                    // oracle answers `undefined_getter` on `List<dynamic>`
                    // there (probed, /tmp/probe2431/lib/b.dart:38).
                    if let Ty::Named {
                        name: arg_name,
                        nullable: on_nullable,
                        args,
                    } = on_arg
                        && args == ArgListId::EMPTY
                        && let Some(param_index) =
                            face.type_param_names.iter().position(|&n| n == arg_name)
                    {
                        let bound = face
                            .type_param_bounds
                            .get(param_index)
                            .copied()
                            .unwrap_or(Ty::Dynamic);
                        let bound = boundary.ty(face_file, bound);
                        if matches!(bound, Ty::Dynamic) {
                            return true;
                        }
                        return satisfies_bound(
                            bind_through_nullable(on_nullable, receiver_arg),
                            bound,
                            &strict_bindings,
                        );
                    }
                    // Covariant positions: the receiver's argument must be
                    // a subtype of the `on` argument. Only a bare concrete
                    // pair can fail that test at the name level — a nested
                    // argument list on either side stays unmodeled and
                    // compatible (under-report).
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
                        // A `dynamic` receiver argument subtypes every
                        // top-shaped `on` argument (`dynamic <: Object` and
                        // `dynamic <: Object?` — the oracle's `applicableTo`
                        // runs the real subtype test, and both tops accept
                        // it); a concrete `on` argument still rejects it.
                        // Every other non-named pair stays compatible.
                        if matches!(receiver_arg, Ty::Dynamic) {
                            return match on_arg {
                                Ty::Dynamic => true,
                                Ty::Named { name, .. } => name == object,
                                _ => false,
                            };
                        }
                        return true;
                    };
                    if arg_args != ArgListId::EMPTY || receiver_args != ArgListId::EMPTY {
                        return true;
                    }
                    reaches(receiver_head, arg_head)
                })
        };
        // The receiver-container provenance gate (#2504): the oracle's
        // `applicableTo` runs the extended type against the receiver's
        // RESOLVED type, so a face applies only when the receiver's
        // declaration — or a written supertype of it, each resolved in its
        // own declaring scope — IS the declaration the face's `on` head
        // names. The name-level `reaches` walk cannot tell a local
        // `class Iterable<T>` from `dart:core`'s, and its fiat collection
        // edges (`List`/`Set` → `Iterable`, the alias table) are facts
        // about the SDK's own containers alone — a local `List` with no
        // written clause never reaches the SDK's `Iterable`, and a local
        // `Iterable` shadow must not attract the SDK's `EnumByName`. The
        // gate engages only when the face's `on` head is declared in the
        // face's own file (the bundled SDK excerpts and every
        // self-contained user face); a face whose `on` head imports from
        // elsewhere keeps the ungated posture (nothing in-file to compare
        // against). The walk accepts only a step that lands ON the `on`
        // head's own declaration (same name, same file) — the receiver
        // itself or a WRITTEN clause hop (`extends`/`with`/`implements`, a
        // mixin's `on` constraints), each resolved in its owner's own
        // scope so a shadow in the diagnosing unit never reaches a
        // foreign declaration's clause. An unresolvable receiver head or
        // clause hop keeps the face (the under-report direction); the
        // walk stays bounded by its visited set with a visit cap.
        let provenance_ok =
            |face_file: FileId, receiver_head: StringId, on_head: StringId| -> bool {
                if self
                    .index
                    .get(face_file)
                    .and_then(|elements| elements.container(on_head))
                    .is_none()
                {
                    return true;
                }
                let mut queue: Vec<(StringId, LibraryId, FileId)> =
                    vec![(receiver_head, self.lib, self.file)];
                let mut seen: Vec<(FileId, StringId)> = Vec::new();
                let mut unresolved = false;
                let mut visits = 0usize;
                while let Some((name, scope_lib, scope_file)) = queue.pop() {
                    visits += 1;
                    if visits > 256 {
                        return true;
                    }
                    let Some((decl, owner)) = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        scope_lib,
                        scope_file,
                        name,
                    ) else {
                        unresolved = true;
                        continue;
                    };
                    if !seen.contains(&(owner, name)) {
                        seen.push((owner, name));
                    } else {
                        continue;
                    }
                    if name == on_head && owner == face_file {
                        return true;
                    }
                    // The clause names resolve in the owner's own library;
                    // an owner the graph never registered as a unit (the
                    // single-file rigs) falls back to the scope that
                    // resolved this hop — for an in-file owner that IS its
                    // library (the under-report direction for a foreign
                    // one).
                    let owner_lib = self.graph.library_of_file(owner).unwrap_or(scope_lib);
                    for sup in decl
                        .superclass
                        .iter()
                        .chain(decl.with_clause.iter())
                        .chain(decl.implements_clause.iter())
                        .chain(decl.on_clause.iter())
                    {
                        queue.push((*sup, owner_lib, owner));
                    }
                }
                // The walk exhausted without landing on the on-head's own
                // declaration: inapplicable, unless a hop it could not
                // resolve may have hidden the path (keep the face).
                unresolved
            };
        // The oracle's applicability test instantiates the extended type
        // against the receiver's RESOLVED type — an `InvalidType` receiver
        // never applies an extension (`applicableTo` returns no
        // `ExtensionResolution`), so the member stays uncredited and the
        // access keeps its own `undefined_*` face. The name-level
        // `InvalidType` read is [`Self::extension_receiver_resolves`]; a
        // `dynamic` receiver dispatches dynamically — no extension binds it
        // either.
        // An extension body's `this` receiver carries the container as its
        // head (the extension's own name resolves NonType — a getter-scope
        // element, not a type): the `on` clause's interned `Ty` is the
        // resolvable receiver shape, so the head check defers to it there.
        let receiver_valid = !matches!(receiver_ty, Ty::Dynamic)
            && if receiver_ty == Ty::UNKNOWN {
                self.type_resolution(receiver).0 == TypeResolution::Type
            } else {
                self.extension_receiver_resolves(receiver_ty)
            };
        // #2508: the mounted `LibraryTypes` snapshot answers the
        // applicability test structurally — `applicableTo` instantiates
        // the extended type's parameters against the receiver's RESOLVED
        // type (`constrainArgument` over the supertype projection) and
        // asks `isSubtypeOf(receiver, extendedType)`, so a generic hop's
        // argument projection (`Box<int>` reaching `Iterable<Wrapper<int>>`
        // against `on Iterable<T>`) decides by the real lattice instead
        // of the name-level argument gate. The `on` type's own parameter
        // leaves bind through the face's declared parameter names
        // (`snapshot.type_params` is name-keyed — a name no declaration
        // registers simply binds nothing). A `None` verdict — an
        // unmodeled receiver/on shape, an unregistered head, a bound the
        // capture never fed — keeps the whole name-level gate chain
        // verbatim (the under-report direction).
        let ops = self.ops;
        let applies_strict = |face: &ExtensionFace, face_file: FileId, on_ty: Ty| -> Option<bool> {
            let ops = ops?;
            // The face's own bounds ride the question explicitly
            // ([`Engine::strict_face_params`]); a bound the snapshot cannot
            // map declines the whole strict verdict.
            let face_params = self.strict_face_params(face, face_file)?;
            ops.extension_applies_strict(receiver_ty, on_ty, &face_params, &boundary.args.borrow())
        };
        let mut applicable: Vec<(&ExtensionFace, Ty, FileId, Option<dartq_types::TypeId>)> =
            Vec::new();
        if receiver_valid {
            for &(face, face_file) in faces {
                let Some(on) = face.on_type else { continue };
                // Only the extension's INSTANCE members participate in an
                // application (`extension_member_resolver.dart` instantiates
                // the extension for the receiver and looks the member up in
                // the instantiated interface — a `static` member of the
                // extension never answers an instance access, so
                // `extension E on C { static void a() {} }` leaves `c.a()`
                // the class's own `undefined_method` face).
                if !(face.members.get(&member).is_some_and(|e| !e.is_static)
                    || (accept_setter
                        && face.members.get(&setter).is_some_and(|e| !e.is_static)))
                {
                    continue;
                }
                // The full `on` type projects into the diagnosing pool
                // ahead of both gates — the structural arm needs the
                // projected shape, and the name-level arm reads it
                // verbatim below.
                let on_ty = boundary.ty(face_file, face.on_ty);
                // An `on` clause spelling the extension's OWN parameter
                // (`extension<T extends AstNode> on T`): the parameter
                // instantiates to the receiver's type, so the head gate
                // becomes the parameter's declared bound — every receiver
                // the bound admits applies (an absent bound is the top).
                // The nullable spelling (`extension<T> on T?`) answers the
                // same bound question against the receiver's non-null
                // core — `T?` accepts every receiver whose core satisfies
                // the bound, nullable or not (the face's whole purpose:
                // `extension<T> on T? { mapOrNull }` over any `T?`
                // receiver, a safe application on the nullable side).
                let own_param_applies = (|| {
                    let Some(head) = face.on_type else {
                        return false;
                    };
                    let Ty::Named { args, .. } = on_ty else {
                        return false;
                    };
                    if args != ArgListId::EMPTY {
                        return false;
                    }
                    let Some(index) = face.type_param_names.iter().position(|&n| n == head) else {
                        return false;
                    };
                    match face.type_param_bounds.get(index).copied() {
                        None | Some(Ty::Dynamic) | Some(Ty::Unknown) => true,
                        Some(bound) => match boundary.ty(face_file, bound) {
                            Ty::Named { name, .. } => name == object || reaches(receiver, name),
                            _ => false,
                        },
                    }
                })();
                let applicable_face = matches!(applies_strict(face, face_file, on_ty), Some(true))
                    || (reaches(receiver, on)
                        && provenance_ok(face_file, receiver, on)
                        && arg_applicable(face, face_file))
                    || own_param_applies;
                if !applicable_face {
                    continue;
                }
                // #3090: the receiver-instantiated `on` type rides the
                // step-3-4 comparison below. A face parameter bound to the
                // receiver's argument (`on Bag<T>` over `Bag<SubFragment>`)
                // then orders by the real lattice instead of its spelling;
                // a declined binding keeps the name-level read.
                let instantiated = self.strict_face_params(face, face_file).and_then(|params| {
                    self.ops.and_then(|ops| {
                        ops.instantiated_on(receiver_ty, on_ty, &params, &boundary.args.borrow())
                    })
                });
                applicable.push((face, on_ty, face_file, instantiated));
            }
        }

        // `_isMoreSpecific` steps 3-4: the oracle compares the extended
        // types as `applicableTo` inferred them against the receiver
        // (`extension_member_resolver.dart:484-497`), never their bare
        // parameter spellings. `instantiated_on` supplies that form; when
        // BOTH faces instantiate structurally the real lattice is
        // authoritative in both directions — a definitive `false` must not
        // fall back to the name-level shape read, which answers compatible
        // for nested argument lists (`on Bag<List<T>>` vs
        // `on Bag<List<Fragment>>`) and would hide the order. A declined
        // binding (`None`) keeps the name-level arms verbatim.
        let on_subtype = |sub: Ty,
                          sup: Ty,
                          sub_inst: Option<dartq_types::TypeId>,
                          sup_inst: Option<dartq_types::TypeId>|
         -> bool {
            if let (Some(ops), Some(sub_inst), Some(sup_inst)) = (self.ops, sub_inst, sup_inst) {
                return ops.subtype_ids(sub_inst, sup_inst);
            }
            // #2508: the snapshot orders the instantiated `on` types
            // structurally where it can — a generic face's
            // `Iterable<Wrapper<int>>` subtyping its `Iterable<Base>`
            // twin decides through the real lattice, not the covariant
            // head-argument read below. `None` keeps the name-level arms.
            if self
                .ops
                .and_then(|ops| ops.try_subtype_strict(sub, sup, &self.boundary.args.borrow()))
                == Some(true)
            {
                return true;
            }
            let (
                Ty::Named {
                    name: sub_head,
                    nullable: sub_nullable,
                    args: sub_args,
                },
                Ty::Named {
                    name: sup_head,
                    nullable: sup_nullable,
                    args: sup_args,
                },
            ) = (sub, sup)
            else {
                return true;
            };
            if sub_nullable && !sup_nullable {
                return false;
            }
            if sub_head != sup_head {
                return reaches(sub_head, sup_head);
            }
            if sub_args == ArgListId::EMPTY || sup_args == ArgListId::EMPTY {
                return true;
            }
            // The pools are cold-path reads (ambiguity resolution only);
            // the argument gate's same to_vec copy sidesteps the temporary
            // borrow.
            let sub_args = boundary.pool().get(sub_args).to_vec();
            let sup_args = boundary.pool().get(sup_args).to_vec();
            if sub_args.len() != sup_args.len() {
                return true;
            }
            sub_args.iter().zip(sup_args.iter()).all(|(&a, &b)| {
                let (
                    Ty::Named {
                        name: a_head,
                        nullable: a_nullable,
                        args: a_args,
                    },
                    Ty::Named {
                        name: b_head,
                        nullable: b_nullable,
                        args: b_args,
                    },
                ) = (a, b)
                else {
                    return true;
                };
                if a_nullable && !b_nullable {
                    return false;
                }
                if a_args != ArgListId::EMPTY || b_args != ArgListId::EMPTY {
                    return true;
                }
                a_head == b_head || reaches(a_head, b_head)
            })
        };
        // Step 5's instantiate-to-bounds projection
        // (`extension_member_resolver.dart:459-466` `_instantiateToBounds`):
        // every bare own-parameter position in the `on` type's arguments is
        // replaced by its declared bound — a bound-less parameter by
        // `dynamic`, the Dart reading of unbounded. A bound written in
        // terms of its own parameter (the recursive
        // `T extends Comparable<T>`) substitutes as written; its residual
        // parameter rides the comparison as the unmodeled shape. A
        // non-parameter argument, an argument-less `on` type, or a face
        // without type parameters projects to itself.
        let bound_projected = |face: &ExtensionFace, face_file: FileId, on_ty: Ty| -> Ty {
            let Ty::Named {
                name,
                nullable,
                args,
            } = on_ty
            else {
                return on_ty;
            };
            if args == ArgListId::EMPTY || face.type_param_names.is_empty() {
                return on_ty;
            }
            let on_args = boundary.pool().get(args).to_vec();
            let projected: Vec<Ty> = on_args
                .iter()
                .map(|&arg| {
                    let Ty::Named {
                        name: param,
                        args: param_args,
                        ..
                    } = arg
                    else {
                        return arg;
                    };
                    if param_args != ArgListId::EMPTY {
                        return arg;
                    }
                    let Some(index) = face.type_param_names.iter().position(|&n| n == param) else {
                        return arg;
                    };
                    let bound = face
                        .type_param_bounds
                        .get(index)
                        .copied()
                        .unwrap_or(Ty::Dynamic);
                    boundary.ty(face_file, bound)
                })
                .collect();
            let projected_args = boundary.args.borrow_mut().named(name, nullable, &projected);
            match projected_args {
                Ty::Named { args, .. } => Ty::Named {
                    name,
                    nullable,
                    args,
                },
                _ => on_ty,
            }
        };
        // Step 5's pair test at the projections' name level. Unlike the
        // steps 3-4 read, the projections' arguments carry the bounds, so
        // the unbounded parameter's `dynamic` instantiation decides
        // strictly: it subtypes only the tops (`dynamic` itself and the
        // argument-less `Object`/`Object?`) and every concrete bound
        // rejects it — the `IterableComparableExtension<T extends
        // Comparable<T>>`-over-`IterableExtension<T>` ordering (probed,
        // /tmp/probe2431/lib/a.dart and d.dart). A shape the name level
        // cannot model (a residual parameter inside a bound, nested
        // argument lists) answers unordered — the pair stays mutually
        // compatible and step 5 decides nothing (the under-report
        // direction).
        let bounds_subtype = |sub: Ty, sup: Ty| -> bool {
            let (
                Ty::Named {
                    name: sub_head,
                    nullable: sub_nullable,
                    args: sub_args,
                },
                Ty::Named {
                    name: sup_head,
                    nullable: sup_nullable,
                    args: sup_args,
                },
            ) = (sub, sup)
            else {
                return false;
            };
            if sub_nullable && !sup_nullable {
                return false;
            }
            if sub_head != sup_head {
                return reaches(sub_head, sup_head);
            }
            if sub_args == ArgListId::EMPTY || sup_args == ArgListId::EMPTY {
                return true;
            }
            let sub_args = boundary.pool().get(sub_args).to_vec();
            let sup_args = boundary.pool().get(sup_args).to_vec();
            if sub_args.len() != sup_args.len() {
                return true;
            }
            sub_args.iter().zip(sup_args.iter()).all(|(&a, &b)| {
                // `dynamic <: dynamic`, and `dynamic <: Object`/`Object?` —
                // nothing else accepts it.
                if matches!(b, Ty::Dynamic) {
                    return true;
                }
                if matches!(a, Ty::Dynamic) {
                    return match b {
                        Ty::Named {
                            name,
                            args: ArgListId::EMPTY,
                            ..
                        } => name == object,
                        _ => false,
                    };
                }
                let (
                    Ty::Named {
                        name: a_head,
                        nullable: a_nullable,
                        ..
                    },
                    Ty::Named {
                        name: b_head,
                        nullable: b_nullable,
                        ..
                    },
                ) = (a, b)
                else {
                    return false;
                };
                if a_nullable && !b_nullable {
                    return false;
                }
                if a_head != b_head {
                    return reaches(a_head, b_head);
                }
                // Equal heads with argument lists on both sides: only a
                // concrete bare pair could still order, and steps 3-4
                // would have ordered it — what reaches here carries a
                // residual parameter (the recursive bound) and stays
                // unordered.
                false
            })
        };
        // The oracle's specificity order opens with provenance, not types
        // (`extension_member_resolver.dart:470-479`): a non-SDK extension is
        // unconditionally more specific than an SDK one, so a user-space
        // twin silently wins over the platform's own face (package:
        // collection's `IterableExtension` over dart:core's
        // `IterableExtensions`) before any subtype comparison runs — the
        // oracle never reports the pair ambiguous. A face file the graph
        // never registered answers non-SDK (the under-report direction:
        // the twin pair stays unordered). Only the synthetic face files
        // carry `dart:` library URIs — real source libraries are
        // `package:`/path URIs.
        let sdk_origin: Vec<bool> = applicable
            .iter()
            .map(|&(_, _, face_file, _)| {
                self.graph
                    .library_of_file(face_file)
                    .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"))
            })
            .collect();
        let mut specific: Vec<(&ExtensionFace, FileId, (String, String))> = Vec::new();
        let mut nullable: Vec<(&ExtensionFace, FileId, (String, String))> = Vec::new();
        for (index, &(face, on_ty, face_file, on_inst)) in applicable.iter().enumerate() {
            let dominated = applicable.iter().enumerate().any(|(other, _)| {
                if other == index {
                    return false;
                }
                let (other_face, other_on_ty, other_file, other_inst) = applicable[other];
                // A face whose `on` type is non-nullable is inapplicable to
                // a nullable receiver (`applicableTo` rejects it: `C?` is
                // not assignable to the non-null `on` type), so it never
                // gets a domination vote against a nullable-`on` twin —
                // the vote would kill the ONLY applicable face and flip the
                // access to the unsafe arm (`element.supertype
                // .isAbiSpecificInteger` over the `on DartType?`/`on
                // DartType` twins, ffi_verifier.dart:2644). The oracle's
                // `_getApplicable` drops the non-null face before
                // `_isMoreSpecific` ever runs.
                if receiver_nullable && !other_face.on_type_nullable && face.on_type_nullable {
                    return false;
                }
                // Steps 1-2: the SDK-precedence order decides before the
                // subtype steps; same-origin pairs fall through to them.
                if sdk_origin[other] != sdk_origin[index] {
                    return !sdk_origin[other];
                }
                // A re-exported platform extension answers under every
                // exporting library (`dart:core` re-exports `dart:
                // collection`'s `IterableExtensions`, core.dart:182), so
                // the projection registers one declaration as a face per
                // library — two same-named same-`on` platform faces are
                // that declaration's aliases, never a competing pair. The
                // earlier entry survives (a stable positional tiebreak).
                if sdk_origin[index]
                    && other_face.name == face.name
                    && other_face.on_type == face.on_type
                {
                    return other < index;
                }
                if on_subtype(other_on_ty, on_ty, other_inst, on_inst) {
                    if !on_subtype(on_ty, other_on_ty, on_inst, other_inst) {
                        // Steps 3-4: the instantiated `on` types order.
                        return true;
                    }
                    // Step 5 (`extension_member_resolver.dart:500-506`):
                    // mutually compatible `on` types order on the
                    // instantiate-to-bounds projections — the bounded face
                    // dominates its unbounded twin when the projections
                    // order one way and not the other. #2508: the
                    // snapshot's own `ParamFacts` bounds drive the
                    // projection where both `on` types map; a `None`
                    // verdict keeps the name-level projection and pair
                    // test verbatim.
                    // Each face's OWN bounds (`strict_face_params`) feed
                    // the projection — the snapshot's name-keyed registry
                    // would fold two faces' same-spelled parameters into
                    // one bound.
                    let strict = match (
                        self.ops,
                        self.strict_face_params(other_face, other_file),
                        self.strict_face_params(face, face_file),
                    ) {
                        (Some(ops), Some(sub_params), Some(sup_params)) => ops.try_bounds_subtype(
                            other_on_ty,
                            &sub_params,
                            on_ty,
                            &sup_params,
                            &self.boundary.args.borrow(),
                        ),
                        _ => None,
                    };
                    // Either world's `true` proves a direction; the
                    // negative needs both worlds (a structural `false` over
                    // an incompletely fed hierarchy is no fact).
                    let sub = bound_projected(other_face, other_file, other_on_ty);
                    let sup = bound_projected(face, face_file, on_ty);
                    let (strict_sub_le_sup, strict_sup_le_sub) = strict.unwrap_or((false, false));
                    (strict_sub_le_sup || bounds_subtype(sub, sup))
                        && !(strict_sup_le_sub || bounds_subtype(sup, sub))
                } else {
                    false
                }
            });
            if dominated {
                continue;
            }
            let display = self.extension_display(face);
            // An `on` clause naming the face's OWN type parameter
            // (`extension<T> on T`) instantiates that parameter to the
            // receiver, so a nullable receiver is admitted SAFELY when the
            // parameter's bound admits it — the implicit unbounded `Object?`
            // (a `Ty::Dynamic`/absent bound) or a nullable written bound
            // (`T extends num?`). Such a face is the nullable-safe match; it
            // must not force the `UnsafeSingle` verdict (the
            // `unchecked_use_of_nullable_value` row) that a non-nullable
            // `on C` face earns. A non-nullable written bound (`T extends
            // num`) over a nullable receiver stays in the `specific` bucket
            // and keeps the unchecked row, exactly as the oracle reports it.
            // Mirrors `this_extension_member_applies`'s `on T` arm (the
            // oracle's `applicableTo` instantiates the parameter to the
            // receiver, so the declared bound decides; upstream Dart SDK
            // source, pkg/analyzer resolver/extension_member_resolver.dart
            // `_getApplicable`).
            let self_param_nullable_safe = receiver_nullable
                && matches!(on_ty, Ty::Named { args: ArgListId::EMPTY, .. })
                && face
                    .on_type
                    .and_then(|head| {
                        face.type_param_names.iter().position(|&param| param == head)
                    })
                    .is_some_and(|index| match face.type_param_bounds.get(index).copied() {
                        None | Some(Ty::Dynamic) | Some(Ty::Unknown) => true,
                        Some(bound) => matches!(
                            boundary.ty(face_file, bound),
                            Ty::Named { nullable: true, .. }
                        ),
                    });
            if face.on_type_nullable || self_param_nullable_safe {
                nullable.push((face, face_file, display));
            } else {
                specific.push((face, face_file, display));
            }
        }
        // On a nullable receiver a nullable `on` face is the safe match;
        // only when none applies does the unsafe non-nullable set answer.
        let (candidates, unsafe_match) = if receiver_nullable {
            if nullable.is_empty() {
                (specific, true)
            } else {
                (nullable, false)
            }
        } else if specific.is_empty() {
            (nullable, false)
        } else {
            (specific, false)
        };
        // A uniquely applied member is used (the official `usedElements.members`
        // face): the application resolves the access, so the member's
        // declaration is spared. Both the read and — when the write
        // accepted it — the setter spelling join; a spelling no face
        // declares never matches a finalize key, so over-inserting is
        // inert. The applied face also credits its contributing import
        // (`ImportsTracking.notifyExtensionUsed`, scope.dart:308-321): the
        // import that provided the extension is used, an own-library face
        // contributes nothing.
        // An unsafe match credits nothing: `applicableTo` rejects the
        // non-nullable `on` type against the nullable receiver (the access
        // reads a possibly-null receiver — the oracle's
        // `unchecked_use_of_nullable_value` row, emitted by the caller's
        // `UnsafeSingle` arm), so the member never enters `usedElements`
        // and the import stays uncredited — the oracle's `unused_element`/
        // `unused_import` still fire on both.
        // An ambiguous access resolves no member (the oracle leaves every
        // candidate unreferenced — `unused_element` still fires on each).
        if candidates.len() == 1 && !unsafe_match {
            let (face, face_file, _) = candidates[0];
            // The element key: the face's position in its declaring file's
            // `extension_faces()` pool (pointer identity IS element identity
            // within an analysis — the pool is frozen before the walk). A
            // face the index cannot see credits nothing rather than guessing
            // a slot.
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
                    .insert((face_file, face_index, member));
                if accept_setter {
                    self.extension_member_used
                        .insert((face_file, face_index, setter));
                }
            }
            self.credit_extension_import(face, face_file);
        }

        // The single-face conclusion latches its reference-edge target
        // (#2618) — the winning face's declaring file, the resolved lookup
        // spelling (a write through a face declaring only `name=` targets
        // the setter), the face's own name (the edge's declaring
        // container; an unnamed extension has none to name — the sentinel
        // keeps its rows out of every container-scoped join, the unnamed
        // face's standing degrade), and the member's kind (the tear-off
        // shape).
        if let [(face, face_file, _)] = candidates.as_slice() {
            let spelling = if accept_setter && face.members.contains_key(&setter) {
                setter
            } else {
                member
            };
            let kind = face
                .members
                .get(&spelling)
                .map_or(MemberKind::Method, |entry| entry.kind);
            self.members.ext_ref.set(Some((
                *face_file,
                spelling,
                face.name.unwrap_or(StringId::NONE),
                kind,
            )));
        }
        match candidates.len() {
            0 => ExtensionApp::None,
            1 if unsafe_match => ExtensionApp::UnsafeSingle,
            1 => ExtensionApp::Single,
            _ => ExtensionApp::Ambiguous(
                candidates
                    .into_iter()
                    .map(|(_, _, display)| display)
                    .collect(),
            ),
        }
    }

    /// Whether the receiver's interned type resolves at its head — the
    /// oracle's `InvalidType` test `extension_application` gates on. Only
    /// the top-level `Named` head decides: `Missing x` leaves the receiver
    /// an `InvalidType`, so `applicableTo` returns no `ExtensionResolution`
    /// and the access resolves nothing. A resolved head carrying an
    /// unresolvable name deeper inside is still a valid interface/record/
    /// function type, not an `InvalidType`: `List<InvalidType>` stays an
    /// `InterfaceType`, a `(Missing,)` record stays a `RecordType`, and a
    /// callee's unbound type-parameter spelling (the `A` in `seq2`'s
    /// declared `Parser<(A, B)>`) instantiates at the send — so the member
    /// face runs and the `on`-clause gate decides applicability. Probed
    /// against the oracle: `MyList<Missing>` applies `on MyList<C>` (the
    /// member resolves, no `undefined_*`), while a `(Missing,)` receiver
    /// reports `undefined_getter` through the argument gate's own verdict.
    pub(crate) fn extension_receiver_resolves(&self, receiver_ty: Ty) -> bool {
        match receiver_ty {
            Ty::Named { name, .. } => {
                self.type_resolution(name).0 == TypeResolution::Type
                    // A head the diagnosing scope cannot name but exactly
                    // one graph library declares is still a valid static
                    // type (it arrived through a resolved member
                    // signature) — extensions imported here apply to it
                    // (`node.parentElement` on the unimported `XmlNode`).
                    || self.unique_declared_elsewhere(name, self.lib).is_some()
            }
            _ => true,
        }
    }

    /// The function-receiver twin of [`Self::record_extension_face`]: a
    /// receiver whose static type is a FUNCTION type (`extension on
    /// PostMessage` over `typedef PostMessage = void Function(..)`) names
    /// no container either. Both sides fold through aliases; the folded
    /// `on` face then answers `applicableTo`'s subtype ask whole —
    /// `receiver <: on` ([`Self::resolved_subtype`],
    /// applicable_extensions.dart:251-270): a concrete `on` shape decides
    /// by covariance/contravariance over the signature (`on void
    /// Function()` accepts the `int Function()` literal's return face), a
    /// NAMED `on` head spells one of the shape's named supertypes — every
    /// function type is a `Function`, and a non-nullable one is an
    /// `Object` — and the nullability pair rides the Left Nullable legs
    /// (`int Function()? <: Object` fails, `<: Object?` holds). A face
    /// whose `on` type spells the extension's own type parameters inside
    /// its shape (`extension E<T> on T Function(T)`) asks
    /// [`Self::fn_param_binding_applies`]'s inference-shaped question; a
    /// parameter the binding walk cannot spell keeps the face undecided
    /// (no credit, the under-report direction). Single hit only, like the
    /// record face.
    pub(crate) fn function_extension_face(
        &mut self,
        receiver: Ty,
        member: StringId,
    ) -> Option<(&'a ExtensionFace, FileId)> {
        if !matches!(receiver, Ty::Function { .. }) {
            return None;
        }
        self.accessible_extension_faces();
        let faces = self.accessible_extensions.as_deref().unwrap_or(&[]);
        let mut hits: Vec<(&'a ExtensionFace, FileId)> = Vec::new();
        for &(face, face_file) in faces {
            if !face.members.contains_key(&member) {
                continue;
            }
            let on_ty = self.boundary.ty(face_file, face.on_ty);
            // A face whose `on` type spells its own bare parameter
            // (`extension E<T> on T`) binds `T := receiver` wholesale —
            // `applicableTo` applies iff the receiver's non-null core
            // satisfies the parameter's declared bound (an absent or
            // `dynamic` bound is the top). Every other param-spelling
            // `on` type is left undecided (no credit, the under-report
            // direction this face has always kept).
            let applies = if let Ty::Named {
                name: on_head,
                args: ArgListId::EMPTY,
                ..
            } = on_ty
                && let Some(index) = face.type_param_names.iter().position(|&n| n == on_head)
            {
                match face.type_param_bounds.get(index).copied() {
                    None | Some(Ty::Dynamic) | Some(Ty::Unknown) => true,
                    Some(bound) => {
                        let bound =
                            Self::non_null_form(self.boundary.ty(face_file, bound));
                        self.resolved_subtype(Self::non_null_form(receiver), bound)
                    }
                }
            } else {
                let folded = self.fold_alias_ty(on_ty);
                // `applicableTo`'s subtype ask (`targetType <: extendedType`,
                // applicable_extensions.dart:251-270): the concrete `on` face
                // — `on void Function()`, `on Function`, `on Object` — folds
                // to the same subtype question the interface lattice answers
                // whole (a function's only named supertypes are `Function`
                // and `Object`, and the nullability pair rides the Left
                // Nullable legs). An `on` face spelling the face's own type
                // parameters inside its shape (`extension E<T> on T
                // Function(T)`) asks the inference-shaped question: gather
                // each bare own-param leaf's binding off the receiver's
                // matching shape slot, substitute, then answer the same
                // subtype ask — plus the declared-bound check every bound
                // parameter owes (`tryChooseFinalTypes`). A leaf the binding
                // walk cannot spell (a parameter nested inside a compound
                // leaf, an unaligned skeleton) leaves the face undecided.
                if face.type_param_names.is_empty()
                    || !self.ty_mentions_params(folded, &face.type_param_names)
                {
                    self.resolved_subtype(receiver, folded)
                } else {
                    self.fn_param_binding_applies(receiver, folded, face, face_file)
                }
            };
            if applies {
                hits.push((face, face_file));
            }
        }
        (hits.len() == 1).then(|| hits[0])
    }

    /// The `extension E<T> on T Function(..)` application ask
    /// ([`Self::function_extension_face`]'s param-spelling arm): each bare
    /// own-parameter leaf in the `on` shape binds the receiver's aligned
    /// leaf (`applicableTo`'s `constrainArgument(targetType, extendedType)`
    /// — the return slot covariant, the formal slots contravariant; at the
    /// binding level both read the same `T := leaf` fact). An unaligned
    /// skeleton, an inconsistent pair, or a parameter a compound leaf hides
    /// leaves the application undecided (`false` — the under-report
    /// direction). A consistent binding substitutes through
    /// [`Self::ty_subst_named`], then the whole `recv <: on[subst]` ask
    /// decides — arity, variance, and the declared-bound leg included.
    fn fn_param_binding_applies(
        &self,
        receiver: Ty,
        on_ty: Ty,
        face: &ExtensionFace,
        face_file: FileId,
    ) -> bool {
        let (
            Ty::Function {
                shape: recv_shape, ..
            },
            Ty::Function { shape: on_shape, .. },
        ) = (receiver, on_ty)
        else {
            return false;
        };
        let (recv_shape, on_shape) = {
            let pool = self.boundary.pool();
            (
                pool.function_shape(recv_shape).clone(),
                pool.function_shape(on_shape).clone(),
            )
        };
        if on_shape.type_params != 0
            || on_shape.positional.len() != recv_shape.positional.len()
            || on_shape.named.len() != recv_shape.named.len()
            || on_shape
                .named
                .iter()
                .zip(recv_shape.named.iter())
                .any(|(on, recv)| on.0 != recv.0)
        {
            return false;
        }
        // Each bare own-param leaf records a constraint, not a binding:
        // the return slot is covariant (`recv <: on`, so the receiver's
        // leaf LOWER-bounds the parameter — `T >= leaf`), the formal
        // slots are contravariant (`on <: recv`, so they UPPER-bound —
        // `T <= leaf`). `constrainArgument` collects exactly these; a
        // leaf nested in a compound slot keeps the whole face undecided.
        let mut lowers: FxHashMap<StringId, Vec<Ty>> = FxHashMap::default();
        let mut uppers: FxHashMap<StringId, Vec<Ty>> = FxHashMap::default();
        if let Ty::Named {
            name: leaf,
            args: ArgListId::EMPTY,
            ..
        } = on_shape.return_ty
            && face.type_param_names.contains(&leaf)
        {
            lowers.entry(leaf).or_default().push(recv_shape.return_ty);
        }
        for (on_leaf, recv_leaf) in on_shape
            .positional
            .iter()
            .zip(recv_shape.positional.iter())
            .chain(
                on_shape
                    .named
                    .iter()
                    .zip(recv_shape.named.iter())
                    .map(|(on, recv)| (&on.1, &recv.1)),
            )
        {
            let Ty::Named {
                name: leaf,
                args: ArgListId::EMPTY,
                ..
            } = on_leaf
            else {
                continue;
            };
            if face.type_param_names.contains(leaf) {
                uppers.entry(*leaf).or_default().push(*recv_leaf);
            }
        }
        // Every parameter must bind somewhere — an unconstrained slot is
        // one the inferrer solves to its bound, a face the name level
        // cannot spell inside the substituted `on`.
        if face.type_param_names.iter().any(|param| {
            !lowers.contains_key(param) && !uppers.contains_key(param)
        }) {
            return false;
        }
        // Solve each parameter's interval: a candidate binding must lie
        // above every lower constraint and below every upper one. The
        // lower candidates are the only answers the ask commits to —
        // try each in turn; the first that satisfies every leg (and its
        // declared bound — `tryChooseFinalTypes` rejects a solution
        // outside it) wins. A parameter with only upper constraints
        // takes its own first upper as the trial face.
        let mut bindings: FxHashMap<StringId, Ty> = FxHashMap::default();
        for param in &face.type_param_names {
            let lower_set = lowers.get(param).map(Vec::as_slice).unwrap_or(&[]);
            let upper_set = uppers.get(param).map(Vec::as_slice).unwrap_or(&[]);
            let mut candidates: Vec<Ty> = lower_set.to_vec();
            for upper in upper_set {
                if !candidates.contains(upper) {
                    candidates.push(*upper);
                }
            }
            let solved = candidates.iter().copied().find(|candidate| {
                lower_set
                    .iter()
                    .all(|lower| self.resolved_subtype(*lower, *candidate))
                    && upper_set
                        .iter()
                        .all(|upper| self.resolved_subtype(*candidate, *upper))
            });
            let Some(binding) = solved else {
                return false;
            };
            bindings.insert(*param, binding);
        }
        // Every bound parameter's declared bound still owes its check —
        // `tryChooseFinalTypes` rejects a solution outside it (an absent
        // or `dynamic` bound is the top and accepts any binding).
        for (index, bound) in face.type_param_bounds.iter().enumerate() {
            if matches!(bound, Ty::Dynamic | Ty::Unknown) {
                continue;
            }
            let Some(param) = face.type_param_names.get(index) else {
                continue;
            };
            let Some(binding) = bindings.get(param) else { continue };
            let bound = Self::non_null_form(self.boundary.ty(face_file, *bound));
            if !self.resolved_subtype(Self::non_null_form(*binding), bound) {
                return false;
            }
        }
        let on_subst = self.ty_subst_named(on_ty, &bindings);
        self.resolved_subtype(receiver, on_subst)
    }

    /// `ty` with every bare `Named` leaf in `map` replaced by its binding
    /// (the `?` flag joins through [`Self::join_nullable_ty`]'s rule —
    /// a `T?` leaf over a non-null binding stays nullable). Compound
    /// leaves rebuild through the boundary pool; anything unmapped keeps
    /// its written face. The `on`-shape twin of `assignability.rs`'s
    /// signature-level `subst_ty` — this face substitutes a standalone
    /// [`Ty`], not an `FnVarSig`.
    fn ty_subst_named(&self, ty: Ty, map: &FxHashMap<StringId, Ty>) -> Ty {
        if let Ty::Named {
            name,
            nullable,
            args: ArgListId::EMPTY,
        } = ty
            && let Some(replacement) = map.get(&name)
        {
            return Self::join_nullable_ty(*replacement, nullable);
        }
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } if args != ArgListId::EMPTY => {
                let written: Vec<Ty> = {
                    let pool = self.boundary.pool();
                    pool.get(args).to_vec()
                };
                let substituted: Vec<Ty> = written
                    .iter()
                    .map(|arg| self.ty_subst_named(*arg, map))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .named(name, nullable, &substituted)
            }
            Ty::Function { shape, nullable } => {
                let (type_params, type_params_named, return_ty, positional, named) = {
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
                let positional: Vec<Ty> = positional
                    .iter()
                    .map(|ty| self.ty_subst_named(*ty, map))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(name, ty)| (*name, self.ty_subst_named(*ty, map)))
                    .collect();
                let names: Vec<(StringId, Option<Ty>)> = type_params_named
                    .iter()
                    .map(|&(name, bound)| (name, bound.map(|b| self.ty_subst_named(b, map))))
                    .collect();
                // The return face substitutes before the re-intern guard:
                // `ty_subst_named` re-enters the pool in every compound arm
                // (`pool()`'s borrow), which a live `RefMut` here panics
                // against (the same discipline `subst_ty` keeps).
                let return_ty = self.ty_subst_named(return_ty, map);
                let mut pool = self.boundary.args.borrow_mut();
                if names.is_empty() {
                    pool.function(type_params, return_ty, &positional, &named, nullable)
                } else {
                    pool.function_generic(&names, return_ty, &positional, &named, nullable)
                }
            }
            Ty::Record { shape, nullable } => {
                let (positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.record_shape(shape);
                    (shape.positional.to_vec(), shape.named.to_vec())
                };
                let positional: Vec<Ty> = positional
                    .iter()
                    .map(|ty| self.ty_subst_named(*ty, map))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(name, ty)| (*name, self.ty_subst_named(*ty, map)))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, nullable)
            }
            other => other,
        }
    }

    /// `ty` with the `?` flag set when `nullable` holds (the join rule a
    /// substituted `T?` leaf owes — `T := int` under `T?` reads `int?`).
    fn join_nullable_ty(ty: Ty, nullable: bool) -> Ty {
        if !nullable {
            return ty;
        }
        match ty {
            Ty::Named { name, args, .. } => Ty::Named {
                name,
                nullable: true,
                args,
            },
            Ty::Record { shape, .. } => Ty::Record {
                shape,
                nullable: true,
            },
            Ty::Function { shape, .. } => Ty::Function {
                shape,
                nullable: true,
            },
            other => other,
        }
    }

    /// Whether `ty` spells any of `params` in a leaf position — a bare
    /// `Named` head or nested inside an argument list, a record field, or
    /// a function shape's return/formal/bound faces. The own-parameter
    /// gate [`Self::function_extension_face`]'s `on` walk runs: a shape
    /// that mentions none takes the plain subtype ask, one that does owes
    /// the binding pass.
    fn ty_mentions_params(&self, ty: Ty, params: &[StringId]) -> bool {
        // The pool's canonical shapes read under `&self` — the
        // `boundary.pool()` snapshots below drop their borrow before the
        // recursion re-enters them.
        match ty {
            Ty::Named { name, args, .. } => {
                if params.contains(&name) {
                    return true;
                }
                if args == ArgListId::EMPTY {
                    return false;
                }
                let list = {
                    let pool = self.boundary.pool();
                    pool.get(args).to_vec()
                };
                list.iter().any(|arg| self.ty_mentions_params(*arg, params))
            }
            Ty::Record { shape, .. } => {
                let (positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.record_shape(shape);
                    (shape.positional.to_vec(), shape.named.to_vec())
                };
                positional
                    .iter()
                    .chain(named.iter().map(|(_, ty)| ty))
                    .any(|ty| self.ty_mentions_params(*ty, params))
            }
            Ty::Function { shape, .. } => {
                let (return_ty, positional, named, bounds) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                        shape
                            .type_params_named
                            .iter()
                            .filter_map(|(_, bound)| *bound)
                            .collect::<Vec<_>>(),
                    )
                };
                std::iter::once(return_ty)
                    .chain(positional.iter().copied())
                    .chain(named.iter().map(|(_, ty)| *ty))
                    .chain(bounds.iter().copied())
                    .any(|ty| self.ty_mentions_params(ty, params))
            }
            _ => false,
        }
    }

    /// The record-receiver extension application (`extension<BoxedT> on
    /// (BoxedT,)?` — riverpod's `unwrapSentinel`): a receiver whose static
    /// type is a RECORD has no container name, so the named-`on` face loop
    /// of [`Self::extension_application`] never sees it — this face answers
    /// the same `applicableTo` question over shapes: both sides fold
    /// through type aliases to records, the nullability and field
    /// skeletons must agree (positional arity + named-field names), and an
    /// `on` field spelling one of the face's own type parameters
    /// instantiates (the inference always binds, exactly like the named
    /// arm's parameter arguments); a concrete field pair decides by head
    /// equality. Returns the single applicable face (the credit's element
    /// key), `None` when zero or several apply — an ambiguous record
    /// application resolves no member, the same posture the named loop
    /// keeps.
    pub(crate) fn record_extension_face(
        &mut self,
        receiver_ty: Ty,
        member: StringId,
    ) -> Option<(&'a ExtensionFace, FileId)> {
        let receiver = self.fold_alias_ty(receiver_ty);
        if let Ty::Function { .. } = receiver {
            return self.function_extension_face(receiver, member);
        }
        let Ty::Record {
            shape: recv_shape,
            nullable: recv_nullable,
        } = receiver
        else {
            return None;
        };
        let recv_shape = self.boundary.pool().record_shape(recv_shape).clone();
        self.accessible_extension_faces();
        let faces = self.accessible_extensions.as_deref().unwrap_or(&[]);
        let mut hits: Vec<(&'a ExtensionFace, FileId)> = Vec::new();
        for &(face, face_file) in faces {
            if !face.members.contains_key(&member) {
                continue;
            }
            let on_ty = self.boundary.ty(face_file, face.on_ty);
            // A bare own-parameter `on` head (`extension E<T> on T`) binds
            // `T := receiver` — the record receiver applies iff its
            // non-null core satisfies the parameter's declared bound (an
            // absent or `dynamic` bound is the top), the same
            // `applicableTo` ask the function/named arms answer.
            if let Ty::Named {
                name: on_head,
                args: ArgListId::EMPTY,
                ..
            } = on_ty
                && let Some(index) = face.type_param_names.iter().position(|&n| n == on_head)
            {
                let applies = match face.type_param_bounds.get(index).copied() {
                    None | Some(Ty::Dynamic) | Some(Ty::Unknown) => true,
                    Some(bound) => {
                        let bound =
                            Self::non_null_form(self.boundary.ty(face_file, bound));
                        self.resolved_subtype(Self::non_null_form(receiver), bound)
                    }
                };
                if applies {
                    hits.push((face, face_file));
                }
                continue;
            }
            let Ty::Record {
                shape: on_shape,
                nullable: on_nullable,
            } = self.fold_alias_ty(on_ty)
            else {
                continue;
            };
            if !on_nullable && recv_nullable {
                // `applicableTo`'s subtype ask: a non-nullable `on` record
                // never accepts a nullable receiver; the nullable `on`
                // accepts both (a non-null value fits the nullable type).
                continue;
            }
            let on_shape = self.boundary.pool().record_shape(on_shape).clone();
            if on_shape.positional.len() != recv_shape.positional.len()
                || on_shape.named.len() != recv_shape.named.len()
                || on_shape
                    .named
                    .iter()
                    .zip(recv_shape.named.iter())
                    .any(|(on, recv)| on.0 != recv.0)
            {
                continue;
            }
            let fields_agree = on_shape
                .positional
                .iter()
                .chain(on_shape.named.iter().map(|(_, ty)| ty))
                .zip(
                    recv_shape
                        .positional
                        .iter()
                        .chain(recv_shape.named.iter().map(|(_, ty)| ty)),
                )
                .all(|(on_field, recv_field)| {
                    let Ty::Named { name, .. } = on_field else {
                        return false;
                    };
                    // An `on` field spelling the extension's own parameter
                    // instantiates against the receiver (the inference
                    // binds; the bound — unbounded here — accepts).
                    face.type_param_names.contains(name)
                        || matches!(
                            (on_field, recv_field),
                            (
                                Ty::Named {
                                    name: on_name,
                                    args: on_args,
                                    ..
                                },
                                Ty::Named {
                                    name: recv_name,
                                    ..
                                },
                            ) if on_args == &ArgListId::EMPTY && on_name == recv_name
                        )
                });
            if fields_agree {
                hits.push((face, face_file));
            }
        }
        (hits.len() == 1).then(|| hits[0])
    }

    /// Credits the record application's member use (the `usedElements`
    /// face — the same insert [`Self::extension_application`]'s unique-hit
    /// arm performs) plus the contributing import. A receiver whose alias
    /// folds to a record (or a function shape) reaches the `Unknown`
    /// receiver dispatch (no container name exists), so this is the only
    /// credit path its member sends ever see. A `?.` access already
    /// unwrapped the receiver — the shape comparison reads the non-null
    /// shape.
    pub(crate) fn credit_record_extension_use(
        &mut self,
        receiver_ty: Ty,
        member: StringId,
        member_offset: u32,
    ) {
        let receiver_ty = if self.null_aware_members.contains(&member_offset) {
            match receiver_ty {
                Ty::Record {
                    shape,
                    nullable: true,
                } => Ty::Record {
                    shape,
                    nullable: false,
                },
                Ty::Function {
                    shape,
                    nullable: true,
                } => Ty::Function {
                    shape,
                    nullable: false,
                },
                other => other,
            }
        } else {
            receiver_ty
        };
        if let Some((face, face_file)) = self.record_extension_face(receiver_ty, member) {
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
                    .insert((face_file, face_index, member));
            }
            self.credit_extension_import(face, face_file);
        }
    }

    /// The `accessibleExtensions` list `extension_application` probes
    /// (`LibraryFragmentScope.accessibleExtensions`, scope.dart:473-488):
    /// every extension face this library declares (all unit files) plus the
    /// named extension faces each import edge's target exports under its
    /// combinators — prefixed imports' faces included (a prefix scope still
    /// contributes its extensions to implicit application), deferred edges
    /// too (the prefix scope adds a deferred library's elements the same).
    /// An unnamed extension never reaches the import scope — export entries
    /// key on names — but an own-library unnamed face applies all the same.
    /// Built once per pass; the probe list is small and the member-miss
    /// path re-reads it per access.
    pub(crate) fn accessible_extension_faces(&mut self) -> &[(&'a ExtensionFace, FileId)] {
        if self.accessible_extensions.is_none() {
            let mut faces: Vec<(&'a ExtensionFace, FileId)> = Vec::new();
            // The oracle's list is a SET of elements
            // (`LibraryFragmentScope.accessibleExtensions`,
            // scope.dart:482-487): a face reachable through several routes
            // — the own unit files AND one or more import edges — enters
            // the application pool once. Pointer identity is element
            // identity here: each face lives once in its declaring file's
            // `extension_faces` pool.
            let mut seen: FxHashSet<*const ExtensionFace> = FxHashSet::default();
            let mut push = |faces: &mut Vec<(&'a ExtensionFace, FileId)>,
                            face: &'a ExtensionFace,
                            file: FileId| {
                if seen.insert(face as *const ExtensionFace) {
                    faces.push((face, file));
                }
            };
            // The diagnosed unit always contributes its own faces first —
            // `unit_files` is the graph's registration face, which a caller
            // that never folds a part tree (the test rigs' `declare`-only
            // shape) leaves empty; the file under diagnosis is a unit of
            // `self.lib` by construction.
            if let Some(elements) = self.index.get(self.file) {
                for face in elements.extension_faces() {
                    push(&mut faces, face, self.file);
                }
            }
            for &unit in self.graph.unit_files(self.lib) {
                if unit == self.file {
                    continue;
                }
                if let Some(elements) = self.index.get(unit) {
                    for face in elements.extension_faces() {
                        push(&mut faces, face, unit);
                    }
                }
            }
            for edge in self.graph.import_edges(self.lib) {
                let Some(target) = edge.target else { continue };
                // The pool's face order reaches the ambiguity message's
                // candidate enumeration (`emit_extension_ambiguity` lists
                // the members in pool order), so an import target's
                // export-scope iteration must not leak container order
                // into the output. The sort key is the name's BYTES, not
                // its intern id: ids are allocation-ordered and a parallel
                // intern race can drift them across runs, while the byte
                // order is content-addressed and run-stable (the pool is a
                // set — order carries no resolution semantics).
                let strings = self.strings;
                let mut scope: Vec<_> = self.graph.export_scope(target).collect();
                scope.sort_by_key(|&(name, _)| strings.resolve(name));
                for (name, entry) in scope {
                    if !edge.combinators.allows(name) {
                        continue;
                    }
                    let Some(elements) = self.index.get(entry.file) else {
                        continue;
                    };
                    for face in elements
                        .extension_faces()
                        .iter()
                        .filter(|face| face.name == Some(name))
                    {
                        push(&mut faces, face, entry.file);
                    }
                }
            }
            self.accessible_extensions = Some(faces);
        }
        self.accessible_extensions.as_deref().unwrap_or(&[])
    }

    /// The `notifyExtensionUsed` credit of one applied face: a face
    /// declared in another library marks the spelling under which it
    /// exports — the import-site `unused_import` face spares every import
    /// that provides the name (the oracle credits the element to each
    /// import whose export scope carries it; the name-level model credits
    /// the exported spelling). An own-library face contributes nothing —
    /// its name resolved locally, never through an import.
    pub(crate) fn credit_extension_import(&mut self, face: &ExtensionFace, face_file: FileId) {
        if face_file == self.file || self.graph.library_of_file(face_file) == Some(self.lib) {
            return;
        }
        if let Some(name) = face.name {
            self.import_used.insert(name);
        }
    }

    /// The display pair of one applicable face: the element shape
    /// (`extension E on C`, the `ExtensionElement` toString — the official
    /// two-variant argument, `ambiguous_extension_member_access_test.dart:54`)
    /// and the resolver's inline shape (`extension 'E'` / `unnamed extension
    /// on 'C'`, `extension_member_resolver.dart:119-124`).
    pub(crate) fn extension_display(&self, face: &ExtensionFace) -> (String, String) {
        let name = face.name.map(|n| self.strings.resolve(n).to_string());
        let on_type = self
            .strings
            .resolve(face.on_type.expect("an applicable face has an on type"))
            .to_string();
        let element = match &name {
            Some(name) => format!("extension {name} on {on_type}"),
            None => format!("extension on {on_type}"),
        };
        let inline = match name {
            Some(name) => format!("extension '{name}'"),
            None => format!("unnamed extension on '{on_type}'"),
        };
        (element, inline)
    }

    /// Emits the `ambiguous_extension_member_access` verdict for the
    /// applicable candidates: the two-variant message for exactly two, the
    /// joined-list variant for more (`diagnostic.g.dart:338-348` for
    /// three-or-more, :363-374 for two), anchored at the access. The list
    /// renders per `commaSeparatedWithAnd`
    /// (`utilities/extensions/string.dart:41-104`): `a and b`, then
    /// `a, b, and c` from three on.
    pub(crate) fn emit_extension_ambiguity(
        &mut self,
        member: StringId,
        range: SourceRange,
        candidates: Vec<(String, String)>,
    ) {
        let name_text = self.strings.resolve(member).to_string();
        let (code, a1, a2) = if let [first, second] = candidates.as_slice() {
            (
                SemanticCode::AmbiguousExtensionMemberTwo,
                first.0.clone(),
                second.0.clone(),
            )
        } else {
            let inlines: Vec<String> = candidates.into_iter().map(|(_, inline)| inline).collect();
            let list = match inlines.len() {
                0 | 1 => inlines.into_iter().next().unwrap_or_default(),
                2 => {
                    let mut it = inlines.into_iter();
                    format!(
                        "{} and {}",
                        it.next().unwrap_or_default(),
                        it.next().unwrap_or_default()
                    )
                }
                _ => {
                    let last = inlines.last().expect("non-empty above").clone();
                    let head = &inlines[..inlines.len() - 1];
                    format!("{}, and {}", head.join(", "), last)
                }
            };
            (
                SemanticCode::AmbiguousExtensionMemberThreeOrMore,
                list,
                String::new(),
            )
        };
        self.emit3(code, range, &name_text, &a1, &a2);
    }

    /// Whether the plain member walk finds `member` on `container`.
    pub(crate) fn member_found(
        &self,
        container: StringId,
        member: StringId,
        super_walk: bool,
    ) -> bool {
        let walk = if super_walk {
            self.walk_super(container, member)
        } else {
            self.walk_members(container, member)
        };
        matches!(walk, Walk::Found(_))
    }

    pub(crate) fn container_kind_of(&self, container: StringId) -> Option<ContainerKind> {
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .map(|(c, _)| c.kind)
    }

    /// The anonymous-methods experiment's receiver faces, over the
    /// collector's captured invocations (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/fasta/ast_builder.dart:918-929` shape check,
    /// then `pkg/analyzer/lib/src/generated/resolver.dart:1977-1982` and
    /// `:2063-2079` receiver check): a written parameter list must be
    /// exactly one required positional parameter
    /// (`anonymous_method_wrong_parameter_list`, reported at the list's
    /// parens — the list DROPS on failure upstream, so the type check never
    /// co-fires), and that parameter's written type must accept the
    /// receiver's static type (`anonymous_method_wrong_parameter_type`,
    /// reported at the written annotation; null-aware punctuation promotes
    /// the receiver to non-null first). Undecidable receiver types
    /// suppress — the under-report direction.
    pub(crate) fn finalize_anonymous_methods(&mut self) {
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        let entries = elements.anon_methods();
        if entries.is_empty() {
            return;
        }
        let mut rows = Vec::new();
        for entry in entries {
            let Some((open, close)) = entry.parens else {
                continue;
            };
            if entry.param_count != 1 || entry.first_named || entry.first_optional {
                let code = SemanticCode::AnonymousMethodWrongParameterList;
                rows.push(Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    code.message("", ""),
                    self.file,
                    SourceRange {
                        offset: open.offset,
                        length: close.end().saturating_sub(open.offset),
                    },
                    code.correction_message(""),
                ));
                continue;
            }
            let Some((param_ty, anchor)) = entry.param_ty else {
                continue;
            };
            let Some(recv_ty) = self.anon_receiver_ty(entry) else {
                continue;
            };
            let recv_ty = if entry.null_aware {
                Self::non_null_form(recv_ty)
            } else {
                recv_ty
            };
            if self.assignable(recv_ty, param_ty, false) == Some(false) {
                let code = SemanticCode::AnonymousMethodWrongParameterType;
                let message = code.message(&self.render_ty(recv_ty), &self.render_ty(param_ty));
                rows.push(Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    message,
                    self.file,
                    anchor,
                    code.correction_message(""),
                ));
            }
        }
        if rows.is_empty() {
            return;
        }
        // The capture seals innermost-first (nested members close inside
        // out); the merge wants source order.
        rows.sort_by_key(|d| d.range.offset);
        self.report.diagnostics =
            merge_source_ordered(std::mem::take(&mut self.report.diagnostics), rows);
    }

    /// The receiver's static type at one captured invocation: the last
    /// token before the punctuation carries the expression's type. Literal
    /// receivers answer from the primitive face (the corpus's `1.()` /
    /// `"".()` shapes); everything else re-derives through the shared
    /// receiver walk (the flow read face, then the member-read chain — an
    /// identifier or `a.b` receiver). A `)`-ended compound receiver finds
    /// no read record and suppresses.
    fn anon_receiver_ty(&self, entry: &crate::model::AnonMethodInvocation) -> Option<Ty> {
        let tokens = self.tokens.as_slice();
        let at = tokens
            .partition_point(|t| t.utf16_offset < entry.punctuation.offset)
            .checked_sub(1)?;
        let token = tokens.get(at)?;
        let end = dartq_source::byte_offset_to_utf16(
            self.source,
            (token.byte_offset + token.byte_len) as usize,
        );
        let span = SourceRange {
            offset: token.utf16_offset,
            length: end.saturating_sub(token.utf16_offset),
        };
        match token.kind {
            dartq_scanner::TokenKind::Int => Some(Ty::named(self.members.primitive_ids[0])),
            dartq_scanner::TokenKind::Double => Some(Ty::named(self.members.primitive_ids[1])),
            dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::True)
            | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::False) => {
                Some(Ty::named(self.members.primitive_ids[3]))
            }
            dartq_scanner::TokenKind::String => Some(Ty::named(self.members.primitive_ids[4])),
            dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Null) => Some(Ty::Null),
            _ => self.recv_expression_ty(span),
        }
    }
}

/// Pins for the `UndefinedGuard` face scoping (#2186 S1): the observed
/// oracle (analyzer 8.1.1, the 3.13.1 SDK of record) consults
/// `shouldIgnoreUndefined` only on the unqualified/prefixed *name* faces —
/// `undefined_function` (`_reportUndefinedFunction`,
/// method_invocation_resolver.dart:403) and `undefined_identifier`
/// (`shouldIgnoreUndefinedIdentifier`, simple_identifier_resolver.dart:222)
/// plus the named-type family (`shouldIgnoreUndefinedNamedType`,
/// named_type_resolver.dart:326) and `undefined_prefixed_name`
/// (property_element_resolver.dart:891). The member faces — the
/// `undefined_method`/`_getter`/`_setter` trio — never consult it
/// (`_reportUndefinedMethodOrNew` is guard-free, same file :424-431), and
/// `undefined_identifier_await` fires before the guard in its else-if
/// chain (simple_identifier_resolver.dart:217-221).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use crate::diagnose::test_rig::report_ops;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_resolution::{CombinatorClause, Combinators, LibraryGraphBuilder};
    use dartq_scanner::ScanConfig;
    use dartq_source::FileTable;
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library carrying one
    /// broken import edge (`target: None`) built by `make_edge`.
    fn report_with_edge(
        source: &str,
        make_edge: impl FnOnce(&StringTable) -> ImportEdge,
    ) -> FileReport {
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
        builder.import(lib, make_edge(&strings));
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

    /// A broken import showing `name` (the S1 trigger shape).
    fn broken_show(name: &'static str) -> impl FnOnce(&StringTable) -> ImportEdge {
        move |strings| ImportEdge {
            target: None,
            prefix: None,
            deferred: false,
            combinators: Combinators::new(
                [CombinatorClause::Show(
                    vec![strings.intern(name)].into_boxed_slice(),
                )],
                strings,
            ),
        }
    }

    /// An unfiltered broken import (the guard's no-show arm never fires on
    /// bare names — only the prefixed arm can).
    fn broken_bare() -> impl FnOnce(&StringTable) -> ImportEdge {
        |_| ImportEdge {
            target: None,
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        }
    }

    /// Diagnoses `source` as the sole file of a library importing one SDK
    /// face library: `sdk_lib` is the import URI (`dart:async`) and
    /// `sdk_source`'s declarations are owned by a registered-but-empty unit
    /// — the namespace resolves the names to a declaration whose file
    /// carries no container model, the exact posture of the real `dart:`
    /// faces (the frozen member tables own their members, and the
    /// supertype hops into them classify dead).
    fn report_sdk_face(source: &str, sdk_lib: &str, sdk_source: &str) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let sdk_file = files.intern(Path::new("sdk_face.dart"));
        let scan_cfg = ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        };
        let scanned = dartq_scanner::scan(source, scan_cfg, file);
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let sdk_scanned = dartq_scanner::scan(sdk_source, scan_cfg, sdk_file);
        let sdk_elements = collect(sdk_source, &sdk_scanned.tokens, features, &strings);
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
        let lib_sdk = builder.intern(sdk_lib);
        builder.unit(lib_sdk, sdk_file);
        for (name, decl) in sdk_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_sdk, name, sdk_file);
        }
        // The SDK unit registers but carries no container model: the
        // names resolve, the member faces answer from the frozen tables.
        let empty_scanned = dartq_scanner::scan("", scan_cfg, sdk_file);
        index.insert(
            sdk_file,
            collect("", &empty_scanned.tokens, features, &strings),
        );
        builder.import(
            lib,
            ImportEdge {
                target: Some(lib_sdk),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
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

    /// S1 fixture (a): a `show`-listed name of a broken import invoked as a
    /// constructor inside a class body is `undefined_method` — the oracle's
    /// `_reportUndefinedMethodOrNew` consults no suppression, so the guard
    /// must not silence it (the ~430 oracle-only rows' root cause).
    #[test]
    fn shown_name_invoked_in_class_reports_undefined_method() {
        let report = report_with_edge("class C { void m() { Foo(); } }", broken_show("Foo"));
        assert_eq!(code_names(&report), ["undefined_method"]);
        let row = &report.diagnostics[0];
        assert_eq!(
            &"class C { void m() { Foo(); } }"[row.range.offset as usize..row.range.end() as usize],
            "Foo"
        );
    }

    /// S1 fixture (b): the same `show`-listed name invoked at top level
    /// stays silent — `undefined_function` is a guarded face
    /// (`_reportUndefinedFunction` consults `shouldIgnoreUndefined`). The
    /// miss still lands in the names table.
    #[test]
    fn shown_name_invoked_at_top_level_stays_suppressed() {
        let report = report_with_edge("void f() { Foo(); }", broken_show("Foo"));
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Invoke);
    }

    /// The guarded read face keeps its suppression: a `show`-listed name
    /// read as a bare identifier stays silent (`undefined_identifier`
    /// consults `shouldIgnoreUndefinedIdentifier`).
    #[test]
    fn shown_name_bare_read_stays_suppressed() {
        let report = report_with_edge("void f() { gone; }", broken_show("gone"));
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Read);
    }

    /// S1 fixture (c): an unfiltered broken import never suppresses a bare
    /// miss — `undefined_method` reports for the in-class invocation, the
    /// same verdict the show-listed shape now reaches.
    #[test]
    fn unfiltered_broken_import_invoke_in_class_reports() {
        let report = report_with_edge("class C { void m() { Foo(); } }", broken_bare());
        assert_eq!(code_names(&report), ["undefined_method"]);
    }

    /// S1 fixture (d): a `show`-listed name read in a const context (the
    /// `const Foo.bar()`-class prefixed-ctor head shape) reports
    /// `undefined_identifier` through the guarded read face — the guard
    /// still applies, so this stays silent.
    #[test]
    fn shown_name_prefixed_ctor_head_stays_suppressed() {
        let report = report_with_edge("const x = Foo.bar();", broken_show("Foo"));
        assert!(
            !code_names(&report).contains(&"undefined_method"),
            "{:?}",
            code_names(&report)
        );
        // The `Foo` head of `Foo.bar` resolves through the guarded
        // identifier face: show-listed on a broken import stays silent.
        assert!(
            !code_names(&report).contains(&"undefined_identifier"),
            "{:?}",
            code_names(&report)
        );
    }

    /// The `.new` selector on an unresolved receiver: the member family on
    /// `InvalidType` is guard-free like every member face — a `show`-listed
    /// receiver name suppresses its own `undefined_identifier`, but the
    /// `.new` member miss still reports.
    #[test]
    fn new_selector_on_shown_broken_receiver_reports() {
        let report = report_with_edge("void f() { gone.new(); }", broken_show("gone"));
        assert_eq!(code_names(&report), ["undefined_method"]);
    }

    /// Diagnoses `source` as a single-file library with a clean unfiltered
    /// import edge to a second unit `a.dart` (the two-unit counterpart of
    /// [`report_with_edge`] — the drift repro's cross-file member faces).
    fn report_two(source_a: &str, source: &str) -> FileReport {
        report_units(&[("a.dart", source_a)], "main.dart", source, false)
    }

    /// [`report_two`] with the import edge `show`-filtered to one name: the
    /// unit's other declarations stay out of the diagnosing library's
    /// namespace while their types still flow through member signatures
    /// (the xml corpus's `XmlNodeList` receiver shape).
    fn report_show(source_a: &str, shown: &str, source: &str) -> FileReport {
        report_units_edge(
            &[("a.dart", source_a)],
            "main.dart",
            source,
            false,
            Some(shown),
        )
    }

    /// Diagnoses `source` as the defining unit of a library whose
    /// declarations live partly in `part` files (the drift `part of`
    /// receiver shape): `parts` is `(path, source)` pairs joined to the
    /// main unit's library through `part of`.
    fn report_with_parts(parts: &[(&str, &str)], source: &str) -> FileReport {
        report_units(parts, "main.dart", source, true)
    }

    /// The shared two-shape rig behind [`report_two`] and
    /// [`report_with_parts`]: `units` are foreign units (imported) when
    /// `as_parts` is false, or same-library parts when true — the part
    /// files declare into the main library's namespace either way, so the
    /// member walk reaches them identically (upstream `part of` units
    /// share the enclosing library's scope).
    fn report_units(
        units: &[(&str, &str)],
        main_path: &str,
        source: &str,
        as_parts: bool,
    ) -> FileReport {
        report_units_edge(units, main_path, source, as_parts, None)
    }

    /// The rig core behind [`report_units`]: `show` filters every import
    /// edge to one visible name (the xml corpus's `XmlNodeList` receiver
    /// shape — the unit's other declarations stay out of the diagnosing
    /// library's namespace while their types still flow through member
    /// signatures).
    fn report_units_edge(
        units: &[(&str, &str)],
        main_path: &str,
        source: &str,
        as_parts: bool,
        show: Option<&str>,
    ) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_m = files.intern(Path::new(main_path));
        let unit_files: Vec<FileId> = units
            .iter()
            .map(|(path, _)| files.intern(Path::new(path)))
            .collect();
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let scan_cfg = ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        };
        let scanned_m = dartq_scanner::scan(source, scan_cfg, file_m);
        assert!(
            scanned_m.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let scanned_units: Vec<_> = units
            .iter()
            .zip(&unit_files)
            .map(|((_, unit_source), unit_file)| {
                let scanned = dartq_scanner::scan(unit_source, scan_cfg, *unit_file);
                assert!(
                    scanned.diagnostics.is_empty(),
                    "scanner diagnostics: {unit_source:?}"
                );
                scanned
            })
            .collect();
        let elements_m = collect(source, &scanned_m.tokens, features, &strings);
        let elements_units: Vec<_> = units
            .iter()
            .zip(&scanned_units)
            .map(|((_, unit_source), scanned)| {
                collect(unit_source, &scanned.tokens, features, &strings)
            })
            .collect();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_m = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file_m, elements_m.clone());
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        for (i, elements_u) in elements_units.iter().enumerate() {
            let unit_file = unit_files[i];
            index.insert(unit_file, elements_u.clone());
            if as_parts {
                // A part file folds into the main library's own namespace
                // (upstream `part of` shares the enclosing library scope).
                builder.unit(lib_m, unit_file);
                for (name, decl) in elements_u.declarations() {
                    if decl.kind == MicroKind::TypeParam {
                        continue;
                    }
                    builder.declare(lib_m, name, unit_file);
                }
            } else {
                let lib_u = builder.intern(&format!("package:t/unit{i}.dart"));
                builder.unit(lib_u, unit_file);
                for (name, decl) in elements_u.declarations() {
                    if decl.kind == MicroKind::TypeParam {
                        continue;
                    }
                    builder.declare(lib_u, name, unit_file);
                }
                let combinators = show.map(|shown| {
                    Combinators::new(
                        [CombinatorClause::Show(
                            vec![strings.intern(shown)].into_boxed_slice(),
                        )],
                        &strings,
                    )
                });
                builder.import(
                    lib_m,
                    ImportEdge {
                        target: Some(lib_u),
                        prefix: None,
                        deferred: false,
                        combinators: combinators.unwrap_or_else(Combinators::unfiltered),
                    },
                );
            }
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut fold = fold_unit(
            source,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        fold.report.diagnostics =
            merge_source_ordered(fold.parse_diagnostics, fold.report.diagnostics);
        fold.report
    }

    /// #2190 shape 1 — the drift3_preview `abstract interface class
    /// DriftSession` face: its abstract getter and method members are own
    /// members of a same-library declaration, resolved through a declared
    /// parameter type. The oracle's interface walk resolves both silently.
    #[test]
    fn drift_wrapper_interface_members_resolve() {
        let report = report_two(
            "abstract interface class DriftSession {\n\
             \x20 Future<DriftSession> exec(int i);\n\
             \x20 int? get transactionParent;\n\
             }\n\
             class Future<T> {}",
            "void f(DriftSession s) { s.transactionParent; s.exec(1); }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// #2190 shape 2 — the `_CollectByDialect` record-type field whose
    /// annotation runs onto the next line (`final List<({…})>\n
    /// collectedStatements = [];`): the field must land in the member map
    /// under its written name regardless of the annotation split.
    #[test]
    fn drift_wrapper_record_field_multiline_resolves() {
        let report = report_two(
            "class B {}\n\
             final class C extends B {\n\
             \x20 final List<({int x, String y})>\n\
             \x20 collectedStatements = [];\n\
             }\n\
             class List<T> {}",
            "void f(C c) { c.collectedStatements; }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// #2190 shape 3 — the `part of` plain-class field
    /// (`DatabaseConnection.executor`): a trivial field declared in a part
    /// file of the diagnosing library resolves through the member walk.
    /// Its constructor initializer carries a `switch` expression (the
    /// corpus shape) — included to keep the population honest.
    #[test]
    fn drift_wrapper_part_file_field_resolves() {
        let report = report_with_parts(
            &[(
                "conn.dart",
                "part of 'main.dart';\n\
                 class QueryExecutor {}\n\
                 class StreamQueryStore {}\n\
                 class DatabaseConnection implements QueryExecutor {\n\
                 \x20 final QueryExecutor executor;\n\
                 \x20 final StreamQueryStore streamQueries;\n\
                 \x20 DatabaseConnection(this.executor, {StreamQueryStore? streamQueries})\n\
                 \x20   : streamQueries = streamQueries ?? switch (executor) {\n\
                 \x20       DatabaseConnection() => executor.streamQueries,\n\
                 \x20       _ => StreamQueryStore(),\n\
                 \x20     };\n\
                 }",
            )],
            "void f(DatabaseConnection db) { db.executor; }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// #2190 discriminator — every corpus receiver is bound from
    /// `await expr` (`final s = await currentSession()`). The var-inferred
    /// declarator binds the initializer's expression type; the oracle's
    /// `await` flattens `Future<T>` to `T` before the member lookup runs
    /// (upstream `typeSystem.flatten`, type_system.dart:425-471 —
    /// `Future<S>`/`FutureOr<S>` → `S`, nullability preserved).
    ///
    /// Measured on HEAD: dartq emits `undefined_getter`/`undefined_method`
    /// **naming `Future`** — the receiver binds `Future<DriftSession>`
    /// because `end_await_expression` never folds the operand row's `ty`
    /// and `initializer_static_ty`/`bind_var_type` record the callee's raw
    /// return type. This is the await-fold loss (evidence-2186 §S4), NOT a
    /// member-face miss — all three member shapes verify clean below. The
    /// fix site is the inference/flow side (`listener.rs
    /// end_await_expression` folding `rows.last().ty`, or
    /// `engine_core.rs::initializer_static_ty` unwrapping `Future<T>` when
    /// the row sits under an await span) — outside this lane's file scope.
    /// `#[ignore]`d so the oracle-truth pin rides in the tree and flips to
    /// live the moment the flatten lands.
    #[test]
    fn drift_wrapper_await_receiver_discriminator() {
        let report = report_two(
            "abstract interface class DriftSession {\n\
             \x20 int? get transactionParent;\n\
             \x20 Future<int> exec(int i);\n\
             }\n\
             class Future<T> {}",
            "Future<DriftSession> g() => g();\n\
             void f() async {\n\
             \x20 final s = await g();\n\
             \x20 s.transactionParent;\n\
             \x20 s.exec(1);\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// #2190 control — a genuinely missing member on the same wrapper face
    /// still reports `undefined_getter` (the under-report bias guard: the
    /// fix must not silence true misses).
    #[test]
    fn drift_wrapper_missing_member_still_reports() {
        let report = report_two(
            "abstract interface class DriftSession {\n\
             \x20 int? get transactionParent;\n\
             }",
            "void f(DriftSession s) { s.onMissing; }",
        );
        assert_eq!(code_names(&report), ["undefined_getter"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The getter 'onMissing' isn't defined for the type 'DriftSession'."
        );
    }

    /// The receiver anchor the diagnosing scope cannot name (the unit is
    /// `show`-filtered to the wrapper, so `XmlNodeList` itself is not in
    /// the namespace — the xml corpus's `w.list.whereType()` shape) still
    /// climbs: exactly one graph library declares the type, so the frozen
    /// verdict re-anchors there, the dead `extends` hop (`Missing` is
    /// undeclared anywhere) contributes nothing, and the miss reports
    /// naming the resolved class. Without the re-anchor the climb answers
    /// `Unknown` and stays silent while the oracle reports.
    ///
    /// Verified 1:1 against the oracle on the real engine
    /// (/tmp/probe2293/showprobe — `import 'a.dart' show Wrapper` +
    /// `w.list.whereType()` → both sides `undefined_method` naming
    /// `XmlNodeList`). `#[ignore]`d here: the two-unit rig resolves the
    /// foreign getter's return type one grade short of the CLI's graph
    /// wiring, so the receiver anchor never reaches the re-anchor arm —
    /// the same rig gap the await-fold discriminator documents. Flips live
    /// when the rig wires the foreign-unit boundary pool the CLI builds.
    #[test]
    #[ignore = "rig gap: foreign-unit receiver ty projection (see doc)"]
    fn unimported_declared_receiver_member_reports() {
        let report = report_show(
            "class XmlNodeList extends Missing {}\n\
             class Wrapper {\n\
             \x20 XmlNodeList get list => XmlNodeList();\n\
             }",
            "Wrapper",
            "import 'a.dart' show Wrapper;\n\
             void f(Wrapper w) {\n\
             \x20 w.list.whereType();\n\
             }",
        );
        assert_eq!(code_names(&report), ["undefined_method"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'whereType' isn't defined for the type 'XmlNodeList'."
        );
    }

    /// An extension on a bare SDK type (`Stream<T>`) reads the frozen
    /// member table the element index cannot carry: `this` inside the body
    /// IS the `on` type, `map` is one of `Stream`'s members
    /// (`SDK_FACES_EXTRA`, upstream Dart SDK source, sdk/lib/async/
    /// stream.dart), so the implicit application resolves and stays
    /// silent.
    #[test]
    fn stream_extension_member_resolves_through_frozen_face() {
        let report = report_sdk_face(
            "import 'dart:async';\n\
             extension Tap<T> on Stream<T> {\n\
             \x20 Stream<T> tap(void Function(T) f) {\n\
             \x20   return this.map(f);\n\
             \x20 }\n\
             }",
            "dart:async",
            "class Stream<T> {}",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// The miss control: the same extension shape accessing a member
    /// `Stream` genuinely lacks reports `undefined_method` naming the
    /// `on` type — the frozen verdict's `Absent` answer, never the
    /// silent `Unknown`.
    #[test]
    fn stream_extension_member_miss_reports_on_type() {
        let report = report_sdk_face(
            "import 'dart:async';\n\
             extension Tap<T> on Stream<T> {\n\
             \x20 Stream<T> tap() {\n\
             \x20   return this.nope();\n\
             \x20 }\n\
             }",
            "dart:async",
            "class Stream<T> {}",
        );
        assert_eq!(code_names(&report), ["undefined_method"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'nope' isn't defined for the type 'Stream'."
        );
    }

    /// The dead-anchored sibling `if`'s promotion consumption (#2293
    /// rider): inside a region the flow marked dead (the always-true
    /// `if (s == null) return null` — `s` narrowed to `Null` by the
    /// earlier `s is Src` exit), the `if (file != null)` promotion still
    /// holds for the guarded read — the oracle analyzes dead code with
    /// the same flow state (probe: dead-region `g(z)` with `z` promoted
    /// reports no argument row) and the staged argument check consumes
    /// the promotion, never reporting `FS?` into the `FS` formal.
    ///
    /// Ignored pending a flow-side fix: the promotion fact for a
    /// dead-region read is suppressed by the flow layer's reachability
    /// gate (`dartq-flow` `interp_expr.rs` — the `PromotedRead`/`ReadType`
    /// emits both require `self.reachable()`), so the staged check finds
    /// no row and the parse-time verdict stands. The consumption side is
    /// proven correct on the live path (the same fixture's final
    /// `UR.f(file)` read stages `FS` and cleans). Upstream runs the same
    /// flow state in dead regions (dead-region promotions materialize;
    /// only the `dead_code` row marks the region), so the fix is dropping
    /// the reachability gate from those two fact emits — `dartq-flow` is
    /// #2288/#2292's lane; revive this pin when that lands.
    #[ignore = "flow gate: dead-region PromotedRead suppressed (dartq-flow interp_expr.rs reachable gate, #2288/#2292)"]
    #[test]
    fn dead_region_null_check_promotion_cleans_argument() {
        let report = report_units(
            &[],
            "/p.dart",
            "class FS {}\n\
             class UR { UR.f(FS f); UR.e(dynamic s); }\n\
             class Src {}\n\
             class SrcFactory { Src? forUri2(dynamic u) => null; }\n\
             class Map<K, V> { V? operator [](K key) => null; }\n\
             Map<dynamic, FS?> m1 = {};\n\
             Map<dynamic, FS?> m2 = {};\n\
             UR? f(SrcFactory fac, dynamic uri) {\n\
             \x20 var s = fac.forUri2(uri);\n\
             \x20 if (s is Src) {\n\
             \x20   return UR.e(s);\n\
             \x20 }\n\
             \x20 FS? file = m1[uri];\n\
             \x20 if (file == null) {\n\
             \x20   if (s == null) {\n\
             \x20     return null;\n\
             \x20   }\n\
             \x20   file = m2[uri];\n\
             \x20   if (file != null) {\n\
             \x20     return UR.f(file);\n\
             \x20   }\n\
             \x20 }\n\
             \x20 return UR.f(file);\n\
             }",
            false,
        );
        // The `dead_code` row matches the oracle (the always-true sibling
        // if's first dead statement); the argument row must clean.
        assert_eq!(
            code_names(&report),
            ["dead_code"],
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// The prefixed-head unresolved-reference code (#2289 rider): the
    /// head of a value-position `missing.name` selector is a plain bare
    /// miss — the oracle reports `undefined_identifier` at the head's own
    /// span beside the constant-eval row (`undefined_class` is the
    /// TYPE-position code and never fires here), and the row must survive
    /// to the report, not drop at the merge. The `+`-operand head
    /// (`missing + 1`) rides the same face (#2306): the operator's
    /// operand reports the identifier alone, the operator lookup itself
    /// never cascades over the invalid operand.
    #[test]
    fn prefixed_head_value_miss_reports_identifier() {
        let report = report_units(
            &[],
            "/p.dart",
            "class Object {}\n\
             class C {\n\
             \x20 const C(Object? a);\n\
             }\n\
             const a = C(missing);\n\
             const b = C(missing.name);\n\
             const c = C(missing + 1);\n\
             const d = C(!missing);\n\
             const e = C((missing));",
            false,
        );
        let mut heads: Vec<u32> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_identifier")
            .map(|d| d.range.offset)
            .collect();
        heads.sort_unstable();
        assert_eq!(heads, [62, 84, 111, 138, 161]);
    }

    /// #2306: the operator's undefined operand reports itself at the head
    /// — `missing + 1` and `missing[0]` carry the same
    /// `undefined_identifier` face a bare read carries (upstream Dart SDK
    /// source, `pkg/analyzer/lib/src/dart/resolver/simple_identifier_resolver.dart`,
    /// the operand's own `simpleIdentifier` visit), and the operator
    /// lookup never cascades a second row over the invalid operand (the
    /// operand's dynamic face swallows it — probed against the oracle,
    /// both shapes report the identifier alone). `final` heads keep the
    /// const-initializer face out of the pin's scope.
    #[test]
    fn operator_head_miss_reports_identifier_without_cascade() {
        let report = report_units(
            &[],
            "/p.dart",
            "class Object {}\n\
             final a = missing + 1;\n\
             final b = missing[0];",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["undefined_identifier", "undefined_identifier"],
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
        let mut heads: Vec<u32> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_identifier")
            .map(|d| d.range.offset)
            .collect();
        heads.sort_unstable();
        assert_eq!(heads, [26, 49]);
    }

    /// The guarded read face keeps its suppression in operator position: a
    /// `show`-listed name of a broken import as an operator operand stays
    /// silent — the operator head takes the Read arm's guard verbatim
    /// (`undefined_identifier` consults `shouldIgnoreUndefined`), and the
    /// miss still lands in the names table.
    #[test]
    fn shown_name_operator_head_stays_suppressed() {
        let report = report_with_edge("void f() { Missing + 1; }", broken_show("Missing"));
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Read);
    }

    /// #2190 M1a residue — the extends-walk face: `escapedNameFor` is
    /// declared on `Column<T>` (`dsl/columns.dart:53`), the direct
    /// superclass of `GeneratedColumn` (`column_impl.dart:9` —
    /// `GeneratedColumn<T extends Object> extends Column<T>`; `Column`
    /// itself extends `Expression<T>`). The member walk must climb the
    /// generic `extends` edge and find it: both the implicit-this call
    /// inside `GeneratedColumn` (`column_impl.dart:123,187` —
    /// `escapedNameFor(into.dialect)`) and the declared-parameter access
    /// (`migration.dart:532` — `column.escapedNameFor(context.dialect)`)
    /// resolve silently in the oracle.
    #[test]
    fn drift_wrapper_extends_walk_member_resolves() {
        let report = report_two(
            "class Object {}\n\
             class String {}\n\
             class int {}\n\
             abstract class Expression<D extends Object> {}\n\
             abstract class Column<T extends Object> extends Expression<T> {\n\
             \x20 String get name;\n\
             \x20 String escapedNameFor(int dialect) => name;\n\
             }\n\
             class GenerationContext { int get dialect => 0; }",
            "class GeneratedColumn<T extends Object> extends Column<T> {\n\
             \x20 @override\n\
             \x20 String get name => throw '';\n\
             \x20 void writeColumnDefinition(GenerationContext into) {\n\
             \x20   final escapedName = escapedNameFor(into.dialect);\n\
             \x20   into.dialect;\n\
             \x20   escapedName;\n\
             \x20 }\n\
             }\n\
             void rename(GeneratedColumn column, GenerationContext context) {\n\
             \x20 column.escapedNameFor(context.dialect);\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// #2190 M1a residue — the deeper generic hop: `GeneratedColumnWith
    /// TypeConverter<D, S extends Object> extends GeneratedColumn<S>`
    /// (`column_impl.dart:296-297`) reads `escapedNameFor` two generic
    /// instantiations up (`GeneratedColumn<S>` → `Column<S>`), and the
    /// `implements` face contributes members too (`Expression implements
    /// FunctionParameter` — `expression.dart:32`).
    #[test]
    fn drift_wrapper_deep_generic_and_interface_walk_resolves() {
        let report = report_two(
            "class Object {}\n\
             class String {}\n\
             class int {}\n\
             abstract class FunctionParameter { int get arity; }\n\
             abstract class Expression<D extends Object> implements FunctionParameter {}\n\
             abstract class Column<T extends Object> extends Expression<T> {\n\
             \x20 String get name;\n\
             \x20 String escapedNameFor(int dialect) => name;\n\
             }\n\
             class GenerationContext { int get dialect => 0; }",
            "abstract class GeneratedColumn<T extends Object> extends Column<T> {}\n\
             class GeneratedColumnWithTypeConverter<D, S extends Object>\n\
             \x20   extends GeneratedColumn<S> {\n\
             \x20 @override\n\
             \x20 String get name => throw '';\n\
             \x20 @override\n\
             \x20 int get arity => 0;\n\
             \x20 String write(GenerationContext into) => escapedNameFor(into.dialect);\n\
             }\n\
             void f(GeneratedColumnWithTypeConverter<int, String> c, Expression<String> e) {\n\
             \x20 c.escapedNameFor(0);\n\
             \x20 c.arity;\n\
             \x20 e.arity;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// #2190 M1a residue — the `Object`-member face: `inner.hashCode` where
    /// `inner` is a declared `Expression` (the corpus's
    /// `expression.dart:484/521/607` overrides read `hashCode` through the
    /// `Object` interface — upstream `objectMembers` merge, never an
    /// `undefined_getter`).
    #[test]
    fn drift_wrapper_object_member_on_expression_resolves() {
        let report = report_two(
            "class Object {}\n\
             class int {}\n\
             abstract class Expression<D extends Object> {}",
            "class Eq extends Expression<Object> {\n\
             \x20 final Expression<Object> inner;\n\
             \x20 Eq(this.inner);\n\
             \x20 @override\n\
             \x20 int get hashCode => inner.hashCode;\n\
             }\n\
             void f(Expression<Object> e) { e.hashCode; }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// #2190 M1a residue — the ctor-reference face: `QueryRow(row, db)` is
    /// a bare constructor call whose head resolves as the class
    /// (`custom_select.dart:55` — `rows.map((row) => QueryRow(row, _db))`).
    /// The wrapper receivers around it are user classes reached through the
    /// library namespace; the bare-ctor classification must not leak an
    /// `undefined_*` row naming the wrapper type.
    #[test]
    fn drift_wrapper_ctor_reference_resolves() {
        let report = report_two(
            "class Object {}\n\
             class QueryRow {\n\
             \x20 final Object data;\n\
             \x20 final Object db;\n\
             \x20 QueryRow(this.data, this.db);\n\
             }\n\
             class DatabaseConnection {}\n\
             class List<T> {\n\
             \x20 Iterable<R> map<R>(R Function(T) f) => throw '';\n\
             }\n\
             class Iterable<T> {\n\
             \x20 List<T> toList() => throw '';\n\
             }",
            "void f(List<Object> rows, Object db) {\n\
             \x20 rows.map((row) => QueryRow(row, db)).toList();\n\
             \x20 final conn = DatabaseConnection();\n\
             \x20 conn;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    // —— #2225 S2 residue pins ——————————————————————————————————————————

    /// [`report_two`] variant whose foreign unit declares `name` into the
    /// library graph but is withheld from the element index — the
    /// synthetic-SDK-face shape (`dart:core`/`dart:async`/`dart:io`
    /// declarations resolve through the namespace but carry no indexed
    /// members), which chain-breaks `walk_members` where an undeclared or
    /// indexed name would not. The unit's source declares nothing else —
    /// the frozen member tables answer for the face.
    fn report_unindexed_unit(source_a: &str, source: &str) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_m = files.intern(Path::new("main.dart"));
        let file_a = files.intern(Path::new("a.dart"));
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let scan_cfg = ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        };
        let scanned_m = dartq_scanner::scan(source, scan_cfg, file_m);
        assert!(
            scanned_m.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let scanned_a = dartq_scanner::scan(source_a, scan_cfg, file_a);
        assert!(
            scanned_a.diagnostics.is_empty(),
            "scanner diagnostics: {source_a:?}"
        );
        let elements_m = collect(source, &scanned_m.tokens, features, &strings);
        let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_m = builder.intern("package:t/main.dart");
        let lib_a = builder.intern("package:t/a.dart");
        builder.unit(lib_a, file_a);
        let mut index = ElementIndex::default();
        index.insert(file_m, elements_m.clone());
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        for (name, decl) in elements_a.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_a, name, file_a);
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_a),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        // `file_a` is deliberately absent from `index` — its declarations
        // resolve through the namespace but model no members.
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut fold = fold_unit(
            source,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        fold.report.diagnostics =
            merge_source_ordered(fold.parse_diagnostics, fold.report.diagnostics);
        fold.report
    }

    /// `(x as dynamic)` keeps the operand's own static type — `dynamic` —
    /// not the boolish latch's `bool` (the `AsExpression.staticType` face):
    /// an `int` return position accepts a `dynamic` expression silently,
    /// where a `bool`-collapsed row would report `return_of_invalid_type`.
    /// The `bool`-target control: `bool` is undeclared in the harness, so
    /// the cast reports `cast_to_non_type` and the expression's static type
    /// is the oracle's `InvalidType` — assignable into the `int` return
    /// position, so no `return_of_invalid_type` stacks on top. The
    /// trailing bare-statement shape guards the #2225 face-4 false
    /// positive: a discarded `(x as dynamic);` asks no type question at
    /// all.
    #[test]
    fn dynamic_cast_paren_keeps_the_dynamic_type() {
        let report = report_units(
            &[],
            "main.dart",
            "class Object {}\n\
             class int {}\n\
             int f(Object x) => (x as dynamic);\n\
             int g(Object x) => (x as bool);\n\
             void h(Object x) {\n\
             \x20 (x as dynamic);\n\
             \x20 ((x as dynamic));\n\
             }",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["cast_to_non_type"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The condition-sparing latch survives the `dynamic` fix: `x as bool`
    /// in a condition context still spares `non_bool_condition` (a `bool`
    /// cast is statically `bool`), and `x as dynamic` stays spared through
    /// the `dynamic`-assignable arm rather than the latch — same oracle
    /// verdict, no collapsed row. `bool` is undeclared in the harness, so
    /// each `as bool` site carries its `cast_to_non_type` row — the
    /// condition faces must not join them.
    #[test]
    fn bool_cast_condition_sparing_holds() {
        let report = report_units(
            &[],
            "main.dart",
            "class Object {}\n\
             void f(Object x) {\n\
             \x20 if (x as bool) {}\n\
             \x20 if ((x as bool)) {}\n\
             \x20 if (x as dynamic) {}\n\
             \x20 if ((x as dynamic)) {}\n\
             }",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["cast_to_non_type", "cast_to_non_type"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The interface-typed receiver face: a `Match`-typed local whose
    /// declaration resolves to an unindexed unit (the synthetic-SDK shape)
    /// chain-breaks the member walk — the frozen `Match` face
    /// (`sdk/lib/core/pattern.dart:85`) answers: `absent` reports the
    /// access-intent trio while `group`/`start`/`[]` stay silent.
    #[test]
    fn unindexed_interface_receiver_reports_absent_members() {
        let report = report_unindexed_unit(
            "abstract class Match {\n\
             \x20 int get start;\n\
             \x20 int get end;\n\
             \x20 String? group(int group);\n\
             }",
            "import 'a.dart';\n\
             void f(Match m) {\n\
             \x20 m.absent;\n\
             \x20 m.group(0);\n\
             \x20 m.start;\n\
             \x20 m.missing();\n\
             \x20 m.gone = 1;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_getter", "undefined_method", "undefined_setter"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The deep-chain shape: `class C implements Match` breaks the member
    /// walk on the interface hop (`Match` resolves to the unindexed unit) —
    /// the frozen face still answers, so `c.absent` reports and `c.group`
    /// stays silent. The rig has no dart:core, so `C`'s own member type
    /// spellings (`int`, `String`, `int`) carry their `undefined_class`
    /// rows beside the access miss.
    #[test]
    fn unindexed_interface_hop_reports_absent_members() {
        let report = report_unindexed_unit(
            "abstract class Match {\n\
             \x20 int get start;\n\
             \x20 String? group(int group);\n\
             }",
            "import 'a.dart';\n\
             class C implements Match {\n\
             \x20 int get start => 0;\n\
             \x20 String? group(int group) => '';\n\
             }\n\
             void f(C c) { c.absent; c.group(0); c.start; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_class",
                "undefined_class",
                "undefined_class",
                "undefined_getter"
            ],
            "{:?}",
            code_names(&report)
        );
    }

    /// The core-type operator face (#2235): an `int`/`double`/`num`/
    /// `String`/`bool` receiver whose declaration resolves to an unindexed
    /// unit chain-breaks the member walk, and the frozen member table now
    /// carries each face's SDK-declared operator spellings — the declared
    /// operators (`<`/`>=`/`+`/`&`/`<<`, `[]`/`+`/`*` on `String`, `&` on
    /// `bool`) stay silent exactly as the oracle answers, while a genuinely
    /// absent spelling (`[]` on `int`, `+` on the frozen `Match` face)
    /// still reports `undefined_operator`.
    #[test]
    fn unindexed_core_type_operators_decide_through_frozen_face() {
        let report = report_unindexed_unit(
            "abstract class Match {\n\
             \x20 int get start;\n\
             }\n\
             class int {}\n\
             class double {}\n\
             class num {}\n\
             class String {}\n\
             class bool {}\n",
            "import 'a.dart';\n\
             void f(int a, double d, num n, String s, bool b, Match m) {\n\
             \x20 if (a < 2 && a >= 0) {}\n\
             \x20 if (a + 1 - 2 * 3 % 4 / 5 > 0) {}\n\
             \x20 if ((a & 1 | 2 ^ 3) > 0 && (a << 1 >> 2 >>> 3) > 0) {}\n\
             \x20 if (d < 2 && n < 2) {}\n\
             \x20 s[0];\n\
             \x20 s * 2;\n\
             \x20 s + s;\n\
             \x20 if (b & true | false ^ true) {}\n\
             \x20 a[0];\n\
             \x20 m + 1;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_operator", "undefined_operator"],
            "{:?}",
            code_names(&report)
        );
        // The two misses are `a[0]`'s `[]` on `int` and `m + 1`'s `+` on
        // `Match` — in source order, each message carrying its spelling.
        let messages: Vec<&str> = report
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect();
        assert!(
            messages[0].contains("'[]'") && messages[0].contains("'int'"),
            "{messages:?}"
        );
        assert!(
            messages[1].contains("'+'") && messages[1].contains("'Match'"),
            "{messages:?}"
        );
    }

    /// The anchor discipline: a receiver whose type is genuinely
    /// undeclared (`Gone` resolves to nothing — the oracle's `InvalidType`)
    /// answers every member face silently; only the frozen-face spellings
    /// decide through a broken chain.
    #[test]
    fn undeclared_receiver_stays_silent() {
        let report = report_units(
            &[],
            "main.dart",
            "void f(Gone g) { g.absent; g.missing(); g.gone = 1; }",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["undefined_class"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The `super(name:)` public-name join (#2214's face, routed through
    /// `named_formal_for`): a `this._x` field formal's element carries the
    /// public `x` (`correspondingPublicName`, token_impl.dart:364), so
    /// `super(x: 0)` binds silently while `super(y: 0)` keeps its
    /// `undefined_named_parameter` miss. The fixture's `_x` is written by
    /// the field formals and never read — the private field carries its
    /// `unused_field` row beside the named miss.
    #[test]
    fn super_named_argument_binds_the_public_formal_name() {
        let report = report_units(
            &[],
            "main.dart",
            "class int {}\n\
             class B {\n\
             \x20 B({required this._x});\n\
             \x20 final int _x;\n\
             }\n\
             class C extends B {\n\
             \x20 C() : super(x: 0);\n\
             }\n\
             class D extends B {\n\
             \x20 D() : super(y: 0);\n\
             }",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["unused_field", "undefined_named_parameter"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The `on`-face union in `check_instance_member` (#2186 S2's
    /// `on_face_walk`, routed through the Mixin/Extension arm): a
    /// declared-typed receiver's miss decidable through the constraint
    /// climb reports the access intent naming the static type — the
    /// oracle's interface lookup (the constraint members ride `M`'s
    /// interface; `lookup_failure_reporter.dart`'s instance domain).
    #[test]
    fn mixin_typed_receiver_miss_names_the_mixin() {
        let report = report_units(
            &[],
            "main.dart",
            "class C { void hit() {} } \
             mixin M on C {} \
             void f(M m) { m.miss(); }",
            false,
        );
        let names = code_names(&report);
        assert_eq!(names, ["undefined_method"], "{names:?}");
        assert!(
            report.diagnostics[0].message.contains("'M'"),
            "{:?}",
            report.diagnostics[0].message
        );
    }

    /// The union's resolved side: a constraint member reached through the
    /// `on` climb silences the access exactly like an own member.
    #[test]
    fn mixin_typed_receiver_hit_on_the_constraint_stays_silent() {
        let report = report_units(
            &[],
            "main.dart",
            "class C { void hit() {} } \
             mixin M on C {} \
             void f(M m) { m.hit(); }",
            false,
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// A `this` receiver's decidable `on`-face miss inside an extension
    /// body reports naming the `on` type (#2230's emit-side flip — the
    /// oracle's `InstanceLookupDomain` names `this`'s static type: the
    /// extended `C`, never the extension). An unresolved `on` name keeps
    /// the record-only arm — nothing honest names the report.
    #[test]
    fn this_miss_inside_extension_body_reports_on_the_on_type() {
        let report = report_units(
            &[],
            "main.dart",
            "class C { void hit() {} } \
             extension X on C { void g() { this.miss(); } }",
            false,
        );
        let names = code_names(&report);
        assert_eq!(names, ["undefined_method"], "{names:?}");
        assert!(
            report.diagnostics[0].message.contains("'C'"),
            "{:?}",
            report.diagnostics[0].message
        );
    }

    // —— the #3058 is-narrow receiver-typing pins —————————————————————
    //
    // An `is` test on a `dynamic`/untyped receiver re-types the narrowed
    // reads so the `[]`/`[]=` operator faces consult the promoted
    // interface, never an unnamed carrier. The historical board
    // shared-root (~2895 co19 aux rows): `expected[key]` under
    // `expected is Map` in co19's `expect_common.dart` typed the `[]`
    // receiver as the empty-named face and cascaded `undefined_operator`
    // across nearly every co19 case. The substrate resolves the promoted
    // face (the flow's typed promotion spells the tested interface —
    // upstream `isExpression_end`'s promotion split through
    // `tryPromoteForTypeCheck` into `FlowModel.write`'s
    // `promoteToTypeOfInterest` → `_tryPromoteToTypeOfInterest`, official
    // Dart SDK source,
    // pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart
    // :2026, :5955, :3863-3866, :3982) and
    // [`Self::check_operator_expr_in`]'s `Ty::Named` arm
    // walks it; the dynamic face itself stays silent
    // (type_system.dart:899's `isDynamicBounded`
    // short-circuit). Every pin was A/B'd against the oracle
    // (`dart analyze`: the shapes silent or reporting identically); these
    // tests lock that 1:1.

    /// The `undefined_operator` messages, one per reported row (the
    /// reported receiver type rides inside the quotes).
    fn undefined_operator_messages(report: &FileReport) -> Vec<String> {
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_operator")
            .map(|d| d.message.to_string())
            .collect()
    }

    /// `d is Map` narrows a `dynamic` receiver to the `Map` interface: the
    /// index read resolves on it, no `undefined_operator`.
    #[test]
    fn is_map_narrowed_dynamic_index_silent() {
        let report = report_ops("void f(dynamic d) {\n\
             \x20 if (d is Map) {\n\
             \x20   d[0];\n\
             \x20 }\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// `d is List` — the same face on the list twin.
    #[test]
    fn is_list_narrowed_dynamic_index_silent() {
        let report = report_ops("void f(dynamic d) {\n\
             \x20 if (d is List) {\n\
             \x20   d[0];\n\
             \x20 }\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// Scalar narrows ride the same substrate: `d is int` re-types the
    /// read so `+` resolves on `int` (oracle-silent).
    #[test]
    fn is_int_narrowed_dynamic_plus_silent() {
        let report = report_ops("void f(dynamic d) {\n\
             \x20 if (d is int) {\n\
             \x20   d + 1;\n\
             \x20 }\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// `d is String` — the scalar twin through `String`'s own `+`.
    #[test]
    fn is_string_narrowed_dynamic_concat_silent() {
        let report = report_ops("void f(dynamic d) {\n\
             \x20 if (d is String) {\n\
             \x20   d + 'a';\n\
             \x20 }\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// The written-argument narrow (`is Map<String, int>`) types the index
    /// read against the instantiated interface.
    #[test]
    fn is_generic_map_narrowed_index_silent() {
        let report = report_ops("void f(dynamic d) {\n\
             \x20 if (d is Map<String, int>) {\n\
             \x20   d['k'];\n\
             \x20 }\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// The `[]=` write face on a narrowed `dynamic` receiver: the promoted
    /// interface's write spelling resolves.
    #[test]
    fn is_narrowed_dynamic_index_write_silent() {
        let report = report_ops("class Box {\n\
             \x20 operator []=(int i, int v) {}\n\
             }\n\
             void f(dynamic d) {\n\
             \x20 if (d is Box) {\n\
             \x20   d[0] = 1;\n\
             \x20 }\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// The `&&`-joined narrow promotes both conjuncts into the
    /// then-branch — the co19 `_deepEquals` shape (`expected is Map &&
    /// actual is Map`).
    #[test]
    fn and_joined_is_narrow_both_index_silent() {
        let report = report_ops("void f(dynamic expected, dynamic actual) {\n\
             \x20 if (expected is Map && actual is Map) {\n\
             \x20   expected['a'];\n\
             \x20   actual['b'];\n\
             \x20 }\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// The negative narrow feeds the guard's tail: `is!` + return promotes
    /// the remainder, the index read resolves (co19 `checkFailures` shape).
    #[test]
    fn is_not_guard_tail_index_silent() {
        let report = report_ops("void f(dynamic failures) {\n\
             \x20 if (failures is! List) return;\n\
             \x20 failures[0];\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// The co19 `expect_common.dart` `deepEquals` shape itself: untyped
    /// parameters narrowed through an if/else-if chain, index reads on
    /// both branches feeding a call. The exact face that cascaded the
    /// ~2895 aux rows — silent on both engines.
    #[test]
    fn deep_equals_shape_index_silent() {
        let report = report_ops("void plan2check(a, b) {}\n\
             void deepEquals(expected, actual) {\n\
             \x20 if (expected is Map) {\n\
             \x20   plan2check(expected['k'], actual['k']);\n\
             \x20 } else if (expected is List) {\n\
             \x20   plan2check(expected[0], actual[0]);\n\
             \x20 }\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// Negative (oracle-matching): a narrow to a type that genuinely lacks
    /// the spelling still reports — `d is int` then `d[0]` names `int`,
    /// not `''` and not `dynamic`.
    #[test]
    fn is_int_narrowed_index_still_reports_named_type() {
        let report = report_ops("void f(dynamic d) {\n\
             \x20 if (d is int) {\n\
             \x20   d[0];\n\
             \x20 }\n\
             }\n");
        assert_eq!(
            undefined_operator_messages(&report),
            vec!["The operator '[]' isn't defined for the type 'int'.".to_string()]
        );
    }

    /// Negative (oracle-matching): the write twin — `d is int` then
    /// `d[0] = 1` reports `[]=` on `int`.
    #[test]
    fn is_int_narrowed_index_write_still_reports_named_type() {
        let report = report_ops("void f(dynamic d) {\n\
             \x20 if (d is int) {\n\
             \x20   d[0] = 1;\n\
             \x20 }\n\
             }\n");
        assert_eq!(
            undefined_operator_messages(&report),
            vec!["The operator '[]=' isn't defined for the type 'int'.".to_string()]
        );
    }

    // —— the #3058 display face ————————————————————————————————————————
    //
    // A narrowed head renders the oracle's `getDisplayString` face: a
    // raw `Map` fills instantiate-to-bounds (`Map<dynamic, dynamic>`),
    // a written-argument test keeps its spelling (`G<int>`).

    /// `d is Map` + `d + 1` — the miss names `Map<dynamic, dynamic>`,
    /// not the bare head.
    #[test]
    fn is_map_narrowed_operator_miss_renders_instantiated() {
        let report = report_ops("void f(dynamic d) {\n\
             \x20 if (d is Map) {\n\
             \x20   d + 1;\n\
             \x20 }\n\
             }\n");
        assert_eq!(
            undefined_operator_messages(&report),
            vec![
                "The operator '+' isn't defined for the type 'Map<dynamic, dynamic>'."
                    .to_string()
            ]
        );
    }

    /// `d is G<int>` — the written argument rides the promoted face.
    #[test]
    fn is_generic_narrowed_operator_miss_renders_written_args() {
        let report = report_ops("class G<T> {}\n\
             void f(dynamic d) {\n\
             \x20 if (d is G<int>) {\n\
             \x20   d + 1;\n\
             \x20 }\n\
             }\n");
        assert_eq!(
            undefined_operator_messages(&report),
            vec!["The operator '+' isn't defined for the type 'G<int>'.".to_string()]
        );
    }

    // —— the #3081 function-identifier receiver ————————————————————————
    //
    // A bare function identifier evaluates to the tear-off — its static
    // type is the signature face (`binary_expression_resolver.dart`:
    // 425-478). `f + 1` reports the operator miss on `int Function(int)`.

    /// Top-level function identifier as an operator operand.
    #[test]
    fn fn_identifier_binary_reports_function_type() {
        let report = report_ops("int f(int x) => x;\n\
             void g() {\n\
             \x20 f + 1;\n\
             }\n");
        assert_eq!(
            undefined_operator_messages(&report),
            vec![
                "The operator '+' isn't defined for the type 'int Function(int)'."
                    .to_string()
            ]
        );
    }

    /// The same face on a local function declaration.
    #[test]
    fn local_fn_identifier_binary_reports_function_type() {
        let report = report_ops("void g() {\n\
             \x20 int f(int x) => x;\n\
             \x20 f + 1;\n\
             }\n");
        assert_eq!(
            undefined_operator_messages(&report),
            vec![
                "The operator '+' isn't defined for the type 'int Function(int)'."
                    .to_string()
            ]
        );
    }

    /// The index spelling on the same tear-off (`f[0]` over `int f(int)`
    /// — the `[]` the co19 cascades apply).
    #[test]
    fn fn_identifier_index_reports_function_type() {
        let report = report_ops("int f(int x) => x;\n\
             void g() {\n\
             \x20 f[0];\n\
             }\n");
        assert_eq!(
            undefined_operator_messages(&report),
            vec![
                "The operator '[]' isn't defined for the type 'int Function(int)'."
                    .to_string()
            ]
        );
    }

    // —— the #3082 compound receiver ————————————————————————————————————
    //
    // The `?:`/`??` receiver's `static_type` is the arm join (resolver
    // :2426 → static_type_analyzer.dart:74-92). A member defined on the
    // joined face resolves; a miss names the join.

    /// `(c ? l1 : l2)[0]` — both arms `List`-faced: the index resolves.
    #[test]
    fn conditional_receiver_same_head_index_silent() {
        let report = report_ops("void f(bool c, List<int> l1, List<int> l2) {\n\
             \x20 (c ? l1 : l2)[0];\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// `(l1 ?? l2)[0]` — the if-null twin.
    #[test]
    fn if_null_receiver_index_silent() {
        let report = report_ops("void f(List<int>? l1, List<int> l2) {\n\
             \x20 (l1 ?? l2)[0];\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// The joined face's member check: `LUB(A, B)` on sibling classes
    /// without a common user base joins left; the `[]` miss names the
    /// arm type the oracle's LUB produced.
    #[test]
    fn conditional_receiver_member_on_joined_face() {
        let report = report_ops("class A { int get v => 0; }\n\
             class B extends A {}\n\
             void f(bool c, A a, B b) {\n\
             \x20 (c ? a : b).v;\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }

    /// A `??` receiver whose member miss reports the joined face's
    /// display (the under-report direction stays silent for unordered
    /// arms; subtype-ordered arms name the wider side).
    #[test]
    fn if_null_receiver_member_miss_names_join() {
        let report = report_ops("class A {}\n\
             class B extends A {}\n\
             void f(B? l, B r) {\n\
             \x20 (l ?? r).nope;\n\
             }\n");
        // `LUB(B?, B) = B`; the `nope` miss names `B`.
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|d| d.code.name == "undefined_getter")
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>(),
            vec!["The getter 'nope' isn't defined for the type 'B'.".to_string()]
        );
    }

    // —— cluster H: the record field's write face ————————————————————————
    //
    // Records are immutable upstream (`property_element_resolver`'s record
    // arm consults the field getters; no setter element exists): a write to
    // an existing field reports `undefined_setter` on the record display.

    /// `r.bar = 3` over `var r = (bar: 1)`: the field resolves the read
    /// face only, so the write reports `undefined_setter 'bar'` on
    /// `({int bar})` — the oracle's own row, not silence.
    #[test]
    fn record_named_field_write_reports_undefined_setter() {
        let report = report_ops("void f() {\n\
             \x20 var r = (bar: 1);\n\
             \x20 r.bar = 3;\n\
             }\n");
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|d| d.code.name == "undefined_setter")
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>(),
            vec![
                "The setter 'bar' isn't defined for the type '({int bar})'.".to_string()
            ]
        );
    }

    /// The compound `+=` reads the field fine and reports the setter face
    /// alone (`r.bar += 1` — the read resolved, the write's face misses).
    #[test]
    fn record_named_field_compound_write_reports_undefined_setter() {
        let report = report_ops("void f() {\n\
             \x20 var r = (bar: 1);\n\
             \x20 r.bar += 1;\n\
             }\n");
        assert_eq!(
            report
                .diagnostics
                .iter()
                .filter(|d| d.code.name == "undefined_setter")
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>(),
            vec![
                "The setter 'bar' isn't defined for the type '({int bar})'.".to_string()
            ]
        );
    }

    /// A READ of the same field stays silent (the pin's other half: the
    /// write face alone reports).
    #[test]
    fn record_named_field_read_stays_silent() {
        let report = report_ops("int f() {\n\
             \x20 var r = (bar: 1);\n\
             \x20 return r.bar;\n\
             }\n");
        assert_eq!(code_names(&report), Vec::<&'static str>::new());
    }
}

/// The #3175 pins — the type-literal receiver face on plain member reads:
/// a receiver head spelling `dynamic`/`Never` or an ungrounded alias
/// denotes the `Type` instance (the miss trio names `Type`, the `Object`
/// face resolves), while value receivers of the same shapes keep their own
/// displays.
#[cfg(test)]
#[path = "ticket_3175_tests.rs"]
mod ticket_3175_tests;

// The #3183 pins — the object-pattern shorthand env's foreign param
// names: `Circle(area: v)` substitutes `Unit<T>` → `Unit<MetricUnits>`
// through the declaring unit's `type_param_names` fallback in
// `object_pattern_binder_ty`, never the raw schema nor the unknown the
// same-file `container_params` miss used to leak.
#[cfg(test)]
#[path = "ticket_3183_tests.rs"]
mod ticket_3183_tests;

// The #3186 pins — the type-literal receiver face runs BEFORE the
// invalid-receiver `new` arm: `Alias.new()` reports ONE `Type` row (the
// alias invoke twin is `report_alias_selector_face`'s, claimed here)
// instead of pairing it with `<unknown>`, while a head that spells no type
// keeps the `<unknown>` display.
#[cfg(test)]
#[path = "ticket_3186_tests.rs"]
mod ticket_3186_tests;

// The #3187 pins — the instantiated type literal as a MEMBER target:
// `C<T>.sel<T>()` / `Alias<T>.sel<T>()` resolve `sel` on the `Type`
// instance (the send's own type arguments keep it out of the creation
// face), so a static miss reports `undefined_method` naming `Type`.
#[cfg(test)]
#[path = "ticket_3187_tests.rs"]
mod ticket_3187_tests;

// The #3210 (F1) pins — the aux-file `[]` receiver face in the
// materialized co19 posture (`expect_common.dart` as a `part of` unit of
// the `expect` library): the `deepEquals` local functions' index reads
// resolve (promoted `Map`/`List` face, dynamic sibling) and the
// empty-display `undefined_operator` cascade stays gone — the '' carrier's
// production site was removed with #3058; these lock the family silent
// beside the negative faces (a genuine miss still reports named types).
#[cfg(test)]
#[path = "ticket_f1_tests.rs"]
mod ticket_f1_tests;
