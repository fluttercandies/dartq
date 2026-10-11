/// Barrel of the complex corpus package: one export per module, in layer
/// order. All top-level names across modules are globally unique, so the
/// combined namespace is conflict-free; the cross-package surface is
/// re-exported through a hide-combinator to exercise that grammar too.
library;

export 'src/core/identifiers.dart';
export 'src/core/money.dart';
export 'src/core/timestamps.dart';
export 'src/core/units.dart';

export 'src/domain/catalog.dart';
export 'src/domain/customer.dart';
export 'src/domain/discount.dart';
export 'src/domain/events.dart';
export 'src/domain/option_matrix.dart';
export 'src/domain/order.dart';
export 'src/domain/order_status.dart';
export 'src/domain/product.dart';
export 'src/domain/shipment.dart';
export 'src/domain/supplier.dart';

export 'src/pricing/discount_pipeline.dart';
export 'src/pricing/pricing_engine.dart';
export 'src/pricing/tax_rules.dart';

export 'src/matching/destructuring.dart';
export 'src/matching/pattern_matrix.dart';

export 'src/flow/definite_assignment.dart';
export 'src/flow/promotion_chains.dart';

export 'src/operators/operator_matrix.dart';

export 'src/text/label_format.dart';
export 'src/text/search.dart';

export 'src/annotations/annotations.dart';

export 'src/registry/component_registry.dart';
export 'src/registry/lifecycle.dart';

export 'src/import_graph/deep_core.dart';
export 'src/import_graph/deep_mid.dart';
export 'src/import_graph/deep_edge.dart';
export 'src/import_graph/deep_upper.dart';
export 'src/import_graph/deferred_report.dart';

export 'src/warehouse/bin.dart';
export 'src/warehouse/picker.dart';
export 'src/warehouse/route.dart';
export 'src/warehouse/zoning.dart';

export 'src/reporting/summary.dart';
export 'src/reporting/table.dart';

export 'src/notifications/audit_log.dart';

export 'src/validation/validator.dart';

export 'package:stock_policy/stock_policy.dart' hide RestockRule;
