package dev.frust.backgroundtask

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat

/**
 * Keeps the process alive while any `frust::BackgroundTask` guard is live. Every start of
 * this service comes from [FrustBackgroundTaskHost.begin].
 */
class FrustBackgroundTaskService : Service() {
    private var inForeground = false
    private var lastStartId = 0

    override fun onCreate() {
        super.onCreate()
        FrustBackgroundTaskHost.attach(this)
    }

    override fun onDestroy() {
        FrustBackgroundTaskHost.detach(this)
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    /**
     * Always calls `startForeground` first, even if every task already ended: a service
     * started with `startForegroundService` must reach it before it may stop.
     *
     * A refusal here comes after `begin` returned true, so the live tasks are expired
     * instead. A refused re-call on a service already in the foreground is only a failed
     * extension, and the service keeps running.
     */
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        lastStartId = startId
        try {
            val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                ServiceInfo.FOREGROUND_SERVICE_TYPE_SHORT_SERVICE
            } else {
                0
            }
            ServiceCompat.startForeground(
                this,
                FrustBackgroundTaskHost.NOTIFICATION_ID,
                buildNotification(),
                type,
            )
            inForeground = true
            FrustBackgroundTaskHost.armWatchdog()
        } catch (e: Exception) {
            FrustBackgroundTaskHost.logRefusal("startForeground", e)
            if (!inForeground) {
                FrustBackgroundTaskHost.expireLive()
                stopSelf(lastStartId)
                return START_NOT_STICKY
            }
        }
        if (FrustBackgroundTaskHost.isIdle()) stopNow()
        return START_NOT_STICKY
    }

    override fun onTimeout(startId: Int) {
        FrustBackgroundTaskHost.timeout()
    }

    override fun onTimeout(startId: Int, fgsType: Int) {
        FrustBackgroundTaskHost.timeout()
    }

    internal fun stopIfForeground() {
        if (inForeground) stopNow()
    }

    /** `stopSelf(lastStartId)`, so a begin whose start is already in flight keeps it alive. */
    internal fun stopNow() {
        if (inForeground) {
            ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
            inForeground = false
        }
        stopSelf(lastStartId)
    }

    /** Deferred, with no actions or call/media category, so a short task can end unseen. */
    private fun buildNotification(): Notification {
        createChannelOnce()
        return NotificationCompat.Builder(this, FrustBackgroundTaskHost.CHANNEL_ID)
            .setSmallIcon(applicationInfo.icon)
            .setContentTitle(applicationInfo.loadLabel(packageManager))
            .setContentText(FrustBackgroundTaskHost.firstTaskName())
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_DEFERRED)
            .build()
    }

    private fun createChannelOnce() {
        if (channelCreated) return
        NotificationManagerCompat.from(this).createNotificationChannel(
            NotificationChannel(
                FrustBackgroundTaskHost.CHANNEL_ID,
                "Background work",
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
        channelCreated = true
    }

    private companion object {
        var channelCreated = false
    }
}
