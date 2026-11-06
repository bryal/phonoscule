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
import kotlin.math.abs
import java.util.concurrent.Executors

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

    private val onPlayback: (Playback, Long) -> Unit = { playback, positionMs ->
        player.follow(playback, positionMs)
        focus.follow(playback.playing)
    }

    /** Fetches the queue off the main thread. One thread, so that a slow fetch cannot land after a newer one. */
    private val fetcher = Executors.newSingleThreadExecutor()

    private val onQueue: () -> Unit = {
        fetcher.execute {
            val queue = try {
                Core.queue()
            } catch (e: CoreException) {
                return@execute
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
        fetcher.shutdown()
        session.release()
        player.release()
        super.onDestroy()
    }
}

/** The core's queue and playback, as a Media3 player: what the media session shows, and how its buttons reach the core.
 *
 * The session's playlist is only the tracks either side of the playing one, enough for previous and next: Media3
 * rebuilds and sends the whole playlist on every change of track, which for a queue of thousands stalls the main
 * thread. Its indices are offsets from [windowStart].
 *
 * Holds no state of its own beyond the last queue it was given. Call [follow] when the playback changes. */
@OptIn(UnstableApi::class)
private class CorePlayer(looper: Looper) : SimpleBasePlayer(looper) {
    private var queue = Queue.EMPTY

    /** The playback the state was last built for, how far into it, and the playlist built with it, starting at queue
     * index [windowStart]. */
    private var shown: Playback? = null
    private var shownPositionMs = 0L
    private var playlist = emptyList<MediaItemData>()
    private var windowStart = 0

    fun setQueue(queue: Queue) {
        this.queue = queue
        shown = null
        this.invalidateState()
    }

    /** Rebuild the state if `playback` changed, or its position more than playing moves it, which the session
     * extrapolates by itself. A seek moves it further than playing could have. */
    fun follow(playback: Playback, positionMs: Long) {
        val same = shown == playback && abs(positionMs - shownPositionMs) < SEEK_THRESHOLD_MS
        if (!same) this.invalidateState()
    }

    override fun getState(): State {
        val playback = App.playback
        val positionMs = App.positionMs
        if (shown != playback) playlist = this.playlist(playback)
        shown = playback
        shownPositionMs = positionMs
        val builder = State.Builder()
            .setAvailableCommands(COMMANDS)
            .setPlayWhenReady(playback.playing, PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST)
            .setPlaybackState(if (queue.tracks.isEmpty()) STATE_IDLE else STATE_READY)
            .setPlaylist(playlist)
        val current = playback.current
        if (current != null && current in queue.tracks.indices) {
            builder.setCurrentMediaItemIndex(current - windowStart)
            builder.setContentPositionMs(
                if (playback.playing) {
                    PositionSupplier.getExtrapolating(positionMs, 1f)
                } else {
                    PositionSupplier.getConstant(positionMs)
                },
            )
        }
        return builder.build()
    }

    /** The window of the queue around the playing track as media items, and sets [windowStart]. Only the playing one
     * carries a length and artwork, which the core knows of no other. */
    private fun playlist(playback: Playback): List<MediaItemData> {
        val tracks = queue.tracks
        val middle = (playback.current ?: 0).coerceIn(0, (tracks.size - 1).coerceAtLeast(0))
        windowStart = (middle - 1).coerceAtLeast(0)
        val window = windowStart until (middle + 2).coerceAtMost(tracks.size)
        val albums = queue.runs.mapNotNull { it.album }.associateBy { it.id }
        return window.map { ix ->
            val track = tracks[ix]
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
        val ix = windowStart + mediaItemIndex
        if (ix != App.playback.current && ix in queue.tracks.indices) {
            App.command { Core.jumpTo(ix) }
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
        /** How far the position may move between reports before it is taken for a seek rather than playing. Reports
         * come at least twice a second. */
        private const val SEEK_THRESHOLD_MS = 1500L

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
            // Refused during a call, say: then there is nothing to play over.
            if (audio.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_FAILED) {
                App.command { Core.setPlaying(false) }
            }
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
