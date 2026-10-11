/// Pick labor: employee hierarchy with super parameters, and the covariant
/// override pair for capability merging.
library;

/// Base labor record; abstract getter forces concrete name storage.
abstract base class Employee {
  final String id;

  Employee({required this.id});

  String get name;

  int get basePayCents => 1_800;

  @override
  String toString() => '$name (#$id)';
}

/// A picker with a rate; [name] is promoted from the abstract getter.
final class Picker extends Employee {
  final String _name;
  final int picksPerHour;

  Picker({required super.id, required String name, required this.picksPerHour})
      : _name = name;

  @override
  String get name => _name;

  @override
  int get basePayCents => super.basePayCents + picksPerHour * 12;
}

/// Mergeable capability set: the base accepts any capability set.
abstract class CapabilitySet {
  void absorb(CapabilitySet other);

  int get strength;
}

/// Picker certifications narrow the parameter with `covariant`: callers
/// holding the base type still type-check, the runtime check enforces the
/// family invariant.
final class PickerCertifications extends CapabilitySet {
  final Set<String> certifications = {};

  @override
  int get strength => certifications.length * 2;

  void certify(String code) => certifications.add(code);

  @override
  void absorb(covariant PickerCertifications other) {
    certifications.addAll(other.certifications);
  }
}

/// Senior pickers add the forklift endorsement through the same family.
final class SeniorPickerCertifications extends PickerCertifications {
  @override
  void absorb(covariant PickerCertifications other) {
    super.absorb(other);
    if (other is SeniorPickerCertifications) {
      certifications.add('forklift');
    }
  }
}

/// Weekly labor plan: heterogeneous employees keyed by shift.
class LaborPlan {
  final Map<String, List<Employee>> byShift;

  LaborPlan({required this.byShift});

  int get headcount => byShift.values.fold(0, (sum, crew) => sum + crew.length);

  Iterable<Employee> crewFor(String shift) => byShift[shift] ?? const [];

  int payrollCents() {
    var total = 0;
    for (final crew in byShift.values) {
      for (final member in crew) {
        total += member.basePayCents;
      }
    }
    return total;
  }
}
