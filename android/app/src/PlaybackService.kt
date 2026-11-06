package zone.jo.phonoscule

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.Uri
import android.os.Looper
import android.os.PowerManager
import androidx.annotation.OptIn
import androidx.core.content.ContextCompat
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.common.SimpleBasePlayer
import androidx.media3.common.util.UnstableApi
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import com.google.common.util.concurrent.Futures
import com.google.common.util.concurrent.ListenableFuture
import kotlin.concurrent.thread

/** Keeps playback going with the screen off, and puts it on the lock screen, in the notification shade, and under the
 * headset's buttons.
 *
 * The engine plays from the Rust core whatever the service does. This only tells the system about it, through a media
 * session over [CorePlayer], and stops for what a music player should stop for: another app taking the audio, or the
 * headphones coming out. */
@OptIn(UnstableApi::class)
class PlaybackService : MediaSessionService() {
    private lateinit var player: CorePlayer
    private lateinit var session: MediaSession
    private lateinit var focus: Focus

    private val onPlayback: (Playback) -> Unit = { playback ->
        player.refresh()
        focus.follow(playback.playing)
    }

    private val onQueue: () -> Unit = {
        thread(name = "phonoscule-session-queue") {
            val queue = try {
                Core.queue()
            } catch (e: CoreException) {
                return@thread
            }
            mainExecutor.execute { player.setQueue(queue) }
        }
    }

    override fun onCreate() {
        super.onCreate()
        player = CorePlayer(mainLooper)
        focus = Focus(this)
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        session = MediaSession.Builder(this, player).setSessionActivity(open).build()
        App.onPlayback(onPlayback)
        App.onQueue(onQueue)
        onQueue()
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession = session

    // Swiped away from the recents while paused: nothing is left to keep going for.
    override fun onTaskRemoved(rootIntent: Intent?) {
        if (!App.playback.playing) this.stopSelf()
    }

    override fun onDestroy() {
        App.removePlaybackListener(onPlayback)
        App.removeQueueListener(onQueue)
        focus.follow(false)
        session.release()
        player.release()
        super.onDestroy()
    }
}

/** The core's queue and playback, as a Media3 player: what the media session shows, and how its buttons reach the core.
 *
 * Holds no state of its own beyond the last queue it was given. Call [refresh] when the playback changes. */
@OptIn(UnstableApi::class)
private class CorePlayer(looper: Looper) : SimpleBasePlayer(looper) {
    private var queue = Queue.EMPTY

    fun setQueue(queue: Queue) {
        this.queue = queue
        this.invalidateState()
    }

    fun refresh() = this.invalidateState()

    override fun getState(): State {
        val playback = App.playback
        val builder = State.Builder()
            .setAvailableCommands(COMMANDS)
            .setPlayWhenReady(playback.playing, PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST)
            .setPlaybackState(if (queue.tracks.isEmpty()) STATE_IDLE else STATE_READY)
            .setPlaylist(this.playlist(playback))
        val current = playback.current
        if (current != null && current in queue.tracks.indices) {
            builder.setCurrentMediaItemIndex(current)
            builder.setContentPositionMs(
                if (playback.playing) {
                    PositionSupplier.getExtrapolating(playback.positionMs, 1f)
                } else {
                    PositionSupplier.getConstant(playback.positionMs)
                },
            )
        }
        return builder.build()
    }

    /** The queue as media items. Only the playing one carries a length and artwork, which the core knows of no other. */
    private fun playlist(playback: Playback): List<MediaItemData> {
        val albums = queue.runs.mapNotNull { it.album }.associateBy { it.id }
        return queue.tracks.mapIndexed { ix, track ->
            val album = albums[track.album]
            val playing = ix == playback.current
            val metadata = MediaMetadata.Builder()
                .setTitle(track.title)
                .setArtist(album?.artist)
                .setAlbumTitle(album?.title)
                .setAlbumArtist(album?.artist)
                .apply { if (playing) playback.coverFile?.let { setArtworkUri(Uri.fromFile(it)) } }
                .build()
            MediaItemData.Builder(ix)
                .setMediaItem(MediaItem.Builder().setMediaId("$ix").setMediaMetadata(metadata).build())
                .setDurationUs(if (playing) playback.lengthMs?.times(1000) ?: C.TIME_UNSET else C.TIME_UNSET)
                .build()
        }
    }

    override fun handleSetPlayWhenReady(playWhenReady: Boolean): ListenableFuture<*> {
        App.command { Core.setPlaying(playWhenReady) }
        return Futures.immediateVoidFuture()
    }

    override fun handleSeek(mediaItemIndex: Int, positionMs: Long, seekCommand: Int): ListenableFuture<*> {
        if (mediaItemIndex != App.playback.current && mediaItemIndex in queue.tracks.indices) {
            App.command { Core.jumpTo(mediaItemIndex) }
        } else {
            App.command { Core.seek(positionMs.coerceAtLeast(0)) }
        }
        return Futures.immediateVoidFuture()
    }

    override fun handleStop(): ListenableFuture<*> {
        App.command { Core.setPlaying(false) }
        return Futures.immediateVoidFuture()
    }

    companion object {
        private val COMMANDS = Player.Commands.Builder()
            .addAll(
                COMMAND_PLAY_PAUSE,
                COMMAND_STOP,
                COMMAND_SEEK_IN_CURRENT_MEDIA_ITEM,
                COMMAND_SEEK_TO_PREVIOUS,
                COMMAND_SEEK_TO_PREVIOUS_MEDIA_ITEM,
                COMMAND_SEEK_TO_NEXT,
                COMMAND_SEEK_TO_NEXT_MEDIA_ITEM,
                COMMAND_SEEK_TO_MEDIA_ITEM,
                COMMAND_GET_CURRENT_MEDIA_ITEM,
                COMMAND_GET_TIMELINE,
                COMMAND_GET_METADATA,
            )
            .build()
    }
}

/** What a music player owes the rest of the phone while it plays: it takes the audio focus, and gives it back; it
 * pauses for a call and comes back after; it pauses when the headphones come out; and it keeps the CPU awake to decode.
 *
 * Driven by [follow], with whether the engine is playing. */
private class Focus(private val context: Context) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val wake = context.getSystemService(PowerManager::class.java)
        .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "phonoscule:playback")
        .apply { setReferenceCounted(false) }

    private var held = false

    /** Paused because another app took the audio for a while, to resume when it is given back. */
    private var lent = false

    private val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
        .setAudioAttributes(
            AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .build(),
        )
        .setOnAudioFocusChangeListener { change ->
            when (change) {
                AudioManager.AUDIOFOCUS_LOSS -> {
                    lent = false
                    App.command { Core.setPlaying(false) }
                }
                AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                    lent = App.playback.playing
                    App.command { Core.setPlaying(false) }
                }
                AudioManager.AUDIOFOCUS_GAIN -> if (lent) {
                    lent = false
                    App.command { Core.setPlaying(true) }
                }
            }
        }
        .build()

    private val noisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) App.command { Core.setPlaying(false) }
        }
    }

    fun follow(playing: Boolean) {
        if (playing && !held) {
            held = true
            audio.requestAudioFocus(request)
            val filter = IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
            ContextCompat.registerReceiver(context, noisy, filter, ContextCompat.RECEIVER_NOT_EXPORTED)
            wake.acquire()
        } else if (!playing && held) {
            held = false
            context.unregisterReceiver(noisy)
            wake.release()
            // Kept while lent, or the focus could never be given back.
            if (!lent) audio.abandonAudioFocusRequest(request)
        }
    }
}
