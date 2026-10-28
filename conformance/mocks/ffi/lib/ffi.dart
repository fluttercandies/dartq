
import 'dart:ffi';

const CallocAllocator calloc = CallocAllocator._();

final class CallocAllocator implements Allocator {}

final class Utf8 extends Opaque {}
