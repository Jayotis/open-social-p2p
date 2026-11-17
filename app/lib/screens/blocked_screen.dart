import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import 'trust_screen.dart' show shownName;

/// What the owner blocked (ADR 0042 §5), each with Unblock.
class BlockedScreen extends StatefulWidget {
  const BlockedScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<BlockedScreen> createState() => _BlockedScreenState();
}

class _BlockedScreenState extends State<BlockedScreen> {
  late Future<List<BlockedThing>> _blocks = widget.node.app.blocks();

  Future<void> _unblock(BlockedThing b) async {
    final messenger = ScaffoldMessenger.of(context);
    try {
      await widget.node.app.unblock(kind: b.kind, id: b.id);
    } catch (e) {
      messenger.showSnackBar(SnackBar(content: Text(describeError(e))));
      return;
    }
    messenger.showSnackBar(
      SnackBar(
        content: Text(
          b.kind == 'account'
              ? '${shownName(b.name ?? '')} is unblocked. Follow them again '
                    'to see their posts.'
              : 'Unblocked. It comes back as your node finds it again.',
        ),
      ),
    );
    setState(() {
      _blocks = widget.node.app.blocks();
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Blocked')),
      body: FutureBuilder<List<BlockedThing>>(
        future: _blocks,
        builder: (context, blocks) {
          if (blocks.hasError) {
            return Center(child: Text(describeError(blocks.error!)));
          }
          if (!blocks.hasData) {
            return const Center(child: CircularProgressIndicator());
          }
          final list = blocks.data!;
          if (list.isEmpty) {
            return const Padding(
              padding: EdgeInsets.all(24),
              child: Text(
                "You haven't blocked anyone or anything. Block someone from "
                'the menu on their profile or one of their posts.',
              ),
            );
          }
          return ListView(
            children: [
              for (final b in list)
                ListTile(
                  leading: Icon(switch (b.kind) {
                    'account' => Icons.person_off_outlined,
                    'post' => Icons.visibility_off_outlined,
                    _ => Icons.hide_image_outlined,
                  }),
                  title: Text(switch (b.kind) {
                    'account' => shownName(b.name ?? ''),
                    'post' => 'A post',
                    _ => 'A file',
                  }),
                  subtitle: Text(_since(b.since)),
                  trailing: TextButton(
                    onPressed: () => _unblock(b),
                    child: const Text('Unblock'),
                  ),
                ),
            ],
          );
        },
      ),
    );
  }
}

String _since(String since) {
  final at = DateTime.tryParse(since);
  return at == null ? 'Blocked' : 'Blocked ${formatAgo(at)}';
}
