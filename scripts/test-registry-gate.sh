#!/bin/sh
# The test-registry gate: every `#[path]` / `mod NAME;` registration in the
# workspace must resolve to a file that exists, every `*_tests.rs` file must
# be mounted by a registration, test-kind targets must declare at least one
# `#[test]`, tracked registries must not reference untracked files, and no
# registration may silently disappear from HEAD.
#
# Failure history this guards: ticket-file registrations dropped four times
# in one session, and HEAD once referenced four `#[path]` modules whose
# files were never committed — the lib-test target did not even compile.
#
# Registration kinds (a registration is any `mod NAME;`/`mod NAME {` in a
# file that carries `#[path]` or `#[cfg(test)]`):
#   path+test  `#[cfg(test)] #[path = "..."] mod NAME;` — a test target
#   cfgtest    `#[cfg(test)] mod NAME;` — convention-resolved test target
#   path       `#[path = "..."] mod NAME;` — a production submodule; must
#              exist but is not expected to hold tests
#
# Checks (each names the offender; any failure exits 1):
#   missing    the target file does not exist
#   empty      a test-kind target declares zero `#[test]` — a shell
#   broken-at-head  a registration in HEAD's copy of a registry resolves to
#              a file HEAD does not contain — a fresh clone cannot compile
#              `cargo test` (the failure that motivated this gate). Pure
#              HEAD-integrity check: worktree-only registrations are
#              uncommitted work by definition and are never flagged here.
#   removed    a `mod NAME` in HEAD's copy of a registry is gone from the
#              worktree copy — silent coverage loss (`--allow-remove`
#              exempts one name; additions never fail)
#   unmounted  a `*_tests.rs` file exists on disk but nothing in the worktree
#              mounts it — no `mod NAME;` by that name, and no `#[path]`
#              target resolving to it. The module never compiles and its
#              tests never run, and nothing else in the tree notices. This
#              is the exact inverse of `missing`: `missing` catches a
#              registration whose file is gone, `unmounted` catches a file
#              whose registration is gone. Eight parallel lanes appending to
#              one registry tail dropped twelve registrations that way.
#              `--allow-unmounted` (or KNOWN_UNMOUNTED below) exempts a
#              deliberately unmounted rig.
#
# Discovery is by grep over crates/ — no crate names are hardcoded, so new
# crates are covered automatically. Inline `mod NAME { ... }` bodies need
# no file check but count toward the coverage table.
#
# Usage:
#   scripts/test-registry-gate.sh [--allow-remove <name>]...
#                                  [--allow-empty <name>]...
#                                  [--allow-unmounted <name>]...
#   --allow-remove <name>     accept one disappeared registration by mod name
#   --allow-empty <name>      accept a zero-#[test] target that is a known
#                             helper rig, by mod or file name. Two rigs are
#                             exempted in the script itself (see KNOWN_RIGS).
#   --allow-unmounted <name>  accept a `*_tests.rs` module nothing mounts, by
#                             mod or file name (see KNOWN_UNMOUNTED).
#
# Runs in about a second: grep + awk + git ls-files/show, no cargo.
# Exit codes: 0 pass · 1 registry violations · 2 usage/environment.

set -eu

cd "$(dirname "$0")/.."
TAB=$(printf '\t')

ALLOW_REMOVE=""
ALLOW_EMPTY=""
ALLOW_UNMOUNTED=""
while [ $# -gt 0 ]; do
    case "$1" in
        --allow-remove)
            [ $# -ge 2 ] || { echo "error: --allow-remove needs a name" >&2; exit 2; }
            ALLOW_REMOVE="$ALLOW_REMOVE
$2"; shift 2 ;;
        --allow-remove=*)
            ALLOW_REMOVE="$ALLOW_REMOVE
${1#--allow-remove=}"; shift ;;
        --allow-empty)
            [ $# -ge 2 ] || { echo "error: --allow-empty needs a file name" >&2; exit 2; }
            ALLOW_EMPTY="$ALLOW_EMPTY
$2"; shift 2 ;;
        --allow-empty=*)
            ALLOW_EMPTY="$ALLOW_EMPTY
${1#--allow-empty=}"; shift ;;
        --allow-unmounted)
            [ $# -ge 2 ] || { echo "error: --allow-unmounted needs a name" >&2; exit 2; }
            ALLOW_UNMOUNTED="$ALLOW_UNMOUNTED
$2"; shift 2 ;;
        --allow-unmounted=*)
            ALLOW_UNMOUNTED="$ALLOW_UNMOUNTED
${1#--allow-unmounted=}"; shift ;;
        -h|--help) sed -n '2,/^set /p' "$0" | sed '$d'; exit 0 ;;
        *) echo "error: unknown argument $1" >&2; exit 2 ;;
    esac
done

git rev-parse --is-inside-work-tree >/dev/null 2>&1 || {
    echo "error: must run inside the git work tree" >&2; exit 2
}

# Known zero-#[test] registrations that are legitimate helper rigs, not
# shells — exempt from the `empty` class by construction (each entry names
# why it is a rig, not a registration that forgot its tests):
#   test_rig             — `diagnose/test_rig.rs`, the shared single-file
#                          diagnosis rig: fixtures/helpers consumed by the
#                          `diagnose/*_tests.rs` callers, which hold the
#                          #[test]s.
#   ticket_2568_tests    — `diagnose/ticket_2568_tests.rs`, the
#                          `ticket_2564` rig's scaffolding module: doc +
#                          helpers only, the #[test]s live in
#                          `ticket_2564_tests.rs`.
KNOWN_RIGS="test_rig
ticket_2568_tests
ticket_2568_tests.rs"

# Known `*_tests.rs` modules that are deliberately mounted by nobody, exempt
# from the `unmounted` class by construction (each entry states why the
# absence of a `mod` is the intended state, not a dropped registration):
#   (empty) — every `*_tests.rs` file in the worktree is expected to be
#             mounted; add `<stem> <file name>` here only with a reason.
KNOWN_UNMOUNTED=""

# --- discover registry files ----------------------------------------------
# A file is a registry when it contains a `#[path]` attribute or a
# `#[cfg(test)]` attribute followed by a `mod NAME;` declaration.
REG_FILES=$(git grep -l '#\[path' -- 'crates/*.rs' || true)
CFG_FILES=$(git grep -lE '#\[cfg\(test\)\]' -- 'crates/*.rs' || true)
if [ -n "$CFG_FILES" ]; then
    CFG_MOD_FILES=$(printf '%s\n' "$CFG_FILES" | xargs grep -lE '^\s*mod [a-z_0-9]+;' 2>/dev/null || true)
else
    CFG_MOD_FILES=""
fi
REG_FILES=$(printf '%s\n%s\n' "$REG_FILES" "$CFG_MOD_FILES" | sed '/^$/d' | sort -u)

# --- extract registrations ------------------------------------------------
# Emit TSV: registry \t kind \t mod_name \t path(or -) \t inline(0|1)
# kind: path+test | cfgtest | path | other (a bare `mod NAME;` in a registry
# file — module structure, not a checked registration, but still tracked
# for the HEAD diff so a deleted submodule decl is seen).
extract_all() {
    awk '
        function flush(    kind) {
            if (name == "") { has_path = 0; path = ""; cfg = 0; return }
            if (has_path && cfg)      kind = "path+test"
            else if (has_path)        kind = "path"
            else if (cfg)             kind = "cfgtest"
            else                      kind = "other"
            p = (path == "") ? "-" : path
            printf "%s\t%s\t%s\t%s\t%d\n", FILENAME, kind, name, p, inline
            name = ""; has_path = 0; path = ""; cfg = 0; inline = 0
        }
        match($0, /#\[path[[:space:]]*=[[:space:]]*"/) {
            rest = substr($0, RSTART + RLENGTH)
            q = index(rest, "\"")
            if (q > 0) { has_path = 1; path = substr(rest, 1, q - 1) }
        }
        /#\[cfg\(test\)\]/ { cfg = 1 }
        FNR == 1 { name = ""; has_path = 0; path = ""; cfg = 0; inline = 0 }
        match($0, /^[[:space:]]*(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?mod[[:space:]]+[A-Za-z_][A-Za-z_0-9]*[[:space:]]*[;{]/) {
            seg = substr($0, RSTART, RLENGTH)
            n = seg
            sub(/.*mod[[:space:]]+/, "", n); sub(/[[:space:]]*[;{].*/, "", n)
            name = n
            inline = (seg ~ /\{/) ? 1 : 0
            flush()
        }
    ' "$@"
}

TRACKED=$(git ls-files -- 'crates/' | sort)
AT_HEAD=$(git ls-tree -r HEAD --name-only -- 'crates/' | sort)
TMPD="${TMPDIR:-/tmp}/trg.$$"; mkdir -p "$TMPD"
trap 'rm -rf "$TMPD"' EXIT

# --- per-registration checks ----------------------------------------------
# One awk pass over every registry file that exists on disk (FILENAME
# carries the registry; a registry deleted outright is caught by the HEAD
# diff, not here).
EXISTING_REG_FILES=$(printf '%s\n' "$REG_FILES" | while IFS= read -r r; do
    [ -n "$r" ] && [ -f "$r" ] && printf '%s\n' "$r"; done)
# shellcheck disable=SC2086
extract_all $EXISTING_REG_FILES > "$TMPD/regs.tsv"

# Resolve every target path in one awk pass (no per-row forks): emit
# resolved rows plus the candidate target paths for existence/test-count
# batching. Convention candidates (stem/NAME.rs, stem/NAME/mod.rs,
# d/NAME.rs) are all emitted when there is no #[path]; the join phase
# picks the first that exists, or the default for the miss report.
awk -F'\t' -v OFS='\t' '
    function dirname(p,  q) { q = p; sub(/\/[^\/]*$/, "", q); return q }
    function basename(p,  q) { q = p; sub(/.*\//, "", q); return q }
    function norm(p) {
        gsub(/\/+/, "/", p)
        while (sub(/\/[^\/]+\/\.\.\//, "/", p)) {}
        sub(/\/[^\/]+\/\.\.$/, "", p)
        gsub(/\/\.\//, "/", p); sub(/^\.\//, "", p)
        return p
    }
    function stem_of(reg,  d, b) {
        d = dirname(reg); b = basename(reg); sub(/\.rs$/, "", b)
        return (b == "lib" || b == "main" || b == "mod") ? d : d "/" b
    }
    $2 == "other" || $3 == "" { next }
    {
        reg = $1; kind = $2; name = $3; path = $4; inline = $5 + 0
        if (inline) {
            print reg, kind, name, "(inline)", inline > RESOLVED
            print reg > INLINE_HOSTS
            next
        }
        if (path != "-") {
            t = (path ~ /^\//) ? path : dirname(reg) "/" path
            t = norm(t)
            print reg, kind, name, t, inline > RESOLVED
            print t
            next
        }
        # convention target: emit every candidate, first-existing wins later
        stem = stem_of(reg); d = dirname(reg)
        c1 = stem "/" name ".rs"; c2 = stem "/" name "/mod.rs"; c3 = d "/" name ".rs"
        print reg, kind, name, c1 SUBSEP c2 SUBSEP c3, inline > RESOLVED
        print c1; print c2; print c3
    }
' RESOLVED="$TMPD/resolved.tsv" INLINE_HOSTS="$TMPD/inline_hosts.txt" \
    "$TMPD/regs.tsv" > "$TMPD/candidates.txt"

# Existence of every unique candidate in one pass (shell builtin, no fork
# per file).
: > "$TMPD/existing.txt"
while IFS= read -r t; do
    [ -f "$t" ] && printf '%s\n' "$t"
done < "$TMPD/candidates.txt" | sort -u > "$TMPD/existing.txt"

# Count #[test] in every existing candidate in one grep batch (file:count
# output with multiple files), plus #[test] in each inline-mod host file.
: > "$TMPD/test_counts.txt"
if [ -s "$TMPD/existing.txt" ]; then
    xargs grep -Hc '#\[test\]' < "$TMPD/existing.txt" 2>/dev/null \
        | sed 's/:\([0-9][0-9]*\)$/\t\1/' > "$TMPD/test_counts.txt" || true
fi
if [ -s "$TMPD/inline_hosts.txt" ]; then
    sort -u "$TMPD/inline_hosts.txt" \
        | xargs grep -Hc '#\[test\]' 2>/dev/null \
        | sed 's/:\([0-9][0-9]*\)$/\t\1/' >> "$TMPD/test_counts.txt" || true
fi

# Join in one awk pass: resolved rows + counts + tracked + at-head +
# exemptions. Emits miss/empty/untracked/table. Convention candidates are
# SUBSEP-joined; the first that exists wins, else the first stands in the
# miss report.
printf '%s\n' "$TRACKED" > "$TMPD/tracked.txt"
printf '%s\n' "$AT_HEAD" > "$TMPD/at_head.txt"
printf '%s\n%s\n' "$KNOWN_RIGS" "$ALLOW_EMPTY" | sed '/^$/d' > "$TMPD/allow_empty.txt"
awk -F'\t' \
    -v miss="$TMPD/miss.tsv" -v empty="$TMPD/empty.tsv" \
    -v table="$TMPD/table.tsv" \
    -v allowe_file="$TMPD/allow_empty.txt" \
    -v existing_file="$TMPD/existing.txt" -v counts="$TMPD/test_counts.txt" '
    function basename(p,  q) { q = p; sub(/.*\//, "", q); return q }
    function is_test_target(kind, name, base) {
        if (kind == "path") return 0
        if (base ~ /common|fixture|^mod\.rs$/) return 0
        if (base ~ /_tests\.rs$|_pins\.rs$|^tests\.rs$/) return 1
        if (name ~ /fixture|common/) return 0
        if (name ~ /_tests$|_pins$|^tests$|^test$|^tests_|^test_|sealed_contract_tests/) return 1
        return 0
    }
    FNR == NR { # existing.txt
        existing[$1] = 1; next
    }
    FILENAME == counts { tc[$1] = $2 + 0; next }
    {
        reg = $1; kind = $2; name = $3; f4 = $4
        if (f4 == "(inline)") {
            cnt = tc[reg] + 0
            printf "%s\t%s\t%s\t%s\n", reg, name, "(inline)", cnt > table
            next
        }
        # pick target: single path or first-existing convention candidate
        nc = split(f4, cand, "\034")
        target = ""
        for (i = 1; i <= nc; i++) if (cand[i] in existing) { target = cand[i]; break }
        if (target == "") target = cand[1]  # miss: report the default shape
        if (!(target in existing)) {
            printf "%s\t%s\t%s\n", reg, name, target > miss; next
        }
        cnt = tc[target] + 0
        base = basename(target)
        if (is_test_target(kind, name, base) && cnt == 0 \
            && !(base in allowE) && !(name in allowE))
            printf "%s\t%s\t%s\n", reg, name, target > empty
        printf "%s\t%s\t%s\t%s\n", reg, name, target, cnt > table
    }
    BEGIN {
        while ((getline l < allowe_file) > 0) allowE[l] = 1
        close(allowe_file)
    }
    ' "$TMPD/existing.txt" "$TMPD/test_counts.txt" "$TMPD/resolved.tsv"

# --- HEAD integrity: broken-at-head ----------------------------------------
# The failure this class exists for: HEAD's copy of a registry names a
# module HEAD does not contain, so a fresh clone cannot compile `cargo
# test`. Extract the registrations from `git show HEAD:<registry>` (each
# registry file that exists at HEAD and carries `#[path]` or
# `#[cfg(test)]`), resolve each target with the same rules as the worktree
# side, and assert the resolved path is itself in HEAD (`git ls-tree`).
# Worktree-only registrations are uncommitted work — never flagged here.
# `git grep -l <pat> HEAD` prints `HEAD:<path>` lines — strip the ref so the
# result is a repo-relative path usable with `git show`/`ls-tree`.
HEAD_REG_FILES=$(
    {
        git grep -l '#\[path' HEAD -- 'crates/*.rs' 2>/dev/null || true
        git grep -lE '#\[cfg\(test\)\]' HEAD -- 'crates/*.rs' 2>/dev/null || true
    } | sed -e 's/^HEAD://' -e '/^$/d' | sort -u
)
mkdir -p "$TMPD/hr"
: > "$TMPD/hr_map.tsv"
i=0
printf '%s\n' "$HEAD_REG_FILES" | while IFS= read -r reg; do
    [ -n "$reg" ] || continue
    git show "HEAD:$reg" > "$TMPD/hr/$i.rs" 2>/dev/null || continue
    printf '%s\t%s\n' "$TMPD/hr/$i.rs" "$reg" >> "$TMPD/hr_map.tsv"
    i=$((i + 1))
done
# Extract registrations from the HEAD snapshots (temp filenames), then
# translate temp path -> repo path via hr_map.
if [ -s "$TMPD/hr_map.tsv" ]; then
    # shellcheck disable=SC2086
    extract_all $(cut -f1 "$TMPD/hr_map.tsv" | tr '\n' ' ') \
        | awk -F'\t' -v OFS='\t' '
            NR == FNR { map[$1] = $2; next }
            { $1 = map[$1]; print }
        ' "$TMPD/hr_map.tsv" - > "$TMPD/head_regs.tsv" 2>/dev/null || true
    # Resolve each HEAD registration's target(s); existence = in AT_HEAD.
    awk -F'\t' -v OFS='\t' '
        function dirname(p,  q) { q = p; sub(/\/[^\/]*$/, "", q); return q }
        function basename(p,  q) { q = p; sub(/.*\//, "", q); return q }
        function norm(p) {
            gsub(/\/+/, "/", p)
            while (sub(/\/[^\/]+\/\.\.\//, "/", p)) {}
            sub(/\/[^\/]+\/\.\.$/, "", p)
            gsub(/\/\.\//, "/", p); sub(/^\.\//, "", p)
            return p
        }
        function stem_of(reg,  d, b) {
            d = dirname(reg); b = basename(reg); sub(/\.rs$/, "", b)
            return (b == "lib" || b == "main" || b == "mod") ? d : d "/" b
        }
        $2 == "other" || $3 == "" { next }
        {
            reg = $1; name = $3; path = $4; inline = $5 + 0
            if (inline) next  # inline mods need no file at HEAD either
            if (path != "-") {
                t = (path ~ /^\//) ? path : dirname(reg) "/" path
                print reg, name, norm(t)
                next
            }
            stem = stem_of(reg); d = dirname(reg)
            print reg, name, stem "/" name ".rs" SUBSEP stem "/" name "/mod.rs" SUBSEP d "/" name ".rs"
        }
    ' "$TMPD/head_regs.tsv" > "$TMPD/head_targets.tsv"
    # Check each candidate set against at_head: first candidate in HEAD
    # wins; none -> broken-at-head on the default (first) candidate.
    awk -F'\t' -v OFS='\t' -v out="$TMPD/broken.tsv" '
        NR == FNR { athead[$1] = 1; next }
        {
            reg = $1; name = $2; nc = split($3, cand, "\034")
            ok = 0
            for (i = 1; i <= nc; i++) if (cand[i] in athead) { ok = 1; break }
            if (!ok) printf "%s\t%s\t%s\n", reg, name, cand[1] > out
        }
    ' "$TMPD/at_head.txt" "$TMPD/head_targets.tsv"
fi

# --- HEAD diff: disappeared registrations ----------------------------------
# HEAD-side mod names come from one `git grep` at HEAD over every .rs file —
# it sees `mod NAME;`/`mod NAME {` in all of them, including any registry
# deleted outright (all its registrations are losses, not silence).
git grep -hE 'mod[[:space:]]+[A-Za-z_][A-Za-z_0-9]*[[:space:]]*[;{]' HEAD -- 'crates/*.rs' 2>/dev/null \
    | awk '
        match($0, /mod[[:space:]]+[A-Za-z_][A-Za-z_0-9]*[[:space:]]*[;{]/) {
            n = substr($0, RSTART, RLENGTH)
            sub(/.*mod[[:space:]]+/, "", n); sub(/[[:space:]]*[;{].*/, "", n)
            print n
        }' | sort -u > "$TMPD/head_mods.txt"

# Worktree side is the symmetric sweep: every `mod NAME` in every .rs file
# that exists on disk (tracked AND untracked — a mod moved into an
# uncommitted file is still present, which is exactly the #3482 hazard
# this diff must not misreport as a removal).
git ls-files -co --exclude-standard -- 'crates/*.rs' \
    | xargs grep -hE 'mod[[:space:]]+[A-Za-z_][A-Za-z_0-9]*[[:space:]]*[;{]' 2>/dev/null \
    | awk '
        match($0, /mod[[:space:]]+[A-Za-z_][A-Za-z_0-9]*[[:space:]]*[;{]/) {
            n = substr($0, RSTART, RLENGTH)
            sub(/.*mod[[:space:]]+/, "", n); sub(/[[:space:]]*[;{].*/, "", n)
            print n
        }' | sort -u > "$TMPD/wt_mods.txt"

comm -23 "$TMPD/head_mods.txt" "$TMPD/wt_mods.txt" > "$TMPD/gone_mods.txt"
comm -13 "$TMPD/head_mods.txt" "$TMPD/wt_mods.txt" > "$TMPD/new_mods.txt"

# --- missing registration: a *_tests.rs module nobody mounts ---------------
# The inverse of `missing`. A test module file on disk that no `mod NAME;`
# in the worktree declares (and that no `#[path]` target resolves to) never
# compiles, so its tests never run — a failure with no other symptom, which
# is why it survived eight parallel lanes and a manual ledger cross-check.
# Mount is matched by stem: `mod NAME;` anywhere in crates/, or any
# registration `#[path]` whose basename is `NAME.rs`. Ticket stems are
# globally unique in this repo, so a name match is a real mount. Only files
# under a crate's `src/` tree count: a `*_tests.rs` under `tests/` is an
# integration target Cargo discovers on its own and needs no `mod`.
git ls-files -co --exclude-standard -- 'crates/*_tests.rs' \
    | grep '/src/' | sort -u > "$TMPD/test_files.txt"
sed 's#^.*/##; s#\.rs$##' "$TMPD/test_files.txt" | sort -u > "$TMPD/test_stems.txt"
{
    cat "$TMPD/wt_mods.txt"
    awk -F'\t' '$4 != "-" && $4 != "" { print $4 }' "$TMPD/regs.tsv" \
        | sed 's#^.*/##; s#\.rs$##'
} | sort -u > "$TMPD/mounted_stems.txt"
comm -23 "$TMPD/test_stems.txt" "$TMPD/mounted_stems.txt" > "$TMPD/unmounted_stems.txt"

# Join the unmounted stems back to their file paths, then split off the
# exemptions (KNOWN_UNMOUNTED + --allow-unmounted, by stem or file name).
awk -F'\t' -v OFS='\t' '
    NR == FNR { off[$1] = 1; next }
    {
        stem = $1; sub(/^.*\//, "", stem); sub(/\.rs$/, "", stem)
        if (stem in off) print $1, stem
    }
' "$TMPD/unmounted_stems.txt" "$TMPD/test_files.txt" > "$TMPD/unmounted_all.tsv"
printf '%s\n%s\n' "$KNOWN_UNMOUNTED" "$ALLOW_UNMOUNTED" | sed '/^$/d' \
    | sort -u > "$TMPD/allow_unmounted.txt"
: > "$TMPD/unmounted.tsv"
: > "$TMPD/unmounted_allowed.txt"
awk -F'\t' -v allow="$TMPD/allow_unmounted.txt" \
    -v allowed="$TMPD/unmounted_allowed.txt" -v denied="$TMPD/unmounted.tsv" '
    BEGIN { while ((getline l < allow) > 0) ok[l] = 1; close(allow) }
    {
        file = $1; stem = $2
        if ((stem in ok) || (file in ok)) printf "  allowed  %s\n", file > allowed
        else print file "\t" stem > denied
    }
' "$TMPD/unmounted_all.tsv"

# --- report -----------------------------------------------------------------
crate_of() { printf '%s\n' "$1" | cut -d/ -f2; }
report_tsv() {
    file="$1"
    [ -f "$file" ] || return 0
    while IFS="$TAB" read -r reg name target; do
        printf '  %-58s %s -> %s\n' "$reg" "$name" "$target"
    done < "$file"
}

for f in miss empty broken; do
    if [ -f "$TMPD/$f.tsv" ]; then
        case "$f" in
            miss)   echo "missing target files (registration resolves to nothing):" ;;
            empty)  echo "empty registrations (target declares zero #[test]):" ;;
            broken) echo "broken at HEAD (registered module HEAD does not contain):" ;;
        esac
        report_tsv "$TMPD/$f.tsv"
    fi
done

if [ -s "$TMPD/gone_mods.txt" ]; then
    echo "disappeared registrations (in HEAD, gone from the worktree):"
    while IFS= read -r name; do
        if printf '%s\n' "$ALLOW_REMOVE" | grep -qxF "$name"; then
            printf '  allowed  %s\n' "$name"
        else
            printf '  removed  %s\n' "$name"
            echo "$name" >> "$TMPD/denied.txt"
        fi
    done < "$TMPD/gone_mods.txt"
fi
if [ -s "$TMPD/new_mods.txt" ]; then
    newc=$(wc -l < "$TMPD/new_mods.txt" | tr -d ' ')
    echo "new registrations since HEAD: $newc (additions are not failures)"
fi
if [ -s "$TMPD/unmounted.tsv" ]; then
    echo "missing registration (a *_tests.rs module exists but nothing mounts it):"
    while IFS="$TAB" read -r file stem; do
        printf '  %-58s %s\n' "$stem" "$file"
    done < "$TMPD/unmounted.tsv"
fi
if [ -s "$TMPD/unmounted_allowed.txt" ]; then
    echo "deliberately unmounted (exempt by KNOWN_UNMOUNTED/--allow-unmounted):"
    cat "$TMPD/unmounted_allowed.txt"
fi

echo ""
echo "=== test registry coverage ==="
printf '%-16s %-58s %5s %6s\n' crate registry regs tests
if [ -f "$TMPD/table.tsv" ]; then
    awk -F'\t' '{print $1}' "$TMPD/table.tsv" | sort -u | while IFS= read -r reg; do
        regs=$(awk -F'\t' -v r="$reg" '$1==r' "$TMPD/table.tsv" | wc -l | tr -d ' ')
        tests=$(awk -F'\t' -v r="$reg" '$1==r {s+=$4} END {print s+0}' "$TMPD/table.tsv")
        printf '%-16s %-58s %5s %6s\n' "$(crate_of "$reg")" "$reg" "$regs" "$tests"
    done
fi

missing=$( [ -f "$TMPD/miss.tsv" ]   && wc -l < "$TMPD/miss.tsv"   | tr -d ' ' || echo 0)
empty=$(   [ -f "$TMPD/empty.tsv" ]  && wc -l < "$TMPD/empty.tsv"  | tr -d ' ' || echo 0)
broken=$(  [ -f "$TMPD/broken.tsv" ] && wc -l < "$TMPD/broken.tsv" | tr -d ' ' || echo 0)
removed=$( [ -f "$TMPD/denied.txt" ] && wc -l < "$TMPD/denied.txt" | tr -d ' ' || echo 0)
unmounted=$( [ -f "$TMPD/unmounted.tsv" ] && wc -l < "$TMPD/unmounted.tsv" | tr -d ' ' || echo 0)

echo ""
if [ "$missing" -eq 0 ] && [ "$empty" -eq 0 ] && [ "$broken" -eq 0 ] \
    && [ "$removed" -eq 0 ] && [ "$unmounted" -eq 0 ]; then
    total_regs=$(wc -l < "$TMPD/table.tsv" 2>/dev/null | tr -d ' ' || echo 0)
    total_tests=$(awk -F'\t' '{s+=$4} END {print s+0}' "$TMPD/table.tsv" 2>/dev/null || echo 0)
    echo "test-registry gate: PASS ($total_regs registrations, $total_tests tests)"
    exit 0
fi
echo "test-registry gate: FAIL -- missing=$missing empty=$empty broken-at-head=$broken" \
    "removed=$removed unmounted=$unmounted"
exit 1
