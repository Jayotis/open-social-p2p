import 'package:flutter/material.dart';

import '../node.dart';
import '../theme.dart';

/// The app's icon in the top bar, lit as your node is (ADR 0040 §7): in
/// full colour while it runs, fading in and out while the phone reaches
/// it, and grey when it can't. Every screenshot carries the brand, and
/// the node's state is always in sight. Tapping it opens the node's
/// summary.
class NodeMark extends StatefulWidget {
  const NodeMark({super.key, required this.reachability, required this.onTap});

  final Reachability reachability;
  final VoidCallback onTap;

  /// What the mark says, to a screen reader and as its tooltip: the state
  /// in words, as colour alone doesn't tell everyone.
  static String words(Reachability r) => switch (r) {
    Reachability.online => 'Your node: running',
    Reachability.offline => "Your node: can't reach it",
    _ => 'Your node: reaching it',
  };

  @override
  State<NodeMark> createState() => _NodeMarkState();
}

class _NodeMarkState extends State<NodeMark>
    with SingleTickerProviderStateMixin {
  /// One slow breath while the phone reaches the node.
  late final AnimationController _breath = AnimationController(
    vsync: this,
    duration: const Duration(milliseconds: 1600),
  );

  bool get _reaching =>
      widget.reachability == Reachability.connecting ||
      widget.reachability == Reachability.unknown;

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _breathe();
  }

  @override
  void didUpdateWidget(NodeMark old) {
    super.didUpdateWidget(old);
    _breathe();
  }

  /// Breathes only while reaching, and never under reduced motion: then
  /// it holds half lit.
  void _breathe() {
    final moving = Motion.of(context, Motion.quick) != Duration.zero;
    if (_reaching && moving) {
      if (!_breath.isAnimating) _breath.repeat(reverse: true);
    } else {
      _breath.stop();
      _breath.value = 0.5;
    }
  }

  @override
  void dispose() {
    _breath.dispose();
    super.dispose();
  }

  /// Grey: the icon's colours taken away, as for a node out of reach.
  static const _grey = ColorFilter.matrix([
    0.2126, 0.7152, 0.0722, 0, 0, //
    0.2126, 0.7152, 0.0722, 0, 0, //
    0.2126, 0.7152, 0.0722, 0, 0, //
    0, 0, 0, 1, 0, //
  ]);

  @override
  Widget build(BuildContext context) {
    final r = widget.reachability;
    final words = NodeMark.words(r);
    final image = Image.asset(
      'assets/brand/mark.png',
      width: 28,
      height: 28,
      filterQuality: FilterQuality.medium,
      excludeFromSemantics: true,
    );
    final Widget mark = switch (r) {
      Reachability.online => image,
      Reachability.offline => Opacity(
        opacity: 0.45,
        child: ColorFiltered(colorFilter: _grey, child: image),
      ),
      _ => FadeTransition(
        opacity: _breath.drive(Tween(begin: 0.35, end: 0.9)),
        child: image,
      ),
    };
    return IconButton(
      key: const Key('node-mark'),
      tooltip: words,
      onPressed: widget.onTap,
      icon: mark,
    );
  }
}
