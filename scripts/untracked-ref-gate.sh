#!/usr/bin/env bash
# untracked-ref-gate.sh — forbid repository content from referencing
# never-tracked local workspace material: vendored reference checkouts, the
# untracked evidence/ledger/charter paths, and absolute home paths. A fresh
# clone has none of these, so every such reference dangles. Two faces are
# scanned: tracked files, and untracked candidates (`git ls-files --others
# --exclude-standard`) — files queued for the next integration commit must
# satisfy the same rules before they land, not after.
#
# The pattern below is written with character classes ([.], [d], [a-z]) so
# this script never literally contains what it forbids — otherwise it would
# flag itself. Matched forms: dot-prefixed vendored checkouts, untracked
# evidence/ledger directory references, the local charter file names, absolute
# scratch ([/]tmp/...) paths recorded in data files, and absolute
# /Users/<user> home paths..gitignore entries and Cargo.lock are
# out of scope (ignore rules legitimately name the paths; lockfiles are
# generated).
#
# Section-mark forms (the second check): internal spec/charter section
# citations (`spec section 19.1`, bare `§2.4`) point at never-tracked
# planning documents and dangle the same way (any `PRD` mention is already
# caught by the main pattern — the charter file is never-tracked, so even a
# bare word-form mention dangles). The digit requirement makes the check
# false-positive-free for external published-spec citations: LSP anchors
# cite by section name (`LSP §cancelRequest`, never a digit), and the only
# digit form in tracked content is the IETF RFC cite (`RFC 3986 §5.2.4`),
# which cites a published standard and is exempted below.
#
# Word-form citations to the never-tracked planning documents (`design doc
# section 4`, `parity plan section 4.2`, `plan section 7`, `ledger section
# 3.2`, `charter section 2`, `spec(ification) section 19`) dangle
# identically, so the check requires a planning-doc name immediately before
# `section|§ <digit>` — doc names are matched case-insensitively on the
# initial letter since they legitimately appear at comment start. A bare
# `section <n>` with no doc-name prefix is NOT flagged: it cannot be
# distinguished from a tracked document's own section, so keeping the
# doc-name anchor is what keeps plain English (`section 3 of the report`)
# exempt while every planning-doc cite is caught.

set -euo pipefail
cd "$(dirname "$0")/.."

PATTERN='[.]third|(^|[^[:alnum:]_./])[d]ocs/|AGENTS[.]md|[P]RD|GOALS[.]md|REF[.]md|[.]goal/|/Users/[a-z]'

# tests/plugin-validation/vendor and tests/corpus/real/*/lib are vendored
# upstream content (Cargo.lock class), not our reference. The corpus form
# needs the trailing /**: a single-* pathspec only matches paths ending at
# .../lib, never the subtree beneath it (wildcarded pathspecs get no
# implicit directory-prefix matching — 1260 vendored corpus files leaked
# through the bare form). tests/plugin-validation/corpora is the same
# upstream-fixed class: its Dart sources are verbatim third-party test
# cases, so a `this.third` constructor parameter there legitimately trips
# the `[.]third` vendored-checkout pattern — identifier rewrites would
# change the tested behavior, so the tree is excluded like the vendor dir
# (a literal directory pathspec already covers its whole subtree; the /**
# caveat above applies only to paths with a wildcarded component).
# The skills/ tree is a shipped user-facing artifact (the agent skill
# package). It legitimately names the public AGENTS.md convention — the
# file it ships (skills/adapters/agents-md/AGENTS.md) is tracked, so the
# reference dangles nowhere. Same class as the vendored-content excludes
# above: the heuristic would false-positive on the convention's own name.
excludes=(':(exclude).gitignore' ':(exclude)scripts/untracked-ref-gate.sh' ':(exclude)Cargo.lock' ':(exclude)tests/plugin-validation/vendor' ':(exclude)tests/plugin-validation/corpora' ':(exclude)tests/corpus/real/*/lib/**' ':(exclude)skills')

# Sanctioned standard-tool position (charter exemption face): the fvm SDK
# default (`~/fvm/...`, or its absolute-user spelling) inside recorded
# oracle/dartq command strings is allowed. Scrub that one token and keep the
# line only if it still matches something else; arbitrary /Users/... paths
# and every other never-tracked reference stay flagged. Shared by the
# tracked and untracked faces.
fvm_scrub() {
  printf '%s\n' "$1" | while IFS= read -r line; do
    scrubbed="$(printf '%s\n' "$line" | sed -E 's#([~]|[/]Users/[a-z][a-z0-9_-]*)[/]fvm/#~/fvm/#g')"
    if printf '%s\n' "$scrubbed" | grep -qE "$PATTERN"; then
      printf '%s\n' "$line"
    fi
  done
}

hits="$(git grep -nE "$PATTERN" -- "${excludes[@]}" || true)"
if [ -n "$hits" ]; then
  hits="$(fvm_scrub "$hits")"
fi
# Scratch provenance in recorded data files (corpus nails, goldens, docs,
# manifests): an embedded absolute scratch path dangles in a fresh clone.
# Scoped to data extensions because code and scripts legitimately use
# runtime scratch that the same file creates and consumes — never provenance.
data_hits="$(git grep -nE '[/]tmp/' -- '*.json' '*.md' '*.toml' '*.yaml' '*.yml' '*.txt' '*.tsv' "${excludes[@]}" || true)"
hits="${hits}${data_hits:+$'\n'${data_hits}}"
# Section-mark citations to internal planning docs: a planning-doc name
# (design doc, parity plan, plan, ledger, charter, spec, specification)
# followed by `section|§ <digit>`, and any bare `§<digit>` not part of a
# published-standard cite (the RFC exemption above). The doc-name anchor
# keeps plain English `section <n>` exempt — see the header.
SECTION_CITE='([Dd]esign[[:space:]_-][Dd]oc|[Pp]arity[[:space:]_-][Pp]lan|[Pp]lan|[Ll]edger|[Cc]harter|[Ss]pec|[Ss]pecification)[[:space:]]+(section|§)[[:space:]]*[0-9]|§[0-9]'
section_hits="$(git grep -nE "$SECTION_CITE" -- "${excludes[@]}" | grep -vE 'RFC [0-9]+ §[0-9]' || true)"
hits="${hits}${section_hits:+$'\n'${section_hits}}"

# Untracked candidate face: `git ls-files --others --exclude-standard` lists
# the files on their way into the repository (the next integration commit);
# their content passes the same rule group. Ignored files drop out of the
# listing naturally; the pathspec excludes below additionally drop the
# never-tracked boundary directories, which legitimately contain every
# forbidden form and are never committed.
untracked_excludes=("${excludes[@]}"
  ':(exclude).third' ':(exclude)docs' ':(exclude)AGENTS.md' ':(exclude)PRD.md'
  ':(exclude)GOALS.md' ':(exclude)REF.md' ':(exclude).goal')

# One xargs batch per face, never per-file spawns (the candidate list is
# ~10k files). The /dev/null operand forces multi-file grep mode so every
# hit line carries its path even for a single candidate, and keeps the
# pipeline well-defined on an empty candidate list.
untracked_main="$(git ls-files -z --others --exclude-standard -- "${untracked_excludes[@]}" \
  | xargs -0 grep -nE "$PATTERN" /dev/null || true)"
untracked_data="$(git ls-files -z --others --exclude-standard -- '*.json' '*.md' '*.toml' '*.yaml' '*.yml' '*.txt' '*.tsv' "${untracked_excludes[@]}" \
  | xargs -0 grep -nE '[/]tmp/' /dev/null || true)"
untracked_section="$(git ls-files -z --others --exclude-standard -- "${untracked_excludes[@]}" \
  | xargs -0 grep -nE "$SECTION_CITE" /dev/null \
  | grep -vE 'RFC [0-9]+ §[0-9]' || true)"
if [ -n "$untracked_main" ]; then
  untracked_main="$(fvm_scrub "$untracked_main")"
fi
untracked_hits="${untracked_main}${untracked_data:+$'\n'${untracked_data}}"
untracked_hits="${untracked_hits}${untracked_section:+$'\n'${untracked_section}}"

if [ -n "$hits" ]; then
  echo "untracked-ref-gate: tracked files reference never-tracked local paths:" >&2
  printf '%s\n' "$hits" >&2
  echo "untracked-ref-gate: rewrite the reference (self-describing prose, an" >&2
  echo "untracked-ref-gate: env var, or the target/evidence root) and rerun." >&2
  exit 1
fi
if [ -n "$untracked_hits" ]; then
  echo "untracked-ref-gate: untracked candidate files reference never-tracked local paths:" >&2
  printf '%s\n' "$untracked_hits" >&2
  echo "untracked-ref-gate: rewrite the reference (self-describing prose, an" >&2
  echo "untracked-ref-gate: env var, or the target/evidence root) and rerun." >&2
  exit 1
fi
echo "untracked-ref-gate: clean"
