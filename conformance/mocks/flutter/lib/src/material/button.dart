
import 'package:flutter/widgets.dart';

class RawMaterialButton extends StatefulWidget {
  const RawMaterialButton({
    super.key,
    required this.onPressed,
    this.child,
  });

  final VoidCallback? onPressed;

  final Widget? child;
}
