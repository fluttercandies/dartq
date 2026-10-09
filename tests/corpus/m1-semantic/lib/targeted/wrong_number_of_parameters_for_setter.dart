// Targets wrong_number_of_parameters_for_setter: a setter is declared with
// two parameters where exactly one is required (overlaps the M0 syntax gate
// by PRD wording; independent M1 filter).
set s(int a, int b) {}
