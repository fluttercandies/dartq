//! The multi-context substrate (PRD §16): configuration-forked contexts
//! over one base, per-context overlays, writer-priority scheduling, and
//! epoch-based cancellation.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::Fixture;
use dartq_engine::{
    CancellationToken, ContextConfig, ContextId, EngineConfig, OverlayChange, Runtime, Tier,
    memory_store,
};
use dartq_sdk::SdkSnapshot;

/// Clean base library.
const MAIN_0: &str = "var a = 1;\n";
/// Overlay payload: a syntax error, so the fork is diagnostic-visible.
const MAIN_BROKEN: &str = "void broken( {}\n";
/// Overlay churn payloads (sweep test).
const MAIN_V2: &str = "var a = 2;\n";
const MAIN_V3: &str = "var a = 3;\n";
/// A broken file in the `test/` surface the exclude fork removes from one
/// context's discovery.
const BROKEN_TEST_0: &str = "void broken( {}\n";

fn context_config(fixture: &Fixture, flags: u64) -> ContextConfig {
    ContextConfig {
        engine: EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(&fixture.root.join("sdk"), None).expect("synthetic SDK loads"),
            packages: fixture.packages.clone(),
            pub_config: Arc::clone(&fixture.config),
            config_dir: fixture.config_dir.clone(),
            semdiag: Some(dartq_engine::semdiag::diagnoser()),
            target: dartq_engine::DEFAULT_TARGET.to_owned(),
            declared_variables: rustc_hash::FxHashMap::default(),
            experiments: Vec::new(),
        },
        flags,
    }
}

/// Path-tail diagnostic summary (file-level — enough to pin which files a
/// context reports, without pinning messages).
fn analyze_context(runtime: &mut Runtime, id: ContextId) -> Vec<String> {
    let engine = runtime.context_state_mut(id).expect("open context");
    engine.apply_pending(&mut |_| {});
    let diagnostics = engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze completes");
    let table = engine.file_table();
    diagnostics
        .iter()
        .map(|diagnostic| {
            table
                .path(diagnostic.file)
                .map(path_tail)
                .unwrap_or_default()
        })
        .collect()
}

/// Discover + apply + analyze a context to a settled diagnostic set.
fn settle_context(runtime: &mut Runtime, id: ContextId) -> Vec<String> {
    runtime
        .context_state_mut(id)
        .expect("open context")
        .discover_workspace()
        .expect("discovery reads the fixture");
    analyze_context(runtime, id)
}

fn path_tail(path: &Path) -> String {
    path.to_string_lossy()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The `FileId` a context's engine interned for `path`.
fn file_id(runtime: &Runtime, id: ContextId, path: &Path) -> dartq_source::FileId {
    runtime
        .context_state(id)
        .expect("open context")
        .file_table()
        .try_file_id(path)
        .expect("path interned")
}

/// Two opens with equal configuration are the same context (dedup on
/// insert); any fork dimension moving — here the reserved flag word —
/// opens a distinct one.
#[test]
fn equal_configs_dedup_flag_forks_split() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0)]);
    let mut runtime = Runtime::new();

    let first = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");
    let dedup = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");
    assert_eq!(first, dedup, "equal keys are one context");
    assert_eq!(runtime.len_contexts(), 1);

    let forked = runtime
        .open_context(context_config(&fixture, 1))
        .expect("open");
    assert_ne!(first, forked, "a flag fork is a distinct context");
    assert_eq!(runtime.len_contexts(), 2);
}

/// Two contexts over one base coexist: the one whose options exclude
/// `test/**` never reports the broken test file, the other one does, and
/// neither leaks into the other (PRD §16.2's options fork dimension).
#[test]
fn options_fork_isolates_discovery_and_diagnostics() {
    let fixture = Fixture::new(&[
        ("lib/main.dart", MAIN_0),
        ("test/broken.dart", BROKEN_TEST_0),
    ]);
    let mut runtime = Runtime::new();

    // Context A opens before any options file exists: full discovery.
    let with_tests = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");
    let base_diagnostics = settle_context(&mut runtime, with_tests);
    assert!(
        base_diagnostics.iter().any(|tail| tail == "broken.dart"),
        "context A reports the broken test file: {base_diagnostics:?}"
    );
    assert!(
        !base_diagnostics.iter().any(|tail| tail == "main.dart"),
        "the base library is clean: {base_diagnostics:?}"
    );

    // Context B opens after an exclude lands: the test surface leaves its
    // discovery set, so the broken file never reaches its analysis.
    std::fs::write(
        fixture.root.join("analysis_options.yaml"),
        "analyzer:\n  exclude:\n    - \"test/**\"\n",
    )
    .expect("options file");
    let without_tests = runtime
        .open_context(context_config(&fixture, 1))
        .expect("open");
    assert_ne!(with_tests, without_tests, "the options fork is a new key");
    assert_eq!(runtime.len_contexts(), 2, "both forks coexist");
    let excluded_diagnostics = settle_context(&mut runtime, without_tests);
    assert!(
        !excluded_diagnostics
            .iter()
            .any(|tail| tail == "broken.dart"),
        "context B never sees the excluded file: {excluded_diagnostics:?}"
    );
    assert!(
        !excluded_diagnostics.iter().any(|tail| tail == "main.dart"),
        "context B still analyzes the clean base: {excluded_diagnostics:?}"
    );
}

/// An overlay edit is immediately visible through its owner's read path,
/// never touches the base tree on disk, and stays invisible to a context
/// without the overlay (PRD §16.3).
#[test]
fn overlay_visible_to_owner_only_disk_untouched() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0)]);
    let mut runtime = Runtime::new();
    let owner = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");
    let bystander = runtime
        .open_context(context_config(&fixture, 1))
        .expect("open");
    settle_context(&mut runtime, owner);
    settle_context(&mut runtime, bystander);

    let main = fixture.path("lib/main.dart");
    let disk_bytes = std::fs::read(&main).expect("base bytes");

    let report = runtime
        .apply_overlay(
            owner,
            &[OverlayChange::Set {
                path: main.clone(),
                bytes: MAIN_BROKEN.as_bytes().to_vec(),
            }],
            &mut |_| {},
        )
        .expect("apply");
    assert!(!report.changed.is_empty(), "the overlay edit applied");

    // The base tree is untouched.
    assert_eq!(
        std::fs::read(&main).expect("base bytes"),
        disk_bytes,
        "overlay writes never reach disk"
    );

    // The owner reads and reports overlay bytes; the bystander still reads
    // and reports the base.
    let owner_file = file_id(&runtime, owner, &main);
    let bystander_file = file_id(&runtime, bystander, &main);
    assert_eq!(
        runtime
            .context_state(owner)
            .expect("open")
            .content(owner_file),
        Some(MAIN_BROKEN.as_bytes()),
    );
    assert_eq!(
        runtime
            .context_state(bystander)
            .expect("open")
            .content(bystander_file),
        Some(MAIN_0.as_bytes()),
    );

    let owner_diagnostics = analyze_context(&mut runtime, owner);
    assert!(
        owner_diagnostics.iter().any(|tail| tail == "main.dart"),
        "the owner reports the overlay's syntax error: {owner_diagnostics:?}"
    );
    let bystander_diagnostics = analyze_context(&mut runtime, bystander);
    assert!(
        bystander_diagnostics.is_empty(),
        "the bystander stays on the clean base: {bystander_diagnostics:?}"
    );
}

/// Overlay removal hides the file from the owner's view only.
#[test]
fn overlay_removal_hides_file_from_owner_only() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0)]);
    let mut runtime = Runtime::new();
    let owner = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");
    let bystander = runtime
        .open_context(context_config(&fixture, 1))
        .expect("open");
    settle_context(&mut runtime, owner);
    settle_context(&mut runtime, bystander);
    let main = fixture.path("lib/main.dart");

    let report = runtime
        .apply_overlay(
            owner,
            &[OverlayChange::Remove { path: main.clone() }],
            &mut |_| {},
        )
        .expect("apply");
    assert_eq!(report.removed.len(), 1, "the removal applied");

    let owner_file = file_id(&runtime, owner, &main);
    let bystander_file = file_id(&runtime, bystander, &main);
    assert_eq!(
        runtime
            .context_state(owner)
            .expect("open")
            .content(owner_file),
        None,
        "removed from the owner's view"
    );
    assert_eq!(
        runtime
            .context_state(bystander)
            .expect("open")
            .content(bystander_file),
        Some(MAIN_0.as_bytes()),
        "the bystander still sees the base"
    );
}

/// Every write bumps the context's epoch; tasks enqueued under older epochs
/// drop at the single drain point (PRD §16.6) — whether invalidated by an
/// overlay apply or an explicit cancel.
#[test]
fn stale_epoch_tasks_drop_at_drain() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0)]);
    let mut runtime = Runtime::new();
    let context = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");

    runtime
        .enqueue(context, false, Tier::Normal, 1)
        .expect("enqueue");
    runtime
        .enqueue(context, true, Tier::Normal, 2)
        .expect("enqueue");
    assert_eq!(runtime.epoch(context), Some(0));

    let main = fixture.path("lib/main.dart");
    runtime
        .apply_overlay(
            context,
            &[OverlayChange::Set {
                path: main,
                bytes: MAIN_V2.as_bytes().to_vec(),
            }],
            &mut |_| {},
        )
        .expect("apply");
    assert_eq!(
        runtime.epoch(context),
        Some(1),
        "the write bumped the epoch"
    );

    let fresh = runtime
        .enqueue(context, false, Tier::Normal, 3)
        .expect("enqueue");
    assert_eq!(fresh.epoch, 1, "new tasks carry the new epoch");

    runtime.cancel(context).expect("cancel");
    let last = runtime
        .enqueue(context, false, Tier::Normal, 4)
        .expect("enqueue");
    assert_eq!(last.epoch, 2);

    let report = runtime.drain();
    assert_eq!(report.stale_dropped, 3, "cookies 1, 2, 3 are stale");
    assert_eq!(
        report.tasks,
        vec![dartq_engine::Task {
            context,
            epoch: 2,
            write: false,
            tier: Tier::Normal,
            cookie: 4,
        }],
        "only the newest task survives"
    );
}

/// Writer-priority drain (PRD §16.5): every pending write drains before any
/// read, context-ascending, arrival order within a context — deterministic.
#[test]
fn drain_orders_writes_before_reads() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0)]);
    let mut runtime = Runtime::new();
    let first = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");
    let second = runtime
        .open_context(context_config(&fixture, 1))
        .expect("open");

    runtime
        .enqueue(first, false, Tier::Normal, 101)
        .expect("enqueue");
    runtime
        .enqueue(first, true, Tier::Normal, 102)
        .expect("enqueue");
    runtime
        .enqueue(first, false, Tier::Normal, 103)
        .expect("enqueue");
    runtime
        .enqueue(second, false, Tier::Normal, 201)
        .expect("enqueue");
    runtime
        .enqueue(second, true, Tier::Normal, 202)
        .expect("enqueue");

    let report = runtime.drain();
    assert_eq!(report.stale_dropped, 0);
    let order: Vec<(ContextId, bool, u64)> = report
        .tasks
        .iter()
        .map(|task| (task.context, task.write, task.cookie))
        .collect();
    assert_eq!(
        order,
        vec![
            (first, true, 102),
            (second, true, 202),
            (first, false, 101),
            (first, false, 103),
            (second, false, 201),
        ]
    );
    assert!(
        runtime.drain().tasks.is_empty(),
        "a drain empties every queue"
    );
}

/// The shared overlay store deduplicates bytes across contexts, and sweep
/// reclaims exactly the content no standing overlay references (bounded
/// memory: dead overlay versions leave the store).
#[test]
fn sweep_reclaims_dead_overlay_content_cross_context() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0)]);
    let mut runtime = Runtime::new();
    let owner = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");
    let bystander = runtime
        .open_context(context_config(&fixture, 1))
        .expect("open");
    let main = fixture.path("lib/main.dart");
    let set = |runtime: &mut Runtime, id: ContextId, bytes: &str| {
        runtime
            .apply_overlay(
                id,
                &[OverlayChange::Set {
                    path: main.clone(),
                    bytes: bytes.as_bytes().to_vec(),
                }],
                &mut |_| {},
            )
            .expect("apply");
    };

    set(&mut runtime, owner, MAIN_V2);
    assert_eq!(runtime.stored_contents(), 1);
    assert_eq!(runtime.sweep(), 0, "the live overlay content stays");

    set(&mut runtime, owner, MAIN_V3);
    assert_eq!(runtime.stored_contents(), 2, "v2 is dead but unswept");
    assert_eq!(runtime.sweep(), 1, "the replaced version is reclaimed");
    assert_eq!(runtime.stored_contents(), 1);

    // Content addressing dedups across contexts: the bystander's overlay of
    // the same v2 bytes reoccupies the one content-addressed slot.
    set(&mut runtime, bystander, MAIN_V2);
    assert_eq!(runtime.stored_contents(), 2);
    assert_eq!(runtime.sweep(), 0, "both overlays vouch for their content");

    // Dropping the bystander's overlay makes v2 dead again.
    runtime
        .apply_overlay(
            bystander,
            &[OverlayChange::Remove { path: main.clone() }],
            &mut |_| {},
        )
        .expect("apply");
    assert_eq!(runtime.sweep(), 1);
    assert_eq!(runtime.stored_contents(), 1);

    let owner_file = file_id(&runtime, owner, &main);
    assert_eq!(
        runtime
            .context_state(owner)
            .expect("open")
            .content(owner_file),
        Some(MAIN_V3.as_bytes()),
        "sweep never touches a context's own residency"
    );
}

/// Closing a context invalidates its id, clears its queue, and a reopen of
/// the same configuration is a fresh context (dedup is among live contexts).
#[test]
fn close_context_recycles_cleanly() {
    let fixture = Fixture::new(&[("lib/main.dart", MAIN_0)]);
    let mut runtime = Runtime::new();
    let context = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");
    runtime
        .enqueue(context, false, Tier::Normal, 1)
        .expect("enqueue");

    runtime.close_context(context).expect("close");
    assert_eq!(runtime.len_contexts(), 0);
    assert!(runtime.context_state(context).is_none());
    assert_eq!(runtime.epoch(context), None);
    assert!(runtime.drain().tasks.is_empty(), "the queue cleared");

    assert!(
        runtime.enqueue(context, false, Tier::Normal, 2).is_err(),
        "the closed id no longer addresses anything"
    );
    assert!(
        runtime.close_context(context).is_err(),
        "double close reports unknown"
    );

    // Same configuration reopens — as a fresh context, not a resurrection.
    let reopened = runtime
        .open_context(context_config(&fixture, 0))
        .expect("open");
    assert_eq!(runtime.epoch(reopened), Some(0), "fresh epoch");
    runtime
        .enqueue(reopened, false, Tier::Normal, 3)
        .expect("enqueue");
    let report = runtime.drain();
    assert_eq!(report.stale_dropped, 0);
    assert_eq!(report.tasks.len(), 1);
    assert_eq!(report.tasks[0].cookie, 3);
}
