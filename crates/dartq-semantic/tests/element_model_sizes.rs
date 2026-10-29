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

    // `TopDecl`: payload = kind(1) + type_params u16(2) + two `Ty`(24) +
    // `Option<Box<Formals>>`(8) = 35 bytes, rounded to the 8-byte alignment
    // of the boxed formals — 40 is exact, no padding lever without boxing a
    // `Ty`.
    assert_eq!(
        size_of::<TopDecl>(),
        40,
        "TopDecl budget: 8 + 12 + 12 + 4+4"
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

    // `Container`: ten boxed 16-byte columns (members, ctor formals,
    // annotations, primary formals, constructors, and — since #2246 — the
    // with/implements/on/residue/enum-constant clause columns, which were
    // inline `Vec`s worth 224 bytes total) + superclass(8) + the scalar tail
    // (kind, eight modifier flags, type_params u16 = 11 bytes) = 179, rounded
    // to 184. A clause column reverting to an inline `Vec` re-adds 8 bytes
    // per field and pushes this past the pin.
    assert_eq!(
        size_of::<Container>(),
        184,
        "Container budget: 10*16 + 8 + 11 rounded to 8"
    );
}
