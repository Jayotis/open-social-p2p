import 'dart:async';
import 'dart:io';

import 'package:clock/clock.dart';
import 'package:connectivity_plus/connectivity_plus.dart';
import 'package:device_info_plus/device_info_plus.dart';
import 'package:flutter/widgets.dart';

import 'activity_notifications.dart';
import 'connection_lights.dart';
import 'device_key.dart';
import 'format.dart';
import 'network_events.dart';
import 'search_lights.dart';
import 'src/rust/api/osp.dart';
import 'terms.dart';
import 'uploads.dart';
import 'waiting_deletes.dart';

/// Whether the home node answered recently.
enum Reachability { unknown, connecting, online, offline }

/// The app's view of its home node: pairing, reachability, health and the
/// feed. Screens listen to it and redraw when it changes.
///
/// All networking happens in Rust (`osp-client`); this class only keeps
/// state and decides when to ask.
class HomeNode extends ChangeNotifier with WidgetsBindingObserver {
  HomeNode._(this.app, this.uploads, this._waiting);

  /// For tests: a node with a stand-in for the Rust side, not paired.
  @visibleForTesting
  HomeNode.forTests(this.app, this.uploads, [this._waiting]);

  /// The Rust side.
  final OspApp app;

  /// Where the files of a post being sent are kept.
  final UploadsFolder uploads;

  NodeInfo? paired;
  Reachability reachability = Reachability.unknown;
  NodeStatus? status;

  /// Why the node couldn't be reached, in plain words.
  String? lastError;
  DateTime? lastContact;

  List<Post> posts = [];

  /// How long a delete waits for an undo before it's sent (ADR 0019): once
  /// sent, it can't be taken back.
  static const deleteWait = Duration(minutes: 1);

  /// Deletes waiting out [deleteWait], by what they delete.
  final Map<String, Timer> _deleting = {};

  /// Those deletes kept in a file until the node has them, so the phone
  /// stopping the app doesn't lose one.
  final WaitingDeletes? _waiting;

  /// Deletes on their way to the node now.
  final Set<String> _sending = {};

  /// The feed, less the posts whose delete is waiting.
  List<Post> get visiblePosts =>
      posts.where((p) => !_deleting.containsKey(p.deletable)).toList();

  /// Whether a delete of [id] is waiting or on its way: a reply's, say.
  bool isDeleting(String id) =>
      _deleting.containsKey(id) || _sending.contains(id);

  /// Whether the app is in the background, where the connection is let go.
  bool _inBackground = false;

  /// Set while showing the saved feed instead of a fresh one.
  DateTime? savedAt;
  bool onMobileData = false;

  /// The phone has no network at all.
  bool phoneOffline = false;

  /// While a post is uploading, the connection stays open in the background.
  bool uploading = false;

  /// A post that didn't finish sending, kept to carry on or discard.
  UnsentPost? unsent;

  /// Something to tell the owner on the pairing screen.
  String? notice;

  /// Everyone the node holds a profile of, newest post first: null until
  /// first loaded ([loadPeople]).
  List<FoundPerson>? people;

  /// Why loading [people] failed, if it did.
  String? peopleError;

  bool _loadingPeople = false;

  /// What the Universe opens (ADR 0034 §6), as the node names regions:
  /// empty for the top, `g:<id>` a group, `p:<identity>` standing at
  /// someone, `sky:<l>/<x>/<y>` a cell of the sky.
  String universeRegion = '';

  /// What lights up on the map (ADR 0035 §1): none for people alone.
  UniverseLayer? universeLayer;

  /// The search the map lights up (ADR 0035 §3), if one is shown there.
  SearchLights? searchLights;

  /// "How are we connected?" (ADR 0037), if the map shows it, and why
  /// asking failed, if it did.
  ConnectionLights? connection;
  String? connectionError;

  /// Set when a screen asks for the map, as asking how you're connected
  /// from a profile does; the shell takes it and shows the Universe.
  bool mapWanted = false;

  /// Set when a notification asks for Activity.
  bool activityWanted = false;

  /// An OSP code an `osp:` link brought (ADR 0039 §4), until the shell
  /// opens it.
  String? codeWanted;

  /// Whether the phone asked to show notifications, once.
  File? _askedToNotify;

  /// What the node did or saw lately (ADR 0036 §1), asked for with the
  /// Universe.
  final events = NetworkEvents();
  bool _loadingEvents = false;

  /// The last snapshots, by layer and region, so a region visited lately
  /// shows at once and is asked for since the one held. The top without a
  /// layer is also kept on disk, for the next start.
  final Map<String, UniverseSnapshot> _universes = {};

  /// How a region's snapshot is held with [layer] lit.
  static String _held(String region, UniverseLayer? layer) =>
      layer == null ? region : '${layer.name}|$region';

  /// How many regions' snapshots are held: each is at most 128 KiB.
  static const _universesHeld = 16;

  /// What the Universe may show (ADR 0033) at [universeRegion], as last
  /// fetched. Null until there is one.
  UniverseSnapshot? get universe =>
      _universes[_held(universeRegion, universeLayer)];
  set universe(UniverseSnapshot? s) =>
      _hold(_held(universeRegion, universeLayer), s);

  /// The top's snapshot: everyone the node found.
  UniverseSnapshot? get universeTop => _universes[''];

  void _hold(String held, UniverseSnapshot? s) {
    _universes.remove(held);
    if (s == null) return;
    _universes[held] = s;
    // The least lately used goes first; the top always stays.
    while (_universes.length > _universesHeld) {
      _universes.remove(_universes.keys.firstWhere((r) => r.isNotEmpty));
    }
  }

  /// Why asking for [universe] failed, if the last try did.
  String? universeError;

  final Set<String> _loadingUniverse = {};

  /// Set while the Universe is on screen: [universe] is asked for again
  /// with each status check, so people appear as the node finds them.
  bool watchingUniverse = false;

  /// The owner's own profile (ADR 0032): null until first loaded.
  Profile? me;

  /// Set when this phone has just paired, until the first run is done
  /// (ADR 0031 §3). Not kept: an app stopped halfway skips the rest.
  bool firstRun = false;

  /// Set until the Universe has shown, once, that the map zooms. Kept as a
  /// file, so it shows on this phone only once.
  bool zoomHintDue = false;
  File? _zoomHintShown;

  void zoomHintDone() {
    if (!zoomHintDue) return;
    zoomHintDue = false;
    try {
      _zoomHintShown?.writeAsStringSync('');
    } on FileSystemException catch (e) {
      debugPrint('Zoom hint not kept: $e');
    }
  }

  /// The version of the terms of use this phone agreed to, 0 if none
  /// (ADR 0042 §3). Kept as a file.
  int _termsAgreed = 0;
  File? _termsFile;

  /// Whether the terms must be shown and agreed to before going on: on
  /// first start, and once after they change.
  bool get termsDue => _termsAgreed < termsVersion;

  void agreeToTerms() {
    _termsAgreed = termsVersion;
    try {
      _termsFile?.writeAsStringSync('$termsVersion');
    } on FileSystemException catch (e) {
      debugPrint('Agreement to the terms not kept: $e');
    }
    notifyListeners();
  }

  /// The answer given on this phone to whether to use the project's hide
  /// list (ADR 0042 §6), null if none. Kept as a file, as it's asked before
  /// pairing: once paired, it goes to a node that hasn't been asked.
  bool? _useHideList;
  File? _hideListFile;

  /// The hide list as the node has it, null until first read.
  HideListState? hideList;

  /// Whether to ask about the hide list before going on: on first start,
  /// after the terms, and on a paired phone whose node hasn't been asked.
  bool get hideListDue =>
      _useHideList == null &&
      (paired == null || (hideList != null && !hideList!.asked));

  /// The owner's answer, from the welcome. A paired phone sends it to the
  /// node at once.
  Future<void> answerHideList(bool use) async {
    _useHideList = use;
    try {
      _hideListFile?.writeAsStringSync(use ? 'use' : 'not now');
    } on FileSystemException catch (e) {
      debugPrint('Hide list answer not kept: $e');
    }
    notifyListeners();
    if (paired != null) await setHideList(use);
  }

  /// Turns the list on or off on the node (Advanced).
  Future<void> setHideList(bool on) async {
    hideList = await app.setHideList(inUse: on);
    notifyListeners();
    unawaited(refresh());
  }

  /// Stops the list hiding [identity]'s posts, or takes that back.
  Future<void> hideException(String identity, bool except) async {
    hideList = await app.hideException(identity: identity, except: except);
    notifyListeners();
    unawaited(refresh());
  }

  /// Reads the hide list from the node, which also lets the app check it
  /// and hide what it holds. Sends the answer given before pairing, if the
  /// node wasn't asked. Quiet if the node can't be asked.
  Future<void> loadHideList() async {
    if (paired == null) return;
    try {
      var state = await app.hideList();
      final answer = _useHideList;
      if (!state.asked && answer != null) {
        state = await app.setHideList(inUse: answer);
      }
      hideList = state;
      notifyListeners();
    } catch (e) {
      debugPrint('Hide list not read: $e');
    }
  }

  /// Carrying for other nodes (ADR 0043 §8), as the node last said. None
  /// from a node before carrying's controls.
  CarrierState? get carrier => status?.carrier;

  /// Whether to ask if the hops the node picks may see its address: its
  /// node knows seed carriers and hasn't been asked, as one set up before
  /// the question wasn't. Until then its own video goes through relays.
  bool get hopsDue {
    final c = carrier;
    return paired != null && c != null && c.hops == null && c.seeds > 0;
  }

  /// The owner's answer on the hops seeing the node's address.
  Future<void> answerHops(bool yes) async {
    await app.setHops(yes: yes);
    await checkStatus();
  }

  /// Changes how the node carries; each one left out stays as it is.
  /// Fails with the node's reason, as for a share over 100 percent.
  Future<void> setCarrying({
    bool? paused,
    bool? metered,
    int? jobs,
    int? uploadShare,
    int? uploadMbps,
    int? monthlyGb,
    int? dataCapGb,
  }) async {
    await app.setCarrying(
      paused: paused,
      metered: metered,
      jobs: jobs,
      uploadShare: uploadShare,
      uploadMbps: uploadMbps,
      monthlyGb: monthlyGb,
      dataCapGb: dataCapGb,
    );
    await checkStatus();
  }

  Timer? _poll;
  StreamSubscription<List<ConnectivityResult>>? _network;
  final Map<String, Future<String>> _urls = {};

  static Future<HomeNode> open(
    String dir,
    DeviceKey key,
    UploadsFolder uploads,
  ) async {
    final node = HomeNode._(
      await OspApp.open(dir: dir, deviceKey: key.hex),
      uploads,
      WaitingDeletes(File('$dir/waiting-deletes.json')),
    );
    if (key.isNew && node.app.pairedNode() != null) {
      // The old key was lost, and the node knows only that one.
      await node.app.forget();
      node.notice =
          "This phone's key couldn't be unlocked, so it got a new one. "
          'Pair it with your node again.';
    }
    node._zoomHintShown = File('$dir/zoom-hint-shown');
    node._askedToNotify = File('$dir/asked-to-notify');
    node.zoomHintDue = !node._zoomHintShown!.existsSync();
    node._termsFile = File('$dir/terms-agreed');
    node._hideListFile = File('$dir/hide-list-answer');
    try {
      node._useHideList = switch (node._hideListFile!.readAsStringSync()) {
        'use' => true,
        'not now' => false,
        _ => null,
      };
    } on FileSystemException {
      // Not answered on this phone.
    }
    try {
      node._termsAgreed =
          int.tryParse(node._termsFile!.readAsStringSync().trim()) ?? 0;
    } on FileSystemException {
      // Not agreed yet.
    }
    node.paired = node.app.pairedNode();
    node._showSaved();
    node._hold('', node.app.savedUniverse());
    node.unsent = node.app.pendingPost();
    await uploads.clean(keep: [...?node.unsent?.files.map((f) => f.path)]);
    WidgetsBinding.instance.addObserver(node);
    final connectivity = Connectivity();
    node._setNetwork(await connectivity.checkConnectivity());
    node._network = connectivity.onConnectivityChanged.listen((results) {
      node._setNetwork(results);
      // iroh can't watch Android's network itself (ADR 0008).
      node.app.networkChanged();
      node.checkStatus();
    });
    setOnScreen(true);
    if (node.paired != null) {
      node._start();
      node._startNotices();
    }
    return node;
  }

  /// Deletes [id] once [deleteWait] has passed, unless [undoDelete] comes
  /// first. [onFailed] says why, if sending it fails.
  void deleteLater(String id, {void Function(Object error)? onFailed}) {
    _deleting[id]?.cancel();
    _deleting[id] = Timer(deleteWait, () => _sendDelete(id, onFailed));
    _waiting?.add(id, clock.now().add(deleteWait));
    notifyListeners();
  }

  /// Keeps a post whose delete is still waiting.
  void undoDelete(String id) {
    _deleting.remove(id)?.cancel();
    _waiting?.remove(id);
    notifyListeners();
  }

  /// Sends a delete. It stays in the file until the node has it, unless
  /// the owner is told it failed ([onFailed]): the post is back for them to
  /// delete again. One sent with no one to tell is tried again when the
  /// app next starts or comes back ([_resumeWaitingDeletes]).
  Future<void> _sendDelete(String id, void Function(Object)? onFailed) async {
    _deleting.remove(id);
    _sending.add(id);
    try {
      await app.deletePost(id: id);
      _waiting?.remove(id);
      posts = posts.where((p) => p.deletable != id).toList();
    } catch (e) {
      if (onFailed != null) {
        _waiting?.remove(id);
        onFailed(e);
      }
    } finally {
      _sending.remove(id);
    }
    notifyListeners();
  }

  /// Waits out, or sends, the deletes kept from before the app was stopped
  /// or went to the background.
  void _resumeWaitingDeletes() {
    final waiting = _waiting?.read() ?? {};
    final now = clock.now();
    for (final MapEntry(key: id, value: due) in waiting.entries) {
      if (_deleting.containsKey(id) || _sending.contains(id)) continue;
      final left = due.isAfter(now) ? due.difference(now) : Duration.zero;
      _deleting[id] = Timer(left, () => _sendDelete(id, null));
    }
    if (waiting.isNotEmpty) notifyListeners();
  }

  /// Sends the deletes still waiting at once: leaving the app without
  /// undoing them says they're meant.
  Future<void> _sendDeletesNow() async {
    final waiting = _deleting.keys.toList();
    final now = clock.now();
    for (final id in waiting) {
      _deleting.remove(id)?.cancel();
      // Due now: if it doesn't get there, it goes as soon as the app's back.
      _waiting?.add(id, now);
    }
    await Future.wait(waiting.map((id) => _sendDelete(id, null)));
  }

  @override
  void dispose() {
    for (final timer in _deleting.values) {
      timer.cancel();
    }
    _poll?.cancel();
    _network?.cancel();
    WidgetsBinding.instance.removeObserver(this);
    super.dispose();
  }

  void _start() {
    _resumeWaitingDeletes();
    refresh();
    _poll ??= Timer.periodic(const Duration(seconds: 20), (_) => checkStatus());
  }

  void _stop() {
    _poll?.cancel();
    _poll = null;
  }

  /// As when the phone's network changes.
  @visibleForTesting
  void networkChangedForTests(List<ConnectivityResult> results) =>
      _setNetwork(results);

  void _setNetwork(List<ConnectivityResult> results) {
    onMobileData =
        results.contains(ConnectivityResult.mobile) &&
        !results.contains(ConnectivityResult.wifi) &&
        !results.contains(ConnectivityResult.ethernet);
    // An always-on VPN stays listed with no network under it (seen with
    // ProtonVPN on a Pixel, P11 part 6), so a VPN alone is offline too.
    phoneOffline = results.every(
      (r) => r == ConnectivityResult.none || r == ConnectivityResult.vpn,
    );
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

  /// Asks the node for its health. Cheap; runs every 20 seconds. Over Tor the
  /// node syncs in the background without keeping the phone waiting, so what
  /// arrived is picked up here too.
  Future<void> checkStatus() async {
    if (paired == null) return;
    try {
      status = await app.status();
      if (watchingUniverse) unawaited(loadUniverse());
      if (status?.overTor ?? false) {
        posts = await app.feed(limit: 100);
        savedAt = null;
      }
      _reached();
    } catch (e) {
      _failed(e);
    }
    notifyListeners();
  }

  /// Reloads the feed. With [sync], the node first checks everyone it follows,
  /// or over Tor starts to.
  Future<void> refresh({bool sync = false}) async {
    if (paired == null) return;
    if (reachability != Reachability.online) {
      reachability = Reachability.connecting;
      notifyListeners();
    }
    try {
      if (sync) await app.sync_();
      // First, so the app hides what the list holds in what follows.
      await loadHideList();
      posts = await app.feed(limit: 100);
      savedAt = null;
      _reached();
      status = await app.status();
      unawaited(loadMe());
    } catch (e) {
      _failed(e);
      _showSaved();
    }
    notifyListeners();
  }

  /// Reads the owner's own profile, for the You tab. Quietly keeps the last
  /// one if the node can't be asked.
  Future<void> loadMe() async {
    final identity = paired?.identity;
    if (identity == null) return;
    try {
      me = await app.profile(identity: identity);
      notifyListeners();
    } catch (_) {}
  }

  /// The owner saved a new profile.
  void profileChanged(Profile profile) {
    me = profile;
    notifyListeners();
    unawaited(checkStatus());
  }

  Future<NodeInfo> pair(String ticket) async {
    final info = await app.pair(
      ticket: ticket.trim(),
      deviceName: await _deviceName(),
    );
    paired = info;
    notice = null;
    firstRun = true;
    notifyListeners();
    _start();
    _startNotices();
    return info;
  }

  /// Asks the node for the Universe, since the one the phone holds: an
  /// unchanged one costs a few bytes. One at a time. The one held stays
  /// on screen if the node can't be asked.
  Future<void> loadUniverse() async {
    unawaited(loadEvents());
    final region = universeRegion;
    final layer = universeLayer;
    final key = _held(region, layer);
    if (paired == null || !_loadingUniverse.add(key)) return;
    try {
      final held = _universes[key];
      final fresh = await app.universe(
        region: region.isEmpty ? null : region,
        lit: layer,
        since: held?.version,
      );
      if (fresh != null) {
        _hold(key, fresh);
        // A group that's gone is answered by the one most of its people
        // went to: that's where the Universe is now.
        if (fresh.region != region) {
          _hold(_held(fresh.region, layer), fresh);
          if (universeRegion == region) universeRegion = fresh.region;
        }
      } else if (held != null) {
        _hold(key, held);
      }
      universeError = null;
    } catch (e) {
      universeError = describeError(e);
    } finally {
      _loadingUniverse.remove(key);
    }
    notifyListeners();
  }

  /// Asks the node for its events since the newest held (ADR 0036 §1).
  /// One at a time; a failure leaves those held, and the next try asks
  /// again.
  Future<void> loadEvents() async {
    if (paired == null || _loadingEvents) return;
    _loadingEvents = true;
    try {
      final fresh = await app.universeEvents(since: events.since);
      if (fresh.isEmpty) return;
      events.take(fresh);
      notifyListeners();
    } catch (e) {
      debugPrint('Events not loaded: ${describeError(e)}');
    } finally {
      _loadingEvents = false;
    }
  }

  /// The event drawn on the map is done.
  void drewEvent() {
    events.drew();
    notifyListeners();
  }

  /// Opens [region] in the Universe: the one held at once, if any, then
  /// the node's. Asks no other node anything (ADR 0034 §6).
  Future<void> openUniverse(String region) {
    universeRegion = region;
    notifyListeners();
    return loadUniverse();
  }

  /// Lights up [layer] on the map, or none for people alone, where it is.
  Future<void> lightUniverse(UniverseLayer? layer) {
    universeLayer = layer;
    notifyListeners();
    return loadUniverse();
  }

  /// Shows a search's results on the map (ADR 0035 §3), or none.
  void showSearch(SearchLights? lights) {
    searchLights = lights;
    // One thing lit at a time.
    if (lights != null) connection = null;
    notifyListeners();
  }

  /// Asks the node how its owner is connected to [identity], from the
  /// owner's own tap only (ADR 0037 §2): it may ask their node once. The map
  /// shows it, the search it showed put away.
  Future<void> askConnection(String identity, String name) async {
    final asking = ConnectionLights.asking(
      identity,
      name,
      torStarting: status?.torStarting ?? false,
    );
    connection = asking;
    connectionError = null;
    searchLights = null;
    notifyListeners();
    try {
      final c = await app.howConnected(identity: identity);
      if (!identical(connection, asking)) return;
      connection = ConnectionLights(identity, name, c);
    } catch (e) {
      if (!identical(connection, asking)) return;
      connection = null;
      connectionError = describeError(e);
    }
    notifyListeners();
  }

  /// Puts away the connection the map shows.
  void showConnection(ConnectionLights? lights) {
    connection = lights;
    connectionError = null;
    notifyListeners();
  }

  /// Takes a screen's wish to show the map.
  bool takeMapWanted() {
    final wanted = mapWanted;
    mapWanted = false;
    return wanted;
  }

  /// Opens Activity, as a notification's tap does.
  void showActivity() {
    activityWanted = true;
    notifyListeners();
  }

  /// Takes a notification's wish to show Activity.
  bool takeActivityWanted() {
    final wanted = activityWanted;
    activityWanted = false;
    return wanted;
  }

  /// Opens an OSP code, as tapping an `osp:` link does.
  void openCode(String code) {
    codeWanted = code;
    notifyListeners();
  }

  /// Takes a link's OSP code to open.
  String? takeCodeWanted() {
    final wanted = codeWanted;
    codeWanted = null;
    return wanted;
  }

  /// Starts Activity's notifications (Android), and asks once to show them.
  void _startNotices() {
    unawaited(
      startActivityChecks(onTap: showActivity).then((opened) {
        if (opened) showActivity();
      }),
    );
    final asked = _askedToNotify;
    if (asked != null && !asked.existsSync()) {
      asked.writeAsStringSync('');
      unawaited(askToNotify());
    }
  }

  /// Asks the node for everyone it holds a profile of (the list the
  /// Universe opens). One at a time.
  Future<void> loadPeople() async {
    if (paired == null || _loadingPeople) return;
    _loadingPeople = true;
    try {
      people = await app.people(limit: 200);
      peopleError = null;
    } catch (e) {
      peopleError = describeError(e);
    } finally {
      _loadingPeople = false;
    }
    notifyListeners();
  }

  /// The first run is over: the app shows its usual places from now on.
  void finishFirstRun() {
    firstRun = false;
    notifyListeners();
  }

  /// Looks again for a post that didn't finish sending.
  void checkUnsent() {
    unsent = app.pendingPost();
    notifyListeners();
  }

  /// Stops and discards the post that didn't finish sending, on the phone
  /// and the node. Fails with the node's reason if it's too late.
  Future<void> discardPost() async {
    final files = [...?app.pendingPost()?.files.map((f) => f.path)];
    try {
      await app.discardPost();
    } finally {
      if (app.pendingPost() == null) await uploads.release(files);
      checkUnsent();
    }
  }

  /// Forgets the node on this phone.
  Future<void> forget() async {
    _stop();
    await stopActivityChecks();
    await app.forget();
    await uploads.clean();
    unsent = null;
    paired = null;
    firstRun = false;
    people = null;
    peopleError = null;
    _universes.clear();
    universeRegion = '';
    universeLayer = null;
    searchLights = null;
    connection = null;
    connectionError = null;
    events.clear();
    universeError = null;
    me = null;
    hideList = null;
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
        _inBackground = false;
        setOnScreen(true);
        app.resumed();
        _start();
      case AppLifecycleState.paused:
        _inBackground = true;
        setOnScreen(false);
        _stop();
        // Save the battery, unless a post is still being sent. Deletes
        // still waiting go first.
        _sendDeletesNow().whenComplete(() {
          if (_inBackground && !uploading) app.disconnect();
        });
      default:
        break;
    }
  }

  static Future<String> _deviceName() async {
    if (Platform.isIOS) {
      try {
        // Such as "iPhone 12". The name the owner gave the phone needs an
        // entitlement Apple grants few apps.
        return (await DeviceInfoPlugin().iosInfo).modelName;
      } catch (_) {
        return 'iPhone';
      }
    }
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
