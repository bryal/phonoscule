package zone.jo.phonoscule

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import java.util.Collections
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** Cover art, decoded from the full-size JPEGs the core makes, and kept for as long as memory allows.
 *
 * A cover is decoded at the smallest power-of-two reduction that still covers the edge it is drawn at, which JPEG
 * decodes for little more than the cost of reading it: a grid tile takes a sixteenth of the pixels the cover flow does. */
object Covers {
    /** The edge the core makes covers at. */
    private const val FULL = 800

    private val cache = object : LruCache<Key, Bitmap>((Runtime.getRuntime().maxMemory() / 6).toInt()) {
        override fun sizeOf(key: Key, value: Bitmap) = value.byteCount
    }

    /** Covers the core had no file for, not to be asked for again until [forgetMissing]. */
    private val missing = Collections.synchronizedSet(mutableSetOf<Long>())

    /** Bumped by [forgetMissing], so that a load asked before it does not mark its cover missing after it. */
    @Volatile
    private var generation = 0

    private data class Key(val cover: Long, val sample: Int)

    /** The cover, if it is decoded already at a size good for `edge` pixels. */
    fun peek(cover: Long, edge: Int): Bitmap? = cache[Key(cover, sampleFor(edge))]

    /** The cover at a size good for `edge` pixels, or null if there is no cover to be had yet. Blocks while the core
     * makes the file, so call it off the main thread. */
    fun load(cover: Long, edge: Int): Bitmap? {
        val key = Key(cover, sampleFor(edge))
        cache[key]?.let { return it }
        if (cover in missing) return null
        val asked = generation
        val file = try {
            Core.cover(cover)
        } catch (e: CoreException) {
            null
        }
        if (file == null) {
            if (asked == generation) missing.add(cover)
            return null
        }
        val bitmap = BitmapFactory.decodeFile(file.path, BitmapFactory.Options().apply { inSampleSize = key.sample })
            ?: return null
        cache.put(key, bitmap)
        return bitmap
    }

    /** Forget which covers were missing, because a scan has made more. */
    fun forgetMissing() {
        generation++
        missing.clear()
    }

    private fun sampleFor(edge: Int): Int {
        var sample = 1
        while (FULL / (sample * 2) >= edge && sample < 8) sample *= 2
        return sample
    }
}

/** The cover with id `cover` at a size good for `edge` pixels, loading it if need be. Null until it has loaded, or
 * while there is none. */
@Composable
fun rememberCover(cover: Long?, edge: Int): Bitmap? {
    val bitmap by produceState(cover?.let { Covers.peek(it, edge) }, cover, edge, App.coversVersion) {
        // The value outlives a change of keys, and would go on showing the cover asked for before.
        value = cover?.let { Covers.peek(it, edge) }
        if (cover != null && value == null) value = withContext(Dispatchers.IO) { Covers.load(cover, edge) }
    }
    return bitmap
}
