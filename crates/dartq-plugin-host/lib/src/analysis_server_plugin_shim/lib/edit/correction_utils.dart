// Copyright (c) 2024, the Dart project authors. Please see the AUTHORS file
//
// The line-and-text core of the official `CorrectionUtils`, mirrored from
// the upstream package source (`lib/edit/correction_utils.dart`). The
// bodies are the official ones; the members that need the change-building
// or scanning stack (`getLinesRange`, `indentSourceLeftRight`,
// `replaceSourceIndent`, `invertCondition`, `TokenUtils`) are not mirrored
// — the dartq wire does not carry the analysis session they drive.

import 'dart:io' show Platform;

import 'package:analyzer/dart/analysis/results.dart';
import 'package:analyzer/dart/ast/ast.dart';
import 'package:analyzer/source/source_range.dart';
import 'package:analyzer/src/utilities/extensions/string.dart';

import '../src/utilities/extensions/string_extension.dart';

/// The line, prefix, and range-text helpers a correction producer reads off
/// `utils`, over the unit's content.
final class CorrectionUtils {
  static const String _oneIndent = '  ';

  static const String _twoIndents = _oneIndent + _oneIndent;

  final CompilationUnit _unit;

  final String _buffer;

  String? _endOfLine;

  CorrectionUtils(ParsedUnitResult result)
    : _unit = result.unit,
      _buffer = result.content;

  CorrectionUtils.fromUnitAndContent(CompilationUnit unit, String content)
    : _unit = unit,
      _buffer = content;

  /// The EOL sequence to use for this [CompilationUnit]. The official
  /// fallback for content without EOLs is the platform line terminator.
  String get endOfLine {
    return _endOfLine ??= _buffer.endOfLine ?? Platform.lineTerminator;
  }

  String get oneIndent => _oneIndent;

  String get twoIndents => _twoIndents;

  /// Skips whitespace characters and single EOL on the right from [index].
  ///
  /// If [index] the end of a statement or method, then in most cases this
  /// returns the start of the next line.
  int getLineContentEnd(int index) {
    var length = _buffer.length;
    // Skip whitespace characters.
    while (index < length) {
      var c = _buffer.codeUnitAt(index);
      if (!c.isWhitespace || c.isEOL) {
        break;
      }
      index++;
    }
    // Skip a single '\r' character.
    if (index < length && _buffer.codeUnitAt(index) == 0x0D) {
      index++;
    }
    // Skip a single '\n' character.
    if (index < length && _buffer.codeUnitAt(index) == 0x0A) {
      index++;
    }
    // Done.
    return index;
  }

  /// Skips spaces and tabs on the left from [index].
  ///
  /// If [index] is the start or a statement, then in most cases this returns
  /// the offset of the line in which [index] is found.
  int getLineContentStart(int index) {
    while (index > 0) {
      var c = _buffer.codeUnitAt(index - 1);
      if (!c.isSpace) {
        break;
      }
      index--;
    }
    return index;
  }

  /// Returns the index of the start of the line following the line which
  /// contains the given [index].
  int getLineNext(int index) {
    var length = _buffer.length;
    // skip to the end of the line
    while (index < length) {
      var c = _buffer.codeUnitAt(index);
      if (c == 0xD || c == 0xA) {
        break;
      }
      index++;
    }
    // Skip a single '\r'.
    if (index < length && _buffer.codeUnitAt(index) == 0xD) {
      index++;
    }
    // Skip a single '\n'.
    if (index < length && _buffer.codeUnitAt(index) == 0xA) {
      index++;
    }
    // Done.
    return index;
  }

  /// Returns the whitespace prefix of the line which contains given [index].
  String getLinePrefix(int index) {
    var lineStart = getLineThis(index);
    var length = _buffer.length;
    var lineNonWhitespace = lineStart;
    while (lineNonWhitespace < length) {
      var c = _buffer.codeUnitAt(lineNonWhitespace);
      if (c == 0xD || c == 0xA) {
        break;
      }
      if (!c.isWhitespace) {
        break;
      }
      lineNonWhitespace++;
    }
    return getText(lineStart, lineNonWhitespace - lineStart);
  }

  /// Returns the start index of the line which contains the given [index].
  int getLineThis(int index) {
    while (index > 0) {
      var c = _buffer.codeUnitAt(index - 1);
      if (c == 0xD || c == 0xA) {
        break;
      }
      index--;
    }
    return index;
  }

  /// Returns the whitespace prefix to the left of the given [endIndex].
  String getPrefix(int endIndex) {
    var startIndex = getLineContentStart(endIndex);
    return _buffer.substring(startIndex, endIndex);
  }

  /// Returns the text of the given range in the unit.
  String getRangeText(SourceRange range) => getText(range.offset, range.length);

  /// Returns the text of the given range in the unit.
  String getText(int offset, int length) =>
      _buffer.substring(offset, offset + length);
}
