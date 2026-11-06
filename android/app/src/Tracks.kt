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
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch
import kotlin.math.abs
import kotlin.math.roundToInt

/** Where the queue's track names stand: the queue index in the middle, fractional while they move. Move it with
 * [trackDrag]; it follows what plays otherwise. */
class TrackScroll internal constructor(private val scope: CoroutineScope, first: Int) {
    val position = Animatable(first.toFloat())
    internal var dragging = false
    internal var count = 0
    internal var playing: Int? = null
    internal var onPlay: (Int) -> Unit = {}

    /** Bring track `ix` to the middle, and play it unless it plays already. */
    internal fun settle(ix: Int, velocity: Float = 0f) {
        if (count == 0) return
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

/** A [TrackScroll] over a queue of `count` tracks, following `playing` while not dragged. Coming to rest on a track
 * other than `playing` calls `onPlay` with it. */
@Composable
fun rememberTrackScroll(count: Int, playing: Int?, onPlay: (Int) -> Unit): TrackScroll {
    val scope = rememberCoroutineScope()
    val scroll = remember { TrackScroll(scope, playing ?: 0) }
    scroll.count = count
    scroll.playing = playing
    scroll.onPlay = onPlay
    LaunchedEffect(playing) {
        if (!scroll.dragging && playing != null) scroll.position.animateTo(playing.toFloat(), SETTLE)
    }
    return scroll
}

/** Moves `scroll` a track per [TRACK_PITCH] dragged sideways, as the names follow the finger, and on letting go
 * settles on a track and plays it. Vertical drags and taps are left to what is beneath. */
fun Modifier.trackDrag(scroll: TrackScroll): Modifier = this.pointerInput(scroll) {
    val pitch = TRACK_PITCH.toPx()
    val tracker = VelocityTracker()
    // Where the drag has taken the names. Kept here, since a snap launched for one move has not necessarily landed by
    // the next.
    var target = 0f
    val last = { (scroll.count - 1).coerceAtLeast(0) }
    detectHorizontalDragGestures(
        onDragStart = {
            scroll.dragging = true
            tracker.resetTracking()
            target = scroll.position.value
            scroll.stop()
        },
        onHorizontalDrag = { change, dx ->
            tracker.addPosition(change.uptimeMillis, change.position)
            target = (target - dx / pitch).coerceIn(-OVERSCROLL, last() + OVERSCROLL)
            scroll.snapTo(target)
        },
        onDragEnd = {
            scroll.dragging = false
            val velocity = tracker.calculateVelocity().x / pitch
            scroll.settle((target - velocity * FLING_SECONDS).roundToInt().coerceIn(0, last()), -velocity)
        },
        onDragCancel = {
            scroll.dragging = false
            scroll.settle(target.roundToInt().coerceIn(0, last()))
        },
    )
}

/** The queue's track names hanging from the top edge side by side, each written downwards, like icicles: the one at
 * `scroll`'s position in the middle, the rest fading off to either side. `playing` is lit. Each hangs at most
 * [ICICLE_LENGTH] of the height. They take no touches. */
@Composable
fun TrackIcicles(tracks: List<QueueTrack>, playing: Int?, scroll: TrackScroll, modifier: Modifier = Modifier) {
    val near by remember { derivedStateOf { scroll.position.value.roundToInt() } }
    val first = (near - REACH).coerceAtLeast(0)
    val last = (near + REACH).coerceAtMost(tracks.size - 1)
    Layout(
        content = {
            for (ix in first..last) Icicle(tracks[ix].title, lit = ix == playing)
        },
        modifier = modifier,
    ) { measurables, constraints ->
        val hang = Constraints(maxHeight = (constraints.maxHeight * ICICLE_LENGTH).roundToInt())
        val placeables = measurables.map { it.measure(hang) }
        val pitch = TRACK_PITCH.toPx()
        layout(constraints.maxWidth, constraints.maxHeight) {
            // Read here, not in composition: the names move every frame of a drag without recomposing.
            val at = scroll.position.value
            placeables.forEachIndexed { k, placeable ->
                val d = first + k - at
                val x = constraints.maxWidth / 2 + d * pitch - placeable.width / 2
                placeable.placeWithLayer(x.roundToInt(), 0) { alpha = (1f - abs(d) / (REACH + 1)).coerceIn(0f, 1f) }
            }
        }
    }
}

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

/** How far apart the names stand, which is also how far a drag goes for one track. */
private val TRACK_PITCH = 34.dp

/** How many names show to either side of the middle one. */
private const val REACH = 7

/** The most of the height a name hangs before it is cut short. */
private const val ICICLE_LENGTH = 0.42f

/** How far ahead of where a fling lets go its momentum carries, in seconds at the speed it let go at. */
private const val FLING_SECONDS = 0.3f

/** How far past the first or last track a drag goes, in tracks, before it stops. */
private const val OVERSCROLL = 0.4f

private val SETTLE = spring<Float>(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = Spring.StiffnessLow)
