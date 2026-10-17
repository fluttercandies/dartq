// Copyright (c) 2026, the Dart project authors. Please see the AUTHORS file
//
// The official `edit/change_builder/change_builder.dart`, mirrored verbatim:
// pure re-exports of `package:analyzer_plugin`'s change-builder stack. The
// re-exported types are real: every dartq plugin package_config injects the
// same `package:analyzer_plugin` the official package depends on, and a
// producer's `compute(ChangeBuilder builder)` override names exactly these
// types whether the override site imports them from here or directly from
// `package:analyzer_plugin`.

export 'package:analyzer_plugin/utilities/change_builder/change_builder_core.dart';
export 'package:analyzer_plugin/utilities/change_builder/change_builder_dart.dart';
export 'package:analyzer_plugin/utilities/change_builder/change_builder_yaml.dart';
export 'package:analyzer_plugin/utilities/change_builder/change_workspace.dart';
export 'package:analyzer_plugin/protocol/protocol_common.dart'
    show LinkedEditSuggestionKind, Position, SourceChange;
