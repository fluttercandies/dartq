# legs/05-compat.sh — `dartq compat` over two-plus SDK-shaped roots: a
# differentiated per-axis matrix (grant-less SDK incompatible at the
# variance modifier, grant-carrying SDK compatible) plus a broken root as
# its isolated `sdk_unavailable` row — one successful exit.

legs_05_run() {
  local base="$E2E_WORK/compat"
  mkdir -p "$base"

  # The fixture mirrors the CLI face's own e2e shape: package `fixture`
  # pinned at language 3.13, whose `class Foo<in T> {}` needs the
  # variance grant only one axis carries.
  make_pkg "$base/w" "fixture"
  python3 - "$base/w/.dart_tool/package_config.json" <<'PYEOF'
import json, sys
path = sys.argv[1]
config = json.load(open(path))
config["packages"][0]["languageVersion"] = "3.13"
open(path, "w").write(json.dumps(config))
PYEOF
  wf "$base/w/pubspec.lock" 'packages: {}
sdks:
  dart: ">=3.5.0 <4.0.0"
'
  wf "$base/w/lib/a.dart" 'class Foo<in T> {}
'

  # The grant-less SDK (incompatible axis).
  make_stub_sdk "$base/sdks/old" "3.5.0"
  # The grant-carrying SDK (compatible axis): variance granted to the
  # fixture package at the current language version.
  make_stub_sdk "$base/sdks/new" "3.13.1"
  wf "$base/sdks/new/lib/async/async.dart" 'library dart.async;
'
  python3 - "$base/sdks/new/lib/libraries.json" <<'PYEOF'
import json, sys
table = json.load(open(sys.argv[1]))
table["vm"]["libraries"]["async"] = {"uri": "async/async.dart"}
open(sys.argv[1], "w").write(json.dumps(table))
PYEOF
  wf "$base/sdks/new/lib/_internal/allowed_experiments.json" '{
  "version": 1,
  "experimentSets": {"grants": ["variance"]},
  "sdk": {"default": {"experimentSet": "grants"}},
  "packages": {"fixture": {"experimentSet": "grants"}}
}'
  # The broken root: exists, but no version file.
  mkdir -p "$base/sdks/broken"

  leg_begin compat-matrix "two SDK-shaped roots differentiate the matrix; a broken root is isolated"
  run_dartq_in compat-run "$base/w" compat lib/a.dart \
    --sdk-root "$base/sdks/old" --sdk-root "$base/sdks/new" --sdk-root "$base/sdks/broken" \
    --format json
  assert_eq "$DQ_RC" 0 "incompatible axes are the answer's data, not a failure"
  json_check "$(out_file compat-run)" "j['schema_version']==1" "the frozen JSON contract"
  json_check "$(out_file compat-run)" "len(j['matrix'])==3" "one row per --sdk-root, in CLI order"
  json_check "$(out_file compat-run)" \
    "j['matrix'][0]['sdk']=='3.5.0' and j['matrix'][0]['compatible']==False" \
    "the grant-less axis is incompatible"
  json_check "$(out_file compat-run)" \
    "j['matrix'][0]['first_failure']['code']=='experiment_not_enabled_off_by_default'" \
    "the first failure names the variance modifier"
  json_check "$(out_file compat-run)" \
    "j['matrix'][1]['sdk']=='3.13.1' and j['matrix'][1]['compatible']==True" \
    "the grant-carrying axis is compatible"
  json_check "$(out_file compat-run)" \
    "j['matrix'][2]['error']['kind']=='sdk_unavailable'" \
    "the broken root is one isolated sdk_unavailable row"

  # The default axis falls back to $DART_SDK — unset and unflagged is the
  # usage tier.
  leg_begin compat-usage "no axis source at all is the usage tier"
  rc=0
  ( cd "$base/w" && unset DART_SDK && exec "$DARTQ" compat lib/a.dart ) \
    >"$EVIDENCE/compat-noaxis.out" 2>"$EVIDENCE/compat-noaxis.err" || rc=$?
  DQ_RC=$rc
  assert_eq "$DQ_RC" 64 "no --sdk-root, no context, no DART_SDK is usage"
}
