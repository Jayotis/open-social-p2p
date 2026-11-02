import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../widgets/post_card.dart';

/// Looks through the posts and profiles your home node holds: from those you
/// follow, and from the directories it reads (ADR 0015). It's only that node's
/// copy: nothing leaves it.
class SearchScreen extends StatefulWidget {
  const SearchScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<SearchScreen> createState() => _SearchScreenState();
}

class _SearchScreenState extends State<SearchScreen> {
  final _words = TextEditingController();
  final _box = FocusNode();

  /// The latest search. FutureBuilder drops the answer to an earlier one.
  Future<Found>? _found;

  /// What the latest search was for, unless it failed.
  String? _searched;

  @override
  void dispose() {
    _words.dispose();
    _box.dispose();
    super.dispose();
  }

  void _search() {
    final words = _words.text.trim();
    if (words.isEmpty) return;
    final found = widget.node.app.search(words: words, limit: 50);
    // FutureBuilder listens from the next frame, and shows an error that came
    // before it, so that isn't an unhandled one.
    found.then<void>(
      (_) {},
      onError: (Object _) {
        if (identical(found, _found)) _searched = null;
      },
    );
    setState(() {
      _found = found;
      _searched = words;
    });
  }

  /// Searches if there's something new to look for. Otherwise asking again
  /// would only show the same list, so it gets the box ready for another word.
  void _pressed() {
    final words = _words.text.trim();
    if (words.isNotEmpty && words != _searched) {
      _box.unfocus();
      _search();
      return;
    }
    _words.selection = TextSelection(
      baseOffset: 0,
      extentOffset: _words.text.length,
    );
    _box.requestFocus();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: TextField(
          controller: _words,
          focusNode: _box,
          autofocus: true,
          textInputAction: TextInputAction.search,
          decoration: const InputDecoration(
            hintText: 'Search your node',
            border: InputBorder.none,
          ),
          onSubmitted: (_) => _search(),
        ),
        actions: [
          IconButton(
            tooltip: 'Search',
            onPressed: _pressed,
            icon: const Icon(Icons.search),
          ),
        ],
      ),
      body: _found == null
          ? const _Message(
              "Finds posts and people your home node already holds, from "
              "those you follow and the directories it reads. It only looks "
              "on your node, so nobody sees what you search for.",
            )
          : FutureBuilder<Found>(
              future: _found,
              builder: (context, found) {
                if (found.connectionState != ConnectionState.done) {
                  return const Center(child: CircularProgressIndicator());
                }
                if (found.hasError) {
                  return _Message(describeError(found.error!));
                }
                return _Results(node: widget.node, found: found.data!);
              },
            ),
    );
  }
}

class _Results extends StatelessWidget {
  const _Results({required this.node, required this.found});

  final HomeNode node;
  final Found found;

  @override
  Widget build(BuildContext context) {
    if (found.people.isEmpty && found.posts.isEmpty) {
      return const _Message("Nothing on your node matches.");
    }
    return ListView(
      padding: const EdgeInsets.only(bottom: 24),
      children: [
        if (found.people.isNotEmpty) const _Heading('People'),
        for (final person in found.people)
          _PersonTile(node: node, person: person, you: node.paired?.identity),
        if (found.posts.isNotEmpty) const _Heading('Posts'),
        for (final post in found.posts) PostCard(node: node, post: post),
      ],
    );
  }
}

class _PersonTile extends StatefulWidget {
  const _PersonTile({
    required this.node,
    required this.person,
    required this.you,
  });

  final HomeNode node;
  final FoundPerson person;

  /// This node's identity, if known.
  final String? you;

  @override
  State<_PersonTile> createState() => _PersonTileState();
}

class _PersonTileState extends State<_PersonTile> {
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
            "Their posts reach your feed once their node answers.",
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
    final you = person.identity == widget.you;
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
      if (person.foundThrough != null) 'From directory ${person.foundThrough}',
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

class _Heading extends StatelessWidget {
  const _Heading(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
      child: Text(text, style: Theme.of(context).textTheme.titleMedium),
    );
  }
}

class _Message extends StatelessWidget {
  const _Message(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Text(text, textAlign: TextAlign.center),
      ),
    );
  }
}
