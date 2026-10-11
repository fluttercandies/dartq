// Intentional diagnostics: undefined_class. Both failing lines reference
// the missing types in plain type-annotation positions; a type-argument
// position reports non_type_as_type_argument instead, which is outside the
// frozen M1 universe this corpus draws from.
class DemandForecast {
  // expect: undefined_class
  late ReorderPlan plan;

  // expect: undefined_class
  late WarehouseZone zone;
}
