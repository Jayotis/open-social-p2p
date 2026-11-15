import 'src/rust/api/osp.dart';

/// A search's results as places on the map (ADR 0035 §3): which people,
/// groups and sky cells hold them, so the Universe can light those up and
/// fly there. Built from the `regions` each result carries.
class SearchLights {
  SearchLights(this.words, Found found)
    : people = found.people.length,
      posts = found.posts.length {
    final chains = <List<String>>[];
    void add(String identity, List<String> regions) {
      _identities.add(identity);
      for (final r in regions) {
        final cell = _SkyPlace.parse(r);
        if (cell != null) {
          _sky.add(cell);
        } else if (r.startsWith('g:')) {
          _groups.add(r);
        }
      }
      chains.add([
        for (final r in regions)
          if (!r.startsWith('sky:')) r,
      ]);
    }

    for (final p in found.people) {
      add(p.identity, p.regions);
    }
    for (final p in found.posts) {
      add(p.authorIdentity, p.regions);
    }
    region = _deepestShared(chains);
  }

  /// What was searched for.
  final String words;

  /// How many people and posts were found.
  final int people, posts;

  /// The deepest region holding every result: a group if they're all in
  /// one, otherwise the top (empty). Flying there opens it.
  late final String region;

  final _identities = <String>{};
  final _groups = <String>{};
  final _sky = <_SkyPlace>[];

  bool get isEmpty => _identities.isEmpty;

  /// Whether [p] is someone found, or wrote a post that was.
  bool holdsPerson(UniversePerson p) => _identities.contains(p.identity);

  /// Whether [g], drawn closed, holds a result.
  bool holdsGroup(UniverseGroup g) => _groups.contains('g:${g.id}');

  /// Whether [c] holds a result: a place at full precision is in a cell
  /// when shifting it right by `32 - level` gives the cell's x and y.
  bool holdsCell(SkyCell c) => _sky.any((s) => s.inCell(c));

  /// The longest path every result's chain starts with, its last group.
  static String _deepestShared(List<List<String>> chains) {
    if (chains.isEmpty) return '';
    var shared = chains.first;
    for (final c in chains.skip(1)) {
      var n = 0;
      while (n < shared.length && n < c.length && shared[n] == c[n]) {
        n++;
      }
      shared = shared.sublist(0, n);
    }
    final last = shared.lastOrNull ?? '';
    return last.startsWith('g:') ? last : '';
  }
}

/// A place in the public sky at full precision, `sky:32/x/y`.
class _SkyPlace {
  const _SkyPlace(this.x, this.y);

  final int x, y;

  static _SkyPlace? parse(String region) {
    final parts = region.split('/');
    if (parts.length != 3 || parts[0] != 'sky:32') return null;
    final x = int.tryParse(parts[1]), y = int.tryParse(parts[2]);
    return x == null || y == null ? null : _SkyPlace(x, y);
  }

  bool inCell(SkyCell c) {
    if (c.level > 32) return false;
    final shift = 32 - c.level;
    return x >> shift == c.x && y >> shift == c.y;
  }
}
