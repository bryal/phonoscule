package zone.jo.phonoscule

import android.graphics.Bitmap
import android.graphics.LightingColorFilter
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sin
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** A cover to show in the flow: the cover's id, if it has one, and the accent it stands in before its pixels load. */
data class FlowCover(val cover: Long?, val accent: Int?)

/** The desktop player's Cover Flow, stood on end: the queue's albums stacked above and below the playing one, tilted
 * away like the pages of a flip book, receding and fading with distance.
 *
 * Drag to browse. Tap a cover in the stacks to bring it to the middle, tap the middle one to play it, or, if it is
 * playing already, to pause or resume. Left alone after browsing, the flow drifts back to what is playing. */
@Composable
fun VerticalCoverFlow(
    covers: List<FlowCover>,
    playing: Int,
    onPlay: (Int) -> Unit,
    onToggle: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val position = remember { Animatable(playing.coerceAtLeast(0).toFloat()) }
    val scope = rememberCoroutineScope()
    var dragging by remember { mutableStateOf(false) }
    // Bumped whenever the listener lets go, restarting the wait before drifting back.
    var browsed by remember { mutableIntStateOf(0) }
    var coverPx by remember { mutableStateOf(1f) }

    LaunchedEffect(playing) {
        if (!dragging && playing >= 0) position.animateTo(playing.toFloat(), SETTLE)
    }
    LaunchedEffect(browsed) {
        if (browsed == 0) return@LaunchedEffect
        delay(DRIFT_BACK_MS)
        if (!dragging && playing >= 0) position.animateTo(playing.toFloat(), SETTLE)
    }

    // Pixels for the covers within reach, loaded as the flow moves: the nearest at full size, the rest smaller.
    val near by remember { derivedStateOf { position.value.roundToInt() } }
    val bitmaps = remember { mutableStateMapOf<Long, Bitmap>() }
    LaunchedEffect(near, covers, App.coversVersion, coverPx) {
        val wanted = mutableMapOf<Long, Int>()
        for (ix in (near - VISIBLE).coerceAtLeast(0)..(near + VISIBLE).coerceAtMost(covers.size - 1)) {
            val cover = covers[ix].cover ?: continue
            val edge = if (abs(ix - near) <= SHARP) coverPx.roundToInt() else coverPx.roundToInt() / 4
            wanted[cover] = maxOf(edge, wanted[cover] ?: 0)
        }
        bitmaps.keys.retainAll(wanted.keys)
        for ((cover, edge) in wanted.entries.sortedBy { (cover, _) -> abs(covers.indexOfFirst { it.cover == cover } - near) }) {
            val bitmap = withContext(Dispatchers.IO) { Covers.load(cover, edge) } ?: continue
            if (bitmaps[cover]?.width != bitmap.width) bitmaps[cover] = bitmap
        }
    }

    val paint = remember { Paint(Paint.ANTI_ALIAS_FLAG or Paint.FILTER_BITMAP_FLAG) }
    val matrix = remember { Matrix() }
    val path = remember { Path() }

    Canvas(
        modifier = modifier
            .pointerInput(covers.size) {
                val tracker = VelocityTracker()
                detectVerticalDragGestures(
                    onDragStart = {
                        dragging = true
                        tracker.resetTracking()
                        scope.launch { position.stop() }
                    },
                    onVerticalDrag = { change, dy ->
                        tracker.addPosition(change.uptimeMillis, change.position)
                        val max = (covers.size - 1).coerceAtLeast(0) + OVERSCROLL
                        val to = (position.value - dy / (coverPx * DRAG_PER_COVER)).coerceIn(-OVERSCROLL, max)
                        scope.launch { position.snapTo(to) }
                    },
                    onDragEnd = {
                        dragging = false
                        val velocity = tracker.calculateVelocity().y / (coverPx * DRAG_PER_COVER)
                        val target = (position.value - velocity * FLING_SECONDS).roundToInt().coerceIn(0, (covers.size - 1).coerceAtLeast(0))
                        scope.launch { position.animateTo(target.toFloat(), SETTLE, initialVelocity = -velocity) }
                        browsed++
                    },
                    onDragCancel = {
                        dragging = false
                        browsed++
                    },
                )
            }
            .pointerInput(covers.size) {
                detectTapGestures { tap ->
                    val flow = Flow(size.width.toFloat(), size.height.toFloat())
                    val hit = flow.hitTest(covers.size, position.value, tap) ?: return@detectTapGestures
                    when {
                        hit != position.value.roundToInt() -> {
                            scope.launch { position.animateTo(hit.toFloat(), SETTLE) }
                            browsed++
                        }
                        hit == playing -> onToggle()
                        else -> onPlay(hit)
                    }
                }
            },
    ) {
        val flow = Flow(size.width, size.height)
        coverPx = flow.coverPx
        val at = position.value
        val order = flow.visible(covers.size, at).sortedByDescending { (_, d) -> abs(d) }
        drawIntoCanvas { canvas ->
            val native = canvas.nativeCanvas
            for ((ix, d) in order) {
                val corners = flow.corners(d)
                val alpha = (fade(d) * 255).roundToInt()
                val light = (255 * (1f - 0.4f * min(abs(d), 1f))).roundToInt()
                paint.alpha = alpha
                paint.colorFilter = LightingColorFilter(light * 0x010101, 0)
                val bitmap = covers[ix].cover?.let { bitmaps[it] }
                if (bitmap != null) {
                    val w = bitmap.width.toFloat()
                    val h = bitmap.height.toFloat()
                    matrix.setPolyToPoly(floatArrayOf(0f, 0f, w, 0f, w, h, 0f, h), 0, corners, 0, 4)
                    native.drawBitmap(bitmap, matrix, paint)
                } else {
                    path.reset()
                    path.moveTo(corners[0], corners[1])
                    for (k in 1 until 4) path.lineTo(corners[2 * k], corners[2 * k + 1])
                    path.close()
                    paint.color = 0xff000000.toInt() or (covers[ix].accent ?: 0x2a2a30)
                    paint.alpha = alpha
                    native.drawPath(path, paint)
                }
            }
        }
        paint.colorFilter = null
    }
}

/** The flow's geometry in a canvas `width` by `height` pixels: where each cover stands, and how it projects.
 *
 * World units are cover edges, with the playing cover facing the camera at the origin. A cover at offset `d` from the
 * flow's position stands where [corners] says: within one cover of the middle it swings between facing the camera and
 * the tilted pose of the stacks; beyond that it slides along its stack. */
private class Flow(width: Float, val height: Float) {
    private val cx = width / 2
    private val cy = height / 2

    /** The middle cover's edge in pixels. */
    val coverPx = min(width * 0.72f, height * 0.42f)

    /** Pixels per world unit at the origin's depth. */
    private val focal = coverPx * CAMERA_Z

    /** The covers near enough to `at` to be drawn, with their fractional offsets from it. */
    fun visible(count: Int, at: Float): List<Pair<Int, Float>> {
        val lo = (at - VISIBLE).toInt().coerceAtLeast(0)
        val hi = (at + VISIBLE).toInt().coerceAtMost(count - 1)
        return (lo..hi).map { it to it - at }.filter { (_, d) -> abs(d) <= VISIBLE }
    }

    /** The screen corners of the cover at offset `d`: top left, top right, bottom right, bottom left, as x, y pairs. */
    fun corners(d: Float): FloatArray {
        val swing = d.coerceIn(-1f, 1f)
        val slide = d - swing
        // Later covers go below, so down the screen is down the queue.
        val y = -(swing * SIDE + slide * STEP)
        val z = abs(swing) * SIDE_Z + abs(slide) * STEP_Z
        // The edge nearest the middle recedes, as in the desktop flow.
        val tilt = -swing * TILT
        val scale = 1f - 0.1f * abs(swing)
        val out = FloatArray(8)
        for ((k, corner) in CORNERS.withIndex()) {
            val (lx, ly) = corner
            val wx = lx * scale
            val wy = y + ly * scale * cos(tilt)
            val wz = z + ly * scale * sin(tilt)
            val depth = CAMERA_Z - wz
            out[2 * k] = cx + focal * wx / depth
            out[2 * k + 1] = cy - focal * wy / depth
        }
        return out
    }

    /** The cover under `point`, front-most first, or null if there is none. */
    fun hitTest(count: Int, at: Float, point: Offset): Int? = this.visible(count, at)
        .sortedBy { (_, d) -> abs(d) }
        .firstOrNull { (_, d) -> fade(d) > 0.1f && inside(this.corners(d), point) }
        ?.first

    private fun inside(quad: FloatArray, p: Offset): Boolean {
        var sign = 0
        for (k in 0 until 4) {
            val (ax, ay) = quad[2 * k] to quad[2 * k + 1]
            val (bx, by) = quad[(2 * k + 2) % 8] to quad[(2 * k + 3) % 8]
            val cross = (bx - ax) * (p.y - ay) - (by - ay) * (p.x - ax)
            val s = if (cross > 0) 1 else if (cross < 0) -1 else 0
            if (s != 0) {
                if (sign != 0 && s != sign) return false
                sign = s
            }
        }
        return true
    }

    companion object {
        /** The corners of a cover in its own frame, in [corners]'s order. */
        private val CORNERS = listOf(-0.5f to 0.5f, 0.5f to 0.5f, 0.5f to -0.5f, -0.5f to -0.5f)
    }
}

/** How visible a cover at offset `d` is: wholly up to [FADE_START], smoothly gone at [FADE_END]. */
private fun fade(d: Float): Float {
    val t = ((FADE_END - abs(d)) / (FADE_END - FADE_START)).coerceIn(0f, 1f)
    return t * t * (3 - 2 * t)
}

/** The camera's distance from the middle cover. */
private const val CAMERA_Z = 3.2f

private const val FADE_START = 1f
private const val FADE_END = 5.5f

/** How far either side covers are drawn: just past the fade, so none pops in or out. */
private const val VISIBLE = 6

/** How many covers either side of the middle are loaded at full size. */
private const val SHARP = 2

/** Where the stacks start, from the middle, and how far apart their covers are. */
private const val SIDE = 0.75f
private const val STEP = 0.22f

/** How far back the stacks stand, and how much further back each cover in them. */
private const val SIDE_Z = -0.8f
private const val STEP_Z = -0.12f

/** The stacks' tilt, in radians. */
private const val TILT = 1.1f

/** How far a finger moves the flow by one cover, as a fraction of the middle cover's edge. */
private const val DRAG_PER_COVER = 0.35f

/** How far a fling carries on, as the seconds its release velocity would last. */
private const val FLING_SECONDS = 0.3f

/** How far past either end the flow can be dragged before it stops giving. */
private const val OVERSCROLL = 0.4f

/** How long the flow waits after being browsed before it drifts back to what is playing. */
private const val DRIFT_BACK_MS = 6000L

private val SETTLE = spring<Float>(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = Spring.StiffnessLow)
