//! Path-universe pin (#287): overlay-only files land in the same canonical
//! spelling universe as disk files. `FileTable` normalization derives a
//! missing path's identity from its nearest existing ancestor, so in a
//! symlinked workspace (macOS `/var` → `/private/var`, or an explicit alias
//! directory) an overlay submitted through the alias spelling interns under
//! the canonical spelling — the same universe the resolver composes part
//! targets from and lexical-prefix package attribution matches against.
//!
//! Pre-#287 the overlay kept its literal spelling: one logical file, two
//! ids, the part claim missed residency (oracle C5), and `packages
//! .containing` — a purely lexical prefix match against canonicalized
//! package roots — attributed the overlay to no package at all, forking the
//! language version between disk files and overlays (#272 finding A).

mod common;

use common::Fixture;

/// Pin: an overlay-only file submitted through an alias-directory spelling
/// interns under the canonical lib dir — the same universe as the disk
/// file — and lexical-prefix package attribution therefore assigns both the
/// same package identity and the same language version.
#[cfg(unix)]
#[test]
fn overlay_only_file_shares_the_disk_file_spelling_universe() {
    let fixture = Fixture::new(&[("lib/a.dart", common::MAIN_0)]);
    // An alias directory into the workspace: the overlay is submitted
    // spelled through the link, so identity must survive symlink resolution
    // even where the platform temp dir is not itself symlinked.
    std::os::unix::fs::symlink(&fixture.root, fixture.root.join("link")).expect("alias symlink");

    let mut engine = fixture.engine();
    let overlay_spelling = fixture.root.join("link").join("lib").join("overlay.dart");
    let overlay = engine.set_content(&overlay_spelling, b"int o() => 1;\n");
    let disk = engine.set_content(&fixture.path("lib/a.dart"), common::MAIN_0.as_bytes());

    // One universe: both stored paths live under the same canonical lib dir.
    let files = engine.file_table();
    let overlay_path = files.path(overlay).expect("overlay interned").to_path_buf();
    let disk_path = files.path(disk).expect("disk interned").to_path_buf();
    assert_eq!(
        overlay_path.parent(),
        disk_path.parent(),
        "overlay and disk files share the canonical lib dir"
    );

    // Same package identity — hence the same language version. The #272
    // divergence measured disk 3.5 vs overlay default: the link-spelled
    // overlay prefix-matched no canonical package root.
    let overlay_owner = fixture.packages.containing(&overlay_path);
    let disk_owner = fixture.packages.containing(&disk_path);
    assert_eq!(overlay_owner, disk_owner, "one package identity");
    let owner = overlay_owner.expect("both attribute to the fixture package");
    assert_eq!(
        owner
            .language_version
            .map(|v| (v.into_inner().major, v.into_inner().minor)),
        Some((3, 5)),
        "the overlay carries the package's language version"
    );
}
