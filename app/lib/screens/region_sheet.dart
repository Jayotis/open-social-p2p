import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import '../widgets/person_tile.dart';
import '../widgets/post_card.dart';

/// What a tap on a group opens (ADR 0035 §2): its people, newest post
/// first, and their recent posts, newest first, from the node alone. The
/// order is always plain; nothing here learns from what you look at.
class RegionSheet extends StatefulWidget {
  const RegionSheet({
    super.key,
    required this.node,
    required this.region,
    required this.title,
    required this.onOpenMap,
    this.scroll,
  });

  final HomeNode node;

  /// `g:` or `sky:`, as the snapshot names it.
  final String region;
  final String title;

  /// Opens the region on the map, closing the sheet.
  final VoidCallback onOpenMap;

  /// The sheet's own scrolling, from the frame it's dragged by.
  final ScrollController? scroll;

  /// Shows it as a sheet over the map.
  static Future<void> show(
    BuildContext context, {
    required HomeNode node,
    required String region,
    required String title,
    required VoidCallback onOpenMap,
  }) {
    return showModalBottomSheet<void>(
      context: context,
      isScrollControlled: true,
      showDragHandle: true,
      useSafeArea: true,
      builder: (context) => DraggableScrollableSheet(
        expand: false,
        initialChildSize: 0.6,
        minChildSize: 0.3,
        maxChildSize: 0.95,
        builder: (context, scroll) => RegionSheet(
          node: node,
          region: region,
          title: title,
          scroll: scroll,
          onOpenMap: () {
            Navigator.of(context).pop();
            onOpenMap();
          },
        ),
      ),
    );
  }

  @override
  State<RegionSheet> createState() => _RegionSheetState();
}

class _RegionSheetState extends State<RegionSheet> {
  late final Future<Found> _found = widget.node.app.regionItems(
    region: widget.region,
    limit: 50,
  );

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final node = widget.node;
    return FutureBuilder<Found>(
      future: _found,
      builder: (context, found) {
        final done = found.connectionState == ConnectionState.done;
        final data = found.data;
        return ListView(
          controller: widget.scroll,
          padding: const EdgeInsets.only(bottom: Space.l),
          children: [
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: Space.m),
              child: Row(
                children: [
                  Expanded(
                    child: Text(
                      widget.title,
                      style: theme.textTheme.titleMedium,
                    ),
                  ),
                  TextButton.icon(
                    key: const Key('region-open-map'),
                    onPressed: widget.onOpenMap,
                    icon: const Icon(Icons.zoom_in_map),
                    label: const Text('Open on the map'),
                  ),
                ],
              ),
            ),
            if (!done)
              const Padding(
                padding: EdgeInsets.all(Space.xl),
                child: Center(child: CircularProgressIndicator()),
              )
            else if (found.hasError)
              _Note(describeError(found.error!))
            else if (data!.people.isEmpty && data.posts.isEmpty)
              const _Note('Your node holds nothing of theirs yet.')
            else ...[
              if (data.people.isNotEmpty) const _Heading('People'),
              for (final person in data.people)
                PersonTile(
                  key: ValueKey(person.identity),
                  node: node,
                  person: person,
                ),
              if (data.posts.isNotEmpty) const _Heading('Recent posts'),
              for (final post in data.posts) PostCard(node: node, post: post),
            ],
          ],
        );
      },
    );
  }
}

class _Heading extends StatelessWidget {
  const _Heading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(Space.m, Space.m, Space.m, Space.xs),
      child: Text(text, style: Theme.of(context).textTheme.titleSmall),
    );
  }
}

class _Note extends StatelessWidget {
  const _Note(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(Space.xl),
      child: Text(text, textAlign: TextAlign.center),
    );
  }
}
