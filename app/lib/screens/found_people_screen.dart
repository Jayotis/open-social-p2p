import 'package:flutter/material.dart';

import '../node.dart';
import '../theme.dart';
import '../widgets/person_tile.dart';
import 'search_screen.dart';
import '../widgets/sky_bar.dart';

/// "People your node found" (ADR 0035): everyone your node holds a profile
/// of, newest post first, to browse without knowing a name. Until the
/// Universe's map (P3, P4) shows them around your star.
class FoundPeopleScreen extends StatefulWidget {
  const FoundPeopleScreen({super.key, required this.node});

  final HomeNode node;

  @override
  State<FoundPeopleScreen> createState() => _FoundPeopleScreenState();
}

class _FoundPeopleScreenState extends State<FoundPeopleScreen> {
  @override
  void initState() {
    super.initState();
    widget.node.loadPeople();
  }

  @override
  Widget build(BuildContext context) {
    final node = widget.node;
    return Scaffold(
      appBar: SkyBar(
        AppBar(
          title: const Text('People your node found'),
          actions: [
            IconButton(
              tooltip: 'Search',
              onPressed: () => Navigator.of(context).push(
                MaterialPageRoute(builder: (_) => SearchScreen(node: node)),
              ),
              icon: const Icon(Icons.search),
            ),
          ],
        ),
      ),
      body: ListenableBuilder(
        listenable: node,
        builder: (context, _) {
          final people = node.people;
          if (people == null) {
            return node.peopleError == null
                ? const Center(child: CircularProgressIndicator())
                : _Message("Couldn't load them: ${node.peopleError}");
          }
          return RefreshIndicator(
            onRefresh: node.loadPeople,
            child: people.isEmpty
                ? ListView(
                    children: [
                      _Message(
                        (node.status?.lookedForPeople ?? false)
                            ? 'Your node looked and found nobody yet. It looks '
                                  'again every few minutes: pull down to look '
                                  'now.'
                            : 'Your node is looking for people. It checks '
                                  'again every few minutes: pull down to look '
                                  'now.',
                      ),
                    ],
                  )
                : ListView.builder(
                    padding: const EdgeInsets.only(bottom: Space.l),
                    itemCount: people.length,
                    itemBuilder: (context, i) => PersonTile(
                      key: ValueKey(people[i].identity),
                      node: node,
                      person: people[i],
                    ),
                  ),
          );
        },
      ),
    );
  }
}

class _Message extends StatelessWidget {
  const _Message(this.text);

  final String text;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.all(Space.xl),
      child: Text(text, textAlign: TextAlign.center),
    );
  }
}
