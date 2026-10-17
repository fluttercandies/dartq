import 'package:sqlite3/sqlite3.dart';

/// This entire file was an elaborate hack to workaround https://github.com/simolus3/drift/issues/835.
///
/// Users were running into database deadlocks after (stateless) hot restarts
/// in Flutter when they use transactions. The problem is that we didn't use to
/// have a chance to call `sqlite3_close` before a Dart VM restart, the Dart
/// object is just gone without a trace. This means that we're leaking sqlite3
/// database connections on restarts.
/// Even worse, those connections might have a lock on the database, for
/// instance if they just started a transaction.
///
/// Previous drift versions would use an in-memory sqlite3 database stored in a
/// global variable to survive restarts.
///
/// Starting from version 3.0.0 of `package:sqlite3`, we instead attach a native
/// finalizer to databases and don't need this anymore.
@Deprecated('No longer necessary, databases are closed with native finalizers.')
DatabaseTracker tracker(Sqlite3 sqlite3) {
  return _instance ??= DatabaseTracker(sqlite3);
}

DatabaseTracker? _instance;

/// Internal class that we don't export to drift users. See [tracker] for why
/// this is necessary.
class DatabaseTracker {
  /// Creates a new tracker with necessary tables.
  DatabaseTracker(Sqlite3 sqlite3);

  /// Tracks the [openedDb]. The [path] argument can be used to track the path
  /// of that database, if it's bound to a file.
  void markOpened(String path, Database openedDb) {}

  /// Marks the database [db] as closed.
  void markClosed(Database db) {}

  /// Closes tracked database connections.
  void closeExisting() {}
}
