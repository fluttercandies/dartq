/// Capability vocabulary of the dartq plugin protocol (PRD section 14.1).
///
/// Mirrors `Capability` / `CapabilitySet` in the Rust crate `dartq-plugin`
/// (`src/capability.rs`); both files must change together.
library;

/// A capability a plugin can expose.
enum Capability {
  /// The plugin contributes lint rules.
  lint('lint'),

  /// The plugin contributes fixes for diagnostics.
  fix('fix'),

  /// The plugin contributes assists.
  assist('assist');

  const Capability(this.wireName);

  /// The lowercase name used on the wire.
  final String wireName;

  /// Parses a wire name; unknown names yield null (never an error).
  static Capability? tryParse(String name) {
    for (final capability in Capability.values) {
      if (capability.wireName == name) {
        return capability;
      }
    }
    return null;
  }
}

/// A set of known capabilities. The effective set of a session is always the
/// **intersection** of the core's and the plugin's capabilities; unknown
/// capability names on either side mean "not enabled", never an error.
class CapabilitySet {
  /// A set containing [capabilities] (duplicates collapse).
  CapabilitySet(Iterable<Capability> capabilities) : _caps = {...capabilities};

  /// The empty set.
  CapabilitySet.empty() : _caps = <Capability>{};

  final Set<Capability> _caps;

  /// Parses a JSON `capabilities` array (wire names). Unknown names and
  /// non-string entries are ignored, matching the Rust codec.
  static CapabilitySet parse(Object? value) {
    final set = CapabilitySet.empty();
    if (value is List) {
      for (final entry in value) {
        if (entry is String) {
          final capability = Capability.tryParse(entry);
          if (capability != null) {
            set._caps.add(capability);
          }
        }
      }
    }
    return set;
  }

  /// Whether [capability] is enabled.
  bool contains(Capability capability) => _caps.contains(capability);

  /// Whether no capability is enabled.
  bool get isEmpty => _caps.isEmpty;

  /// Capabilities present in both sets.
  CapabilitySet intersect(CapabilitySet other) =>
      CapabilitySet(_caps.where(other.contains));

  /// The enabled capabilities as wire names, in protocol order.
  List<String> names() =>
      Capability.values.where(contains).map((c) => c.wireName).toList();

  @override
  bool operator ==(Object other) =>
      other is CapabilitySet &&
      other._caps.length == _caps.length &&
      other._caps.every(_caps.contains);

  @override
  int get hashCode => Object.hashAll(_caps);

  @override
  String toString() => 'CapabilitySet(${names().join(', ')})';
}
