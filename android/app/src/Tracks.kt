package zone.jo.phonoscule

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.BoxWithConstraints
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
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import kotlin.math.cos
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlin.math.tan
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

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
            pitch = TRACK_PITCH.toPx()
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

/** The album's track `titles` hanging from the top edge side by side, like icicles: in order from the left, and still,
 * each reading upwards at [ICICLE_TILT] from flat, its end along the top and its start below and to the left. The one
 * `scroll` has picked is lit; `first` is the queue index of the first. Each hangs at most [ICICLE_LENGTH] of the
 * height, cut short at its end. They are centred, leaning and all, and take no touches.
 *
 * Where they do not all fit, those that do show, as many to either side of the picked one as it has tracks before and
 * after it, as the desktop's list does: the view only moves as the pick moves along a long album. */
@Composable
fun TrackIcicles(titles: List<String>, first: Int, scroll: TrackScroll, modifier: Modifier = Modifier) {
    val picked by remember { derivedStateOf { scroll.picked } }
    BoxWithConstraints(modifier = modifier) {
        // As many as fit, if every one were as long as a name may hang.
        val lean = maxHeight * ICICLE_LENGTH / tan(Math.toRadians(ICICLE_TILT.toDouble())).toFloat()
        val fits = ((maxWidth - lean) / TRACK_PITCH).toInt() + 1
        val shown = window(titles.size, picked - first, fits.coerceAtLeast(1))
        Icicles(titles.subList(shown.first, shown.last + 1), first + shown.first, picked)
    }
}

/** Which of `count` names show where `fits` do: all of them, or a window placed so the picked one, at `at`, stands as
 * far along it as it does along the whole. */
private fun window(count: Int, at: Int, fits: Int): IntRange {
    if (count <= fits) return 0 until count
    val pick = at.coerceIn(0, count - 1)
    val place = (pick.toFloat() / (count - 1) * (fits - 1)).roundToInt()
    val start = (pick - place).coerceIn(0, count - fits)
    return start until start + fits
}

/** `titles`, of queue indices from `first`, tilted and placed as [TrackIcicles] says. */
@Composable
private fun Icicles(titles: List<String>, first: Int, picked: Int) {
    Layout(
        content = {
            titles.forEachIndexed { k, title -> Icicle(title, lit = first + k == picked) }
        },
    ) { measurables, constraints ->
        val tilt = Math.toRadians(ICICLE_TILT.toDouble())
        val (sin, cos) = sin(tilt).toFloat() to cos(tilt).toFloat()
        val longest = constraints.maxHeight * ICICLE_LENGTH / sin
        val placeables = measurables.map { it.measure(Constraints(maxWidth = longest.roundToInt())) }
        // How far left of its end the longest name's start reaches.
        val lean = (placeables.maxOfOrNull { it.width } ?: 0) * cos
        val pitch = TRACK_PITCH.toPx()
        val firstEnd = (constraints.maxWidth - lean - pitch * (placeables.size - 1)) / 2 + lean
        layout(constraints.maxWidth, constraints.maxHeight) {
            placeables.forEachIndexed { k, placeable ->
                // Turned about the middle of its end, which stands at its place along the top.
                val endX = firstEnd + pitch * k
                val endY = ICICLE_MARGIN.toPx() + placeable.height / 2 * cos
                val x = (endX - placeable.width).roundToInt()
                placeable.placeWithLayer(x, (endY - placeable.height / 2).roundToInt()) {
                    rotationZ = -ICICLE_TILT
                    transformOrigin = TransformOrigin(1f, 0.5f)
                }
            }
        }
    }
}

/** One track's name, on a line, shadowed to stand out from the covers. */
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
    )
}

/** How far apart the names stand, which is also how far a drag goes for one track. */
private val TRACK_PITCH = 28.dp

/** How far from flat the names read, upwards, in degrees. */
private const val ICICLE_TILT = 72f

/** How far below the top edge the names' ends stand. */
private val ICICLE_MARGIN = 8.dp

/** The most of the height a name hangs before it is cut short. */
private const val ICICLE_LENGTH = 0.42f

/** How far ahead of where a fling lets go its momentum carries, in seconds at the speed it let go at. */
private const val FLING_SECONDS = 0.15f

private val SETTLE = spring<Float>(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = Spring.StiffnessMedium)
