package zone.jo.phonoscule

import android.app.Application
import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.widget.Toast
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlin.concurrent.thread

class PhonosculeApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Core.start(filesDir, cacheDir)
        App.init(this)
    }
}

/** What the screens and the media service share, for the life of the process.
 *
 * The core holds the library and the queue. This holds the little Compose observes to know when to ask again,
 * fed by a thread parked on [Core.nextNotices]. Only touched on the main thread. */
object App {
    private lateinit var context: Context
    private val main = Handler(Looper.getMainLooper())

    var storage by mutableStateOf<Storage?>(null)
        private set

    var playback by mutableStateOf(Playback.NONE)
        private set

    var scanning by mutableStateOf(false)
        private set

    /** Bumped whenever the albums change, so lists showing them ask again. */
    var libraryVersion by mutableIntStateOf(0)
        private set

    /** Bumped whenever the queue changes. */
    var queueVersion by mutableIntStateOf(0)
        private set

    /** Bumped whenever covers become available, so a cover that could not be had is asked for again. */
    var coversVersion by mutableIntStateOf(0)
        private set

    /** Whether a screen is showing: progress is only worth reporting many times a second then. */
    var foreground = false

    private val playbackListeners = mutableListOf<(Playback) -> Unit>()
    private val queueListeners = mutableListOf<() -> Unit>()

    fun init(context: Context) {
        this.context = context
        storage = Storage.restore(context)
        playback = Core.playback()
        Workers { storage }.start()
        thread(name = "phonoscule-notices", isDaemon = true) { this.followNotices() }
        if (storage != null) Core.scan()
    }

    /** Call `listener` on the main thread whenever [playback] changes. */
    fun onPlayback(listener: (Playback) -> Unit) {
        playbackListeners.add(listener)
    }

    fun removePlaybackListener(listener: (Playback) -> Unit) {
        playbackListeners.remove(listener)
    }

    /** Call `listener` on the main thread whenever the queue changes, or the albums its tracks belong to. */
    fun onQueue(listener: () -> Unit) {
        queueListeners.add(listener)
    }

    fun removeQueueListener(listener: () -> Unit) {
        queueListeners.remove(listener)
    }

    /** Adopt the folder the user picked, forget the queue from the old one, and scan.
     *
     * If the folder cannot be adopted, a toast says why, and the previous folder stays. */
    fun adopt(tree: Uri) {
        thread(name = "phonoscule-adopt") {
            try {
                val adopted = Storage.adopt(context, tree)
                main.post {
                    val changed = storage?.tree != adopted.tree
                    storage = adopted
                    if (changed) Core.clear()
                    Core.scan()
                }
            } catch (e: Exception) {
                main.post { Toast.makeText(context, e.message ?: e.toString(), Toast.LENGTH_LONG).show() }
            }
        }
    }

    /** Call on every return to the foreground. There is nothing to watch a folder with through SAF, so look again. */
    fun resume() {
        foreground = true
        // The grant can be revoked while the app is in the background.
        storage = Storage.restore(context)
        if (storage != null) Core.scan()
    }

    fun pause() {
        foreground = false
    }

    /** Run `action` on a thread of its own, toasting rather than crashing if the core refuses. For commands, which
     * are quick but should not hold up a frame. */
    fun command(action: () -> Unit) {
        thread(name = "phonoscule-command") {
            try {
                action()
            } catch (e: CoreException) {
                Log.e(TAG, "core: ${e.message}")
                main.post { Toast.makeText(context, e.message, Toast.LENGTH_LONG).show() }
            }
        }
    }

    private fun followNotices() {
        while (true) {
            val notices = try {
                Core.nextNotices()
            } catch (e: CoreException) {
                Log.e(TAG, "notices: ${e.message}")
                Thread.sleep(1000)
                continue
            }
            main.post { this.apply(notices) }
            // Batches what arrives meanwhile: the engine reports progress twenty-odd times a second.
            Thread.sleep(if (foreground) 40 else 500)
        }
    }

    private fun apply(notices: Notices) {
        scanning = notices.scanning
        if (notices.library) libraryVersion++
        if (notices.queue) queueVersion++
        if (notices.queue || notices.library) for (listener in queueListeners.toList()) listener()
        if (notices.covers.isNotEmpty()) {
            Covers.forgetMissing()
            coversVersion++
        }
        notices.playback?.let { new ->
            playback = new
            for (listener in playbackListeners.toList()) listener(new)
        }
    }

    private const val TAG = "phonoscule"
}
