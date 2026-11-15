import 'dart:math';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import '../format.dart';
import '../node.dart';
import '../search_lights.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import 'found_people_screen.dart';
import 'node_summary_screen.dart';
import 'people_screen.dart';
import 'profile_screen.dart';
import 'region_sheet.dart';
import 'search_screen.dart';

/// The Universe (ADR 0031 §1, ADR 0034): the network as a map, with your
/// star at the centre. One canvas, panned and zoomed by hand, drawn from
/// the node's snapshot of a region (ADR 0033). Tapping a star stands
/// there, tapping a light opens it, and zooming out goes up the region's
/// path without moving the map. Tapping a group opens a sheet of its people
/// and posts, which can open it on the map (ADR 0035 §2). A layer lights
/// up where posts, video or reshares are, and a search lights up where its
/// results are and flies there (ADR 0035 §1, §3). Places are the node's,
/// so they stay put.
/// What it shows comes from the node's real state only (ADR 0036).
class UniverseTab extends StatefulWidget {
  const UniverseTab({super.key, required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  State<UniverseTab> createState() => _UniverseTabState();
}

/// The camera: the world point at the middle of the screen, and how many
/// pixels a unit of the world is. A person is about one unit across.
class _Camera extends ChangeNotifier {
  Offset centre = Offset.zero;
  double scale = _Fit.near;
  Size size = Size.zero;

  Offset toScreen(Offset world) =>
      (world - centre) * scale + size.center(Offset.zero);

  Offset toWorld(Offset screen) =>
      centre + (screen - size.center(Offset.zero)) / scale;

  /// Moves the camera. A value that isn't a finite number is refused, so
  /// the map can't be lost, and the centre stays within reach of the sky.
  void set(Offset c, double s) {
    if (!c.isFinite || !s.isFinite) {
      if (!_warned) {
        _warned = true;
        debugPrint('Universe camera refused centre $c, scale $s');
      }
      return;
    }
    const reach = _Fit.skyOuter * 4;
    centre = Offset(c.dx.clamp(-reach, reach), c.dy.clamp(-reach, reach));
    scale = s.clamp(_Fit.farthest, _Fit.nearest);
    notifyListeners();
  }

  bool _warned = false;
}

/// How the camera frames a region.
abstract final class _Fit {
  /// Closest and farthest the camera goes, in pixels per unit.
  static const nearest = 400.0;
  static const farthest = 0.02;

  /// For a region that is one star alone.
  static const near = 40.0;

  /// Zooming out below this share of a region's own framing goes up its
  /// path (ADR 0034 §6).
  static const up = 0.4;

  /// A map wider and shorter than these (a phone on its side), the card
  /// keeps to the bottom left, this wide.
  static const wide = 600.0;
  static const short = 360.0;
  static const card = 440.0;

  /// What the controls above and the card below cover, in pixels.
  static const clearTop = 64.0;
  static const clearBottom = 190.0;

  /// On a wide screen the card sits low in the corner, and covers less.
  static const clearBottomWide = 100.0;

  /// Where the public sky starts and ends (`osp_proto::universe`).
  static const skyInner = 64.0;
  static const skyOuter = 1024.0;
}

class _UniverseTabState extends State<UniverseTab>
    with SingleTickerProviderStateMixin {
  final _camera = _Camera();
  late final AnimationController _fly;
  Offset _fromCentre = Offset.zero, _toCentre = Offset.zero;
  double _fromScale = _Fit.near, _toScale = _Fit.near;

  /// The region last framed, and each region's own framing.
  String? _framed;
  final Map<String, double> _fits = {};

  /// Set when the next region comes from zooming out: the map stays.
  bool _stay = false;

  /// The last snapshot drawn, kept while the next region's comes.
  UniverseSnapshot? _shown;

  /// The search last flown to, and whether the next region framed is
  /// framed on its results.
  SearchLights? _flown;
  bool _onResults = false;

  /// The last gesture update: where the fingers were, the recognizer's
  /// scale then, and how many fingers. Each update moves the camera by what
  /// changed since, so a finger landing or lifting doesn't make it jump.
  Offset _lastFocal = Offset.zero;
  double _lastFactor = 1;
  int _lastFingers = 0;

  final _labels = _Labels();

  HomeNode get _node => widget.node;

  @override
  void initState() {
    super.initState();
    _fly = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 450),
    )..addListener(_flying);
  }

  @override
  void dispose() {
    _fly.dispose();
    _camera.dispose();
    _labels.dispose();
    super.dispose();
  }

  void _flying() {
    final t = Curves.easeInOutCubic.transform(_fly.value);
    _camera.set(
      Offset.lerp(_fromCentre, _toCentre, t)!,
      // Geometric, so zooming feels even.
      _fromScale * pow(_toScale / _fromScale, t),
    );
  }

  void _flyTo(Offset centre, double scale) {
    _fromCentre = _camera.centre;
    _fromScale = _camera.scale;
    _toCentre = centre;
    _toScale = scale.clamp(_Fit.farthest, _Fit.nearest);
    _fly.forward(from: 0);
  }

  /// The room the controls and the card leave clear.
  Size get _clear {
    final size = _camera.size;
    return Size(
      size.width,
      max(size.height - _Fit.clearTop - _clearBottom, size.height / 3),
    );
  }

  bool get _wide => _sideways(_camera.size);

  double get _clearBottom => _wide ? _Fit.clearBottomWide : _Fit.clearBottom;

  /// Where the camera's centre goes for [middle] to be in the clear room.
  Offset _lifted(Offset middle, double scale) {
    final lift = (_camera.size.height - _clear.height) / 2 - _Fit.clearTop;
    return middle + Offset(0, lift / scale);
  }

  /// Frames a newly arrived region, unless it came from zooming out.
  void _frame(UniverseSnapshot s) {
    if (_framed == s.region || _camera.size.isEmpty) return;
    _framed = s.region;
    final (middle, scale) = _fitOf(s, _clear);
    _fits[s.region] = scale;
    if (_stay) {
      _stay = false;
      return;
    }
    if (_onResults) {
      _onResults = false;
      if (_frameResults(s)) return;
    }
    _flyTo(_lifted(middle, scale), scale);
  }

  /// Flies to the search's results (ADR 0035 §3): opens the deepest region
  /// holding them all, then frames what's lit there.
  void _flyToResults() {
    final lights = _node.searchLights;
    if (lights == null) return;
    _flown = lights;
    final here = _node.universe;
    if (_node.universeRegion == lights.region && here != null) {
      _frameResults(here);
      return;
    }
    _stay = false;
    _onResults = true;
    _framed = null;
    _node.openUniverse(lights.region);
  }

  /// Frames the lit results in [s]; false if none of them is in it.
  bool _frameResults(UniverseSnapshot s) {
    final lights = _node.searchLights;
    if (lights == null || _camera.size.isEmpty) return false;
    final points = _resultsIn(s, lights);
    if (points.isEmpty) return false;
    var bounds = Rect.fromPoints(points.first, points.first);
    for (final p in points) {
      bounds = bounds.expandToInclude(Rect.fromPoints(p, p));
    }
    final half = max(bounds.width, bounds.height) / 2;
    final room = _clear.shortestSide / 2 * 0.8;
    // One alone is shown close; never closer than a region alone.
    final scale = half > 0 ? min(room / half, _Fit.near * 2) : _Fit.near;
    _flyTo(_lifted(bounds.center, scale), scale);
    return true;
  }

  void _pickLayer() {
    showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      // As tall as its rows, so every layer shows at once.
      isScrollControlled: true,
      builder: (context) => _LayerPicker(
        layer: _node.universeLayer,
        onPick: (layer) {
          Navigator.of(context).pop();
          _node.lightUniverse(layer);
        },
      ),
    );
  }

  /// Back to the plain list of the search's results.
  void _resultsList(SearchLights lights) {
    widget.open(
      SearchScreen(
        node: _node,
        words: lights.words,
        onMap: () => Navigator.of(context).pop(),
      ),
    );
  }

  void _open(String region, {Offset? toward, double zoom = 1}) {
    _stay = false;
    if (toward != null) _flyTo(toward, _camera.scale * zoom);
    _framed = null;
    _node.openUniverse(region);
  }

  /// Goes up one step along the path. A pinch leaves the map where it is
  /// (ADR 0034 §6); a button frames everything in the region above, as one
  /// press should show more than the step back.
  void _up({bool frame = false}) {
    final path = (_node.universe ?? _shown)?.path ?? const [];
    if (path.isEmpty) return;
    _stay = !frame;
    _framed = null;
    _node.openUniverse(path.last);
  }

  void _home() {
    _stay = false;
    if (_node.universeRegion.isEmpty) {
      _framed = null;
      final top = _node.universe;
      if (top != null) _frame(top);
      return;
    }
    _open('', toward: Offset.zero);
  }

  void _tap(Offset screen, UniverseSnapshot s) {
    switch (_hit(s, _camera, screen)) {
      case _OnPerson(:final person) when person.step == 0:
        widget.open(NodeSummaryScreen(node: _node, open: widget.open));
      case _OnPerson(:final person):
        _open('p:${person.identity}', toward: Offset(person.x, person.y));
      case _OnGroup(:final group):
        RegionSheet.show(
          context,
          node: _node,
          region: 'g:${group.id}',
          title: group.people == 1
              ? 'A group of 1 person'
              : 'A group of ${group.people} people',
          onOpenMap: () =>
              _open('g:${group.id}', toward: Offset(group.x, group.y), zoom: 2),
        );
      case _OnCell(:final cell):
        _open(
          'sky:${cell.level}/${cell.x}/${cell.y}',
          toward: Offset(cell.atX, cell.atY),
          zoom: 2,
        );
      case null:
        break;
    }
  }

  void _scaleStart(ScaleStartDetails d) {
    _fly.stop();
    _lastFocal = d.localFocalPoint;
    _lastFactor = 1;
    _lastFingers = d.pointerCount;
  }

  void _scaleUpdate(ScaleUpdateDetails d) {
    // A finger landed or lifted: the recognizer starts its scale again.
    final steady =
        d.pointerCount == _lastFingers && _lastFactor > 0 && d.scale > 0;
    if (steady) {
      final mid = _camera.size.center(Offset.zero);
      // The world point under the fingers stays under them.
      final held = _camera.toWorld(_lastFocal);
      final scale = (_camera.scale * d.scale / _lastFactor).clamp(
        _Fit.farthest,
        _Fit.nearest,
      );
      _camera.set(held - (d.localFocalPoint - mid) / scale, scale);
    }
    _lastFocal = d.localFocalPoint;
    _lastFactor = d.scale;
    _lastFingers = d.pointerCount;
  }

  void _scaleEnd() {
    final region = _node.universeRegion;
    final fit = _fits[region];
    if (fit != null && _camera.scale < fit * _Fit.up) _up();
  }

  void _wheel(PointerSignalEvent e) {
    if (e is! PointerScrollEvent) return;
    _fly.stop();
    final before = _camera.toWorld(e.localPosition);
    final scale = (_camera.scale * pow(0.998, e.scrollDelta.dy)).clamp(
      _Fit.farthest,
      _Fit.nearest,
    );
    final mid = _camera.size.center(Offset.zero);
    _camera.set(before - (e.localPosition - mid) / scale, scale);
    _scaleEnd();
  }

  @override
  Widget build(BuildContext context) {
    final colors = OspColors.of(context);
    final node = _node;
    final here = node.universe;
    if (here != null) _shown = here;
    final s = _shown;
    if (here != null && _framed != here.region) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && node.universe == here) _frame(here);
      });
    }
    final standing = _standingAt(s, node.universeRegion);
    final lights = node.searchLights;
    // A search just shown on the map: fly there once.
    if (lights != null && !identical(lights, _flown)) {
      _flown = lights;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && identical(node.searchLights, lights)) _flyToResults();
      });
    }
    return ColoredBox(
      color: colors.sky,
      child: LayoutBuilder(
        builder: (context, box) {
          _camera.size = box.biggest;
          // Every child is placed; the map fills the tab whatever they are.
          return Stack(
            fit: StackFit.expand,
            children: [
              Positioned.fill(
                child: Listener(
                  onPointerSignal: _wheel,
                  child: GestureDetector(
                    key: const Key('universe-map'),
                    behavior: HitTestBehavior.opaque,
                    onScaleStart: _scaleStart,
                    onScaleUpdate: _scaleUpdate,
                    onScaleEnd: (_) => _scaleEnd(),
                    onTapUp: s == null ? null : (d) => _tap(d.localPosition, s),
                    child: Semantics(
                      label: _describe(s, node.universeLayer, lights),
                      child: CustomPaint(
                        painter: _Map(
                          camera: _camera,
                          colors: colors,
                          snapshot: s,
                          standing: standing,
                          labels: _labels,
                          ownName: _ownName(node),
                          layer: node.universeLayer,
                          lights: lights,
                        ),
                      ),
                    ),
                  ),
                ),
              ),
              _YourStar(
                camera: _camera,
                node: node,
                // With others around her, how her node is shows only when
                // it's a problem.
                crowded:
                    (s?.people.length ?? 0) > 1 ||
                    (s?.groups.isNotEmpty ?? false),
                colors: colors,
                open: widget.open,
              ),
              Positioned(
                left: Space.m,
                top: Space.s,
                right: 64,
                child: Wrap(
                  spacing: Space.xs,
                  runSpacing: Space.xs,
                  children: [
                    _Machines(machines: s?.machines ?? const []),
                    if (node.universeLayer case final layer?)
                      InputChip(
                        label: Text(_LayerPicker.chip(layer)),
                        labelStyle: TextStyle(color: colors.onSky),
                        backgroundColor: colors.skyHaze,
                        side: BorderSide.none,
                        onPressed: _pickLayer,
                        deleteIconColor: colors.onSky,
                        deleteButtonTooltipMessage: 'Back to people',
                        onDeleted: () => node.lightUniverse(null),
                      ),
                  ],
                ),
              ),
              Positioned(
                right: Space.s,
                top: Space.s,
                child: Column(
                  children: [
                    _Control(
                      tooltip: 'My star',
                      icon: Icons.my_location,
                      onPressed: _home,
                    ),
                    _Control(
                      tooltip: 'Layers',
                      icon: Icons.layers_outlined,
                      onPressed: _pickLayer,
                    ),
                    if ((s?.path ?? const []).isNotEmpty)
                      _Control(
                        tooltip: 'Zoom out',
                        icon: Icons.zoom_out,
                        onPressed: () => _up(frame: true),
                      ),
                  ],
                ),
              ),
              Positioned(
                left: Space.m,
                // On a wide screen (a phone on its side) it keeps to the
                // left, clear of the buttons on the right.
                right: _sideways(box.biggest) ? null : Space.m,
                width: _sideways(box.biggest) ? _Fit.card : null,
                // Narrow, it stays above the Post button; wide, that's
                // on the right, so the card goes down to the corner.
                bottom: _sideways(box.biggest) ? Space.s : 96,
                child: switch ((standing, lights)) {
                  (final p?, _) => _StandingAt(
                    person: p,
                    snapshot: s!,
                    node: node,
                    onClose: () => _up(frame: true),
                  ),
                  (null, final l?) => _Results(
                    lights: l,
                    onFly: _flyToResults,
                    onList: () => _resultsList(l),
                    onClose: () => node.showSearch(null),
                  ),
                  _ => _FoundPeople(node: node, open: widget.open),
                },
              ),
            ],
          );
        },
      ),
    );
  }

  static String _describe(
    UniverseSnapshot? s,
    UniverseLayer? layer,
    SearchLights? lights,
  ) {
    if (s == null) return 'The map of your Universe, still empty';
    final people = s.people.length;
    final groups = s.groups.length;
    final lit = layer == null ? '' : ' ${_LayerPicker.chip(layer)} lit up.';
    final found = lights == null
        ? ' The list of who your node found is below it.'
        : ' Results for ${lights.words} lit up, their list below it.';
    return 'The map of your Universe: $people '
        '${people == 1 ? 'person' : 'people'}'
        '${groups > 0 ? ' and $groups groups' : ''} shown.$lit$found';
  }
}

/// Whether the map is wide and short, as on a phone on its side.
bool _sideways(Size size) => size.width > _Fit.wide && size.height < _Fit.short;

/// The owner's name, as shown under her star.
String _ownName(HomeNode node) {
  final name = node.status?.name ?? node.paired?.name ?? '';
  return name.isEmpty ? 'You' : name;
}

/// The person stood at, if the region is someone other than the owner.
UniversePerson? _standingAt(UniverseSnapshot? s, String region) {
  if (s == null || !region.startsWith('p:')) return null;
  final identity = region.substring(2);
  for (final p in s.people) {
    if (p.identity == identity) return p.step == 0 ? null : p;
  }
  return null;
}

/// Where in [s] the search's results are: the people found or whose posts
/// were, and the closed groups and sky cells holding them.
List<Offset> _resultsIn(UniverseSnapshot s, SearchLights lights) => [
  for (final p in s.people)
    if (p.step != 0 && lights.holdsPerson(p)) Offset(p.x, p.y),
  for (final g in s.groups)
    if (lights.holdsGroup(g)) Offset(g.x, g.y),
  for (final c in s.sky)
    if (lights.holdsCell(c)) Offset(c.atX, c.atY),
];

/// How a region is framed: its middle, and the scale that shows it.
(Offset, double) _fitOf(UniverseSnapshot s, Size size) {
  final region = s.region;
  final room = size.shortestSide / 2 * 0.8;
  // Standing at someone: them in the middle, their lines around them.
  final standing = region.startsWith('p:')
      ? s.people.where((p) => 'p:${p.identity}' == region).firstOrNull
      : null;
  if (standing != null) {
    final at = Offset(standing.x, standing.y);
    final i = s.people.indexOf(standing);
    var far = 0.0;
    for (final l in s.lines) {
      final other = l.from == i ? l.to : (l.to == i ? l.from : null);
      if (other == null || other >= s.people.length) continue;
      final p = s.people[other];
      far = max(far, (Offset(p.x, p.y) - at).distance);
    }
    return (at, far > 0 ? room / far : _Fit.near);
  }
  final sky = region.startsWith('sky:');
  final points = <Offset>[
    for (final p in s.people)
      if (sky ? p.step != 0 : (p.group != null || p.step == 0))
        Offset(p.x, p.y),
    for (final g in s.groups) Offset(g.x, g.y),
    if (sky)
      for (final c in s.sky) Offset(c.atX, c.atY),
  ];
  if (region.isEmpty) {
    // The top: the owner in the middle (ADR 0034 §4).
    var far = 0.0;
    for (final p in points) {
      far = max(far, p.distance);
    }
    // Nobody placed yet: the whole public sky around her.
    if (far == 0 && (s.sky.isNotEmpty || s.people.length > 1)) {
      far = _Fit.skyOuter;
    }
    return (Offset.zero, far > 0 ? room / far : _Fit.near);
  }
  if (points.isEmpty) return (Offset.zero, _Fit.near);
  var bounds = Rect.fromPoints(points.first, points.first);
  for (final p in points) {
    bounds = bounds.expandToInclude(Rect.fromPoints(p, p));
  }
  final half = max(bounds.width, bounds.height) / 2;
  return (bounds.center, half > 0 ? room / half : _Fit.near);
}

sealed class _Hit {}

class _OnPerson extends _Hit {
  _OnPerson(this.person);
  final UniversePerson person;
}

class _OnGroup extends _Hit {
  _OnGroup(this.group);
  final UniverseGroup group;
}

class _OnCell extends _Hit {
  _OnCell(this.cell);
  final SkyCell cell;
}

/// What a tap at [screen] is on: the nearest within reach, people first.
_Hit? _hit(UniverseSnapshot s, _Camera camera, Offset screen) {
  const reach = 24.0;
  _Hit? best;
  var bestDistance = double.infinity;
  void consider(_Hit hit, Offset world, double radius) {
    final d = (camera.toScreen(world) - screen).distance - radius;
    if (d < reach && d < bestDistance) {
      best = hit;
      bestDistance = d;
    }
  }

  for (final p in s.people) {
    consider(_OnPerson(p), Offset(p.x, p.y), 0);
  }
  if (best != null) return best;
  for (final g in s.groups) {
    consider(_OnGroup(g), Offset(g.x, g.y), _groupRadius(g.people) * 0.5);
  }
  for (final c in s.sky) {
    consider(_OnCell(c), Offset(c.atX, c.atY), _cellRadius(c.people) * 0.5);
  }
  return best;
}

double _log2(num n) => log(n) / ln2;
double _groupRadius(int people) => min(10 + 4 * _log2(people + 1), 44);
double _cellRadius(int people) => min(8 + 3 * _log2(people + 1), 40);
double _personRadius(int bright) => 3 + bright / 100 * 3;

/// Names drawn on the map, laid out once each.
class _Labels {
  final _cache = <String, TextPainter>{};

  TextPainter of(String text, Color color, double size) {
    final key = '$text|${color.toARGB32()}|$size';
    return _cache.putIfAbsent(key, () {
      if (_cache.length > 800) _clear();
      return TextPainter(
        text: TextSpan(
          text: text,
          style: TextStyle(color: color, fontSize: size),
        ),
        textDirection: TextDirection.ltr,
        maxLines: 1,
        ellipsis: '…',
      )..layout(maxWidth: 140);
    });
  }

  void _clear() {
    for (final p in _cache.values) {
      p.dispose();
    }
    _cache.clear();
  }

  void dispose() => _clear();
}

/// The map: the sky, its lights, the lines and the people, in that order.
class _Map extends CustomPainter {
  _Map({
    required this.camera,
    required this.colors,
    required this.snapshot,
    required this.standing,
    required this.labels,
    required this.ownName,
    required this.layer,
    required this.lights,
  }) : super(repaint: camera);

  final _Camera camera;
  final OspColors colors;
  final UniverseSnapshot? snapshot;
  final UniversePerson? standing;
  final _Labels labels;

  /// The owner's name as drawn under her star, to keep clear.
  final String ownName;

  /// What lights up, if anything does but people (ADR 0035 §1).
  final UniverseLayer? layer;

  /// A search's results to light up (ADR 0035 §3).
  final SearchLights? lights;

  /// How bright a light is drawn: as usual, or dimmed when a layer is on
  /// and it holds none of the layer's items, or a search is shown and it
  /// holds no result.
  double _shade(int lit, bool found) {
    if (layer != null && lit == 0) return 0.35;
    if (layer == null && lights != null && !found) return 0.5;
    return 1;
  }

  /// A lit light's extra glow, by how many items it holds, and a found
  /// one's ring.
  void _marks(Canvas canvas, Offset at, double r, int lit, bool found) {
    if (layer != null && lit > 0) {
      _glow(canvas, at, r * 1.6 + 5 * _log2(lit + 1), colors.yourStar, 0.5);
    }
    if (found) {
      canvas.drawCircle(
        at,
        r + 4,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 2
          ..color = colors.yourStar,
      );
    }
  }

  static const _reshared = Color(0xFF7F95E8);
  static const _creditedEarly = Color(0xFFE8C46A);
  static const _group = Color(0xFF9AAEFF);
  static const _near = Color(0xFFE6EBFF);
  static const _followed = Color(0xFFB9CCFF);

  /// Faint background stars, the same each time: decoration, not people.
  static final List<(Offset, double, double)> _dust = () {
    final random = Random(31);
    return [
      for (var i = 0; i < 90; i++)
        (
          Offset(random.nextDouble(), random.nextDouble()),
          0.6 + random.nextDouble() * 1.2,
          0.2 + random.nextDouble() * 0.5,
        ),
    ];
  }();

  @override
  void paint(Canvas canvas, Size size) {
    final view = Offset.zero & size;
    final home = camera.toScreen(Offset.zero);
    final haze = size.shortestSide * 0.6;
    canvas.drawCircle(
      home,
      haze,
      Paint()
        ..shader = RadialGradient(
          colors: [colors.skyHaze, colors.sky],
        ).createShader(Rect.fromCircle(center: home, radius: haze)),
    );
    final paint = Paint();
    for (final (at, r, a) in _dust) {
      paint.color = colors.distantStar.withValues(alpha: a);
      canvas.drawCircle(
        Offset(at.dx * size.width, at.dy * size.height),
        r,
        paint,
      );
    }
    final s = snapshot;
    if (s == null) return;
    bool seen(Offset p, double r) => view.inflate(r).contains(p);

    // Where the public sky starts: a faint ring, named when it's in view.
    final inner = _Fit.skyInner * camera.scale;
    if (inner > 40 && inner < size.longestSide * 4) {
      canvas.drawCircle(
        home,
        inner,
        Paint()
          ..style = PaintingStyle.stroke
          ..strokeWidth = 1
          ..color = colors.distantStar.withValues(alpha: 0.25),
      );
      final top = home - Offset(0, inner);
      if (seen(top, 0)) {
        final label = labels.of('The public sky', colors.onSkyQuiet, 11);
        label.paint(canvas, top - Offset(label.width / 2, label.height + 4));
      }
    }

    // The sky's cells, each many people as one light.
    for (final c in s.sky) {
      final at = camera.toScreen(Offset(c.atX, c.atY));
      final r = _cellRadius(c.people);
      if (!seen(at, r)) continue;
      final share = c.people == 0 ? 0 : c.creators / c.people;
      final found = lights?.holdsCell(c) ?? false;
      _marks(canvas, at, r * 0.6, c.lit, found);
      _glow(
        canvas,
        at,
        r,
        colors.distantStar,
        (0.35 + 0.4 * share) * _shade(c.lit, found),
      );
      _count(canvas, at, r, c.people);
    }

    // Lines between people, faint, the newest first.
    final points = [
      for (final p in s.people) camera.toScreen(Offset(p.x, p.y)),
    ];
    final line = Paint()..style = PaintingStyle.stroke;
    for (final l in s.lines) {
      if (l.from >= points.length || l.to >= points.length) continue;
      final a = points[l.from], b = points[l.to];
      if (!view.overlaps(Rect.fromPoints(a, b).inflate(1))) continue;
      final touches =
          standing != null &&
          (s.people[l.from] == standing || s.people[l.to] == standing);
      line
        ..strokeWidth = min(0.8 + _log2(l.count + 1) * 0.6, 3)
        ..color =
            (l.kind == LineKind.creditedEarly ? _creditedEarly : _reshared)
                .withValues(alpha: touches ? 0.8 : 0.35);
      canvas.drawLine(a, b, line);
    }

    // Groups, closed, each one light with how many it holds.
    for (final g in s.groups) {
      final at = camera.toScreen(Offset(g.x, g.y));
      final r = _groupRadius(g.people);
      if (!seen(at, r)) continue;
      final found = lights?.holdsGroup(g) ?? false;
      _marks(canvas, at, r * 0.6, g.lit, found);
      _glow(
        canvas,
        at,
        r,
        _group,
        (0.45 + g.bright / 100 * 0.5) * _shade(g.lit, found),
      );
      _count(canvas, at, r, g.people);
    }

    // People, one by one. The owner is her node's star.
    // With few people, each is drawn larger and named, the sky's too, so
    // a newcomer's handful stands out.
    final few = s.people.length <= 40;
    final named = few || camera.scale >= 60;
    final names = <(UniversePerson, Offset, double)>[];
    for (var i = 0; i < s.people.length; i++) {
      final p = s.people[i];
      final at = points[i];
      if (!seen(at, 80)) continue;
      if (p.step == 0) {
        _marks(canvas, at, 14, p.lit, false);
        _glow(canvas, at, 34, colors.yourStar, 0.55);
        paint.color = colors.yourStar;
        canvas.drawCircle(at, 11, paint);
        continue;
      }
      final inSky = p.group == null && p.step == null;
      final color = inSky
          ? colors.distantStar
          : (p.followed || p.known ? _followed : _near);
      final r = _personRadius(p.bright) * (few ? 1.8 : 1);
      final found = lights?.holdsPerson(p) ?? false;
      final shade = _shade(p.lit, found);
      _marks(canvas, at, r, p.lit, found);
      if (p.bright > 30) {
        _glow(canvas, at, r * 3, color, p.bright / 200 * shade);
      }
      paint.color = color.withValues(alpha: shade);
      canvas.drawCircle(at, r, paint);
      final here = p == standing;
      if (here) {
        canvas.drawCircle(
          at,
          r + 6,
          Paint()
            ..style = PaintingStyle.stroke
            ..strokeWidth = 1.5
            ..color = colors.yourStar,
        );
      }
      if (here ||
          found ||
          p.followed ||
          p.known ||
          (named && (few || !inSky))) {
        names.add((p, at, r));
      }
    }

    // Names where there's room: the one stood at first, then those the
    // owner follows or knows, then the rest in the snapshot's order. Her
    // star and her own name under it take their space.
    final own = labels.of(ownName, colors.onSky, 16);
    final taken = <Rect>[
      Rect.fromCircle(center: home, radius: 14),
      Rect.fromCenter(
        center: home + Offset(0, 28 + own.height / 2),
        width: own.width + 8,
        height: own.height + 4,
      ),
    ];
    int rank(UniversePerson p) => p == standing
        ? 0
        : (lights?.holdsPerson(p) ?? false)
        ? 1
        : (p.followed || p.known ? 2 : 3);
    names.sort((a, b) => rank(a.$1).compareTo(rank(b.$1)));
    for (final (p, at, r) in names) {
      final here = p == standing;
      final label = labels.of(
        p.name.isEmpty ? 'Someone' : p.name,
        here ? colors.onSky : colors.onSkyQuiet,
        11,
      );
      // Below the ring round the one stood at.
      final below = here ? r + 9 : r + 3;
      final box = (at + Offset(-label.width / 2, below)) & label.size;
      if (!here && taken.any((t) => t.overlaps(box))) continue;
      taken.add(box.inflate(2));
      label.paint(canvas, box.topLeft);
    }
  }

  void _glow(Canvas canvas, Offset at, double r, Color color, double alpha) {
    canvas.drawCircle(
      at,
      r,
      Paint()
        ..shader = RadialGradient(
          colors: [
            color.withValues(alpha: alpha.clamp(0, 1)),
            color.withValues(alpha: 0),
          ],
        ).createShader(Rect.fromCircle(center: at, radius: r)),
    );
  }

  void _count(Canvas canvas, Offset at, double r, int people) {
    final label = labels.of('$people', colors.onSky, 10);
    label.paint(canvas, at + Offset(-label.width / 2, r * 0.5 + 2));
  }

  @override
  bool shouldRepaint(_Map old) =>
      old.snapshot != snapshot ||
      old.colors != colors ||
      old.standing != standing ||
      old.layer != layer ||
      old.lights != lights;
}

/// The owner's star: a button over where the map draws it, with her name
/// and how her node is, moving with the map.
class _YourStar extends StatelessWidget {
  const _YourStar({
    required this.camera,
    required this.node,
    required this.crowded,
    required this.colors,
    required this.open,
  });

  final _Camera camera;
  final HomeNode node;
  final bool crowded;
  final OspColors colors;
  final void Function(Widget screen) open;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final name = _ownName(node);
    final line = switch (node.reachability) {
      Reachability.online => 'Your node is running.',
      Reachability.offline => "Your phone can't reach your node right now.",
      _ => 'Reaching your node…',
    };
    const width = 260.0, star = 56.0;
    return ListenableBuilder(
      listenable: camera,
      builder: (context, child) {
        final at = camera.toScreen(Offset.zero);
        final view = Offset.zero & camera.size;
        // Off screen: still a placed child, so the Stack keeps its size.
        if (!view.inflate(star).contains(at)) {
          return const Positioned(left: 0, top: 0, child: SizedBox.shrink());
        }
        return Positioned(
          left: at.dx - width / 2,
          top: at.dy - star / 2,
          width: width,
          child: child!,
        );
      },
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Semantics(
            button: true,
            label: 'Your node',
            child: Tooltip(
              message: 'Your node',
              child: InkResponse(
                onTap: () => open(NodeSummaryScreen(node: node, open: open)),
                radius: star / 2,
                child: const SizedBox.square(dimension: star),
              ),
            ),
          ),
          Text(
            name,
            style: theme.textTheme.titleMedium?.copyWith(color: colors.onSky),
          ),
          if (!crowded || node.reachability != Reachability.online) ...[
            const SizedBox(height: Space.xs),
            Text(
              line,
              textAlign: TextAlign.center,
              style: theme.textTheme.bodySmall?.copyWith(
                color: colors.onSkyQuiet,
              ),
            ),
          ],
        ],
      ),
    );
  }
}

class _Control extends StatelessWidget {
  const _Control({
    required this.tooltip,
    required this.icon,
    required this.onPressed,
  });

  final String tooltip;
  final IconData icon;
  final VoidCallback onPressed;

  @override
  Widget build(BuildContext context) {
    final colors = OspColors.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: IconButton.filledTonal(
        tooltip: tooltip,
        onPressed: onPressed,
        icon: Icon(icon),
        style: IconButton.styleFrom(
          backgroundColor: colors.skyHaze,
          foregroundColor: colors.onSky,
        ),
      ),
    );
  }
}

/// The machines the node's machine name knows: their own band, apart from
/// people and never beside one (ADR 0033 §4).
class _Machines extends StatelessWidget {
  const _Machines({required this.machines});

  final List<UniverseMachine> machines;

  @override
  Widget build(BuildContext context) {
    if (machines.isEmpty) return const SizedBox.shrink();
    final colors = OspColors.of(context);
    final n = machines.length;
    return ActionChip(
      avatar: Icon(Icons.dns_outlined, size: 18, color: colors.onSky),
      label: Text(n == 1 ? '1 machine' : '$n machines'),
      labelStyle: TextStyle(color: colors.onSky),
      backgroundColor: colors.skyHaze,
      side: BorderSide.none,
      tooltip: 'Machines your node knows',
      onPressed: () => showModalBottomSheet<void>(
        context: context,
        showDragHandle: true,
        builder: (context) => _MachineList(machines: machines),
      ),
    );
  }
}

class _MachineList extends StatelessWidget {
  const _MachineList({required this.machines});

  final List<UniverseMachine> machines;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ListView(
      shrinkWrap: true,
      children: [
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: Space.m),
          child: Text(
            'Machines your node knows',
            style: theme.textTheme.titleMedium,
          ),
        ),
        const Padding(
          padding: EdgeInsets.fromLTRB(Space.m, Space.xs, Space.m, Space.s),
          child: Text(
            'Other people’s nodes, by the names they go by. Which person '
            'runs which isn’t shown.',
          ),
        ),
        for (final m in machines)
          ListTile(
            leading: Icon(switch (m.state) {
              MachineState.answeredLately => Icons.verified_outlined,
              MachineState.met => Icons.check_circle_outline,
              MachineState.heardOf => Icons.help_outline,
              MachineState.faded => Icons.nights_stay_outlined,
            }),
            title: Text(m.words),
            subtitle: Text(
              [
                switch (m.state) {
                  MachineState.heardOf => 'Heard of, hasn’t answered yet',
                  MachineState.met => 'Answered lately',
                  MachineState.answeredLately =>
                    'Has answered for a month, and lately',
                  MachineState.faded => 'Answered once, not lately',
                },
                if (m.directory) 'lists people who asked to be found',
                switch (m.activity) {
                  ActivityBand.none => null,
                  ActivityBand.some => 'some talk with your node',
                  ActivityBand.aLot => 'a lot of talk with your node',
                },
              ].nonNulls.join(' · '),
            ),
          ),
        const SizedBox(height: Space.m),
      ],
    );
  }
}

/// The person stood at: who they are to the owner, and their profile.
class _StandingAt extends StatelessWidget {
  const _StandingAt({
    required this.person,
    required this.snapshot,
    required this.node,
    required this.onClose,
  });

  final UniversePerson person;
  final UniverseSnapshot snapshot;
  final HomeNode node;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = person;
    final name = p.name.isEmpty ? 'Someone' : p.name;
    final i = snapshot.people.indexOf(p);
    final lines = snapshot.lines.where((l) => l.from == i || l.to == i).length;
    final posts = p.posts.toInt();
    final detail = [
      if (p.followed) 'You follow them',
      if (p.known) 'Someone you know',
      if (!p.followed && !p.known && p.foundThrough != null)
        listedBy(p.foundThrough!),
      if (posts > 0) posts == 1 ? '1 post here' : '$posts posts here',
      if (lines > 0)
        lines == 1 ? '1 public line shown' : '$lines public lines shown',
      if (p.group == null && p.step == null) 'In the public sky',
    ].join(' · ');
    return Card(
      child: ListTile(
        leading: CircleAvatar(child: Text(name.characters.first)),
        title: Text(name, style: theme.textTheme.titleSmall),
        subtitle: detail.isEmpty
            ? null
            : Text(detail, maxLines: 2, overflow: TextOverflow.ellipsis),
        onTap: () => Navigator.of(context).push(
          MaterialPageRoute<void>(
            builder: (_) =>
                ProfileScreen(node: node, identity: p.identity, name: p.name),
          ),
        ),
        trailing: IconButton(
          tooltip: 'Step back',
          icon: const Icon(Icons.close),
          onPressed: onClose,
        ),
      ),
    );
  }
}

/// Who your node has found, from the top's snapshot (ADR 0033), as a card
/// on the sky that opens the list (ADR 0035 §4).
class _FoundPeople extends StatelessWidget {
  const _FoundPeople({required this.node, required this.open});

  final HomeNode node;
  final void Function(Widget screen) open;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final universe = node.universeTop;
    // The owner is step 0; the nearest come first after them.
    final people = universe?.people.where((p) => p.step != 0).toList();
    final inLights = [
      for (final g in universe?.groups ?? const <UniverseGroup>[]) g.people,
      for (final c in universe?.sky ?? const <SkyCell>[]) c.people,
    ].fold(0, (a, b) => a + b);
    final count =
        (people?.length ?? 0) + inLights + (universe?.morePeople.toInt() ?? 0);
    final (title, detail) = switch (people) {
      null when node.universeError != null => (
        "Couldn't ask your node who it found",
        'Tap to try again.',
      ),
      null => ('Your node is finding people…', null),
      _ when count == 0 => (
        'Your node is looking for people',
        'It checks every few minutes. Or paste someone’s invite code.',
      ),
      _ when count == 1 => (
        'Your node found 1 person',
        people.firstOrNull?.name,
      ),
      _ => ('Your node found $count people', _names(people, count)),
    };
    return Card(
      child: ListTile(
        leading: const Icon(Icons.people_outline),
        title: Text(title, style: theme.textTheme.titleSmall),
        subtitle: detail == null
            ? null
            : Text(detail, maxLines: 1, overflow: TextOverflow.ellipsis),
        trailing: const Icon(Icons.chevron_right),
        // With nobody found, an invite code is the way in (ADR 0035 §4).
        onTap: () => open(
          people != null && count == 0
              ? PeopleScreen(node: node)
              : FoundPeopleScreen(node: node),
        ),
      ),
    );
  }

  static String _names(List<UniversePerson> people, int count) {
    final named = people.map((p) => p.name).where((n) => n.isNotEmpty);
    final first = named.take(3).toList();
    final more = count - first.length;
    return more > 0 ? '${first.join(', ')} and $more more' : first.join(', ');
  }
}

/// The layers the map can light up (ADR 0035 §1), one at a time.
class _LayerPicker extends StatelessWidget {
  const _LayerPicker({required this.layer, required this.onPick});

  final UniverseLayer? layer;
  final void Function(UniverseLayer? layer) onPick;

  /// How a lit layer is named on the map.
  static String chip(UniverseLayer layer) => switch (layer) {
    UniverseLayer.posts => 'Posts, last 90 days',
    UniverseLayer.video => 'Video, last 90 days',
    UniverseLayer.shared => 'Reshares, last 90 days',
  };

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    Widget option(
      UniverseLayer? value,
      IconData icon,
      String title,
      String detail,
    ) {
      final on = value == layer;
      return ListTile(
        leading: Icon(icon),
        title: Text(title),
        subtitle: Text(detail),
        trailing: on ? const Icon(Icons.check) : null,
        selected: on,
        onTap: () => onPick(value),
      );
    }

    return ListView(
      shrinkWrap: true,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(Space.m, 0, Space.m, Space.s),
          child: Text('What lights up', style: theme.textTheme.titleMedium),
        ),
        option(
          null,
          Icons.people_outline,
          'People',
          'Everyone your node knows, and the public lines between them',
        ),
        option(
          UniverseLayer.posts,
          Icons.article_outlined,
          'Posts',
          'Where public posts from the last 90 days are',
        ),
        option(
          UniverseLayer.video,
          Icons.movie_outlined,
          'Video',
          'Where public video from the last 90 days is',
        ),
        option(
          UniverseLayer.shared,
          Icons.repeat,
          'Shared',
          'Where reshares from the last 90 days are spreading',
        ),
        const ListTile(
          enabled: false,
          leading: Icon(Icons.dns_outlined),
          title: Text('Network'),
          subtitle: Text('Machines and what they hold for people. Not yet.'),
        ),
        const SizedBox(height: Space.m),
      ],
    );
  }
}

/// A search shown on the map: what it found, a way back to it, and its
/// list (ADR 0035 §3).
class _Results extends StatelessWidget {
  const _Results({
    required this.lights,
    required this.onFly,
    required this.onList,
    required this.onClose,
  });

  final SearchLights lights;
  final VoidCallback onFly, onList, onClose;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    String count(int n, String one, String many) =>
        n == 1 ? '1 $one' : '$n $many';
    final found = [
      if (lights.people > 0) count(lights.people, 'person', 'people'),
      if (lights.posts > 0) count(lights.posts, 'post', 'posts'),
    ].join(' and ');
    return Card(
      child: Padding(
        padding: const EdgeInsets.only(bottom: Space.xs),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              leading: const Icon(Icons.search),
              title: Text(
                lights.words,
                style: theme.textTheme.titleSmall,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              subtitle: Text(
                found.isEmpty ? 'Nothing found' : '$found, lit up',
              ),
              trailing: IconButton(
                tooltip: 'Clear the search',
                icon: const Icon(Icons.close),
                onPressed: onClose,
              ),
            ),
            Row(
              mainAxisAlignment: MainAxisAlignment.end,
              children: [
                TextButton.icon(
                  onPressed: onFly,
                  icon: const Icon(Icons.flight_takeoff),
                  label: const Text('Fly there'),
                ),
                TextButton.icon(
                  onPressed: onList,
                  icon: const Icon(Icons.list),
                  label: const Text('List'),
                ),
                const SizedBox(width: Space.s),
              ],
            ),
          ],
        ),
      ),
    );
  }
}
