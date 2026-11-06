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
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.roundToInt
import kotlin.math.sign
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

/** Moves `scroll` along the album's tracks a track per name's width dragged sideways, as if the names were pulled along
 * under the pick, so a drag rightwards picks earlier tracks; on letting go, plays the one it comes to. A flick, a drag
 * still going quickly as it lets go, steps just one track from where it began, however far it went. Vertical drags and
 * taps are left to what is beneath. */
fun Modifier.trackDrag(scroll: TrackScroll): Modifier = this.pointerInput(scroll) {
    val tracker = VelocityTracker()
    // Where the drag has taken the pick. Kept here, since a snap launched for one move has not necessarily landed by
    // the next.
    var target = 0f
    var from = 0
    var pitch = 1f
    detectHorizontalDragGestures(
        onDragStart = {
            scroll.dragging = true
            tracker.resetTracking()
            target = scroll.position.value
            from = scroll.picked
            pitch = TRACK_PITCH.toPx()
            scroll.stop()
        },
        onHorizontalDrag = { change, dx ->
            tracker.addPosition(change.uptimeMillis, change.position)
            val range = scroll.tracks
            if (range.isEmpty()) return@detectHorizontalDragGestures
            target = (target - dx / pitch).coerceIn(range.first.toFloat(), range.last.toFloat())
            scroll.snapTo(target)
        },
        onDragEnd = {
            scroll.dragging = false
            val range = scroll.tracks
            if (range.isEmpty()) return@detectHorizontalDragGestures
            val velocity = tracker.calculateVelocity().x
            // Rightwards is towards earlier tracks.
            val to = if (abs(velocity) > FLICK_SPEED.toPx()) from - velocity.sign.toInt() else target.roundToInt()
            scroll.settle(to.coerceIn(range))
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
 * Where they do not all fit, a window of them shows, sliding along as the pick does, placed so the pick stands as far
 * along it as it does along the whole album, as the desktop's list does. Between the album's ends, a name at either
 * edge of the window may show only in part. */
@Composable
fun TrackIcicles(titles: List<String>, first: Int, scroll: TrackScroll, modifier: Modifier = Modifier) {
    val picked by remember { derivedStateOf { scroll.picked } }
    BoxWithConstraints(modifier = modifier) {
        // As many as fit, if every one were as long as a name may hang, with half a name's width to spare at each
        // side for the ones at the ends. In pixels.
        val (width, hang, pitch) = with(LocalDensity.current) {
            Triple(maxWidth.toPx(), (maxHeight * ICICLE_LENGTH).toPx(), TRACK_PITCH.toPx())
        }
        val lean = hang / tan(Math.toRadians(ICICLE_TILT.toDouble())).toFloat()
        val fits = (((width - lean - pitch) / pitch).toInt() + 1).coerceAtLeast(1)
        val count = titles.size
        if (count <= fits) {
            Icicles(titles, first, picked, slots = count, offset = { 0f }, lean = null)
        } else {
            // Which track stands at the window's first place: from the first track's, at the album's start, to the
            // last's but `fits`, at its end, in step with the pick.
            val start = { (scroll.position.value - first).coerceIn(0f, count - 1f) * (count - fits) / (count - 1) }
            val from by remember(titles, first, fits) { derivedStateOf { floor(start()).toInt() } }
            // One more to either side, which a slide brings partly in.
            val shown = (from - 1).coerceAtLeast(0)..(from + fits).coerceAtMost(count - 1)
            Icicles(
                titles.subList(shown.first, shown.last + 1),
                first + shown.first,
                picked,
                slots = fits,
                offset = { start() - shown.first },
                lean = lean,
            )
        }
    }
}

/** `titles`, of queue indices from `first`, tilted and placed as [TrackIcicles] says: in `slots` places centred across
 * the width, the first of them at `offset` tracks into `titles`. The centring allows for the longest of them to lean
 * left, or for `lean` pixels if given, so that names coming and going do not shift the rest. */
@Composable
private fun Icicles(titles: List<String>, first: Int, picked: Int, slots: Int, offset: () -> Float, lean: Float?) {
    Layout(
        content = {
            titles.forEachIndexed { k, title -> Icicle(title, lit = first + k == picked) }
        },
    ) { measurables, constraints ->
        val tilt = Math.toRadians(ICICLE_TILT.toDouble())
        val (sin, cos) = sin(tilt).toFloat() to cos(tilt).toFloat()
        val longest = constraints.maxHeight * ICICLE_LENGTH / sin
        val placeables = measurables.map { it.measure(Constraints(maxWidth = longest.roundToInt())) }
        // How far left of its end a name's start reaches.
        val reach = lean ?: ((placeables.maxOfOrNull { it.width } ?: 0) * cos)
        val pitch = TRACK_PITCH.toPx()
        val firstPlace = (constraints.maxWidth - reach - pitch * (slots - 1)) / 2 + reach
        layout(constraints.maxWidth, constraints.maxHeight) {
            // Read here, not in composition: the window slides every frame of a drag without recomposing.
            val at = offset()
            placeables.forEachIndexed { k, placeable ->
                // Turned about the middle of its end, which stands at its place along the top.
                val endX = firstPlace + pitch * (k - at)
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
private val TRACK_PITCH = 19.6.dp

/** How far from flat the names read, upwards, in degrees. */
private const val ICICLE_TILT = 90f

/** How far below the top edge the names' ends stand. */
private val ICICLE_MARGIN = 8.dp

/** The most of the height a name hangs before it is cut short. */
private const val ICICLE_LENGTH = 0.42f

/** How fast, per second, a drag must be going as it lets go to be a flick. */
private val FLICK_SPEED = 300.dp

private val SETTLE = spring<Float>(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = Spring.StiffnessMedium)
