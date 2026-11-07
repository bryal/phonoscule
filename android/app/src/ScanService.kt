package zone.jo.phonoscule

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.util.Log

/** Keeps a scan going with the app in the background, where Android would otherwise freeze the process mid-scan, and
 * shows how far it has come.
 *
 * Start it with [start] when a scan starts. It stops itself when the scan ends. */
class ScanService : Service() {
    private val main = Handler(Looper.getMainLooper())
    private val notifications by lazy { this.getSystemService(NotificationManager::class.java) }
    private var updatePending = false

    private val onScan: () -> Unit = {
        if (!App.scanning) {
            this.stopSelf()
        } else if (!updatePending) {
            // Android drops a package's updates past a few a second, which may be the last.
            updatePending = true
            main.postDelayed({
                updatePending = false
                if (App.scanning) notifications.notify(ID, this.notification())
            }, UPDATE_MS)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        notifications.createNotificationChannel(
            NotificationChannel(CHANNEL, "Library scans", NotificationManager.IMPORTANCE_LOW),
        )
        // First, whatever happens next: a service started this way that never calls it crashes the app.
        this.startForeground(ID, this.notification(), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        App.removeScanListener(onScan)
        if (App.scanning) App.onScan(onScan) else this.stopSelf()
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        App.removeScanListener(onScan)
        main.removeCallbacksAndMessages(null)
        super.onDestroy()
    }

    private fun notification(): Notification {
        val progress = App.scanProgress
        val (text, max) = when {
            progress == null -> null to null
            progress.stage == ScanStage.LISTING -> {
                val total = progress.total?.takeIf { it >= progress.done }
                if (total == null) "Listing folders: ${progress.done}" to null
                else "Listing folders: ${progress.done} of about $total" to total
            }
            progress.done == progress.total -> "Finishing up" to null
            else -> "Reading tags: ${progress.done} of ${progress.total} folders" to progress.total
        }
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle("Scanning the music library")
            .setContentText(text)
            .setProgress(max ?: 0, progress?.done ?: 0, max == null)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            // Rather than ten seconds in, by when a rescan is nearly done.
            .setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
            .build()
    }

    companion object {
        private const val CHANNEL = "scan"
        private const val ID = 2
        private const val UPDATE_MS = 250L
        private const val TAG = "phonoscule"

        fun start(context: Context) {
            try {
                context.startForegroundService(Intent(context, ScanService::class.java))
            } catch (e: IllegalStateException) {
                // Started from the background after all. The scan runs on for as long as the process is not frozen.
                Log.w(TAG, "could not keep the scan going in the background: $e")
            }
        }
    }
}
