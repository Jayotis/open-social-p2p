import 'package:flutter/material.dart';

import 'node.dart';
import 'screens/activity_screen.dart';
import 'screens/compose_screen.dart';
import 'screens/first_run_screen.dart';
import 'screens/home_tab.dart';
import 'screens/search_screen.dart';
import 'screens/universe_tab.dart';
import 'screens/you_tab.dart';
import 'theme.dart';

/// The app's three places (ADR 0031 §1).
enum Place { home, universe, you }

/// Home, the Universe and You, with Post and Activity reachable from each.
class AppShell extends StatefulWidget {
  const AppShell({super.key, required this.node});

  final HomeNode node;

  @override
  State<AppShell> createState() => _AppShellState();
}

class _AppShellState extends State<AppShell> {
  Place _place = Place.home;

  HomeNode get _node => widget.node;

  void _open(Widget screen) {
    Navigator.of(context)
        .push(MaterialPageRoute(builder: (_) => screen))
        .then((_) => _node.checkUnsent());
  }

  void _go(Place place) {
    _node.watchingPeople = place == Place.universe;
    if (place == Place.universe) _node.loadPeople();
    setState(() => _place = place);
  }

  @override
  Widget build(BuildContext context) {
    final node = _node;
    if (node.firstRun) {
      return FirstRunScreen(
        node: node,
        onDone: () {
          _place = Place.universe;
          node.watchingPeople = true;
          node.loadPeople();
          node.finishFirstRun();
        },
      );
    }
    final online = node.reachability == Reachability.online;
    final waiting =
        (node.status?.trustRequests ?? 0) + (node.status?.replyRequests ?? 0);
    final onSky = _place == Place.universe;
    final sky = OspColors.of(context);
    return Scaffold(
      appBar: AppBar(
        title: Text(switch (_place) {
          Place.home => 'Home',
          Place.universe => 'Universe',
          Place.you => 'You',
        }),
        backgroundColor: onSky ? sky.sky : null,
        foregroundColor: onSky ? sky.onSky : null,
        actions: [
          if (_place != Place.you)
            IconButton(
              tooltip: 'Search',
              onPressed: () => _open(SearchScreen(node: node)),
              icon: const Icon(Icons.search),
            ),
          IconButton(
            tooltip: 'Activity',
            onPressed: () => _open(ActivityScreen(node: node, open: _open)),
            icon: Badge(
              isLabelVisible: waiting > 0,
              label: Text('$waiting'),
              child: const Icon(Icons.notifications_outlined),
            ),
          ),
        ],
      ),
      body: IndexedStack(
        index: _place.index,
        children: [
          HomeTab(
            node: node,
            open: _open,
            onExplore: () => _go(Place.universe),
          ),
          UniverseTab(node: node, open: _open),
          YouTab(node: node, open: _open),
        ],
      ),
      floatingActionButton: FloatingActionButton.extended(
        tooltip: online ? 'Post' : "Can't post while your node is unreachable",
        onPressed: online ? () => _open(ComposeScreen(node: node)) : null,
        backgroundColor: online ? null : Theme.of(context).disabledColor,
        icon: const Icon(Icons.edit),
        label: const Text('Post'),
      ),
      bottomNavigationBar: NavigationBar(
        selectedIndex: _place.index,
        onDestinationSelected: (i) => _go(Place.values[i]),
        destinations: const [
          NavigationDestination(
            icon: Icon(Icons.home_outlined),
            selectedIcon: Icon(Icons.home),
            label: 'Home',
          ),
          NavigationDestination(
            icon: Icon(Icons.auto_awesome_outlined),
            selectedIcon: Icon(Icons.auto_awesome),
            label: 'Universe',
          ),
          NavigationDestination(
            icon: Icon(Icons.person_outline),
            selectedIcon: Icon(Icons.person),
            label: 'You',
          ),
        ],
      ),
    );
  }
}
