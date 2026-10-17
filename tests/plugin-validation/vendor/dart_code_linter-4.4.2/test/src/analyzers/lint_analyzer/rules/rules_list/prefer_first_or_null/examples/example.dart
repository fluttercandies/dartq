// ignore_for_file: unnecessary_cast, unused_local_variable

import 'dart:collection';

const _array = [1, 2, 3, 4, 5, 6, 7, 8, 9];

void func() {
  const iterable = _array as Iterable<int>;

  final firstOfIterable = iterable.first; // LINT
  final firstOrNullOfIterable = iterable.firstOrNull;
  final firstElementOfIterable = iterable.elementAt(0); // LINT
  final secondElementOfIterable = iterable.elementAt(1);

  iterable
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1);

////////////////////////////////////////////////////////////////////////////////

  const list = _array;

  final firstOfList = list.first; // LINT
  final firstOrNullOfList = list.firstOrNull;
  final firstElementOfList1 = list.elementAt(0); // LINT
  final firstElementOfList2 = list[0]; // LINT
  final secondElementOfList1 = list.elementAt(1);
  final secondElementOfList2 = list[1];

  list
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1)
    ..[2]
    ..[0] // LINT
    ..[1];

  (list
        ..firstOrNull
        ..[2]
        ..[0]) // LINT
      .length;

  list
    ..firstOrNull
    ..[2].toDouble()
    ..[0].toDouble(); // LINT

////////////////////////////////////////////////////////////////////////////////

  final set = _array.toSet();

  final firstOfSet = set.first; // LINT
  final firstOrNullOfSet = set.firstOrNull;
  final firstElementOfSet = set.elementAt(0); // LINT
  final secondElementOfSet = set.elementAt(1);

  set
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1);

////////////////////////////////////////////////////////////////////////////////

  final doubleLinkedQueue = DoubleLinkedQueue.of(_array);

  final firstOfDoubleLinkedQueue = doubleLinkedQueue.first; // LINT
  final firstOrNullOfDoubleLinkedQueue = doubleLinkedQueue.firstOrNull;
  final firstElementOfDoubleLinkedQueue =
      doubleLinkedQueue.elementAt(0); // LINT
  final secondElementOfDoubleLinkedQueue = doubleLinkedQueue.elementAt(1);

  doubleLinkedQueue
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1);

////////////////////////////////////////////////////////////////////////////////

  final hashSet = HashSet.of(_array);

  final firstOfHashSet = hashSet.first; // LINT
  final firstOrNullOfHashSet = hashSet.firstOrNull;
  final firstElementOfHashSet = hashSet.elementAt(0); // LINT
  final secondElementOfHashSet = hashSet.elementAt(1);

  hashSet
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1);

////////////////////////////////////////////////////////////////////////////////

  final linkedHashSet = LinkedHashSet.of(_array);

  final firstOfLinkedHashSet = linkedHashSet.first; // LINT
  final firstOrNullOfLinkedHashSet = linkedHashSet.firstOrNull;
  final firstElementOfLinkedHashSet = linkedHashSet.elementAt(0); // LINT
  final secondElementOfLinkedHashSet = linkedHashSet.elementAt(1);

  linkedHashSet
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1);

////////////////////////////////////////////////////////////////////////////////

  final listQueue = ListQueue.of(_array);

  final firstOfListQueue = listQueue.first; // LINT
  final firstOrNullOfListQueue = listQueue.firstOrNull;
  final firstElementOfListQueue = listQueue.elementAt(0); // LINT
  final secondElementOfListQueue = listQueue.elementAt(1);

  listQueue
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1);

////////////////////////////////////////////////////////////////////////////////

  final queue = Queue.of(_array);

  final firstOfQueue = queue.first; // LINT
  final firstOrNullOfQueue = queue.firstOrNull;
  final firstElementOfQueue = queue.elementAt(0); // LINT
  final secondElementOfQueue = queue.elementAt(1);

  queue
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1);

////////////////////////////////////////////////////////////////////////////////

  final splayTreeSet = SplayTreeSet.of(_array);

  final firstOfSplayTreeSet = splayTreeSet.first; // LINT
  final firstOrNullOfSplayTreeSet = splayTreeSet.firstOrNull;
  final firstElementOfSplayTreeSet = splayTreeSet.elementAt(0); // LINT
  final secondElementOfSplayTreeSet = splayTreeSet.elementAt(1);

  splayTreeSet
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1);

////////////////////////////////////////////////////////////////////////////////

  final unmodifiableListView = UnmodifiableListView<int>(_array);

  final firstOfUnmodifiableListView = unmodifiableListView.first; // LINT
  final firstOrNullOfUnmodifiableListView = unmodifiableListView.firstOrNull;
  final firstElementOfUnmodifiableListView1 =
      unmodifiableListView.elementAt(0); // LINT
  final firstElementOfUnmodifiableListView2 = unmodifiableListView[0]; // LINT
  final secondElementOfUnmodifiableListView1 =
      unmodifiableListView.elementAt(1);
  final secondElementOfUnmodifiableListView2 = unmodifiableListView[1];

  unmodifiableListView
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1)
    ..[2]
    ..[0] // LINT
    ..[1];

////////////////////////////////////////////////////////////////////////////////

  final unmodifiableSetView = UnmodifiableSetView<int>(_array.toSet());

  final firstOfUnmodifiableSetView = unmodifiableSetView.first; // LINT
  final firstOrNullOfUnmodifiableSetView = unmodifiableSetView.firstOrNull;
  final firstElementOfUnmodifiableSetView =
      unmodifiableSetView.elementAt(0); // LINT
  final secondElementOfUnmodifiableSetView = unmodifiableSetView.elementAt(1);

  unmodifiableSetView
    ..firstOrNull
    ..elementAt(2)
    ..elementAt(0) // LINT
    ..elementAt(1);
}
