/// Mass units: an enhanced enum carrying conversion factors plus a small
/// value class built on top. Digit separators keep the factor table legible.
library;

/// Mass unit with its gram factor. The enum is the closed conversion table;
/// the switch in [toGrams] stays exhaustive by construction.
enum MassUnit implements Comparable<MassUnit> {
  milligram(1),
  gram(1_000),
  kilogram(1_000_000),
  tonne(1_000_000_000);

  const MassUnit(this.gramsPerUnit);

  final int gramsPerUnit;

  int toGrams(int amount) => amount * gramsPerUnit;

  MassUnit toLarger() => switch (this) {
        milligram => gram,
        gram => kilogram,
        kilogram => tonne,
        tonne => tonne,
      };

  static MassUnit? tryParseLabel(String label) => switch (label) {
        'mg' => milligram,
        'g' => gram,
        'kg' => kilogram,
        't' => tonne,
        _ => null,
      };

  @override
  int compareTo(MassUnit other) =>
      gramsPerUnit.compareTo(other.gramsPerUnit);
}

/// A measured mass. Kept as a plain class (not an extension type) so it can
/// be null-initialized inside const-free domain records with no ambiguity.
class ParcelMass implements Comparable<ParcelMass> {
  final int grams;

  const ParcelMass(this.grams);

  const ParcelMass.zero() : grams = 0;

  bool get exceedsPalletLimit => grams > 500_000;

  ParcelMass operator +(ParcelMass other) => ParcelMass(grams + other.grams);

  double inKilograms() => grams / MassUnit.kilogram.gramsPerUnit;

  @override
  int compareTo(ParcelMass other) => grams.compareTo(other.grams);

  @override
  String toString() => '${inKilograms()}kg';
}
