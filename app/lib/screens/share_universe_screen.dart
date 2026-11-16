import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:share_plus/share_plus.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import '../widgets/share_card_image.dart';
import '../widgets/sky_bar.dart';
import 'trust_screen.dart' show shownName;

/// Hands a card's picture to the phone's share sheet.
typedef ShareImage = Future<void> Function(Uint8List png);

Future<void> _shareSheet(Uint8List png) => SharePlus.instance.share(
  ShareParams(
    files: [XFile.fromData(png, mimeType: 'image/png')],
    fileNameOverrides: ['my-universe.png'],
  ),
);

/// "Share my Universe" (ADR 0039): the card, made by the node from its
/// own allowlist, shown full size as the very picture that will be
/// shared, with the people it names listed under it. Nobody is named
/// unless ticked here. It leaves the phone only by the Share button.
class ShareUniverseScreen extends StatefulWidget {
  const ShareUniverseScreen({super.key, required this.node, this.share});

  final HomeNode node;

  /// For tests; the phone's share sheet otherwise.
  final ShareImage? share;

  @override
  State<ShareUniverseScreen> createState() => _ShareUniverseScreenState();
}

class _ShareUniverseScreenState extends State<ShareUniverseScreen> {
  final Set<String> _ticked = {};
  Uint8List? _png;
  List<String> _named = const [];
  String? _error;

  /// Which making is the latest: an older one finishing late is dropped.
  int _making = 0;

  /// Whom the owner may name: the people they follow or know, from the
  /// Universe the phone holds.
  late final List<UniversePerson> _choices = [
    for (final p in widget.node.universeTop?.people ?? const <UniversePerson>[])
      if (p.step != 0 && (p.followed || p.known)) p,
  ]..sort((a, b) => a.name.toLowerCase().compareTo(b.name.toLowerCase()));

  @override
  void initState() {
    super.initState();
    _make();
  }

  Future<void> _make() async {
    final making = ++_making;
    setState(() => _error = null);
    try {
      final card = await widget.node.app.shareCard(named: _ticked.toList());
      final png = await renderShareCard(card);
      if (!mounted || making != _making) return;
      setState(() {
        _png = png;
        _named = shareCardNames(card);
      });
    } catch (e) {
      if (!mounted || making != _making) return;
      setState(() => _error = describeError(e));
    }
  }

  void _tick(String identity, bool on) {
    setState(() => on ? _ticked.add(identity) : _ticked.remove(identity));
    _make();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final quiet = theme.textTheme.bodyMedium?.copyWith(
      color: theme.colorScheme.onSurfaceVariant,
    );
    final png = _png;
    // Ticked, but the card doesn't draw them, so doesn't name them.
    final undrawn = {
      for (final p in _choices)
        if (_ticked.contains(p.identity) && !_named.contains(p.name))
          p.identity,
    };
    return Scaffold(
      appBar: SkyBar(AppBar(title: const Text('Share my Universe'))),
      body: ListView(
        padding: const EdgeInsets.fromLTRB(Space.m, Space.m, Space.m, Space.xl),
        children: [
          Text(
            'This is exactly the picture that will be shared.',
            style: quiet,
          ),
          const SizedBox(height: Space.s),
          AspectRatio(
            aspectRatio: shareCardSize.aspectRatio,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(Corner.m),
              child: png == null
                  ? ColoredBox(
                      color: OspColors.of(context).sky,
                      child: Center(
                        child: _error == null
                            ? const CircularProgressIndicator()
                            : Padding(
                                padding: const EdgeInsets.all(Space.l),
                                child: Text(
                                  "Couldn't make the card: $_error",
                                  textAlign: TextAlign.center,
                                  style: TextStyle(
                                    color: OspColors.of(context).onSky,
                                  ),
                                ),
                              ),
                      ),
                    )
                  : Image.memory(
                      png,
                      key: const Key('share-preview'),
                      gaplessPlayback: true,
                      semanticLabel: 'The card to share',
                    ),
            ),
          ),
          const SizedBox(height: Space.s),
          // Said only of a card there is.
          if (png != null)
            Text(
              _named.isEmpty
                  ? 'Nobody is named on it. Everyone is an unnamed point.'
                  : 'Named on it: ${_named.join(', ')}. '
                        'Everyone else is an unnamed point.',
              style: quiet,
            ),
          const SizedBox(height: Space.m),
          FilledButton.icon(
            onPressed: png == null
                ? null
                : () => (widget.share ?? _shareSheet)(png),
            icon: const Icon(Icons.ios_share),
            label: const Text('Share'),
          ),
          if (_choices.isNotEmpty) ...[
            const SizedBox(height: Space.l),
            Text('Name people on it', style: theme.textTheme.titleMedium),
            Text(
              "Only people you tick are named. Who you follow isn't public "
              'in OSP, so nobody is named by default.',
              style: quiet,
            ),
            for (final p in _choices)
              CheckboxListTile(
                contentPadding: EdgeInsets.zero,
                value: _ticked.contains(p.identity),
                onChanged: (on) => _tick(p.identity, on ?? false),
                title: Text(shownName(p.name)),
                subtitle: undrawn.contains(p.identity) && png != null
                    ? const Text("Not drawn on the card, so it can't name them")
                    : null,
              ),
          ],
        ],
      ),
    );
  }
}
