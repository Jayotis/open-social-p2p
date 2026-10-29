import Flutter
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
  }
}
