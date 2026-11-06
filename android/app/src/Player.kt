package zone.jo.phonoscule

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Repeat
import androidx.compose.material.icons.filled.RepeatOn
import androidx.compose.material.icons.filled.RepeatOneOn
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material.icons.filled.SkipPrevious
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** The player: the queue as a vertical Cover Flow over a backdrop glowing with the playing cover's colour, and the
 * transport beneath. */
@Composable
fun PlayerScreen() {
    var queue by remember { mutableStateOf(Queue.EMPTY) }
    LaunchedEffect(App.queueVersion, App.libraryVersion) {
        queue = withContext(Dispatchers.IO) { Core.queue() }
    }
    val covers = remember(queue) { queue.runs.map { FlowCover(it.album?.cover, it.album?.accent) } }
    val playback = App.playback
    val glow by animateColorAsState(accentColor(playback.album?.accent), tween(GLOW_MS), label = "glow")
    val run = queue.runs.getOrNull(queue.runOf(playback.current))
    val runTracks = if (run == null) IntRange.EMPTY else run.start until run.end
    val tracks = rememberTrackScroll(runTracks, playback.current, onPlay = { App.command { Core.jumpTo(it) } })
    var footerPx by remember { mutableIntStateOf(0) }
    val footer = with(LocalDensity.current) { footerPx.toDp() }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Color.Black)
            .drawBehind {
                drawRect(
                    Brush.radialGradient(
                        listOf(glow.copy(alpha = 0.45f), glow.copy(alpha = 0.12f), Color.Transparent),
                        center = Offset(size.width / 2, size.height * 0.36f),
                        radius = size.maxDimension * 0.6f,
                    ),
                )
            },
    ) {
        // The flow runs on under the controls, which float over it, with the playing album's track names hanging over
        // its top. A drag up or down moves through the albums, sideways through the album's tracks.
        Box(modifier = Modifier.fillMaxSize().trackDrag(tracks)) {
            if (queue.runs.isEmpty()) {
                Text(
                    "Nothing queued. Play an album from the library.",
                    modifier = Modifier.align(Alignment.Center).padding(bottom = footer).padding(24.dp),
                    textAlign = TextAlign.Center,
                )
            } else {
                VerticalCoverFlow(
                    covers = covers,
                    playing = queue.runOf(playback.current),
                    onPlay = { run ->
                        val start = queue.runs[run].start
                        App.command { Core.jumpTo(start) }
                    },
                    onToggle = { App.command { Core.togglePlaying() } },
                    modifier = Modifier.fillMaxSize(),
                    obscuredBottom = footer,
                )
            }
            if (run != null) {
                val titles = remember(queue, run) { queue.tracks.subList(run.start, run.end).map { it.title } }
                TrackIcicles(titles, run.start, tracks, modifier = Modifier.fillMaxSize())
            }
        }

        // On the glass the library's bars are of, tinted with the glow. It takes every touch on it, so that none
        // reaches a cover seen through it.
        Column(
            modifier = Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .background(glass(glow))
                .pointerInput(Unit) { awaitPointerEventScope { while (true) awaitPointerEvent() } }
                .onSizeChanged { footerPx = it.height },
        ) {
            GlassEdge(glow)
            NowPlaying(playback)
            SeekBar(playback, glow)
            Transport(playback)
        }
    }
}

@Composable
private fun NowPlaying(playback: Playback) {
    Column(
        modifier = Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, top = 8.dp, bottom = 4.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            playback.title.ifEmpty { " " },
            style = MaterialTheme.typography.titleLarge,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            listOfNotNull(playback.album?.artist, playback.album?.title).joinToString("  -  ").ifEmpty { " " },
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** The position in the track, dragged to seek. While dragged it shows where the finger is, not where playback is. */
@Composable
private fun SeekBar(playback: Playback, accent: Color) {
    val length = playback.lengthMs ?: 0
    var dragging by remember { mutableStateOf<Float?>(null) }
    val shown = dragging ?: if (length > 0) playback.positionMs.toFloat() / length else 0f
    Column(modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp)) {
        Slider(
            value = shown.coerceIn(0f, 1f),
            onValueChange = { dragging = it },
            onValueChangeFinished = {
                dragging?.let { to -> App.command { Core.seek((to * length).toLong()) } }
                dragging = null
            },
            enabled = length > 0,
            colors = SliderDefaults.colors(
                thumbColor = Color.White,
                activeTrackColor = lerp(accent, Color.White, 0.5f),
                inactiveTrackColor = Color.White.copy(alpha = 0.2f),
            ),
        )
        Row(modifier = Modifier.fillMaxWidth().padding(horizontal = 4.dp)) {
            Text(clock(((dragging?.times(length))?.toLong()) ?: playback.positionMs), style = MaterialTheme.typography.labelSmall)
            Box(modifier = Modifier.weight(1f))
            Text(playback.lengthMs?.let(::clock) ?: "--:--", style = MaterialTheme.typography.labelSmall)
        }
    }
}

@Composable
private fun Transport(playback: Playback) {
    var shuffling by remember { mutableStateOf(false) }
    Row(
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 10.dp),
        horizontalArrangement = Arrangement.SpaceEvenly,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconButton(onClick = { App.command { Core.setRepeat(playback.repeat.cycled()) } }) {
            Box {
                Icon(
                    when (playback.repeat) {
                        Repeat.OFF -> Icons.Default.Repeat
                        Repeat.TRACK -> Icons.Default.RepeatOneOn
                        Repeat.ALBUM, Repeat.PLAYLIST -> Icons.Default.RepeatOn
                    },
                    contentDescription = "Repeat: ${playback.repeat.name.lowercase()}",
                    tint = if (playback.repeat == Repeat.OFF) Color.White.copy(alpha = 0.6f) else Color.White,
                )
                // The platform's icons have no album repeat, so it is told from the whole queue's by a letter.
                if (playback.repeat == Repeat.ALBUM) {
                    Text("A", style = MaterialTheme.typography.labelSmall, modifier = Modifier.align(Alignment.BottomEnd))
                }
            }
        }
        IconButton(onClick = { App.command { Core.prev() } }) {
            Icon(Icons.Default.SkipPrevious, contentDescription = "Previous", modifier = Modifier.size(36.dp))
        }
        IconButton(onClick = { App.command { Core.togglePlaying() } }, modifier = Modifier.size(68.dp)) {
            Icon(
                if (playback.playing) Icons.Default.Pause else Icons.Default.PlayArrow,
                contentDescription = if (playback.playing) "Pause" else "Play",
                modifier = Modifier.size(56.dp),
            )
        }
        IconButton(onClick = { App.command { Core.next() } }) {
            Icon(Icons.Default.SkipNext, contentDescription = "Next", modifier = Modifier.size(36.dp))
        }
        Box {
            IconButton(onClick = { shuffling = true }) { Icon(Icons.Default.Shuffle, contentDescription = "Shuffle") }
            DropdownMenu(expanded = shuffling, onDismissRequest = { shuffling = false }) {
                for ((label, albums, all) in SHUFFLES) {
                    DropdownMenuItem(text = { Text(label) }, onClick = {
                        shuffling = false
                        App.command { Core.shuffle(albums, all) }
                    })
                }
                DropdownMenuItem(text = { Text("Clear the queue") }, onClick = {
                    shuffling = false
                    App.command { Core.clear() }
                })
            }
        }
    }
}

/** The shuffles, as the desktop player names them: whole albums or single tracks, and all of the queue or all but what
 * is playing. */
private val SHUFFLES = listOf(
    Triple("Shuffle other albums", true, false),
    Triple("Shuffle other tracks", false, false),
    Triple("Shuffle all albums", true, true),
    Triple("Shuffle all tracks", false, true),
)

/** `ms` as minutes and seconds. */
private fun clock(ms: Long): String {
    val s = ms / 1000
    return "%d:%02d".format(s / 60, s % 60)
}

/** How long the backdrop takes to change colour when the album does. */
private const val GLOW_MS = 900
