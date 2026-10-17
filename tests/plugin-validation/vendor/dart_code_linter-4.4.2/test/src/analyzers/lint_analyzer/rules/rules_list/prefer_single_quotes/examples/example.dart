// ignore_for_file: unnecessary_cast, unused_local_variable

import 'dart:collection';

void func() {
  final String value1 = "some value"; // Lint
  final String value2 = 'some value';

  final String value3 = "some value \"another text\""; // Lint
  final String value4 = 'some value \"another text\"';

  final String value5 = """
  multi line text
  """;
  final String value6 = '''
  multi line text
  ''';
}
