// avoid_slow_async_io — differential corpus (#1160).
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/avoid_slow_async_io_test.dart (transcribed
// 2026-10-14, vendored SDK 3.15 main face).
// Inventory: 11 POSITIVE + 11 NEGATIVE = all 22 upstream tests, none
// skipped. Upstream cases each compiled standalone; here the dart:io
// import is hoisted once and declarations carry block-suffixed names
// (the rule matches resolved member invocations, so renames are
// face-neutral).
// Expected rows (oracle leg): 11 — AVOID_SLOW_ASYNC_IO keyed
// avoid_slow_async_io.dart, one per POSITIVE block.

import 'dart:io';

// POSITIVE upstream test_directory_exists (1 row at `dir.exists()`).
void fDirectoryExists(Directory dir) async {
  await dir.exists();
}

// POSITIVE upstream test_directory_exists_extensionType_implements (1 row
// at `dir.exists()`; the member resolves through the representation).
extension type ExtDirExists(Directory dir) implements Directory {}

void fExtDirExists(ExtDirExists dir) async {
  await dir.exists();
}

// NEGATIVE upstream test_directory_existsSync.
void fDirectoryExistsSync(Directory dir) async {
  dir.existsSync();
}

// POSITIVE upstream test_directory_stat (1 row at `dir.stat()`).
void fDirectoryStat(Directory dir) async {
  await dir.stat();
}

// NEGATIVE upstream test_directory_statSync.
void fDirectoryStatSync(Directory dir) async {
  dir.statSync();
}

// POSITIVE upstream test_file_exists (1 row at `file.exists()`).
void fFileExists(File file) async {
  await file.exists();
}

// POSITIVE upstream test_file_exists_extensionType_implements (1 row at
// `file.exists()`).
extension type ExtFileExists(File file) implements File {}

void fExtFileExists(ExtFileExists file) async {
  await file.exists();
}

// NEGATIVE upstream test_file_exists_extensionType_redeclares (the
// extension type redeclares `exists`, shadowing the slow member).
extension type ExtFileRedeclares(File file) {
  void exists() {}
}

void fExtFileRedeclares(ExtFileRedeclares file) async {
  file.exists();
}

// NEGATIVE upstream test_file_existsSync.
void fFileExistsSync(File file) async {
  file.existsSync();
}

// POSITIVE upstream test_file_lastModified (1 row at
// `file.lastModified()`).
void fFileLastModified(File file) async {
  await file.lastModified();
}

// NEGATIVE upstream test_file_lastModifiedSync.
void fFileLastModifiedSync(File file) async {
  file.lastModifiedSync();
}

// POSITIVE upstream test_file_stat (1 row at `file.stat()`).
void fFileStat(File file) async {
  await file.stat();
}

// NEGATIVE upstream test_file_statSync.
void fFileStatSync(File file) async {
  file.statSync();
}

// POSITIVE upstream test_fileSystemEntity_isDirectory (1 row at
// `FileSystemEntity.isDirectory(path)`).
void fIsDirectory(String path) async {
  await FileSystemEntity.isDirectory(path);
}

// NEGATIVE upstream test_fileSystemEntity_isDirectorySync.
void fIsDirectorySync(String path) async {
  FileSystemEntity.isDirectorySync(path);
}

// POSITIVE upstream test_fileSystemEntity_isFile (1 row at
// `FileSystemEntity.isFile(path)`).
void fIsFile(String path) async {
  await FileSystemEntity.isFile(path);
}

// NEGATIVE upstream test_fileSystemEntity_isFileSync.
void fIsFileSync(String path) async {
  FileSystemEntity.isFileSync(path);
}

// POSITIVE upstream test_fileSystemEntity_isLink (1 row at
// `FileSystemEntity.isLink(path)`).
void fIsLink(String path) async {
  await FileSystemEntity.isLink(path);
}

// NEGATIVE upstream test_fileSystemEntity_isLinkSync.
void fIsLinkSync(String path) async {
  FileSystemEntity.isLinkSync(path);
}

// POSITIVE upstream test_fileSystemEntity_type (1 row at
// `FileSystemEntity.type(path)`; the upstream face has no `await` — the
// bare slow call still reports).
void fFsEntityType(String path) async {
  FileSystemEntity.type(path);
}

// NEGATIVE upstream test_fileSystemEntity_typeSync.
void fFsEntityTypeSync(String path) async {
  FileSystemEntity.typeSync(path);
}
