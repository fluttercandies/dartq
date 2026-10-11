/// Policy thresholds shared between the packages. Pure constants: this file
/// also feeds the cross-package const-evaluation surface of the corpus.
library;

/// Below this many units on hand the fast-lane reorder fires.
const int fastLaneFloor = 40;

/// Above this many pending units the receiving dock queues work.
const int receivingQueueCeiling = 1_250;

/// Days a receiving batch may sit before the audit flag raises.
const int maxReceivingAgeDays = 7;

/// Fixed per-SKU safety margin applied by every restock run.
const double safetyMarginFraction = 0.15;
