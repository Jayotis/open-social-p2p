import Flutter
import OSLog
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  private let backgroundUpload = BackgroundUpload()

  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
    // Keeping a post sending in the background (lib/background_upload.dart).
    FlutterMethodChannel(
      name: "osp/background_upload",
      binaryMessenger: engineBridge.applicationRegistrar.messenger()
    ).setMethodCallHandler { [backgroundUpload] call, result in
      let arguments = call.arguments as? [String: Any]
      switch call.method {
      case "start":
        backgroundUpload.start()
        result(nil)
      case "show":
        backgroundUpload.show(
          title: arguments?["title"] as? String ?? "",
          text: arguments?["text"] as? String ?? "",
          percent: arguments?["percent"] as? Int
        )
        result(nil)
      case "stop":
        backgroundUpload.stop(sent: arguments?["sent"] as? Bool ?? false)
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
    // Keeping the folder of posts waiting to be sent out of backups
    // (lib/uploads.dart).
    FlutterMethodChannel(
      name: "osp/files",
      binaryMessenger: engineBridge.applicationRegistrar.messenger()
    ).setMethodCallHandler { call, result in
      guard call.method == "excludeFromBackup",
        let path = (call.arguments as? [String: Any])?["path"] as? String
      else {
        result(FlutterMethodNotImplemented)
        return
      }
      var url = URL(fileURLWithPath: path, isDirectory: true)
      var values = URLResourceValues()
      values.isExcludedFromBackup = true
      do {
        try url.setResourceValues(values)
        // Read back, so the flag can be checked in the phone's log.
        url.removeAllCachedResourceValues()
        let set = try url.resourceValues(forKeys: [.isExcludedFromBackupKey])
        // Public: NSLog's text shows only as <private> in a release build.
        Logger(subsystem: "osp", category: "files").notice(
          "uploads folder excluded from backup: \(String(describing: set.isExcludedFromBackup), privacy: .public)"
        )
        result(nil)
      } catch {
        result(FlutterError(code: "backup_flag", message: error.localizedDescription, details: nil))
      }
    }
  }
}
