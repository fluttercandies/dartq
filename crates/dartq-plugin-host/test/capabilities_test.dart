/// Unit tests for the capability vocabulary:
/// `package:dartq_plugin_host/capabilities.dart`.
///
/// Pins the "unknown means not enabled, never an error" contract and the
/// intersection rule of PRD section 14.1.
library;

import 'package:dartq_plugin_host/dartq_plugin_host.dart';
import 'package:test/test.dart';

void main() {
  group('Capability.tryParse', () {
    test('parses every wire name', () {
      expect(Capability.tryParse('lint'), Capability.lint);
      expect(Capability.tryParse('fix'), Capability.fix);
      expect(Capability.tryParse('assist'), Capability.assist);
    });

    test('unknown names yield null, never an error', () {
      expect(Capability.tryParse('completions'), isNull);
      expect(Capability.tryParse(''), isNull);
    });

    test('wire names are case-sensitive lowercase', () {
      expect(Capability.tryParse('Lint'), isNull);
      expect(Capability.tryParse('LINT'), isNull);
    });
  });

  group('CapabilitySet', () {
    test('construction collapses duplicates', () {
      final set = CapabilitySet([Capability.lint, Capability.lint]);
      expect(set.contains(Capability.lint), isTrue);
      expect(set.names(), ['lint']);
    });

    test('the empty set contains nothing', () {
      final set = CapabilitySet.empty();
      expect(set.isEmpty, isTrue);
      expect(set.contains(Capability.fix), isFalse);
      expect(set.names(), isEmpty);
    });

    test('parse keeps known wire names and silently ignores the rest', () {
      final set = CapabilitySet.parse([
        'lint',
        'mystery_future_cap',
        42,
        null,
        'assist',
      ]);
      expect(set.contains(Capability.lint), isTrue);
      expect(set.contains(Capability.assist), isTrue);
      expect(set.contains(Capability.fix), isFalse);
    });

    test('parse of a non-list value is the empty set, matching the Rust codec',
        () {
      for (final value in [null, 'lint', 42, {'lint': true}]) {
        final set = CapabilitySet.parse(value);
        expect(set.isEmpty, isTrue, reason: 'input: $value');
      }
    });

    test('intersect keeps only capabilities present in both sets', () {
      final both = CapabilitySet([Capability.lint, Capability.fix])
          .intersect(CapabilitySet([Capability.fix, Capability.assist]));
      expect(both.names(), ['fix']);

      final disjoint =
          CapabilitySet([Capability.lint]).intersect(CapabilitySet([Capability.fix]));
      expect(disjoint.isEmpty, isTrue);

      final nothing =
          CapabilitySet([Capability.lint]).intersect(CapabilitySet.empty());
      expect(nothing.isEmpty, isTrue);
    });

    test('names are listed in protocol order regardless of insertion order',
        () {
      final set = CapabilitySet([Capability.assist, Capability.lint, Capability.fix]);
      expect(set.names(), ['lint', 'fix', 'assist']);
    });

    test('value equality ignores construction order', () {
      final a = CapabilitySet([Capability.lint, Capability.fix]);
      final b = CapabilitySet([Capability.fix, Capability.lint]);
      final c = CapabilitySet([Capability.lint]);
      expect(a, b);
      expect(a.hashCode, b.hashCode);
      expect(a == c, isFalse);
      expect(CapabilitySet.empty(), CapabilitySet.empty());
    });

    test('toString renders the wire names', () {
      expect(CapabilitySet([Capability.lint]).toString(), 'CapabilitySet(lint)');
    });
  });
}
