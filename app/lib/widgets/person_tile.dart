import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';

/// A person the node holds a profile of, with a Follow button.
class PersonTile extends StatefulWidget {
  const PersonTile({super.key, required this.node, required this.person});

  final HomeNode node;
  final FoundPerson person;

  @override
  State<PersonTile> createState() => _PersonTileState();
}

class _PersonTileState extends State<PersonTile> {
  bool _following = false;
  bool _asking = false;

  /// Following is a person's act: the node then asks their own node, and what
  /// they post reaches the feed once it answers, which may take a while.
  Future<void> _follow() async {
    final messenger = ScaffoldMessenger.of(context);
    setState(() => _asking = true);
    try {
      await widget.node.app.followPerson(identity: widget.person.identity);
      if (!mounted) return;
      setState(() => _following = true);
      messenger.showSnackBar(
        SnackBar(
          content: Text(
            "Following ${widget.person.name}. "
            "Their posts reach Home once their node answers.",
          ),
        ),
      );
    } catch (e) {
      messenger.showSnackBar(
        SnackBar(content: Text("Couldn't follow: ${describeError(e)}")),
      );
    } finally {
      if (mounted) setState(() => _asking = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final person = widget.person;
    final you = person.identity == widget.node.paired?.identity;
    final following = person.followed || _following;
    final Widget trailing = you
        ? const Text('You')
        : following
        ? const Text('Following')
        : TextButton(
            onPressed: _asking ? null : _follow,
            child: const Text('Follow'),
          );
    final about = [
      if (person.summary != null) person.summary!,
      if (DateTime.tryParse(person.newest ?? '') case final newest?)
        'Posted ${formatAgo(newest)}',
      if (person.foundThrough != null) 'Found through ${person.foundThrough}',
    ];
    return ListTile(
      leading: CircleAvatar(
        child: Text(person.name.isEmpty ? '?' : person.name[0].toUpperCase()),
      ),
      title: Text(person.name),
      subtitle: about.isEmpty ? null : Text(about.join('\n')),
      trailing: trailing,
    );
  }
}
