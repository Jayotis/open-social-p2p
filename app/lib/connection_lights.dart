import 'dart:ui';

import 'src/rust/api/osp.dart';

/// "How are we connected?" (ADR 0037) as the map shows it: the paths the
/// node knows from its owner to someone, lit up hop by hop, and written out
/// under the map. While the node is still looking, [connection] is none.
class ConnectionLights {
  /// While the node asks. [torStarting]: its Tor hasn't reached anyone
  /// yet, so the node waits for it first.
  ConnectionLights.asking(this.identity, this.name, {this.torStarting = false})
    : connection = null;

  ConnectionLights(this.identity, this.name, Connection this.connection)
    : torStarting = false {
    for (final p in connection!.people) {
      _names[p.identity] = p.name;
      _regions[p.identity] = p.regions;
    }
  }

  /// Who it's about, and their name as the tap knew it.
  final String identity;
  final String name;

  final Connection? connection;
  final bool torStarting;

  final _names = <String, String?>{};
  final _regions = <String, List<String>>{};

  bool get asking => connection == null;

  List<ConnectionPath> get paths => connection?.paths ?? const [];

  /// Everyone on a path, [identity] among them.
  Iterable<String> get _onPaths =>
      paths.expand((p) => p.hops).map((h) => h.identity);

  /// A name for [who]: as their profile or vouch gives it, or "someone".
  String nameOf(String who) {
    if (who == identity && name.isNotEmpty) return name;
    final n = _names[who];
    return n == null || n.isEmpty ? 'Someone' : n;
  }

  /// Whether [p] is on a path.
  bool holdsPerson(UniversePerson p) =>
      p.step != 0 && (_onPaths.contains(p.identity) || p.identity == identity);

  /// Whether [g], drawn closed, holds someone on a path.
  bool holdsGroup(UniverseGroup g) => _groupOf(g.id.toString()) != null;

  String? _groupOf(String id) {
    for (final who in {..._onPaths, identity}) {
      if ((_regions[who] ?? const []).contains('g:$id')) return who;
    }
    return null;
  }

  /// The lines to draw on [s], in the world's units: each hop whose two
  /// ends [s] shows, at the person or the closed group holding them. A hop
  /// with an end the map doesn't show isn't drawn; the words under the map
  /// still say it. The owner is at the centre.
  List<PathLine> lines(UniverseSnapshot s) {
    final out = <PathLine>[];
    final seen = <(Offset, Offset)>{};
    for (final path in paths) {
      Offset? from = Offset.zero;
      for (final hop in path.hops) {
        final to = _at(hop.identity, s);
        if (from != null && to != null && from != to) {
          if (seen.add((from, to))) out.add(PathLine(from, to, hop.how));
        }
        from = to;
      }
    }
    return out;
  }

  Offset? _at(String who, UniverseSnapshot s) {
    for (final p in s.people) {
      if (p.identity == who) return Offset(p.x, p.y);
    }
    final regions = _regions[who] ?? const [];
    for (final g in s.groups) {
      if (regions.contains('g:${g.id}')) return Offset(g.x, g.y);
    }
    return null;
  }

  /// Where the paths came from, in words, under the title.
  String get source {
    final c = connection;
    if (c == null && torStarting) {
      return 'Tor is still starting; your node asks once it reaches others…';
    }
    if (c == null) return 'Looking for how you’re connected…';
    return switch (c.side) {
      SideFrom.replied => 'From when they replied to you',
      SideFrom.asked when c.vouches == 0 =>
        'Their node named nobody who vouched for them',
      SideFrom.asked when c.vouches == 1 =>
        'Their node named 1 person who vouched for them',
      SideFrom.asked =>
        'Their node named ${c.vouches} people who vouched for them',
      SideFrom.noAnswer => 'Their node didn’t answer; only what yours knows',
      SideFrom.notAsked => 'Only what your node knows itself',
    };
  }

  /// [path] in sentences: "You follow Alice. Alice reshared Carol."
  String words(ConnectionPath path) {
    final out = <String>[];
    String? at;
    // Someone unnamed is "someone" inside a sentence.
    String inside(String who) {
      final n = nameOf(who);
      return n == 'Someone' ? 'someone' : n;
    }

    for (final hop in path.hops) {
      final to = inside(hop.identity);
      final by = at == null ? 'You' : nameOf(at);
      out.add(switch (hop.how) {
        HopKind.follows => 'You follow $to.',
        HopKind.knows => 'You know $to.',
        HopKind.reshared => '$by reshared $to.',
        HopKind.creditedEarly =>
          '$by credited $to with finding their post early.',
        HopKind.gap when hop.steps == 1 =>
          '$by’s node says $to is 1 step from $by.',
        HopKind.gap =>
          '$by’s node says $to is within ${hop.steps} steps of $by.',
      });
      at = hop.identity;
    }
    return out.join(' ');
  }
}

/// One hop drawn: from one end to the other, in the world's units.
class PathLine {
  PathLine(this.from, this.to, this.how);
  final Offset from, to;
  final HopKind how;
}
