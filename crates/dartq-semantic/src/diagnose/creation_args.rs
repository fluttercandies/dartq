//! The explicit-argument creation's static type (`var set =
//! SplayTreeSet<E>(comparison)`): the folded `Created` row carries the
//! bare head, and the written `<E>` rides `call_type_args` keyed on the
//! callee row — the inferred local's type is the instantiated head, so
//! `return set;` satisfies a `Set<E>` return through the real
//! `SplayTreeSet<E> implements Set<E>` edge instead of the raw head's
//! to-bounds instantiation. The assignment staging reads the same slot
//! for `x = C<int>()`.
//!
//! Scope against the instance-creation downward inference (recon bucket
//! C): this file is the created row's TYPE READER — [`Engine::creation_row_ty`]
//! (written group first, then the send's published solution, both keyed
//! on the head's offset), [`Engine::with_explicit_creation_args`] (the
//! `var` bind), and [`Engine::instantiated_head_ty`] (the
//! instantiate-to-bounds fill of a raw generic head, inert for a
//! non-generic one). Everything that DECIDES a creation's instantiation
//! lives in `invocation.rs`: the context discovery
//! (`creation_context_ty`), the parameter solve (`creation_call_site_ty`
//! with its `creation_context_seed`), the publish
//! (`publish_creation_instantiation`), and the argument check's own
//! declared/write-context seed. The open residual — a context-seeded
//! parameter must win over a disagreeing argument, so that
//! `A<int, String> x = new A('s', 1)` reads `A<int, String>` and reports
//! the argument rows instead of the to-bounds `A<dynamic, dynamic>` —
//! is `creation_call_site_ty`'s poison arm, whose upstream rule is the
//! already-inferred (downward) type returned for a covariant parameter
//! ahead of any constraint (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer_operations.dart
//! `inferTypeParameterFromAll`). The pinned reader contract those lanes
//! must keep is `ticket_3388_tests.rs`.

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
    pub(crate) fn written_creation_args(&self, name: StringId, offset: u32) -> Option<Vec<Ty>> {
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
        self.literal_group_args(left).or_else(|| {
            // A `void` type argument (`A<void>`): `literal_group_args` reads
            // the registered type-uses only, and `void` carries none — the
            // group's whole parse declines and the raw parameter spelling
            // survives into the argument check (`new A<void>(null)` reported
            // `Null` against `T`). Re-derive the group's segments directly:
            // a `void` segment answers `Ty::Void`, every other shape keeps
            // the under-report posture rather than a partial substitution.
            let bytes = self.source.as_bytes();
            let inner_start = self.byte_of(offset + head_len) + 1;
            let inner_end = cursor;
            let mut segments = Vec::new();
            let mut depth = 0usize;
            let mut seg = inner_start;
            for c in inner_start..inner_end {
                match bytes[c] {
                    b'<' | b'(' | b'[' => depth += 1,
                    b'>' | b')' | b']' => depth = depth.saturating_sub(1),
                    b',' if depth == 0 => {
                        segments.push((seg, c));
                        seg = c + 1;
                    }
                    _ => {}
                }
            }
            segments.push((seg, inner_end));
            segments
                .iter()
                .map(|&(mut s, e)| {
                    while s < e && bytes[s].is_ascii_whitespace() {
                        s += 1;
                    }
                    let mut e = e;
                    while e > s && bytes[e - 1].is_ascii_whitespace() {
                        e -= 1;
                    }
                    (&bytes[s..e] == b"void").then_some(Ty::Void)
                })
                .collect()
        })
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
        if let Some(defaults) = self.bounds.itb_default_args(self.strings, name) {
            let filled: Vec<Ty> = defaults.iter().map(|tree| self.tree_ty(tree)).collect();
            return self
                .boundary
                .args
                .borrow_mut()
                .named(name, nullable, &filled);
        }
        // A head the same-file bounds snapshot never registered — a FOREIGN
        // generic (`Circle<T extends MetricUnits>` living in the pattern
        // helper's own unit): `itb_default_args` misses its snapshot
        // registry, but the container's declared `type_param_bounds` carry
        // the same instantiate-to-bounds defaults (the receiver-declaration
        // bounds row_resolution.rs:8398's raw-head arm reads), projected
        // across the file boundary into the diagnosing pool. A genuinely
        // non-generic or unindexed head keeps the bare face.
        let Some((decl, owner)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) else {
            return ty;
        };
        if decl.type_param_bounds.is_empty() {
            return ty;
        }
        let filled: Vec<Ty> = decl
            .type_param_bounds
            .iter()
            .map(|&bound| self.boundary.ty(owner, bound))
            .collect();
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

// The #3180 pins (the foreign generic object-pattern head's
// instantiate-to-bounds fill — `Circle<T extends MetricUnits>` reaches
// the bound row's face as `Circle<MetricUnits>` through the container's
// declared `type_param_bounds` projected across the file boundary, and
// `bind_var_type` applies the same I2B). Registered here, not in
// `diagnose.rs` (other lanes' file), via `#[path]`: the module stays
// inside the `diagnose` subtree so `super::test_rig` resolves.
#[cfg(test)]
#[path = "ticket_3180_tests.rs"]
mod ticket_3180_tests;
