/// A plugin's registration census: what `register` put into the registry,
/// reported once in the `initialize` reply so the core can resolve per-cycle
/// rule enablement against it (the `lint` capability's meaning).
///
/// Mirrors `RuleCensus` / `RuleKind` / `RuleCodeInfo` in the Rust crate
/// `dartq-plugin` (`src/session.rs`); both files must change together.
library;

/// Registration kind of a plugin rule, mirroring the official registry split
/// (`package:analyzer/src/lint/registry.dart`): lint rules are disabled by
/// default and enabled via rule configs, warning rules are enabled by
/// default and disabled via rule configs. The enum names are the wire names.
enum RuleKind {
  /// Disabled by default; enabled via an explicit rule config.
  lint,

  /// Enabled by default; disabled via an explicit rule config.
  warning,
}

/// One diagnostic code of a registered rule: the lower-case wire name (the
/// identity fixes and dedup key on) and the code's declared severity (the
/// base the core's severity-override resolution re-levels from).
final class RuleCodeInfo {
  /// Builds a code entry; [name] must already be the lower-case wire form.
  const RuleCodeInfo({required this.name, required this.severity});

  /// The lower-case code name (the wire's rule-code identity).
  final String name;

  /// The code's declared severity (the wire severity name).
  final String severity;

  /// The code entry as the census row's `codes` element.
  Map<String, Object?> toJson() => {'name': name, 'severity': severity};
}

/// One registered rule of the census (the `rules` array of the `initialize`
/// reply): name, registration kind, and the codes the rule reports.
final class RuleCensus {
  /// Builds a census row.
  const RuleCensus({
    required this.name,
    required this.kind,
    required this.codes,
  });

  /// The rule's name (matched case-insensitively, like the official
  /// registry's lower-case keys).
  final String name;

  /// The registration kind (the default-enabled face).
  final RuleKind kind;

  /// The codes this rule reports, in declaration order.
  final List<RuleCodeInfo> codes;

  /// The census row as the `rules` array's element.
  Map<String, Object?> toJson() => {
    'name': name,
    'kind': kind.name,
    'codes': [for (final code in codes) code.toJson()],
  };
}
