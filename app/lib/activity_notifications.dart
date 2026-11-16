/// Activity's notifications on Android (ADR 0038 §3): now and then, while the
/// app isn't on screen, it asks its node what's new and says so. An iPhone
/// would need Apple's push service, which needs a server, so there Activity
/// shows when the app is opened.
library;

import 'dart:io';
import 'dart:isolate';
import 'dart:ui';

import 'package:flutter/widgets.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:path_provider/path_provider.dart';
import 'package:workmanager/workmanager.dart';

import 'activity_words.dart';
import 'device_key.dart';
import 'src/rust/api/osp.dart';
import 'src/rust/frb_generated.dart';

/// What a notification says.
class ActivityNotice {
  const ActivityNotice(this.title, this.body, this.lines);

  final String title;
  final String body;

  /// One per new entry, when there are several.
  final List<String> lines;
}

/// Lines a notification lists, at most.
const noticeLines = 5;

/// What to say about [entries] (a page, newest first) for the entries changed
/// since [notified], the highest `seq` already told; null when nothing is
/// new. [unread] is how many the node counts unread in all.
@visibleForTesting
ActivityNotice? noticeFor(
  List<ActivityItem> entries,
  int unread,
  int notified,
) {
  final fresh = [
    for (final e in entries)
      if (!e.read && e.seq > notified) e,
  ];
  if (fresh.isEmpty) return null;
  if (fresh.length == 1 && unread <= 1) {
    final e = fresh.first;
    return ActivityNotice(activityLine(e), activityQuote(e) ?? '', const []);
  }
  final count = unread < fresh.length ? fresh.length : unread;
  final lines = [for (final e in fresh.take(noticeLines)) activityLine(e)];
  return ActivityNotice('$count new in Activity', lines.first, lines);
}

/// The highest `seq` among [entries], or [notified] if that's higher.
@visibleForTesting
int highestSeq(List<ActivityItem> entries, int notified) =>
    entries.fold(notified, (high, e) => e.seq > high ? e.seq : high);

const _notificationId = 1;
const _channelId = 'activity'; // not shown
const _payload = 'activity'; // not shown
const _task = 'activity-check'; // not shown

/// Set while the app is on screen: the check is then left to it.
const _onScreenName = 'osp-on-screen'; // not shown

final _plugin = FlutterLocalNotificationsPlugin();

/// Listed one per line when there are several.
NotificationDetails _details(List<String> lines) => NotificationDetails(
  android: AndroidNotificationDetails(
    _channelId,
    'Activity',
    channelDescription: 'Replies, likes, reshares and new followers',
    category: AndroidNotificationCategory.social,
    // One sound or buzz until it's seen: what comes after changes the words
    // quietly, so a busy night doesn't wake anyone over and over. Android's
    // Do Not Disturb and Bedtime mode silence it at night.
    onlyAlertOnce: true,
    styleInformation: lines.length < 2 ? null : InboxStyleInformation(lines),
  ),
);

Future<void> _initPlugin({void Function()? onTap}) => _plugin.initialize(
  settings: const InitializationSettings(
    android: AndroidInitializationSettings('ic_stat_activity'), // not shown
  ),
  onDidReceiveNotificationResponse: onTap == null
      ? null
      : (response) {
          if (response.payload == _payload) onTap();
        },
);

/// Starts the checks, on Android. [onTap] runs when a notification is tapped
/// while the app is running. Returns whether the app was opened by one.
Future<bool> startActivityChecks({required void Function() onTap}) async {
  if (!Platform.isAndroid) return false;
  try {
    await _initPlugin(onTap: onTap);
    await Workmanager().initialize(activityCheckDispatcher);
    await Workmanager().registerPeriodicTask(
      _task,
      _task,
      // Android's shortest period. It waits longer while the phone sleeps.
      frequency: const Duration(minutes: 15),
      constraints: Constraints(networkType: NetworkType.connected),
      existingWorkPolicy: ExistingPeriodicWorkPolicy.keep,
    );
    final launch = await _plugin.getNotificationAppLaunchDetails();
    return (launch?.didNotificationLaunchApp ?? false) &&
        launch?.notificationResponse?.payload == _payload;
  } catch (e) {
    // Activity still shows in the app.
    debugPrint('activity checks: $e');
    return false;
  }
}

/// Asks to show notifications, on Android 13 and later.
Future<void> askToNotify() async {
  if (!Platform.isAndroid) return;
  try {
    await _plugin
        .resolvePlatformSpecificImplementation<
          AndroidFlutterLocalNotificationsPlugin
        >()
        ?.requestNotificationsPermission();
  } catch (e) {
    debugPrint('asking to notify: $e');
  }
}

/// Stops the checks, as when the phone forgets its node.
Future<void> stopActivityChecks() async {
  if (!Platform.isAndroid) return;
  try {
    await Workmanager().cancelByUniqueName(_task);
    await clearActivityNotice();
  } catch (e) {
    debugPrint('stopping activity checks: $e');
  }
}

/// Takes the notification away, as once Activity is open.
Future<void> clearActivityNotice() async {
  if (!Platform.isAndroid) return;
  try {
    await _plugin.cancel(id: _notificationId);
  } catch (e) {
    debugPrint('clearing the activity notice: $e');
  }
}

ReceivePort? _onScreen;

/// Notes whether the app is on screen, for the checks in the background.
void setOnScreen(bool on) {
  if (on && _onScreen == null) {
    final port = ReceivePort();
    IsolateNameServer.removePortNameMapping(_onScreenName);
    IsolateNameServer.registerPortWithName(port.sendPort, _onScreenName);
    _onScreen = port;
  } else if (!on && _onScreen != null) {
    IsolateNameServer.removePortNameMapping(_onScreenName);
    _onScreen!.close();
    _onScreen = null;
  }
}

/// Runs in the background, in an engine of its own.
@pragma('vm:entry-point')
void activityCheckDispatcher() {
  Workmanager().executeTask((task, input) async {
    try {
      await checkActivity();
    } catch (e) {
      // Not retried: the next check comes anyway.
      debugPrint('activity check: $e');
    }
    return true;
  });
}

/// One check: what the node has that's new, said once.
Future<void> checkActivity() async {
  WidgetsFlutterBinding.ensureInitialized();
  if (IsolateNameServer.lookupPortByName(_onScreenName) != null) return;
  // Never makes a key: the app does that, when it's opened.
  final key = await const PhoneSecretStore().read(deviceKeyName);
  if (key == null) return;
  try {
    await RustLib.init();
  } catch (_) {
    // Already started, in this engine's process.
  }
  final dir = await getApplicationSupportDirectory();
  final app = await OspApp.open(dir: dir.path, deviceKey: key);
  try {
    if (app.pairedNode() == null) return;
    await _initPlugin();
    final status = await app.status();
    if (status.activityUnread == 0) {
      // Read on another device, or in the app.
      await _plugin.cancel(id: _notificationId);
      return;
    }
    final seen = File('${dir.path}/activity-notified');
    final notified = seen.existsSync()
        ? int.tryParse(seen.readAsStringSync().trim()) ?? 0
        : 0;
    final page = await app.activity(limit: 20);
    final notice = noticeFor(page.entries, page.unread, notified);
    if (notice == null) return;
    await _plugin.show(
      id: _notificationId,
      title: notice.title,
      body: notice.body.isEmpty ? null : notice.body,
      notificationDetails: _details(notice.lines),
      payload: _payload,
    );
    seen.writeAsStringSync('${highestSeq(page.entries, notified)}');
  } finally {
    await app.disconnect();
    app.dispose();
  }
}
