// Targets non_final_field_in_enum: an enum instance field that is not
// final. Oracle message (nail-pinned): "Enums can only declare final
// fields."
enum E {
  a;

  var x = 0;
}
