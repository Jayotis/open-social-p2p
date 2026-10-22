import 'dart:async';

import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:device_info_plus/device_info_plus.dart';
import 'package:flutter/widgets.dart';

import 'device_key.dart';
import 'format.dart';
import 'src/rust/api/osp.dart';

/// Whether the home node answered recently.
enum Reachability { unknown, connecting, online, offline }

/// The app's view of its home node: pairing, reachability, health and the
/// feed. Screens listen to it and redraw when it changes.
///
/// All networking happens in Rust (`osp-client`); this class only keeps
/// state and decides when to ask.
class HomeNode extends ChangeNotifier with WidgetsBindingObserver {
  HomeNode._(this.app);

  /// The Rust side.
  final OspApp app;

  NodeInfo? paired;
  Reachability reachability = Reachability.unknown;
  NodeStatus? status;

  /// Why the node couldn't be reached, in plain words.
  String? lastError;
  DateTime? lastContact;

  List<Post> posts = [];

  /// Set while showing the saved feed instead of a fresh one.
  DateTime? savedAt;
  bool onMobileData = false;

  /// While a post is uploading, the connection stays open in the background.
  bool uploading = false;

  /// Something to tell the owner on the pairing screen.
  String? notice;

  Timer? _poll;
  StreamSubscription<List<ConnectivityResult>>? _network;
  final Map<String, Future<String>> _urls = {};

  static Future<HomeNode> open(String dir, DeviceKey key) async {
    final node = HomeNode._(await OspApp.open(dir: dir, deviceKey: key.hex));
    if (key.isNew && node.app.pairedNode() != null) {
      // The old key was lost, and the node knows only that one.
      await node.app.forget();
      node.notice =
          "This phone's key couldn't be unlocked, so it got a new one. "
          'Pair it with your home node again.';
    }
    node.paired = node.app.pairedNode();
    node._showSaved();
    WidgetsBinding.instance.addObserver(node);
    final connectivity = Connectivity();
    node._setNetwork(await connectivity.checkConnectivity());
    node._network = connectivity.onConnectivityChanged.listen((results) {
      node._setNetwork(results);
      // iroh can't watch Android's network itself (ADR 0008).
      node.app.networkChanged();
      node.checkStatus();
    });
    if (node.paired != null) node._start();
    return node;
  }

  @override
  void dispose() {
    _poll?.cancel();
    _network?.cancel();
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  void _start() {
    refresh();
    _poll ??= Timer.periodic(const Duration(seconds: 20), (_) => checkStatus());
  }

  void _stop() {
    _poll?.cancel();
    _poll = null;
  }

  void _setNetwork(List<ConnectivityResult> results) {
    onMobileData =
        results.contains(ConnectivityResult.mobile) &&
        !results.contains(ConnectivityResult.wifi) &&
        !results.contains(ConnectivityResult.ethernet);
    notifyListeners();
  }

  void _showSaved() {
    final saved = app.savedFeed();
    if (saved != null) {
      posts = saved.posts;
      savedAt = DateTime.tryParse(saved.saved);
    }
  }

  void _reached() {
    reachability = Reachability.online;
    lastContact = DateTime.now();
    lastError = null;
  }

  void _failed(Object e) {
    reachability = Reachability.offline;
    lastError = describeError(e);
  }

  /// Asks the node for its health. Cheap; runs every 20 seconds.
  Future<void> checkStatus() async {
    if (paired == null) return;
    try {
      status = await app.status();
      _reached();
    } catch (e) {
      _failed(e);
    }
    notifyListeners();
  }

  /// Reloads the feed. With [sync], the node first checks everyone it follows.
  Future<void> refresh({bool sync = false}) async {
    if (paired == null) return;
    if (reachability != Reachability.online) {
      reachability = Reachability.connecting;
      notifyListeners();
    }
    try {
      if (sync) await app.sync_();
      posts = await app.feed(limit: 100);
      savedAt = null;
      _reached();
      status = await app.status();
    } catch (e) {
      _failed(e);
      _showSaved();
    }
    notifyListeners();
  }

  Future<NodeInfo> pair(String ticket) async {
    final info = await app.pair(
      ticket: ticket.trim(),
      deviceName: await _deviceName(),
    );
    paired = info;
    notice = null;
    notifyListeners();
    _start();
    return info;
  }

  /// Forgets the node on this phone.
  Future<void> forget() async {
    _stop();
    await app.forget();
    paired = null;
    posts = [];
    status = null;
    savedAt = null;
    reachability = Reachability.unknown;
    _urls.clear();
    notifyListeners();
  }

  /// A local URL for a blob on the node, for the player and images.
  Future<String> mediaUrl(String blob, String mediaType) {
    return _urls.putIfAbsent(
      blob,
      () => app.mediaUrl(blob: blob, mediaType: mediaType),
    );
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    if (paired == null) return;
    switch (state) {
      case AppLifecycleState.resumed:
        app.networkChanged();
        _start();
      case AppLifecycleState.paused:
        _stop();
        // Save the battery, unless a post is still being sent.
        if (!uploading) app.disconnect();
      default:
        break;
    }
  }

  static Future<String> _deviceName() async {
    try {
      final info = await DeviceInfoPlugin().androidInfo;
      final maker = info.manufacturer;
      final model = info.model;
      if (model.toLowerCase().startsWith(maker.toLowerCase())) return model;
      return '${maker[0].toUpperCase()}${maker.substring(1)} $model';
    } catch (_) {
      return 'Android phone';
    }
  }
}
