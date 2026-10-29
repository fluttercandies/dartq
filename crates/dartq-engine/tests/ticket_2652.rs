//! The #2652 pins: the reference surface answers a name with that name's
//! rows only — the name-keyed run group, never a neighboring group's —
//! whatever the StringId allocation order of the session. The fixture
//! mirrors the shape that surfaced the defect: an interface with sibling
//! members (several sharing long prefixes), an implementer, callers
//! reaching the members through interface-typed receivers across library
//! borders, and a test-double implementer beside its call sites. Every
//! sibling name is queried and every answer row must spell its own
//! queried name at its own range.

mod common;

use common::Fixture;
use dartq_engine::{CancellationToken, Engine, ReferenceKind, SymbolAnswer};

/// The data library: the interface with the sibling members and the
/// concrete implementer.
const REPO: &str = concat!(
    "abstract class TodoRepositoryClient {\n",
    "  Future<Task> createTask({required String title});\n",
    "  Future<List<Task>> fetchTasks(String filter);\n",
    "  Future<Set<String>> fetchTags();\n",
    "  Future<String> createTag(String name);\n",
    "  Future<void> setTaskCompleted(String id);\n",
    "  Future<void> setTasksCompleted(List<String> ids);\n",
    "}\n",
    "class Task {}\n",
    "class TodoRepository implements TodoRepositoryClient {\n",
    "  @override\n",
    "  Future<Task> createTask({required String title}) async => Task();\n",
    "  @override\n",
    "  Future<List<Task>> fetchTasks(String filter) async => const [];\n",
    "  @override\n",
    "  Future<Set<String>> fetchTags() async => const {};\n",
    "  @override\n",
    "  Future<String> createTag(String name) async => name;\n",
    "  @override\n",
    "  Future<void> setTaskCompleted(String id) async {}\n",
    "  @override\n",
    "  Future<void> setTasksCompleted(List<String> ids) async {}\n",
    "}\n",
);

/// The service library: interface-typed field and constructor parameter
/// (the type uses beside the member sends), one call site per sibling —
/// plus an alt-typed receiver resolving the same member to a DIFFERENT
/// declaring file, so `createTask` spans two runs of one surface (the
/// multi-run shape a neighboring name-group leak rides on).
const SERVICE: &str = concat!(
    "import '../data/todo_repository.dart';\n",
    "import 'alt_repository.dart';\n",
    "class TodoAppService {\n",
    "  final TodoRepositoryClient _repository;\n",
    "  final AltTodoRepository _alt;\n",
    "  TodoAppService({required TodoRepositoryClient repository})\n",
    "      : _repository = repository,\n",
    "        _alt = AltTodoRepository();\n",
    "  Future<Task> create(String title) => _repository.createTask(title: title);\n",
    "  Future<List<Task>> list(String filter) => _repository.fetchTasks(filter);\n",
    "  Future<Set<String>> tags() => _repository.fetchTags();\n",
    "  Future<String> tag(String name) => _repository.createTag(name);\n",
    "  Future<void> complete(String id) => _repository.setTaskCompleted(id);\n",
    "  Future<void> completeAll(List<String> ids) => _repository.setTasksCompleted(ids);\n",
    "  Future<Task> altCreate(String title) => _alt.createTask(title: title);\n",
    "}\n",
);

/// A second implementer in a different library: every sibling name now
/// spans two declaring files, so the queried name's answer is sensitive
/// to a neighboring name-group's run leaking in — whatever target file
/// the leaked run carries, the declaring-file filter passes it.
const ALT: &str = concat!(
    "import '../data/todo_repository.dart';\n",
    "class AltTodoRepository implements TodoRepositoryClient {\n",
    "  @override\n",
    "  Future<Task> createTask({required String title}) async => Task();\n",
    "  @override\n",
    "  Future<List<Task>> fetchTasks(String filter) async => const [];\n",
    "  @override\n",
    "  Future<Set<String>> fetchTags() async => const {};\n",
    "  @override\n",
    "  Future<String> createTag(String name) async => name;\n",
    "  @override\n",
    "  Future<void> setTaskCompleted(String id) async {}\n",
    "  @override\n",
    "  Future<void> setTasksCompleted(List<String> ids) async {}\n",
    "}\n",
);

/// The test library: a failing double implementing the interface beside
/// plain-receiver call sites of the siblings.
const SERVICE_TEST: &str = concat!(
    "import 'package:fixture/src/data/todo_repository.dart';\n",
    "class _FailingTodoRepository implements TodoRepositoryClient {\n",
    "  @override\n",
    "  Future<Task> createTask({required String title}) async => throw 'x';\n",
    "  @override\n",
    "  noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);\n",
    "}\n",
    "void run() async {\n",
    "  final repository = _FailingTodoRepository();\n",
    "  await repository.createTask(title: 'a');\n",
    "  await repository.fetchTasks('f');\n",
    "  await repository.fetchTags();\n",
    "  await repository.createTag('n');\n",
    "  await repository.setTaskCompleted('id');\n",
    "  await repository.setTasksCompleted(['id']);\n",
    "}\n",
);

/// The six sibling member names every query face below walks.
const SIBLINGS: [&str; 6] = [
    "createTask",
    "fetchTasks",
    "fetchTags",
    "createTag",
    "setTaskCompleted",
    "setTasksCompleted",
];

/// The workspace of the shape above.
fn workspace() -> Fixture {
    Fixture::new(&[
        ("lib/src/data/todo_repository.dart", REPO),
        ("lib/src/app/todo_app_service.dart", SERVICE),
        ("lib/src/app/alt_repository.dart", ALT),
        ("test/todo_app_service_test.dart", SERVICE_TEST),
    ])
}

/// One reference row as the comparable `(file tail, offset, length, kind)`
/// tuple.
fn row_shapes(engine: &Engine, answer: &SymbolAnswer<'_>) -> Vec<(String, u32, u32, &'static str)> {
    answer
        .refs
        .iter()
        .map(|hit| {
            let tail = engine
                .file_table()
                .path(hit.file)
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            let kind = match hit.kind {
                ReferenceKind::Referenced => "referenced",
                ReferenceKind::Read => "read",
                ReferenceKind::Invoked => "invoked",
                ReferenceKind::Written => "written",
                ReferenceKind::ReadWritten => "read_written",
                ReferenceKind::Override => "override",
            };
            (tail, hit.range.offset, hit.range.length, kind)
        })
        .collect()
}

/// Every sibling's answer rows spell the queried name at their range —
/// the cross-name invariant over the whole serving chain (fold mint,
/// library surface, name-keyed run groups, workspace join), whatever
/// order the session's StringId allocation gave the names.
#[test]
fn sibling_answers_spell_their_own_name() {
    let fixture = workspace();
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");
    let sources: Vec<(String, String)> = [
        "lib/src/data/todo_repository.dart",
        "lib/src/app/todo_app_service.dart",
        "lib/src/app/alt_repository.dart",
        "test/todo_app_service_test.dart",
    ]
    .iter()
    .map(|path| {
        (
            path.rsplit('/').next().expect("tail").to_owned(),
            std::fs::read_to_string(fixture.path(path)).expect("fixture file"),
        )
    })
    .collect();
    for sibling in SIBLINGS {
        let answer = engine.query_symbols(sibling);
        let rows = row_shapes(&engine, &answer);
        assert!(
            rows.iter().any(|(tail, _, _, kind)| {
                tail == "todo_app_service.dart" && *kind == "invoked"
            }),
            "{sibling}: the service library's send must be among the rows"
        );
        for (tail, offset, length, _kind) in &rows {
            let source = sources
                .iter()
                .find(|(name, _)| name == tail)
                .map(|(_, text)| text)
                .unwrap_or_else(|| panic!("{sibling}: source of {tail} not found"));
            let units = source.encode_utf16().collect::<Vec<_>>();
            let start = *offset as usize;
            let end = start + *length as usize;
            let text = if end <= units.len() {
                String::from_utf16_lossy(&units[start..end])
            } else {
                String::new()
            };
            assert_eq!(
                text, sibling,
                "{sibling}: the row at {tail}:{offset} spells `{text}` — cross-name"
            );
        }
    }
}

/// A re-query over the standing analyze serves the same rows — the
/// output-determinism invariant over the member surface.
#[test]
fn requery_serves_identical_rows() {
    let fixture = workspace();
    let mut engine = fixture.engine();
    engine.discover_workspace().expect("discovery");
    engine
        .analyze(&[], &mut |_| {}, &CancellationToken::default())
        .expect("analyze");
    for sibling in SIBLINGS {
        let first = engine.query_symbols(sibling);
        let rows_a = row_shapes(&engine, &first);
        let second = engine.query_symbols(sibling);
        let rows_b = row_shapes(&engine, &second);
        assert_eq!(
            rows_a, rows_b,
            "{sibling}: a re-query over the standing analyze is stable"
        );
    }
}
