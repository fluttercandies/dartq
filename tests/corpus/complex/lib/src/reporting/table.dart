/// Reporting tables: a generic table type with column-aware rendering and
/// extensions over nested iterables for the text grid.
library;

/// One rendered table row: column name to cell text.
typedef TableRow = Map<String, String>;

/// Generic table with typed rows and a generic transform entry point.
class Table<RowT> {
  final List<String> columns;
  final List<RowT> rows;

  Table({required this.columns, required this.rows});

  int get width => columns.length;

  bool get isEmpty => rows.isEmpty;

  Table<R> mapRows<R>(R Function(RowT row) transform) => Table<R>(
        columns: columns,
        rows: [for (final row in rows) transform(row)],
      );

  Table<RowT> sortedBy(String Function(RowT row) keyOf) {
    final sorted = [...rows]..sort(
        (a, b) => keyOf(a).compareTo(keyOf(b)),
      );
    return Table<RowT>(columns: columns, rows: sorted);
  }
}

/// Grid text helpers: nested-iterable shape is the natural render target.
extension GridText on Iterable<Iterable<String>> {
  List<String> toLines({String separator = ' | '}) => [
        for (final line in this) line.join(separator),
      ];

  int get widestCell {
    var widest = 0;
    for (final line in this) {
      for (final cell in line) {
        if (cell.length > widest) {
          widest = cell.length;
        }
      }
    }
    return widest;
  }
}

/// Renders the table as aligned text; the header line repeats on wraps.
String renderTable(Table<TableRow> table, {int wrapAfter = 40}) {
  if (table.isEmpty) {
    return '(no rows)';
  }
  final grid = [
    table.columns,
    for (final row in table.rows) [for (final column in table.columns) row[column] ?? '-'],
  ];
  final padded = [
    for (final line in grid)
      [
        for (final cell in line)
          cell.length > wrapAfter ? '${cell.substring(0, wrapAfter)}…' : cell.padRight(wrapAfter),
      ],
  ];
  return padded.toLines().join('\n');
}
