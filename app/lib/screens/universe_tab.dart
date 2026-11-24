import 'dart:async';
import 'dart:math';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';

import '../connection_lights.dart';
import '../format.dart';
import '../network_events.dart';
import '../node.dart';
import '../search_lights.dart';
import '../src/rust/api/osp.dart';
import '../theme.dart';
import '../widgets/person_avatar.dart';
import 'events_screen.dart';
import 'found_people_screen.dart';
import 'node_summary_screen.dart';
import 'people_screen.dart';
import 'profile_screen.dart';
import 'region_sheet.dart';
import 'search_screen.dart';
import 'share_universe_screen.dart';

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
    with TickerProviderStateMixin {
  final _camera = _Camera();
  late final AnimationController _fly;
  Offset _fromCentre = Offset.zero, _toCentre = Offset.zero;
  double _fromScale = _Fit.near, _toScale = _Fit.near;

  /// The region last framed, and each region's own framing.
  String? _framed;
  final Map<String, double> _fits = {};

  /// Set when the next region comes from zooming out: the map stays.
  bool _stay = false;

  /// What the map draws now: the last snapshot, kept while the next
  /// region's comes, with the layer it was asked with and what's lit.
  _Scene _scene = const _Scene();

  /// What the map drew before the last change, fading out as [_scene]
  /// fades in (ADR 0040 §4). Nothing fades under reduced motion.
  _Scene? _before;
  late final AnimationController _fade;
  UniverseSnapshot? get _shown => _scene.snapshot;

  /// The search last flown to, and whether the next region framed is
  /// framed on its results.
  SearchLights? _flown;
  bool _onResults = false;

  /// The connection last flown to (ADR 0037), and whether the next region
  /// framed is framed on its paths.
  ConnectionLights? _flownPaths;
  bool _onPaths = false;

  /// The last gesture update: where the fingers were, the recognizer's
  /// scale then, and how many fingers. Each update moves the camera by what
  /// changed since, so a finger landing or lifting doesn't make it jump.
  Offset _lastFocal = Offset.zero;
  double _lastFactor = 1;
  int _lastFingers = 0;

  final _labels = _Labels();

  /// The event being drawn (ADR 0036 §1), one at a time, and the last one
  /// whose line still shows under the map.
  late final AnimationController _event;
  UniverseEvent? _playing;
  UniverseEvent? _captioned;
  Timer? _captionGone;

  HomeNode get _node => widget.node;

  @override
  void initState() {
    super.initState();
    _fly = AnimationController(vsync: this, duration: Motion.long)
      ..addListener(_flying);
    _fade = AnimationController(vsync: this, duration: Motion.medium)
      ..addStatusListener((status) {
        if (status == AnimationStatus.completed) _before = null;
      });
    _event = AnimationController(vsync: this, duration: _Play.length)
      ..addStatusListener((status) {
        if (status != AnimationStatus.completed) return;
        _playing = null;
        _captionGone?.cancel();
        _captionGone = Timer(_Play.caption, () {
          if (mounted) setState(() => _captioned = null);
        });
        _node.drewEvent();
      });
  }

  /// Draws the next event the node sent, if the map is on screen and none
  /// is being drawn. With none, the map stays still (ADR 0036 §2).
  void _playNext() {
    // One drawn lately has grown: its line again, with the total.
    final grown = _node.events.grown;
    if (grown != null && _playing == null) {
      _node.events.grown = null;
      _captionGone?.cancel();
      setState(() => _captioned = grown);
      _captionGone = Timer(_Play.caption, () {
        if (mounted) setState(() => _captioned = null);
      });
    }
    final next = _node.events.next;
    if (_playing != null || next == null || !_node.watchingUniverse) return;
    _captionGone?.cancel();
    setState(() => _playing = _captioned = next);
    _event.forward(from: 0);
  }

  void _openEvents() =>
      widget.open(EventsScreen(node: _node, open: widget.open));

  /// Each movement's length, zero when the phone asks for reduced motion:
  /// the camera jumps, the map changes at once, and an event is told in
  /// words under the map without moving on it.
  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _fly.duration = Motion.of(context, Motion.long);
    _fade.duration = Motion.of(context, Motion.medium);
    _event.duration = Motion.of(context, _Play.length);
  }

  @override
  void dispose() {
    _fly.dispose();
    _fade.dispose();
    _event.dispose();
    _captionGone?.cancel();
    _camera.dispose();
    _labels.dispose();
    super.dispose();
  }

  void _flying() {
    final t = Motion.curve.transform(_fly.value);
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
    if (_onPaths) {
      _onPaths = false;
      if (_framePaths(s)) return;
    }
    _flyTo(_lifted(middle, scale), scale);
  }

  /// Shows the paths of "How are we connected?" (ADR 0037): they start at
  /// the owner, so from the top, framed on what they light.
  void _flyToPaths() {
    final here = _node.universe;
    if (_node.universeRegion.isEmpty && here != null) {
      _framePaths(here);
      return;
    }
    _stay = false;
    _onPaths = true;
    _framed = null;
    _node.openUniverse('');
  }

  /// Frames the owner and the ends of the paths drawn on [s]; false if
  /// none is drawn.
  bool _framePaths(UniverseSnapshot s) {
    final c = _node.connection;
    if (c == null || _camera.size.isEmpty) return false;
    final lines = c.lines(s);
    if (lines.isEmpty) return false;
    var bounds = Rect.fromPoints(Offset.zero, Offset.zero);
    for (final l in lines) {
      bounds = bounds
          .expandToInclude(Rect.fromPoints(l.from, l.from))
          .expandToInclude(Rect.fromPoints(l.to, l.to));
    }
    final half = max(bounds.width, bounds.height) / 2;
    final room = _clear.shortestSide / 2 * 0.8;
    final scale = half > 0 ? min(room / half, _Fit.near * 2) : _Fit.near;
    _flyTo(_lifted(bounds.center, scale), scale);
    return true;
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

  void _tap(Offset screen, UniverseSnapshot s) =>
      _act(_hit(s, _camera, screen));

  /// What a tap on [hit] does, by finger or by a screen reader.
  void _act(_Hit? hit) {
    switch (hit) {
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

  /// Zooms by [factor] about the middle, for a screen reader's actions,
  /// which can't pinch. Out far enough, it goes up the path as a pinch does.
  void _zoom(double factor) {
    _flyTo(_camera.centre, _camera.scale * factor);
    if (factor < 1) {
      final fit = _fits[_node.universeRegion];
      if (fit != null && _toScale < fit * _Fit.up) _up();
    }
  }

  /// The pinch hint is done: shown to the end, or the map already pinched.
  void _hinted() {
    if (!_node.zoomHintDue) return;
    _node.zoomHintDone();
    if (mounted) setState(() {});
  }

  void _scaleStart(ScaleStartDetails d) {
    if (d.pointerCount > 1) _hinted();
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
    _hinted();
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
    final lights = node.searchLights;
    final paths = node.connection;
    // The map changed: what it drew fades out as the new fades in. A layer
    // shows with the snapshot asked for it, not before.
    final scene = _Scene(
      snapshot: here ?? _scene.snapshot,
      layer: here != null ? node.universeLayer : _scene.layer,
      lights: lights,
      connection: paths,
    );
    if (!scene.sameAs(_scene)) {
      _before = _scene;
      _fade.forward(from: 0);
    }
    _scene = scene;
    final s = _shown;
    if (here != null && _framed != here.region) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && node.universe == here) _frame(here);
      });
    }
    if (_playing == null &&
        (node.events.next != null || node.events.grown != null)) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted) _playNext();
      });
    }
    final standing = _standingAt(s, node.universeRegion);
    // A connection just answered: fly to its paths once.
    if (paths != null && !paths.asking && !identical(paths, _flownPaths)) {
      _flownPaths = paths;
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (mounted && identical(node.connection, paths)) _flyToPaths();
      });
    }
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
                  // The map as a whole, then each light on screen as its own
                  // button, so a screen reader can go star by star. Outside
                  // the gestures, so the map's tap is named too.
                  child: Semantics(
                    container: true,
                    label: _describe(s, scene.layer, lights),
                    customSemanticsActions: {
                      _zoomIn: () => _zoom(2),
                      _zoomOut: () => _zoom(0.5),
                    },
                    child: GestureDetector(
                      key: const Key('universe-map'),
                      behavior: HitTestBehavior.opaque,
                      onScaleStart: _scaleStart,
                      onScaleUpdate: _scaleUpdate,
                      onScaleEnd: (_) => _scaleEnd(),
                      onTapUp: s == null
                          ? null
                          : (d) => _tap(d.localPosition, s),
                      // Rebuilt as the camera moves, so the buttons a screen
                      // reader finds are where the lights are drawn.
                      child: ListenableBuilder(
                        listenable: _camera,
                        builder: (context, _) => CustomPaint(
                          painter: _Map(
                            camera: _camera,
                            colors: colors,
                            scene: scene,
                            standing: standing,
                            labels: _labels,
                            text: MediaQuery.textScalerOf(context),
                            ownName: _ownName(node),
                            event: _playing,
                            played: _event,
                            onTap: _act,
                            before: switch (_before) {
                              final b? => _Map(
                                camera: _camera,
                                colors: colors,
                                scene: b,
                                standing: _standingAt(
                                  b.snapshot,
                                  b.snapshot?.region ?? '',
                                ),
                                labels: _labels,
                                text: MediaQuery.textScalerOf(context),
                                ownName: _ownName(node),
                                played: _event,
                              ),
                              null => null,
                            },
                            fade: _fade,
                          ),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
              // Once, with someone on the map: that it zooms. Only while
              // the Universe is the tab shown, as the others keep running.
              if (node.zoomHintDue &&
                  node.watchingUniverse &&
                  s != null &&
                  (s.people.length > 1 || s.groups.isNotEmpty))
                Positioned(
                  left: 0,
                  right: 0,
                  top: _Fit.clearTop,
                  height: _clear.height,
                  child: _PinchHint(colors: colors, onDone: _hinted),
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
                    _Machines(
                      machines: s?.machines ?? const [],
                      turn: _playing?.kind == EventKind.machine
                          ? _playing!.met
                          : null,
                      onNode: () => widget.open(
                        NodeSummaryScreen(node: node, open: widget.open),
                      ),
                    ),
                    // Her node out of reach: what's drawn is from before
                    // (ADR 0036 §3).
                    if (s != null && node.reachability == Reachability.offline)
                      Chip(
                        avatar: Icon(
                          Icons.history,
                          size: 18,
                          color: colors.onSky,
                        ),
                        label: Text(switch (node.lastContact) {
                          final at? => 'As last seen, ${formatAgo(at)}',
                          null => 'As last seen',
                        }),
                        labelStyle: TextStyle(color: colors.onSky),
                        backgroundColor: colors.skyHaze,
                        side: BorderSide.none,
                      ),
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
                    _Control(
                      tooltip: 'Share my Universe',
                      icon: Icons.ios_share,
                      onPressed: () =>
                          widget.open(ShareUniverseScreen(node: node)),
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
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    if (_captioned case final e?)
                      _EventLine(
                        event: node.events.latest(e),
                        names: EventNames([
                          ...?node.universeTop?.people,
                          ...?s?.people,
                        ]),
                        onTap: _openEvents,
                      ),
                    if (paths != null || node.connectionError != null)
                      _PathsCard(
                        paths: paths,
                        error: node.connectionError,
                        onClose: () => node.showConnection(null),
                      )
                    else
                      switch ((standing, lights)) {
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
                  ],
                ),
              ),
            ],
          );
        },
      ),
    );
  }

  static const _zoomIn = CustomSemanticsAction(label: 'Zoom in');
  static const _zoomOut = CustomSemanticsAction(label: 'Zoom out');

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

/// Two fingertips spreading and closing, twice, over the map: it zooms.
/// Touches go through to the map.
class _PinchHint extends StatefulWidget {
  const _PinchHint({required this.colors, required this.onDone});

  final OspColors colors;
  final VoidCallback onDone;

  @override
  State<_PinchHint> createState() => _PinchHintState();
}

class _PinchHintState extends State<_PinchHint>
    with SingleTickerProviderStateMixin {
  /// Each spread and close.
  static const _once = Duration(milliseconds: 1400);
  static const _times = 2;

  late final AnimationController _play = AnimationController(
    vsync: this,
    duration: _once * _times,
  )..forward().whenComplete(() => widget.onDone());

  @override
  void dispose() {
    _play.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final colors = widget.colors;
    // Asked for less motion: the fingertips held apart, shown as long.
    final still = Motion.of(context, Motion.quick) == Duration.zero;
    // A screen reader has the map's own zoom actions instead.
    return ExcludeSemantics(
      child: IgnorePointer(
        child: AnimatedBuilder(
          animation: _play,
          builder: (context, _) {
            final t = _play.value;
            // In and out gently, at the start and the end.
            final shown = still ? 1.0 : min(1.0, min(t, 1 - t) * 10);
            // Out and back within each one.
            final round = (t * _times) % 1;
            final spread = still
                ? 0.5
                : Curves.easeInOut.transform(
                    round < 0.5 ? round * 2 : 2 - round * 2,
                  );
            final apart = 16 + 44 * spread;
            return Opacity(
              opacity: shown,
              // Low on the map, clear of her star and name in the middle.
              child: Column(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  SizedBox(
                    width: 160,
                    height: 120,
                    child: CustomPaint(
                      painter: _Fingertips(apart, colors.onSky),
                    ),
                  ),
                  DecoratedBox(
                    decoration: BoxDecoration(
                      color: colors.skyHaze,
                      borderRadius: BorderRadius.circular(16),
                    ),
                    child: Padding(
                      padding: const EdgeInsets.symmetric(
                        horizontal: Space.m,
                        vertical: Space.xs,
                      ),
                      child: Text(
                        'Pinch to zoom',
                        style: TextStyle(color: colors.onSky),
                      ),
                    ),
                  ),
                ],
              ),
            );
          },
        ),
      ),
    );
  }
}

class _Fingertips extends CustomPainter {
  _Fingertips(this.apart, this.color);

  final double apart;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final mid = size.center(Offset.zero);
    // Along a slant, as a thumb and finger are.
    final along = Offset(cos(-pi / 4), sin(-pi / 4)) * apart;
    final fill = Paint()..color = color.withValues(alpha: 0.35);
    final ring = Paint()
      ..style = PaintingStyle.stroke
      ..strokeWidth = 2
      ..color = color.withValues(alpha: 0.8);
    for (final at in [mid + along, mid - along]) {
      canvas.drawCircle(at, 14, fill);
      canvas.drawCircle(at, 14, ring);
    }
  }

  @override
  bool shouldRepaint(_Fingertips old) =>
      old.apart != apart || old.color != color;
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

/// A person on the map in words, for a screen reader: their name, who they
/// are to the owner, how far, and what's lit on them.
@visibleForTesting
String describePerson(
  UniversePerson p, {
  UniverseLayer? layer,
  bool found = false,
  bool standing = false,
}) => [
  p.name.isEmpty ? 'Someone' : p.name,
  if (standing) 'You are standing here',
  if (p.followed) 'You follow them',
  if (p.known) 'Someone you know',
  if (!p.followed && !p.known && p.foundThrough != null)
    listedBy(p.foundThrough!),
  switch (p.step) {
    null when p.group == null => 'In the public sky',
    1 => '1 step from you',
    final n? => '$n steps from you',
    null => null,
  },
  ?_litWords(layer, p.lit),
  if (found) 'lit up',
].nonNulls.join('. ');

/// How much of [layer] a light holds, in words, if it holds any.
String? _litWords(UniverseLayer? layer, int lit) {
  if (layer == null || lit == 0) return null;
  final one = lit == 1;
  return switch (layer) {
    UniverseLayer.posts => one ? '1 post lately' : '$lit posts lately',
    UniverseLayer.video => one ? '1 video lately' : '$lit videos lately',
    UniverseLayer.shared => one ? '1 reshare lately' : '$lit reshares lately',
  };
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
    // Nobody placed yet: the sky's people and cells shown around her, so
    // a newcomer's handful fills the screen with room for their names.
    if (far == 0) {
      for (final p in s.people) {
        far = max(far, Offset(p.x, p.y).distance);
      }
      for (final c in s.sky) {
        far = max(far, Offset(c.atX, c.atY).distance);
      }
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

/// How long an event is drawn, and how long its line stays after.
abstract final class _Play {
  static const length = Duration(milliseconds: 3200);
  static const caption = Duration(seconds: 8);
}

/// Each event drawn on the map, by `seq`, so a test can check the map
/// draws nothing it wasn't sent (ADR 0036 §2).
@visibleForTesting
final debugDrawnEvents = <BigInt>[];

/// A mark an event leaves on the map, in the world's units.
sealed class EventMark {}

/// A ring opening at [at], where people just found are; with none, the
/// public sky's edge, where they went.
class EventRing extends EventMark {
  EventRing(this.at);
  final Offset? at;
}

/// Media flowing from [from] to [to]: for a fetch, from the author's star
/// when their own node sent it, or a hosting resharer's. One that's none is
/// "another node":
/// the edge of the view at [angle], the same for the same post, which
/// stands for no place.
class EventFlow extends EventMark {
  EventFlow({this.from, this.to, required this.angle, this.kept = false});
  final Offset? from, to;
  final double angle;

  /// Kept available by a copy (ADR 0036 §1).
  final bool kept;
}

/// A reshare: a line from the author out to whoever reshared.
class EventLine extends EventMark {
  EventLine(this.from, this.to);
  final Offset from, to;
}

/// What [e] draws on [s]: only what [e] names, where [s] places it. Someone
/// it names who isn't on the map draws nothing; a machine is shown on the
/// machines chip, not here.
@visibleForTesting
List<EventMark> eventMarks(UniverseEvent e, UniverseSnapshot s) {
  Offset? at(String? identity) {
    if (identity == null) return null;
    for (final p in s.people) {
      if (p.identity == identity) return Offset(p.x, p.y);
    }
    return null;
  }

  // The owner is at the centre.
  const home = Offset.zero;
  final angle = _angleOf(e.post ?? e.author ?? e.kind.name);
  switch (e.kind) {
    case EventKind.found:
      final found = [
        for (final p in s.people)
          if (p.step != 0 &&
              p.foundThrough != null &&
              p.foundThrough == e.words)
            EventRing(Offset(p.x, p.y)),
      ];
      return found.isEmpty ? [EventRing(null)] : found.take(24).toList();
    case EventKind.reshared:
      final author = at(e.author), from = at(e.from);
      if (author == null || from == null || author == from) return [];
      return [EventLine(author, from)];
    case EventKind.fetched:
      return [
        EventFlow(
          from: at(e.fromAuthor ? e.author : e.host),
          to: home,
          angle: angle,
          kept: e.keptByCopy,
        ),
      ];
    case EventKind.served:
      return [EventFlow(from: home, angle: angle)];
    case EventKind.machine:
      return [];
  }
}

/// An angle from [text], the same each time.
double _angleOf(String text) {
  var h = 0x811c9dc5;
  for (final c in text.codeUnits) {
    h = ((h ^ c) * 0x01000193) & 0xffffffff;
  }
  return h / 0xffffffff * 2 * pi;
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

  /// [text] at [size], or with [halo] its letters' rim alone, in that
  /// colour, to draw under it.
  TextPainter of(String text, Color color, double size, {Color? halo}) {
    final key = '$text|${color.toARGB32()}|$size|${halo?.toARGB32()}';
    return _cache.putIfAbsent(key, () {
      if (_cache.length > 800) _clear();
      return TextPainter(
        text: TextSpan(
          text: text,
          style: TextStyle(
            color: halo == null ? color : null,
            foreground: halo == null
                ? null
                : (Paint()
                    ..style = PaintingStyle.stroke
                    ..strokeWidth = 3
                    ..strokeJoin = StrokeJoin.round
                    ..color = halo.withValues(alpha: 0.85)),
            fontSize: size,
            fontFamily: Fonts.family,
            fontWeight: FontWeight.w500,
          ),
        ),
        textDirection: TextDirection.ltr,
        maxLines: 1,
        ellipsis: '…',
        // About 13 letters at 11 px, more as the text grows.
      )..layout(maxWidth: size * 13);
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

/// What the map draws: a snapshot, the layer it was asked with, and what a
/// search or a connection lights on it.
class _Scene {
  const _Scene({this.snapshot, this.layer, this.lights, this.connection});

  final UniverseSnapshot? snapshot;

  /// What lights up, if anything does but people (ADR 0035 §1).
  final UniverseLayer? layer;

  /// A search's results to light up (ADR 0035 §3).
  final SearchLights? lights;

  /// The paths of "How are we connected?" to draw (ADR 0037).
  final ConnectionLights? connection;

  /// Whether the map would draw the same: the node hands back the same
  /// snapshot while nothing changed.
  bool sameAs(_Scene o) =>
      identical(snapshot, o.snapshot) &&
      layer == o.layer &&
      identical(lights, o.lights) &&
      identical(connection, o.connection);
}

/// Names on the map grow with the phone's text size up to this much, past
/// which they'd cover the stars they name.
const _mapTextMost = 1.5;

/// The map: the sky, its lights, the lines and the people, in that order.
/// Each light on screen is also a button for screen readers.
class _Map extends CustomPainter {
  _Map({
    required this.camera,
    required this.colors,
    required _Scene scene,
    required this.standing,
    required this.labels,
    required TextScaler text,
    required this.ownName,
    this.event,
    required this.played,
    this.onTap,
    this.before,
    Animation<double>? fade,
  }) : snapshot = scene.snapshot,
       layer = scene.layer,
       lights = scene.lights,
       connection = scene.connection,
       text = text.clamp(maxScaleFactor: _mapTextMost),
       fade = fade ?? kAlwaysCompleteAnimation,
       centre = camera.centre,
       scale = camera.scale,
       super(repaint: Listenable.merge([camera, played, ?fade]));

  final _Camera camera;
  final OspColors colors;
  final UniverseSnapshot? snapshot;
  final UniversePerson? standing;
  final _Labels labels;

  /// The phone's text size for names on the map, held to [_mapTextMost].
  final TextScaler text;

  /// The owner's name as drawn under her star, to keep clear.
  final String ownName;

  final UniverseLayer? layer;
  final SearchLights? lights;
  final ConnectionLights? connection;

  /// The event being drawn (ADR 0036 §1), and how far along it is.
  final UniverseEvent? event;
  final Animation<double> played;

  /// What a tap on a light does, for screen readers.
  final void Function(_Hit hit)? onTap;

  /// The map before its last change, fading out as this one fades in.
  final _Map? before;
  final Animation<double> fade;

  /// Where the camera was when this was made, for the screen reader's
  /// buttons to follow it.
  final Offset centre;
  final double scale;

  /// How bright a light is drawn: as usual, or dimmed when a layer is on
  /// and it holds none of the layer's items, or a search is shown and it
  /// holds no result.
  double _shade(int lit, bool found) {
    if (layer != null && lit == 0) return 0.35;
    final lighting = lights != null || connection != null;
    if (layer == null && lighting && !found) return 0.5;
    return 1;
  }

  /// Whether a person or group is lit by the search or the connection.
  bool _foundPerson(UniversePerson p) =>
      (lights?.holdsPerson(p) ?? false) ||
      (connection?.holdsPerson(p) ?? false);
  bool _foundGroup(UniverseGroup g) =>
      (lights?.holdsGroup(g) ?? false) || (connection?.holdsGroup(g) ?? false);

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
    _sky(canvas, size);
    final t = fade.value;
    final b = before;
    if (b == null || t >= 1) {
      _lights(canvas, size);
      return;
    }
    // Each drawn whole, then faded, so where they overlap neither shows
    // through the other.
    final view = Offset.zero & size;
    final eased = Motion.curve.transform(t);
    canvas.saveLayer(view, Paint()..color = Color.fromRGBO(0, 0, 0, 1 - eased));
    b._lights(canvas, size);
    canvas.restore();
    canvas.saveLayer(view, Paint()..color = Color.fromRGBO(0, 0, 0, eased));
    _lights(canvas, size);
    canvas.restore();
  }

  /// The night behind everything: the haze round her star and the dust.
  void _sky(Canvas canvas, Size size) {
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
  }

  /// Everything the snapshot places, and the names.
  void _lights(Canvas canvas, Size size) {
    final view = Offset.zero & size;
    final home = camera.toScreen(Offset.zero);
    final paint = Paint();
    final s = snapshot;
    // Nothing from her node yet, or it couldn't answer: her star still.
    if (s == null) {
      _yourStar(canvas, home);
      return;
    }
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
        final label = labels.of(
          'The public sky',
          colors.onSkyQuiet,
          text.scale(11),
        );
        label.paint(canvas, top - Offset(label.width / 2, label.height + 4));
      }
    }

    // How many each cell and group holds, placed after the lights where
    // they don't cover one another.
    final counts = <(Offset, double, int)>[];

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
      counts.add((at, r, c.people));
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
            (l.kind == LineKind.creditedEarly
                    ? colors.creditedEarly
                    : colors.reshared)
                .withValues(alpha: touches ? 0.8 : 0.35);
      canvas.drawLine(a, b, line);
    }

    if (connection case final c?) _drawPaths(canvas, c.lines(s), view);

    // Groups, closed, each one light with how many it holds.
    for (final g in s.groups) {
      final at = camera.toScreen(Offset(g.x, g.y));
      final r = _groupRadius(g.people);
      if (!seen(at, r)) continue;
      final found = _foundGroup(g);
      _marks(canvas, at, r * 0.6, g.lit, found);
      _glow(
        canvas,
        at,
        r,
        colors.group,
        (0.45 + g.bright / 100 * 0.5) * _shade(g.lit, found),
      );
      counts.add((at, r, g.people));
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
        _yourStar(canvas, at);
        continue;
      }
      final inSky = p.group == null && p.step == null;
      final color = inSky
          ? colors.distantStar
          : (p.followed || p.known ? colors.followed : colors.near);
      final r = _personRadius(p.bright) * (few ? 1.8 : 1);
      final found = _foundPerson(p);
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

    if (event case final e?) _drawEvent(canvas, size, e, s, home);

    // Her star and her own name under it take their space first.
    final own = labels.of(ownName, colors.onSky, text.scale(16));
    final taken = <Rect>[
      Rect.fromCircle(center: home, radius: 14),
      Rect.fromCenter(
        center: home + Offset(0, 28 + own.height / 2),
        width: own.width + 8,
        height: own.height + 4,
      ),
    ];

    // Then the counts, the largest first: one that would cover another is
    // left out, as a name is.
    counts.sort((a, b) => b.$3.compareTo(a.$3));
    for (final (at, r, people) in counts) {
      final label = labels.of('$people', colors.onSky, text.scale(10));
      final box =
          (at + Offset(-label.width / 2, r * 0.5 + 2)) &
          Size(label.width, label.height);
      if (taken.any((t) => t.overlaps(box))) continue;
      taken.add(box.inflate(2));
      label.paint(canvas, box.topLeft);
    }

    // Names where there's room: the one stood at first, then those the
    // owner follows or knows, then the rest in the snapshot's order.
    int rank(UniversePerson p) => p == standing
        ? 0
        : _foundPerson(p)
        ? 1
        : (p.followed || p.known ? 2 : 3);
    names.sort((a, b) => rank(a.$1).compareTo(rank(b.$1)));
    // Each named person's own lines leave their star: their name keeps off
    // them, where it can.
    final index = {for (final (i, p) in s.people.indexed) p: i};
    final ends = <int, List<Offset>>{};
    if (names.isNotEmpty) {
      for (final l in s.lines) {
        if (l.from >= points.length || l.to >= points.length) continue;
        ends.putIfAbsent(l.from, () => []).add(points[l.to]);
        ends.putIfAbsent(l.to, () => []).add(points[l.from]);
      }
    }
    final size11 = text.scale(11);
    for (final (p, at, r) in names) {
      final here = p == standing;
      final name = p.name.isEmpty ? 'Someone' : p.name;
      final color = here ? colors.onSky : colors.onSkyQuiet;
      final label = labels.of(name, color, size11);
      // Clear of the ring round the one stood at.
      final gap = here ? r + 9 : r + 3;
      final w = label.width, h = label.height;
      // Below first, as names usually are; then above, right and left.
      final boxes = [
        Rect.fromLTWH(at.dx - w / 2, at.dy + gap, w, h),
        Rect.fromLTWH(at.dx - w / 2, at.dy - gap - h, w, h),
        Rect.fromLTWH(at.dx + gap + 2, at.dy - h / 2, w, h),
        Rect.fromLTWH(at.dx - gap - 2 - w, at.dy - h / 2, w, h),
      ];
      final mine = ends[index[p]] ?? const <Offset>[];
      bool free(Rect box) => !taken.any((t) => t.overlaps(box));
      bool clear(Rect box) => !mine.any((end) => _crosses(box, at, end));
      final box =
          boxes.where((b) => free(b) && clear(b)).firstOrNull ??
          boxes.where(free).firstOrNull ??
          (here ? boxes.first : null);
      if (box == null) continue;
      taken.add(box.inflate(2));
      // A rim of the night round the letters, so a line passing under
      // them doesn't cross them out.
      labels
          .of(name, color, size11, halo: colors.sky)
          .paint(canvas, box.topLeft);
      label.paint(canvas, box.topLeft);
    }
  }

  /// Whether the line from [a] to [b] passes through [box].
  static bool _crosses(Rect box, Offset a, Offset b) {
    if (box.contains(a) || box.contains(b)) return true;
    // Clipped to the box's sides in turn (Liang–Barsky).
    var t0 = 0.0, t1 = 1.0;
    final d = b - a;
    for (final (p, q) in [
      (-d.dx, a.dx - box.left),
      (d.dx, box.right - a.dx),
      (-d.dy, a.dy - box.top),
      (d.dy, box.bottom - a.dy),
    ]) {
      if (p == 0) {
        if (q < 0) return false;
        continue;
      }
      final t = q / p;
      if (p < 0) {
        if (t > t1) return false;
        t0 = max(t0, t);
      } else {
        if (t < t0) return false;
        t1 = min(t1, t);
      }
    }
    return t0 <= t1;
  }

  @override
  SemanticsBuilderCallback? get semanticsBuilder =>
      onTap == null ? null : _semantics;

  /// A button for each light on screen, in words: who, and who they are to
  /// the owner. Her own star is a button of its own over the map.
  List<CustomPainterSemantics> _semantics(Size size) {
    final s = snapshot;
    final tap = onTap;
    if (s == null || tap == null) return const [];
    final view = Offset.zero & size;
    final out = <CustomPainterSemantics>[];
    void add(
      String key,
      Offset world,
      double r,
      String label,
      String hint,
      _Hit hit,
    ) {
      final at = camera.toScreen(world);
      if (!view.contains(at) || out.length >= 80) return;
      out.add(
        CustomPainterSemantics(
          key: ValueKey(key),
          // At least a fingertip across, as buttons are.
          rect: Rect.fromCircle(center: at, radius: max(r, 24)),
          properties: SemanticsProperties(
            button: true,
            label: label,
            hintOverrides: SemanticsHintOverrides(onTapHint: hint),
            onTap: () => tap(hit),
            textDirection: TextDirection.ltr,
          ),
        ),
      );
    }

    final few = s.people.length <= 40;
    for (final p in s.people) {
      if (p.step == 0) continue;
      final r = _personRadius(p.bright) * (few ? 1.8 : 1);
      add(
        'p:${p.identity}',
        Offset(p.x, p.y),
        r,
        describePerson(
          p,
          layer: layer,
          found: _foundPerson(p),
          standing: p == standing,
        ),
        'stand there',
        _OnPerson(p),
      );
    }
    for (final g in s.groups) {
      add(
        'g:${g.id}',
        Offset(g.x, g.y),
        _groupRadius(g.people),
        [
          g.people == 1
              ? 'A group of 1 person'
              : 'A group of ${g.people} people',
          ?_litWords(layer, g.lit),
          if (_foundGroup(g)) 'lit up',
        ].join('. '),
        'see who is in it',
        _OnGroup(g),
      );
    }
    for (final c in s.sky) {
      add(
        'sky:${c.level}/${c.x}/${c.y}',
        Offset(c.atX, c.atY),
        _cellRadius(c.people),
        [
          c.people == 1
              ? 'Part of the public sky, 1 person'
              : 'Part of the public sky, ${c.people} people',
          ?_litWords(layer, c.lit),
          if (lights?.holdsCell(c) ?? false) 'lit up',
        ].join('. '),
        'go closer',
        _OnCell(c),
      );
    }
    return out;
  }

  @override
  bool shouldRebuildSemantics(_Map old) =>
      old.snapshot != snapshot ||
      old.standing != standing ||
      old.layer != layer ||
      old.lights != lights ||
      old.connection != connection ||
      old.centre != centre ||
      old.scale != scale ||
      old.camera.size != camera.size;

  /// The paths of "How are we connected?" (ADR 0037 §3): the owner's own
  /// follows and the people she knows in one colour, signed reshares and
  /// credits in theirs, and a gap dashed, with nobody inside.
  void _drawPaths(Canvas canvas, List<PathLine> lines, Rect view) {
    for (final l in lines) {
      final a = camera.toScreen(l.from), b = camera.toScreen(l.to);
      if (!view.overlaps(Rect.fromPoints(a, b).inflate(2))) continue;
      final paint = Paint()
        ..strokeWidth = 2.5
        ..strokeCap = StrokeCap.round
        ..color = switch (l.how) {
          HopKind.follows || HopKind.knows => colors.followed,
          HopKind.reshared => colors.reshared,
          HopKind.creditedEarly => colors.creditedEarly,
          HopKind.gap => colors.onSkyQuiet,
        }.withValues(alpha: 0.95);
      if (l.how != HopKind.gap) {
        canvas.drawLine(a, b, paint);
        continue;
      }
      final length = (b - a).distance;
      if (length == 0) continue;
      final step = (b - a) / length;
      const dash = 6.0, space = 5.0;
      for (var d = 0.0; d < length; d += dash + space) {
        canvas.drawLine(a + step * d, a + step * min(d + dash, length), paint);
      }
    }
  }

  /// [e]'s marks, from where the snapshot places what it names, faded in
  /// and out as it plays. Nothing else is drawn for it.
  void _drawEvent(
    Canvas canvas,
    Size size,
    UniverseEvent e,
    UniverseSnapshot s,
    Offset home,
  ) {
    final marks = eventMarks(e, s);
    if (marks.isEmpty) return;
    debugDrawnEvents.add(e.seq);
    final t = played.value;
    final fade = sin(pi * t);
    // A flow to or from "another node" comes from the edge of the view.
    final edge = size.shortestSide * 0.45;
    Offset far(double angle) => home + Offset(cos(angle), sin(angle)) * edge;
    for (final m in marks) {
      switch (m) {
        case EventRing(:final at):
          final c = at == null ? home : camera.toScreen(at);
          final r = at == null
              ? _Fit.skyInner * camera.scale
              : 6 + 28 * Curves.easeOut.transform(t);
          canvas.drawCircle(
            c,
            r,
            Paint()
              ..style = PaintingStyle.stroke
              ..strokeWidth = at == null ? 1 + 3 * fade : 2
              ..color = colors.yourStar.withValues(alpha: 0.8 * fade),
          );
        case EventFlow(:final from, :final to, :final angle, :final kept):
          final a = from == null ? far(angle) : camera.toScreen(from);
          final b = to == null ? far(angle) : camera.toScreen(to);
          final color = kept
              ? colors.kept
              : (e.kind == EventKind.served
                    ? colors.yourStar
                    : colors.reshared);
          canvas.drawLine(
            a,
            b,
            Paint()
              ..strokeWidth = 1.5
              ..color = color.withValues(alpha: 0.3 * fade),
          );
          final r = min(2.5 + _log2(e.bytes.toDouble() / 65536 + 1) * 0.4, 5.0);
          for (var i = 0; i < 4; i++) {
            final along = (t * 1.45 - i * 0.15).clamp(0.0, 1.0);
            if (along <= 0 || along >= 1) continue;
            final at = Offset.lerp(a, b, along)!;
            _glow(canvas, at, r * 3, color, 0.6 * fade);
            canvas.drawCircle(
              at,
              r,
              Paint()..color = color.withValues(alpha: fade),
            );
          }
        case EventLine(:final from, :final to):
          final a = camera.toScreen(from), b = camera.toScreen(to);
          final reach = Curves.easeOut.transform((t * 1.6).clamp(0.0, 1.0));
          canvas.drawLine(
            a,
            Offset.lerp(a, b, reach)!,
            Paint()
              ..strokeWidth = 2.5
              ..strokeCap = StrokeCap.round
              ..color = colors.reshared.withValues(alpha: 0.9 * fade),
          );
      }
    }
  }

  /// The owner's star: her node.
  void _yourStar(Canvas canvas, Offset at) {
    _glow(canvas, at, 34, colors.yourStar, 0.55);
    canvas.drawCircle(at, 11, Paint()..color = colors.yourStar);
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

  @override
  bool shouldRepaint(_Map old) =>
      old.snapshot != snapshot ||
      old.colors != colors ||
      old.standing != standing ||
      old.layer != layer ||
      old.lights != lights ||
      old.connection != connection ||
      old.text != text ||
      old.event != event;
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
          // A name on the map, held to the size the others grow to.
          Text(
            name,
            style: theme.textTheme.titleMedium?.copyWith(color: colors.onSky),
            textScaler: MediaQuery.textScalerOf(
              context,
            ).clamp(maxScaleFactor: _mapTextMost),
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
  const _Machines({required this.machines, required this.onNode, this.turn});

  final List<UniverseMachine> machines;

  /// While a machine event is drawn: one met (true) or faded.
  final bool? turn;

  /// Opens the owner's node's summary.
  final VoidCallback onNode;

  @override
  Widget build(BuildContext context) {
    if (machines.isEmpty) return const SizedBox.shrink();
    final colors = OspColors.of(context);
    final n = machines.length;
    return ActionChip(
      avatar: Icon(Icons.dns_outlined, size: 18, color: colors.onSky),
      label: Text(n == 1 ? '1 machine' : '$n machines'),
      labelStyle: TextStyle(color: colors.onSky),
      backgroundColor: switch (turn) {
        true => colors.yourStar.withValues(alpha: 0.45),
        false => colors.sky,
        null => colors.skyHaze,
      },
      side: BorderSide.none,
      tooltip: 'Machines your node knows',
      onPressed: () => showModalBottomSheet<void>(
        context: context,
        showDragHandle: true,
        builder: (context) => _MachineList(
          machines: machines,
          onNode: () {
            Navigator.of(context).pop();
            onNode();
          },
        ),
      ),
    );
  }
}

class _MachineList extends StatelessWidget {
  const _MachineList({required this.machines, required this.onNode});

  final List<UniverseMachine> machines;
  final VoidCallback onNode;

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
        // What machines are, for a newcomer: only what the node does
        // (ADR 0015), and nothing promised about reputation.
        const Padding(
          padding: EdgeInsets.fromLTRB(Space.m, Space.xs, Space.m, 0),
          child: Text(
            'Machines are other people’s nodes, met under names made for '
            'meeting. A machine’s name isn’t tied to the person who runs it.\n\n'
            'Your node meets them on its own, while you’re away. They greet '
            'each other and pass on the names of other machines they’ve met. '
            'Some list people who asked to be found: that’s how your node '
            'found people for you.\n\n'
            'Your node remembers which ones keep answering. That record '
            'stays on your node.',
          ),
        ),
        Padding(
          padding: const EdgeInsets.symmetric(
            horizontal: Space.m,
            vertical: Space.xs,
          ),
          child: Align(
            alignment: Alignment.centerLeft,
            child: TextButton(
              onPressed: onNode,
              child: const Text('What your node did'),
            ),
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
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          ListTile(
            leading: InitialAvatar(
              name: name,
              seed: p.identity,
              yours: p.step == 0,
            ),
            title: Text(name, style: theme.textTheme.titleSmall),
            subtitle: detail.isEmpty
                ? null
                : Text(detail, maxLines: 2, overflow: TextOverflow.ellipsis),
            onTap: () => Navigator.of(context).push(
              MaterialPageRoute<void>(
                builder: (_) => ProfileScreen(
                  node: node,
                  identity: p.identity,
                  name: p.name,
                ),
              ),
            ),
            trailing: IconButton(
              tooltip: 'Step back',
              icon: const Icon(Icons.close),
              onPressed: onClose,
            ),
          ),
          // Only this tap asks (ADR 0037 §2): selecting them never does.
          Align(
            alignment: Alignment.centerRight,
            child: Padding(
              padding: const EdgeInsets.only(right: Space.s, bottom: Space.xs),
              child: TextButton.icon(
                onPressed: () => node.askConnection(p.identity, p.name),
                icon: const Icon(Icons.route),
                label: const Text('How are we connected?'),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// "How are we connected?" under the map (ADR 0037 §1): each path the node
/// knows, in words, nearest first, while the map lights them up.
class _PathsCard extends StatelessWidget {
  const _PathsCard({
    required this.paths,
    required this.error,
    required this.onClose,
  });

  final ConnectionLights? paths;
  final String? error;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final c = paths;
    final who = c == null ? 'How you’re connected' : c.nameOf(c.identity);
    return Card(
      key: const Key('paths-card'),
      child: Padding(
        padding: const EdgeInsets.only(bottom: Space.s),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            ListTile(
              leading: const Icon(Icons.route),
              // The name, not "How you're connected to …": a phone's
              // width cut that off before the name.
              title: Text(
                who,
                style: theme.textTheme.titleSmall,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
              ),
              subtitle: Text(
                error == null ? c!.source : 'Couldn’t ask your node: $error',
              ),
              trailing: IconButton(
                tooltip: 'Put away',
                icon: const Icon(Icons.close),
                onPressed: onClose,
              ),
            ),
            if (c != null && c.asking)
              const Padding(
                padding: EdgeInsets.symmetric(horizontal: Space.m),
                child: LinearProgressIndicator(),
              )
            else if (c != null && c.paths.isEmpty)
              const Padding(
                padding: EdgeInsets.symmetric(horizontal: Space.m),
                child: Text('Your node hasn’t found a connection.'),
              )
            else if (c != null)
              ConstrainedBox(
                constraints: const BoxConstraints(maxHeight: 150),
                child: ListView(
                  shrinkWrap: true,
                  padding: const EdgeInsets.symmetric(horizontal: Space.m),
                  children: [
                    for (final (i, path) in c.paths.indexed)
                      Padding(
                        padding: const EdgeInsets.only(bottom: Space.xs),
                        child: Text('${i + 1}. ${c.words(path)}'),
                      ),
                  ],
                ),
              ),
          ],
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

/// The event drawn, in a line under the map (ADR 0036 §1). Tapping it
/// opens the day's list.
class _EventLine extends StatelessWidget {
  const _EventLine({
    required this.event,
    required this.names,
    required this.onTap,
  });

  final UniverseEvent event;
  final EventNames names;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final colors = OspColors.of(context);
    return Padding(
      padding: const EdgeInsets.only(bottom: Space.xs),
      child: Material(
        color: colors.skyHaze,
        borderRadius: BorderRadius.circular(20),
        child: InkWell(
          key: const Key('event-line'),
          borderRadius: BorderRadius.circular(20),
          onTap: onTap,
          child: Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: Space.m,
              vertical: Space.s,
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Icon(eventIcon(event.kind), size: 18, color: colors.onSky),
                const SizedBox(width: Space.s),
                Flexible(
                  child: Text(
                    describeEvent(event, names),
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(color: colors.onSky),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
