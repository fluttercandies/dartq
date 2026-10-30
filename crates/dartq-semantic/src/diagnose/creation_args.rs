//! The explicit-argument creation's static type (`var set =
//! SplayTreeSet<E>(comparison)`): the folded `Created` row carries the
//! bare head, and the written `<E>` rides `call_type_args` keyed on the
//! callee row — the inferred local's type is the instantiated head, so
//! `return set;` satisfies a `Set<E>` return through the real
//! `SplayTreeSet<E> implements Set<E>` edge instead of the raw head's
//! to-bounds instantiation. The assignment staging reads the same slot
//! for `x = C<int>()`.

use super::*;

impl Engine<'_> {
    /// The created head's instantiated static type at `offset`: the written
    /// explicit type arguments (`C<int>()`'s `<int>`, keyed on the callee
    /// row) applied to the bare fold-sentinel head, or `None` when the row
    /// registered no written arguments.
    ///
    /// The lookup is strictly peek-not-consume (#2869's shared-queue rule):
    /// the entry stays latched for the send's own `call_args_for`, and the
    /// fallbacks below only READ surfaces the fold left behind.
    ///
    /// Two recovery arms cover the groups `call_type_args` never sees:
    ///
    /// - A HEADED group (`new C<int>()`, `C<int>.named()`): the head armed
    ///   `pending_type_head`, so `call_args_begin` classified the capture
    ///   an annotation and no entry latched. The written group still sits
    ///   in the source between the head's end and the `.`/`(` — the
    ///   byte-walk re-derives it through [`Self::literal_group_args`], the
    ///   same scan the fold-time rows ran (`invocation.rs`'s
    ///   `fold_unnamed_creation_row`/`creation_class_before`).
    /// - A named-selector creation (`C<int>.named(...)`): the group's key
    ///   is the class segment's offset — exactly `offset` here — so the
    ///   peek above already answers it; the byte-walk is the belt for a
    ///   `Created` row produced before the latch existed.
    pub(crate) fn creation_row_ty(
        &self,
        name: StringId,
        nullable: bool,
        offset: u32,
    ) -> Option<Ty> {
        if let Some((_, entry)) = self.call_type_args.iter().find(|(r, _)| *r == offset) {
            return Some(
                self.boundary
                    .args
                    .borrow_mut()
                    .named(name, nullable, &entry.tys),
            );
        }
        if let Some(args) = self.written_creation_args(name, offset) {
            return Some(self.boundary.args.borrow_mut().named(name, nullable, &args));
        }
        // No written group at all (`new C()`, `C()`): the caller falls
        // back to `row.ty` — the send's solved instantiation when
        // inference answered, the bare head otherwise — so `None` here
        // keeps that face; the instantiate-to-bounds residual belongs to
        // [`Self::instantiated_head_ty`]'s adopters on the declared-type
        // and display paths.
        None
    }

    /// The creation head's written `<...>` group, re-derived from the
    /// source bytes: the head's own name ends the head, whitespace-free
    /// trivia aside, and a balanced `<...>` immediately following it is
    /// the written instantiation. `None` when the head spells no group or
    /// the group's tail is not `.`/`(` (an `is`/`as` comparison the same
    /// bytes could otherwise mimic).
    fn written_creation_args(&self, name: StringId, offset: u32) -> Option<Vec<Ty>> {
        let head_len = self.strings.resolve(name).encode_utf16().count() as u32;
        let mut cursor = self.byte_of(offset + head_len);
        let bytes = self.source.as_bytes();
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
        if depth != 0 || cursor >= bytes.len() {
            return None;
        }
        let left = Token {
            id: 0,
            range: SourceRange {
                offset: self.utf16_of(cursor + 1),
                length: 1,
            },
        };
        self.literal_group_args(left)
    }

    /// The instantiate-to-bounds display of a raw generic head
    /// (`Named{head, EMPTY}` → `Named{head, [defaults]}`): the oracle's
    /// resolved `staticType` for a written `G`/`A` is the I2B
    /// instantiation (`type.dart` `instantiateTypeFormalsToBounds`,
    /// `DartType.getDisplayString`), so a bare parameterized head renders
    /// `G<A<dynamic>>` in every diagnostic message — never the fold's
    /// bare `G`.
    ///
    /// Dependency recorded for the #2918 census face (the ~1200-row
    /// raw-head display residual): the fill belongs where the resolved
    /// `Ty` is produced or rendered — `bind_declared_type` /
    /// `bind_parameter_type` (scopes_bodies.rs:1282/1303), `fold_alias_ty`
    /// (resolution.rs:56), or `ty_display` (assignability.rs:4182). All
    /// of those files are owned by sibling lanes this round, so the
    /// helper lands here; the adopters call `self.instantiated_head_ty`
    /// at the point the `Named{_, EMPTY}` shape is decided, keeping the
    /// judgment-side `Ty` unchanged where the oracle's comparison face
    /// (never the display) reads raw heads.
    pub(crate) fn instantiated_head_ty(&self, ty: Ty) -> Ty {
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return ty;
        };
        if args != crate::model::ArgListId::EMPTY {
            return ty;
        }
        let Some(defaults) = self.bounds.itb_default_args(self.strings, name) else {
            return ty;
        };
        let filled: Vec<Ty> = defaults.iter().map(|tree| self.tree_ty(tree)).collect();
        self.boundary
            .args
            .borrow_mut()
            .named(name, nullable, &filled)
    }

    /// `ty` with the creation row's explicit type arguments applied when
    /// `ty` is the bare head of a `Created` receiver at `row`; every other
    /// shape returns unchanged. The precedence mirrors the oracle's
    /// instance-creation `staticType` (`visitInstanceCreationExpression`):
    /// the send's solved instantiation when inference answered, the
    /// written group when spelled, the instantiate-to-bounds residual
    /// otherwise (`var x = C()` binds `C<dynamic>` over unconstrained
    /// `C<S>`) — never the fold's bare head or the unknown sentinel.
    pub(crate) fn with_explicit_creation_args(&self, row: usize, ty: Ty) -> Ty {
        let Some(row) = self.rows.get(row) else {
            return ty;
        };
        let RowShape::Receiver(Receiver::Created { name, .. }) = &row.shape else {
            return ty;
        };
        let created_head = *name;
        // The send's own solved face wins: `var x = C(0)` binds `C<int>`
        // (`pending_send_ty` wrote the fold row), and the written-shape
        // callers above never reached this arm with a filled list.
        if let Ty::Named { args, .. } = row.ty
            && args != crate::model::ArgListId::EMPTY
        {
            return row.ty;
        }
        let (name, nullable) = match ty {
            Ty::Named {
                name: written,
                nullable,
                args,
            } if args == crate::model::ArgListId::EMPTY => (written, nullable),
            // `row.ty` is `Ty::UNKNOWN` (no `pending_send_ty` answered):
            // the Created row's own head is the type the oracle
            // instantiates to bounds.
            _ => (created_head, false),
        };
        if let Some(ty) = self.creation_row_ty(name, nullable, row.range.offset) {
            return ty;
        }
        self.instantiated_head_ty(Ty::Named {
            name,
            nullable,
            args: crate::model::ArgListId::EMPTY,
        })
    }
}
