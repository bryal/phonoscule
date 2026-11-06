package zone.jo.phonoscule

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import android.util.Log

/** Keeps a scan going with the app in the background, where Android would otherwise freeze the process mid-scan.
 *
 * Start it with [start] when a scan starts. It stops itself when the scan ends. */
class ScanService : Service() {
    private val onScanEnd: () -> Unit = { this.stopSelf() }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val notifications = this.getSystemService(NotificationManager::class.java)
        notifications.createNotificationChannel(
            NotificationChannel(CHANNEL, "Library scans", NotificationManager.IMPORTANCE_LOW),
        )
        val notification = Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle("Scanning the music library")
            .setOngoing(true)
            .build()
        // First, whatever happens next: a service started this way that never calls it crashes the app.
        this.startForeground(ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        App.removeScanEndListener(onScanEnd)
        if (App.scanning) App.onScanEnd(onScanEnd) else this.stopSelf()
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        App.removeScanEndListener(onScanEnd)
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "scan"
        private const val ID = 2
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
