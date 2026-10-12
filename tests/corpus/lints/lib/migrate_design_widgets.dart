// migrate_design_widgets — official rule faces
// (pkg/linter migrate_design_widgets.dart:34-43): an import of the
// deprecated design libraries (`package:flutter/material.dart`,
// `package:flutter/cupertino.dart`) reports at the URI with the library
// value spelled in the message.
// Expected rows: 2 — MIGRATE_DESIGN_WIDGETS keyed
// migrate_design_widgets.dart, message
// "The '{0}' library is deprecated.":
//   `package:flutter/material.dart`
//   `package:flutter/cupertino.dart`
import 'package:flutter/material.dart';
import 'package:flutter/cupertino.dart';

// GOOD: every other import stays silent.
import 'package:flutter/widgets.dart';
import 'package:meta/meta.dart';
import 'dart:async';
