# legs/01-packages.sh — `dartq packages` over real multi-package trees:
# a real git clone and a real `git worktree add` linked tree nested in a
# package's vendor/ as boundaries; two universes side by side (siblings
# stay invisible to each other); plain copies; fresh clones (unresolved);
# broken pubspecs; the orphan workspace member's navigation error.
#
# Universe semantics being pinned: the ladder roots the scan at the
# enclosing package; the walk-down never crosses a foreign tree (a
# checkout marker or gitdir pointer prunes it and registers a BOUNDARY).

legs_01_run() {
  local base="$E2E_WORK/pkgs"
  mkdir -p "$base"

  # --- 01.1 nested checkout + real linked worktree ----------------------
  leg_begin pkgs-nested-worktrees "nested checkout and real git worktree register as boundaries"
  # A real repo the fixture commits a package into; a real clone of it
  # and a real linked worktree land inside the scanned package's vendor/.
  local repo="$base/gitrepo"
  git init -q "$repo"
  wf "$repo/pubspec.yaml" 'name: main_pkg
environment:
  sdk: ^3.5.0
'
  wf "$repo/lib/main.dart" 'int main() => 1;
'
  git -C "$repo" add -A
  git -C "$repo" -c user.email=dq@e2e -c user.name=dq commit -q -m pkg
  local wts="$base/wts"
  make_pkg "$wts" "outer"
  git clone -q "$repo" "$wts/vendor/main"
  git -C "$repo" worktree add -q "$wts/vendor/linked" -b wt-branch
  # A stale gitdir pointer: the pointed-at tree is gone.
  wf "$wts/vendor/stale/pubspec.yaml" 'name: stale_pkg
environment:
  sdk: ^3.5.0
'
  wf "$wts/vendor/stale/.git" "gitdir: /gone/.git/worktrees/x
"
  run_dartq_in pkgs-nested "$wts" packages --format machine
  assert_eq "$DQ_RC" 0 "packages exits clean over a tree holding foreign checkouts"
  local rows
  rows="$(cat "$(out_file pkgs-nested)")"
  assert_contains "$rows" "PKG|$wts|outer|independent|^3.5.0|yes" \
    "the scanned package itself is the one work point"
  assert_contains "$rows" "BOUNDARY|$wts/vendor/main|nested-checkout" \
    "the real clone is a nested-checkout boundary"
  assert_contains "$rows" "BOUNDARY|$wts/vendor/linked|worktree-pointer" \
    "the real linked worktree is a worktree-pointer boundary"
  assert_contains "$rows" "BOUNDARY|$wts/vendor/stale|worktree-pointer" \
    "the stale pointer registers as a worktree-pointer boundary"
  assert_contains "$(printf '%s\n' "$rows" | grep "BOUNDARY|$wts/vendor/stale|")" "missing" \
    "the stale pointer's detail says the target is missing"
  assert_not_contains "$rows" "main_pkg" "the foreign checkout is not a work point"
  assert_not_contains "$rows" "linked_pkg" "the linked worktree is not a work point"
  git -C "$repo" worktree remove --force "$wts/vendor/linked" 2>/dev/null || true

  # --- 01.2 two universes side by side ---------------------------------
  leg_begin pkgs-two-universes "two universes side by side stay separate"
  make_pkg "$base/uni_a" "uni_a"
  make_pkg "$base/uni_b" "uni_b"
  run_dartq_in pkgs-uni-a "$base/uni_a" packages --format machine
  assert_eq "$DQ_RC" 0 "universe A inventories clean"
  rows="$(cat "$(out_file pkgs-uni-a)")"
  assert_contains "$rows" "PKG|$base/uni_a|uni_a|independent|^3.5.0|yes" \
    "universe A lists itself"
  assert_not_contains "$rows" "uni_b" "universe A never sees its sibling universe"
  run_dartq_in pkgs-uni-b "$base/uni_b" packages --format machine
  assert_eq "$DQ_RC" 0 "universe B inventories clean"
  assert_contains "$(cat "$(out_file pkgs-uni-b)")" "PKG|$base/uni_b|uni_b|independent|^3.5.0|yes" \
    "universe B lists itself"
  assert_not_contains "$(cat "$(out_file pkgs-uni-b)")" "uni_a" \
    "universe B never sees its sibling universe"

  # --- 01.3 plain copy + fresh clone -----------------------------------
  leg_begin pkgs-copy-and-fresh "plain copy and fresh clone (unresolved)"
  make_pkg "$base/uni_a/vendor/copy" "vendored_copy"
  # A fresh clone: pubspec but no .dart_tool — discovered, unresolved.
  wf "$base/uni_a/vendor/fresh/pubspec.yaml" 'name: fresh_clone
environment:
  sdk: ^3.5.0
'
  mkdir -p "$base/uni_a/vendor/fresh/lib"
  run_dartq_in pkgs-fresh "$base/uni_a" packages --format machine
  assert_eq "$DQ_RC" 0 "unresolved packages are rows, not failures"
  rows="$(cat "$(out_file pkgs-fresh)")"
  assert_contains "$rows" "PKG|$base/uni_a/vendor/copy|vendored_copy|independent|^3.5.0|yes" \
    "the plain copy under the foreign tree is an independent work point"
  assert_contains "$rows" "PKG|$base/uni_a/vendor/fresh|fresh_clone|independent|^3.5.0|no" \
    "the fresh clone is discovered but unresolved"
  run_dartq_in pkgs-fresh-human "$base/uni_a" packages --format human
  rows="$(cat "$(out_file pkgs-fresh-human)")"
  assert_contains "$rows" "dart pub get" "the human face prints the pub-get hint"
  assert_contains "$rows" "in $base/uni_a/vendor/fresh (needs SDK ^3.5.0)" \
    "the hint names the package dir and its SDK constraint"

  # --- 01.4 broken pubspec ---------------------------------------------
  leg_begin pkgs-broken-pubspec "broken pubspec keeps its NOTE row"
  wf "$base/uni_b/vendor/bad/pubspec.yaml" 'name: [unclosed
'
  run_dartq_in pkgs-broken "$base/uni_b" packages --format machine
  assert_eq "$DQ_RC" 0 "a malformed pubspec degrades to a note, not a failure"
  rows="$(cat "$(out_file pkgs-broken)")"
  assert_contains "$rows" "PKG|$base/uni_b/vendor/bad|-|independent|-|no" \
    "the broken package is an unnamed unresolved row"
  assert_contains "$rows" "NOTE|$base/uni_b/vendor/bad|" "the parse error rides a NOTE row"

  # --- 01.5 orphan workspace member ------------------------------------
  leg_begin pkgs-orphan-member "orphan workspace member refuses with navigation"
  wf "$base/orphan/pkg/pubspec.yaml" 'name: orphan_pkg
environment:
  sdk: ^3.5.0
resolution: workspace
'
  mkdir -p "$base/orphan/pkg/lib"
  run_dartq_in pkgs-orphan "$base/orphan/pkg" packages
  assert_eq "$DQ_RC" 64 "the orphan member ladder is the usage tier"
  assert_contains "$(err_of pkgs-orphan)" "no workspace root" \
    "the error explains the missing workspace root"

  # --- 01.6 no universe: the navigation error ---------------------------
  leg_begin pkgs-no-universe "a tree with no pubspec anywhere is the usage tier"
  mkdir -p "$base/bare/deep/deeper"
  run_dartq_in pkgs-bare "$base/bare" packages
  assert_eq "$DQ_RC" 64 "no pubspec and no checkout marker is usage"
  assert_contains "$(err_of pkgs-bare)" "pubspec" "the navigation text names the pubspec"
}
