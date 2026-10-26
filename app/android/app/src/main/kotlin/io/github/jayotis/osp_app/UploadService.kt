package io.github.jayotis.osp_app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * Keeps the app running while a post is sent to the home node, with a
 * notification showing how far it got.
 *
 * The sending itself happens in the app's Rust code (osp-client). This
 * service only stops Android from stopping the app once it leaves the
 * screen. If Android stops it anyway, the post carries on when the app is
 * next opened, so the service is never restarted on its own.
 */
class UploadService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notification = notification(this)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            // Android 14 and later need the type, as declared in the manifest.
            startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        } else {
            startForeground(ID, notification)
        }
        started = true
        // Stopping a service before it calls startForeground crashes the
        // app, so a post that ended that quickly stops it here.
        if (!wanted) stopSelf()
        return START_NOT_STICKY
    }

    // Android 15 and later allow a dataSync service six hours a day.
    override fun onTimeout(startId: Int, fgsType: Int) = stop(this)

    // Swiped away from the recent apps: the app's screens are gone, so
    // nothing would stop this service.
    override fun onTaskRemoved(rootIntent: Intent?) = stop(this)

    override fun onDestroy() {
        started = false
        getSystemService(NotificationManager::class.java).cancel(ID)
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "uploads"
        private const val ID = 1

        // A post is being sent.
        private var wanted = false

        // The service is running and showing its notification.
        private var started = false

        // What the notification shows.
        private var title = "Sending your post"
        private var text = ""
        private var percent: Int? = null

        /** Starts the service. Only works while the app is on screen. */
        fun start(context: Context) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                val channel = NotificationChannel(
                    CHANNEL,
                    "Sending posts",
                    NotificationManager.IMPORTANCE_LOW,
                )
                context.getSystemService(NotificationManager::class.java)
                    .createNotificationChannel(channel)
            }
            title = "Sending your post"
            text = ""
            percent = null
            wanted = true
            val intent = Intent(context, UploadService::class.java)
            try {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    context.startForegroundService(intent)
                } else {
                    context.startService(intent)
                }
            } catch (e: RuntimeException) {
                wanted = false
                throw e
            }
        }

        /** Updates the notification. `percent` null shows a moving bar. */
        fun show(context: Context, title: String, text: String, percent: Int?) {
            this.title = title
            this.text = text
            this.percent = percent
            if (!started || !wanted) return
            context.getSystemService(NotificationManager::class.java)
                .notify(ID, notification(context))
        }

        fun stop(context: Context) {
            wanted = false
            if (started) context.stopService(Intent(context, UploadService::class.java))
        }

        private fun notification(context: Context): Notification {
            val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                Notification.Builder(context, CHANNEL)
            } else {
                @Suppress("DEPRECATION")
                Notification.Builder(context)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                // Without this, Android 12 and later may wait 10 s to show it.
                builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
            }
            // Tapping it opens the app where it was.
            val open = context.packageManager.getLaunchIntentForPackage(context.packageName)
            if (open != null) {
                builder.setContentIntent(
                    PendingIntent.getActivity(context, 0, open, PendingIntent.FLAG_IMMUTABLE),
                )
            }
            return builder
                .setSmallIcon(android.R.drawable.stat_sys_upload)
                .setContentTitle(title)
                .setContentText(text)
                .setProgress(100, percent ?: 0, percent == null)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .build()
        }
    }
}
