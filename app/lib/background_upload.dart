import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Keeps a post sending while the app is in the background.
///
/// On Android this is a foreground service with a notification showing
/// the post's progress; without one, Android may stop the app soon after
/// it leaves the screen (`UploadService.kt`). Elsewhere it does nothing
/// yet. If the app is stopped anyway, the post carries on when the app is
/// next opened.
class BackgroundUpload {
  static const _channel = MethodChannel('osp/background_upload');

  /// What the notification shows, to skip repeats.
  String? _shown;

  Future<void> start() async {
    _shown = null;
    await _call('start');
  }

  /// Shows how the post is getting on. [percent] is null while there's
  /// nothing to measure.
  Future<void> show(String title, String text, {int? percent}) async {
    final shown = '$title\n$text\n$percent';
    if (shown == _shown) return;
    _shown = shown;
    await _call('show', {'title': title, 'text': text, 'percent': percent});
  }

  Future<void> stop() => _call('stop');

  static Future<void> _call(String method, [Object? arguments]) async {
    try {
      await _channel.invokeMethod<void>(method, arguments);
    } on MissingPluginException {
      // Not on this platform yet.
    } on PlatformException catch (e) {
      // The post goes on while the app is open.
      debugPrint('background upload: $e');
    }
  }
}
