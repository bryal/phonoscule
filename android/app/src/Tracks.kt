package zone.jo.phonoscule

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shadow
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.layout.layout
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

/** Which of the playing album's tracks is picked: a queue index, fractional while it moves. Move it with [trackDrag];
 * it follows what plays otherwise. */
class TrackScroll internal constructor(private val scope: CoroutineScope, first: Int) {
    val position = Animatable(first.toFloat())
    internal var dragging = false
    internal var tracks = IntRange.EMPTY
    internal var playing: Int? = null
    internal var onPlay: (Int) -> Unit = {}

    /** The picked track: the playing one, or the one a drag has come to. */
    val picked: Int get() = position.value.roundToInt()

    /** Pick track `ix`, and play it unless it plays already. */
    internal fun settle(ix: Int, velocity: Float = 0f) {
        if (tracks.isEmpty()) return
        scope.launch { position.animateTo(ix.toFloat(), SETTLE, initialVelocity = velocity) }
        if (ix != playing) onPlay(ix)
    }

    internal fun snapTo(at: Float) {
        scope.launch { position.snapTo(at) }
    }

    internal fun stop() {
        scope.launch { position.stop() }
    }
}

/** A [TrackScroll] over the queue indices `tracks`, the playing album's, following `playing` while not dragged.
 * Coming to rest on a track other than `playing` calls `onPlay` with it. */
@Composable
fun rememberTrackScroll(tracks: IntRange, playing: Int?, onPlay: (Int) -> Unit): TrackScroll {
    val scope = rememberCoroutineScope()
    val scroll = remember { TrackScroll(scope, playing ?: 0) }
    scroll.tracks = tracks
    scroll.playing = playing
    scroll.onPlay = onPlay
    LaunchedEffect(playing) {
        if (!scroll.dragging && playing != null) scroll.position.animateTo(playing.toFloat(), SETTLE)
    }
    return scroll
}

/** Moves `scroll` along the album's tracks as a sideways drag goes, a track per name passed, and on letting go plays
 * the one it comes to. Vertical drags and taps are left to what is beneath. */
fun Modifier.trackDrag(scroll: TrackScroll): Modifier = this.pointerInput(scroll) {
    val tracker = VelocityTracker()
    // Where the drag has taken the pick. Kept here, since a snap launched for one move has not necessarily landed by
    // the next.
    var target = 0f
    var pitch = 1f
    detectHorizontalDragGestures(
        onDragStart = {
            scroll.dragging = true
            tracker.resetTracking()
            target = scroll.position.value
            pitch = pitchFor(size.width.toFloat(), scroll.tracks.count())
            scroll.stop()
        },
        onHorizontalDrag = { change, dx ->
            tracker.addPosition(change.uptimeMillis, change.position)
            val range = scroll.tracks
            if (range.isEmpty()) return@detectHorizontalDragGestures
            target = (target + dx / pitch).coerceIn(range.first.toFloat(), range.last.toFloat())
            scroll.snapTo(target)
        },
        onDragEnd = {
            scroll.dragging = false
            val range = scroll.tracks
            if (range.isEmpty()) return@detectHorizontalDragGestures
            val velocity = tracker.calculateVelocity().x / pitch
            scroll.settle((target + velocity * FLING_SECONDS).roundToInt().coerceIn(range), velocity)
        },
        onDragCancel = {
            scroll.dragging = false
            val range = scroll.tracks
            if (!range.isEmpty()) scroll.settle(target.roundToInt().coerceIn(range))
        },
    )
}

/** The album's track `titles` hanging from the top edge side by side, each written downwards, like icicles: in order
 * from the left, centred, and still. The one `scroll` has picked is lit; `first` is the queue index of the first. Each
 * hangs at most [ICICLE_LENGTH] of the height. They take no touches. */
@Composable
fun TrackIcicles(titles: List<String>, first: Int, scroll: TrackScroll, modifier: Modifier = Modifier) {
    val picked by remember { derivedStateOf { scroll.picked } }
    Layout(
        content = {
            titles.forEachIndexed { k, title -> Icicle(title, lit = first + k == picked) }
        },
        modifier = modifier,
    ) { measurables, constraints ->
        val hang = Constraints(maxHeight = (constraints.maxHeight * ICICLE_LENGTH).roundToInt())
        val placeables = measurables.map { it.measure(hang) }
        val pitch = pitchFor(constraints.maxWidth.toFloat(), placeables.size)
        val left = (constraints.maxWidth - pitch * placeables.size) / 2
        layout(constraints.maxWidth, constraints.maxHeight) {
            placeables.forEachIndexed { k, placeable ->
                placeable.place((left + pitch * (k + 0.5f) - placeable.width / 2).roundToInt(), 0)
            }
        }
    }
}

/** How far apart `count` names stand across `width` pixels: [TRACK_PITCH], or closer, so that they all fit. */
private fun Density.pitchFor(width: Float, count: Int): Float =
    if (count == 0) TRACK_PITCH.toPx() else minOf(TRACK_PITCH.toPx(), width / count)

/** One track's name, written downwards: laid out along the height it may hang, then turned a quarter clockwise. */
@Composable
private fun Icicle(title: String, lit: Boolean) {
    Text(
        title,
        style = MaterialTheme.typography.bodyMedium.copy(
            color = if (lit) Color.White else Color.White.copy(alpha = 0.6f),
            shadow = Shadow(Color.Black.copy(alpha = if (lit) 0.9f else 0.8f), Offset(2f, 2f), blurRadius = 4f),
        ),
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        modifier = Modifier.layout { measurable, constraints ->
            val text = measurable.measure(Constraints(maxWidth = constraints.maxHeight))
            layout(text.height, text.width) {
                text.placeWithLayer((text.height - text.width) / 2, (text.width - text.height) / 2) { rotationZ = 90f }
            }
        },
    )
}

/** How far apart the names stand while they fit, which is also how far a drag goes for one track. */
private val TRACK_PITCH = 34.dp

/** The most of the height a name hangs before it is cut short. */
private const val ICICLE_LENGTH = 0.42f

/** How far ahead of where a fling lets go its momentum carries, in seconds at the speed it let go at. */
private const val FLING_SECONDS = 0.15f

private val SETTLE = spring<Float>(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = Spring.StiffnessMedium)
