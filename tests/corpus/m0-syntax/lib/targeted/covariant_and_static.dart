// Targets covariant_and_static: members cannot be declared to be both
// 'covariant' and 'static'.
class C {
  covariant static int x = 0;
}
