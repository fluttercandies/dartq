//! Size pins for the per-unit element model (#2246): the residency cost of
//! `Arc<FileElements>` payloads is dominated by the per-row struct sizes, so
//! every byte that lands here multiplies through the 10⁵-file corpus. Each
//! pin carries the byte budget it guards; a regression (a field gaining
//! width or losing a niche) must update the pin with the reason in the same
//! commit.

use dartq_semantic::model::{Annotation, ExtensionFace, Formals, Ty};
use dartq_semantic::{Container, MemberEntry, TopDecl};
use std::mem::size_of;

#[test]
fn element_model_row_structs_stay_at_their_pinned_sizes() {
    // `Ty`: niche-optimal — the 1-byte discriminant rides the `Named`
    // variant's tail padding (name u32 + args u32 + nullable bool), so the
    // enum is already at its minimum 12 bytes; no discriminant/niche repack
    // has anything left to win.
    assert_eq!(size_of::<Ty>(), 12, "Ty must stay niche-packed at 12 bytes");

    // `TopDecl`: kind(1) + type_params u16(2) + the alias/closure-solver
    // columns `type_param_names`/`type_param_bounds` (two `Box`es, 32 —
    // the 0.1.21 wave's generic top-level declarations) + two `Ty`(24) +
    // `Option<Box<Formals>>`(8) + the metadata star `annotations`
    // (`Box<[Annotation]>`, 16 — the wave's top-level dmu target face) =
    // 88. Boxing a `Ty` is the only lever left.
    assert_eq!(
        size_of::<TopDecl>(),
        88,
        "TopDecl budget: 16 + 32 + 12 + 12 + 8 + 16"
    );

    // `MemberEntry`: payload = two boxed columns(32) + `Option<Box<Formals>>`
    // (8) + two `Ty`(24) + `SourceRange`(8) + kind(1) + four modifier bools
    // + type_params u16(2) = 79 bytes — 80 is the alignment floor, so flag
    // packing has nothing to win.
    assert_eq!(
        size_of::<MemberEntry>(),
        80,
        "MemberEntry budget: 32 + 8 + 24 + 8 + 8"
    );

    // `ExtensionFace`: payload = two `Option<StringId>`(16) + `Ty`(12) +
    // boxed type-param names(16) + member `NameMap`(16) + nullability bool
    // = 57 bytes — 64 is the alignment floor.
    assert_eq!(
        size_of::<ExtensionFace>(),
        64,
        "ExtensionFace budget: 16 + 8 + 12 + 16 + 16"
    );

    // `Annotation`: name(4) + `SourceRange`(8) + optional argument text(8) =
    // exactly 20 at its 4-byte alignment — zero padding.
    assert_eq!(size_of::<Annotation>(), 20, "Annotation budget: 4 + 8 + 8");

    // `Formals`: two boxed formal columns — exactly 32.
    assert_eq!(size_of::<Formals>(), 32, "Formals budget: 16 + 16");

    // `Container`: seventeen boxed 16-byte columns (members, ctor/factory
    // formals, annotations, primary formals, constructors, the
    // with/implements/on/residue/enum-constant clause columns, the
    // type-parameter name/bound columns, and the 0.1.21 wave's variance
    // and clause-argument columns `type_param_variances` /
    // `with/implements/on_clause_args`) + superclass(8) +
    // superclass_args u32(4) + the scalar tail (kind, eleven modifier
    // flags, type_params u16). A clause column reverting to an inline
    // `Vec` re-adds 8 bytes per field and pushes this past the pin.
    assert_eq!(
        size_of::<Container>(),
        320,
        "Container budget: 17*16 + 8 + 4 + 18 rounded to 8"
    );
}
