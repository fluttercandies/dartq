// Trigger for dart_code_linter `avoid-missing-enum-constant-in-map`.
// Shape: a `Map<Enum, _>` literal missing an enum constant — the rule's
// flagged form.
enum CountyCode { russia, kazachstan, another }

const code = <CountyCode, String>{
  CountyCode.russia: 'RUS',
  CountyCode.another: 'XXX',
};
