package zone.jo.phonoscule

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.QueueMusic
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** The player: the queue as a vertical Cover Flow over a backdrop glowing with the playing cover's colour, and the
 * transport beneath. */
@Composable
fun PlayerScreen(onBack: () -> Unit) {
    var queue by remember { mutableStateOf(Queue.EMPTY) }
    LaunchedEffect(App.queueVersion, App.libraryVersion) {
        queue = withContext(Dispatchers.IO) { Core.queue() }
    }
    val playback = App.playback
    val glow by animateColorAsState(accentColor(playback.album?.accent), tween(GLOW_MS), label = "glow")
    var showTracks by remember { mutableStateOf(false) }

    Column(
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
        Row(modifier = Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Library") }
            Text("Player", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            IconButton(onClick = { showTracks = !showTracks }) {
                Icon(Icons.AutoMirrored.Filled.QueueMusic, contentDescription = "Tracks")
            }
        }

        Box(modifier = Modifier.weight(1f).fillMaxWidth()) {
            if (queue.runs.isEmpty()) {
                Text(
                    "Nothing queued. Play an album from the library.",
                    modifier = Modifier.align(Alignment.Center).padding(24.dp),
                    textAlign = TextAlign.Center,
                )
            } else {
                VerticalCoverFlow(
                    covers = queue.runs.map { FlowCover(it.album?.cover, it.album?.accent) },
                    playing = queue.currentRun,
                    onPlay = { run -> App.command { Core.jumpTo(queue.runs[run].start) } },
                    onToggle = { App.command { Core.togglePlaying() } },
                    modifier = Modifier.fillMaxSize(),
                )
            }
            if (showTracks) RunTracks(queue, playback, modifier = Modifier.align(Alignment.BottomCenter))
        }

        NowPlaying(playback)
        SeekBar(playback, glow)
        Transport(playback)
    }
}

/** The tracks of the playing album run, as the desktop player lists them beside its flow. */
@Composable
private fun RunTracks(queue: Queue, playback: Playback, modifier: Modifier = Modifier) {
    val run = queue.runs.getOrNull(queue.currentRun) ?: return
    val state = rememberLazyListState(initialFirstVisibleItemIndex = (queue.current - run.start - 2).coerceAtLeast(0))
    LazyColumn(
        state = state,
        modifier = modifier.fillMaxWidth().fillMaxSize(0.5f).background(Color.Black.copy(alpha = 0.75f)),
    ) {
        itemsIndexed(queue.tracks.subList(run.start, run.end)) { ix, track ->
            val current = run.start + ix == playback.current
            Text(
                text = track.title,
                style = MaterialTheme.typography.bodyLarge,
                fontWeight = if (current) FontWeight.Bold else FontWeight.Normal,
                color = if (current) Color.White else Color.White.copy(alpha = 0.7f),
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier
                    .fillMaxWidth()
                    .clickable { App.command { Core.jumpTo(run.start + ix) } }
                    .padding(horizontal = 24.dp, vertical = 10.dp),
            )
        }
    }
}

@Composable
private fun NowPlaying(playback: Playback) {
    Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 8.dp),
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
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 24.dp, top = 4.dp),
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
        IconButton(onClick = { App.command { Core.togglePlaying() } }, modifier = Modifier.size(72.dp)) {
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
