#!/usr/bin/env bash
# head-closure-gate.sh -- pre-commit closure gate for Rust commits.
#
# Purpose
#   Rejects a staged changeset when a staged .rs file references a symbol
#   whose definition exists only in unstaged/untracked work -- a commit that
#   would carry a CALLER without the DEFINITION it depends on (the "module
#   mount without the mounted file" incident class). Run it manually before
#   `git commit`; it is NOT a .git/hooks installation and it never mutates
#   the index or branches.
#
# Usage
#   scripts/head-closure-gate.sh [--index-file <path>] [--extra-untracked <dir>]...
#
#   --index-file <path>   Inspect this index instead of the real one. The path
#                         is exported as GIT_INDEX_FILE so every git call
#                         (diff --cached / show :path / ls-files --others)
#                         sees the same index and the real index is never
#                         touched. Typical self-test: seed a scratch copy with
#                         `GIT_INDEX_FILE=<path> git read-tree HEAD`.
#   --extra-untracked <dir>
#                         Treat every *.rs file under <dir> (any depth) as an
#                         extra worktree-only definition source. Debug and
#                         self-test hook: lets a test point at scratch
#                         fixtures outside the repository without polluting
#                         it. Repeatable.
#   -h, --help            Show a short usage summary.
#
#   Exit codes: 0 = no closure gaps; 1 = gaps found (each hit prints one
#   `closure-gap: ...` line); 2 = usage or environment error.
#
# Method (coarse by design)
#   1. Staged .rs adds/modifications and staged deletions come from
#      `git diff --cached` (with --no-renames so both sides of a rename are
#      seen explicitly: the renamed-away file counts as deleted, the
#      renamed-in file is scanned as a caller).
#   2. Definition heads -- fn/struct/enum/trait/mod/const/static/type, with
#      pub/async/unsafe/extern/mut decorations -- are harvested from
#      unstaged-modified plus untracked .rs files (plus --extra-untracked
#      fixtures). `impl` heads are not definitions, but methods inside an
#      impl block are, since matching is line-based.
#   3. A name counts as worktree-only iff it is defined there AND has no
#      definition head left in HEAD (HEAD hits inside staged-deleted files
#      do not count) AND is not itself staged AND is not whitelisted below.
#   4. Each staged .rs file (content read from the index via `git show
#      :<path>`) is scanned for whole-word references to worktree-only
#      names; every hit prints one `closure-gap:` line and the gate exits 1
#      after the full scan. Otherwise it exits 0 with a one-line summary.
#
# Notes
#   - Word boundaries are written as (^|[^A-Za-z0-9_]) / ($|[^A-Za-z0-9_])
#     groups instead of \b, \< or [[:<:]] because word-boundary escapes
#     differ across GNU/BSD grep and ugrep; the class form is portable and
#     identical in semantics for identifiers.
#   - Definition harvesting is deliberately line-grepping (no parsing):
#     macro-generated definitions are out of scope, and a junk candidate can
#     only over-trigger the HEAD/staged subtraction, never silently pass a
#     real gap.

set -euo pipefail

# Known-noise identifiers, never reported as closure gaps. Case-sensitive
# whole-name fixed strings; extend this list as new noise shows up.
WHITELIST=(main test tests Default)

readonly DEF_ITEM_RE='(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?(async[[:space:]]+)?(unsafe[[:space:]]+)?(extern([[:space:]]+"[^"]*")?[[:space:]]+)?(fn|struct|enum|trait|mod|type)[[:space:]]+[A-Za-z_][A-Za-z0-9_]*'
readonly DEF_VALUE_RE='(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?(const|static)[[:space:]]+(mut[[:space:]]+)?[A-Za-z_][A-Za-z0-9_]*'
readonly DEF_HEAD_RE="(${DEF_ITEM_RE}|${DEF_VALUE_RE})"
# The phrase regexes can yield structurally meaningless names (`const fn foo`
# yields `fn`; `const _: ()` yields `_`); filter them so every name set stays
# symbol-shaped. The same filter runs on every side of the set algebra.
readonly NOT_SYMBOL_RE='^(fn|struct|enum|trait|mod|type|const|static|pub|async|unsafe|extern|mut|impl|union|use|let|crate|self|Self|super|dyn|where|as|for|in|if|else|match|loop|while|return|true|false|move|ref|_)$'

usage() {
  cat <<'EOF'
usage: scripts/head-closure-gate.sh [--index-file <path>] [--extra-untracked <dir>]...

Pre-commit closure gate for Rust commits: exits 1 when a staged .rs file
references a symbol whose definition exists only in unstaged/untracked work
(the commit would carry a caller without the definition it depends on).

  --index-file <path>     inspect this index instead of the real one (exported
                          as GIT_INDEX_FILE; the real index is untouched)
  --extra-untracked <dir> add every *.rs under <dir> as an extra worktree-only
                          definition source (self-test hook, repeatable)
  -h, --help              this help

Exit codes: 0 clean; 1 closure gaps found (closure-gap: lines); 2 usage or
environment error.
EOF
}

die() { echo "head-closure-gate: $1" >&2; exit 2; }

# stdin: raw source lines -> stdout: one definition-head identifier per line
def_names() {
  grep -hoE "$DEF_HEAD_RE" \
    | grep -oE '[A-Za-z_][A-Za-z0-9_]*$' \
    | grep -vE "$NOT_SYMBOL_RE" \
    || true
}

abspath() { case $1 in /*) printf '%s\n' "$1" ;; *) printf '%s\n' "$(pwd)/$1" ;; esac; }

index_file=""
extra_dirs=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --index-file)
      [ "$#" -ge 2 ] || die "--index-file requires a path argument"
      index_file=$2; shift 2 ;;
    --extra-untracked)
      [ "$#" -ge 2 ] || die "--extra-untracked requires a directory argument"
      extra_dirs+=("$2"); shift 2 ;;
    -h|--help) usage; exit 0 ;;
    --*) die "unknown option: $1 (see --help)" ;;
    *) die "unexpected argument: $1 (see --help)" ;;
  esac
done

command -v git >/dev/null 2>&1 || die "git not found in PATH"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 \
  || die "not inside a git work tree"

if [ -n "$index_file" ]; then
  [ -f "$index_file" ] || die "--index-file: not a regular file: $index_file"
  index_file=$(abspath "$index_file")
  export GIT_INDEX_FILE=$index_file
fi
for d in ${extra_dirs[@]+"${extra_dirs[@]}"}; do
  [ -d "$d" ] || die "--extra-untracked: not a directory: $d"
done

cd "$(git rev-parse --show-toplevel)"
extra_dirs=($(for d in ${extra_dirs[@]+"${extra_dirs[@]}"}; do abspath "$d"; done))

tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/head-closure-gate.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT

if ! git rev-parse --verify -q HEAD >/dev/null 2>&1; then
  echo "head-closure-gate: OK (no HEAD commit yet; a first commit cannot drop a HEAD-anchored definition)"
  exit 0
fi

git diff --cached --no-renames --diff-filter=AM --name-only -- '*.rs' >"$tmp_dir/staged_add"
git diff --cached --no-renames --diff-filter=D --name-only -- '*.rs' >"$tmp_dir/staged_del"

staged_count=$(awk 'END { print NR }' "$tmp_dir/staged_add")
if [ "$staged_count" -eq 0 ]; then
  echo "head-closure-gate: OK (0 staged .rs files; nothing to check)"
  exit 0
fi

# Worktree-only definition sources: unstaged-modified (deletions excluded --
# a deleted file yields no definitions) + untracked + extra-untracked fixtures.
git diff --name-only --diff-filter=d -- '*.rs' >"$tmp_dir/wt_files"
git ls-files --others --exclude-standard -- '*.rs' >>"$tmp_dir/wt_files"
for d in ${extra_dirs[@]+"${extra_dirs[@]}"}; do
  find "$d" -type f -name '*.rs' >>"$tmp_dir/wt_files"
done
sort -u -o "$tmp_dir/wt_files" "$tmp_dir/wt_files"

# name -> first defining worktree file, one "name file" pair per line
: >"$tmp_dir/wt_pairs"
while IFS= read -r f; do
  [ -f "$f" ] || continue  # listed by git but removed from disk since
  for n in $(grep -hE "$DEF_HEAD_RE" "$f" | def_names | sort -u); do
    printf '%s %s\n' "$n" "$f" >>"$tmp_dir/wt_pairs"
  done
done <"$tmp_dir/wt_files"
sort -u -o "$tmp_dir/wt_pairs" "$tmp_dir/wt_pairs"

# Names defined in the staged content itself (callers of these are fine even
# when the worktree copy has drifted further).
: >"$tmp_dir/staged_names"
while IFS= read -r f; do
  git show ":$f" | def_names >>"$tmp_dir/staged_names" || true
done <"$tmp_dir/staged_add"
sort -u -o "$tmp_dir/staged_names" "$tmp_dir/staged_names"

# Definition heads left in HEAD, excluding files staged for deletion.
head_excludes=()
while IFS= read -r f; do head_excludes+=(":(exclude)$f"); done <"$tmp_dir/staged_del"
git grep -I -h -E "$DEF_HEAD_RE" HEAD -- '*.rs' ${head_excludes[@]+"${head_excludes[@]}"} \
  | def_names \
  | sort -u >"$tmp_dir/head_names" || true

# worktree-only = worktree defs - HEAD-effective defs - staged defs - whitelist
cut -d' ' -f1 "$tmp_dir/wt_pairs" | sort -u >"$tmp_dir/candidate"
printf '%s\n' "${WHITELIST[@]}" | sort -u >"$tmp_dir/whitelist"
comm -23 "$tmp_dir/candidate" "$tmp_dir/head_names" >"$tmp_dir/candidate2"
comm -23 "$tmp_dir/candidate2" "$tmp_dir/staged_names" >"$tmp_dir/candidate3"
comm -23 "$tmp_dir/candidate3" "$tmp_dir/whitelist" >"$tmp_dir/wt_only"

wt_only_count=$(awk 'END { print NR }' "$tmp_dir/wt_only")
if [ "$wt_only_count" -eq 0 ]; then
  echo "head-closure-gate: OK ($staged_count staged .rs file(s) checked; 0 worktree-only symbols in play)"
  exit 0
fi

alt=$(awk '{ printf "%s%s", sep, $0; sep="|" } END { print "" }' "$tmp_dir/wt_only")

gaps=0
while IFS= read -r f; do
  matches=$(git show ":$f" \
    | grep -hoE "(^|[^A-Za-z0-9_])(${alt})(\$|[^A-Za-z0-9_])" \
    | grep -oE '[A-Za-z0-9_]+' \
    | sort -u || true)
  [ -n "$matches" ] || continue
  while IFS= read -r name; do
    dfile=$(awk -v n="$name" '$1 == n { print $2; exit }' "$tmp_dir/wt_pairs")
    printf "closure-gap: %s references '%s' defined only in unstaged %s\n" "$f" "$name" "$dfile"
    gaps=1
  done <<<"$matches"
done <"$tmp_dir/staged_add"

if [ "$gaps" -eq 1 ]; then
  echo "head-closure-gate: FAIL ($staged_count staged .rs file(s) scanned; stage the missing definitions or remove the references)"
  exit 1
fi
echo "head-closure-gate: OK ($staged_count staged .rs file(s) checked; $wt_only_count worktree-only symbol(s) referenced nowhere staged)"
