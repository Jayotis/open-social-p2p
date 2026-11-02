import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../src/rust/api/osp.dart';
import '../widgets/post_card.dart';

/// Looks through the posts and profiles your home node has received
/// (ADR 0014). It's only that node's copy: nothing leaves it.
class SearchScreen extends StatefulWidget {
  const SearchScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<SearchScreen> createState() => _SearchScreenState();
}

class _SearchScreenState extends State<SearchScreen> {
  final _words = TextEditingController();

  /// The latest search. FutureBuilder drops the answer to an earlier one.
  Future<Found>? _found;

  @override
  void dispose() {
    _words.dispose();
    super.dispose();
  }

  void _search() {
    final words = _words.text.trim();
    if (words.isEmpty) return;
    // FutureBuilder listens from the next frame, and shows an error that came
    // before it, so that isn't an unhandled one.
    final found = widget.node.app.search(words: words, limit: 50)..ignore();
    setState(() {
      _found = found;
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: TextField(
          controller: _words,
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
            onPressed: _search,
            icon: const Icon(Icons.search),
          ),
        ],
      ),
      body: _found == null
          ? const _Message(
              "Finds posts and people your home node has already received. "
              "It only looks on your node, so nobody sees what you search for.",
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
          _PersonTile(person: person, you: node.paired?.identity),
        if (found.posts.isNotEmpty) const _Heading('Posts'),
        for (final post in found.posts) PostCard(node: node, post: post),
      ],
    );
  }
}

class _PersonTile extends StatelessWidget {
  const _PersonTile({required this.person, required this.you});

  final FoundPerson person;

  /// This node's identity, if known.
  final String? you;

  @override
  Widget build(BuildContext context) {
    final label = person.identity == you
        ? 'You'
        : person.followed
        ? 'Following'
        : null;
    return ListTile(
      leading: CircleAvatar(
        child: Text(person.name.isEmpty ? '?' : person.name[0].toUpperCase()),
      ),
      title: Text(person.name),
      subtitle: person.summary != null ? Text(person.summary!) : null,
      trailing: label != null ? Text(label) : null,
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
