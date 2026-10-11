/// Definite assignment and late initialization: late finals assigned in
/// both branches, compound null-aware assignment, exhaustive switch
/// expression initialization and deferred write-once fields.
library;

/// A write-once label resolved lazily; definite assignment proves every
/// path assigns before the first read.
class SessionLabel {
  final String raw;

  late final String normalized = _normalize();

  SessionLabel(this.raw);

  String _normalize() {
    late final String prefix;
    final trimmed = raw.trim();
    if (trimmed.startsWith('/')) {
      prefix = 'abs';
    } else if (trimmed.contains('://')) {
      prefix = 'url';
    } else {
      prefix = 'rel';
    }
    return '$prefix:${trimmed.length}';
  }
}

/// Compound null-aware assignment chains; each operator keeps the prior
/// value when the receiver is non-null.
class RetryPolicy {
  int attempts;
  String? channel;

  RetryPolicy({required this.attempts, this.channel});

  void tune({int? maxAttempts, String? preferredChannel}) {
    attempts = maxAttempts ?? attempts;
    channel ??= preferredChannel;
    channel ??= 'default';
  }

  String describe() => '$attempts attempts over ${channel ?? 'n/a'}';
}

/// Exhaustive switch expression used as a direct initializer: the enum is
/// closed, so no default arm and no nullability escape hatch.
enum ImportChannel { rail, road, air, sea }

int channelCapacity(ImportChannel channel) => switch (channel) {
      ImportChannel.rail => 40_000,
      ImportChannel.road => 12_500,
      ImportChannel.air => 900,
      ImportChannel.sea => 250_000,
    };

/// `late` non-final fields model delayed configuration with a guarded
/// write; the assertion documents the single-write contract.
class MutableThreshold {
  late int ceiling;

  bool _ceilingAssigned = false;

  void configure(int value) {
    if (_ceilingAssigned) {
      return;
    }
    ceiling = value;
    _ceilingAssigned = true;
  }

  /// Reading before configure() throws LateInitializationError by design;
  /// the null-safe read goes through [ceilingOrNull] instead.
  int? ceilingOrNull() {
    if (!_ceilingAssigned) {
      return null;
    }
    return ceiling;
  }
}
