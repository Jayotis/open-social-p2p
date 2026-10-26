package io.github.jayotis.osp_app

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        // Keeping a post sending in the background (lib/background_upload.dart).
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "osp/background_upload")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "start" -> {
                        askToShowNotifications()
                        try {
                            UploadService.start(applicationContext)
                            result.success(null)
                        } catch (e: Exception) {
                            // Android refuses when the app isn't on screen.
                            result.error("start", e.message, null)
                        }
                    }
                    "show" -> {
                        UploadService.show(
                            applicationContext,
                            call.argument<String>("title") ?: "",
                            call.argument<String>("text") ?: "",
                            call.argument<Int>("percent"),
                        )
                        result.success(null)
                    }
                    "stop" -> {
                        UploadService.stop(applicationContext)
                        result.success(null)
                    }
                    else -> result.notImplemented()
                }
            }
    }

    /**
     * Android 13 and later show the upload's notification only if the owner
     * allows it. The upload carries on in the background either way.
     */
    private fun askToShowNotifications() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
    }
}
