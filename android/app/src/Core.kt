package zone.jo.phonoscule

import java.io.File
import org.json.JSONArray
import org.json.JSONException
import org.json.JSONObject

/** The entire Rust boundary.
 *
 * One native method taking and returning a byte array. Everything else is
 * request/response types encoded as JSON, mirrored on the Rust side in
 * `src/proto.rs`. The symbol name `Java_zone_jo_phonoscule_Core_call` is derived
 * from this package and class name, so renaming either requires the matching
 * rename in `src/jni.rs` or the app fails at runtime with UnsatisfiedLinkError.
 *
 * Each method below sends one request and decodes its response,
 * and throws [CoreException] if the response is a `Response::Error` or cannot be decoded.
 * Ids are 64-bit hashes, sent as the Long with the same bits. */
object Core {
    init {
        System.loadLibrary("phonoscule_android")
    }

    private external fun call(request: ByteArray): ByteArray?

    /** The core's version, and the architecture and build profile it was compiled for. */
    fun version(): Version = request("version").send { resp ->
        Version(resp.getString("core_version"), resp.getString("arch"), resp.getString("profile"))
    }

    /** Load the library and the session the last run left, and ready the engine.
     *
     * Every method below needs this first. Later calls do nothing. */
    fun start(files: File, cache: File) = request("start")
        .put("files", files.absolutePath)
        .put("cache", cache.absolutePath)
        .send { }

    /** The next piece of I/O the core wants done on the music. Blocks until there is one. */
    fun nextOp(): Op = request("next_op").send { resp ->
        val op = resp.getJSONObject("op")
        val id = op.getLong("id")
        val path = op.getString("path")
        when (val kind = op.getString("kind")) {
            "read_dir" -> Op.ReadDir(id, path)
            "open" -> Op.Open(id, path)
            else -> throw JSONException("unknown op $kind")
        }
    }

    /** Hand back what the op numbered `id` came to. */
    fun answer(id: Long, answer: Answer) {
        val encoded = when (answer) {
            is Answer.Listing -> JSONObject().put("kind", "listing").put(
                "entries",
                JSONArray(answer.entries.map {
                    JSONObject().put("name", it.name).put("dir", it.dir).put("mtime", it.mtime).put("size", it.size)
                }),
            )
            is Answer.Opened -> JSONObject().put("kind", "opened").put("fd", answer.fd)
            is Answer.Failed -> JSONObject().put("kind", "failed").put("message", answer.message)
        }
        request("answer").put("id", id).put("answer", encoded).send { }
    }

    /** What has changed since the last call. Blocks until something has. */
    fun nextNotices(): Notices = request("next_notices").send { resp ->
        Notices(
            library = resp.getBoolean("library"),
            queue = resp.getBoolean("queue"),
            scanning = resp.getBoolean("scanning"),
            scanProgress = resp.optObject("scan_progress")?.toScanProgress(),
            covers = resp.getJSONArray("covers").toLongs(),
            playback = resp.optObject("playback")?.toPlayback(),
        )
    }

    /** Scan the picked folder, or queue one more scan if one is running. */
    fun scan() = request("scan").send { }

    /** The albums matching `query` whose genre is one of `genres` and whose artist is one of `artists`. An empty
     * `query` matches every album, as do no `genres` and no `artists`. */
    fun albums(query: String, genres: List<String>, artists: List<String>): List<AlbumSummary> = request("albums")
        .put("query", query)
        .put("genres", JSONArray(genres))
        .put("artists", JSONArray(artists))
        .send { resp -> resp.getJSONArray("albums").mapObjects { it.toAlbumSummary() } }

    /** One album with its tracks, or null if there is none by that id. */
    fun album(id: Long): AlbumDetail? = request("album").put("id", id).send { resp ->
        resp.optObject("album")?.let { AlbumDetail(it.toAlbumSummary(), it.getJSONArray("track_titles").toStrings()) }
    }

    /** Every genre some album has, sorted. */
    fun genres(): List<String> = request("genres").send { resp -> resp.getJSONArray("genres").toStrings() }

    /** Every artist some album is by, sorted. */
    fun artists(): List<String> = request("artists").send { resp -> resp.getJSONArray("artists").toStrings() }

    /** Every order the albums can be listed in, and which is current. */
    fun sorts(): Sorts = request("sorts").send { resp ->
        Sorts(resp.getJSONArray("labels").toStrings(), resp.getInt("current"))
    }

    /** List the albums in order `index` of what [sorts] returned. */
    fun setSort(index: Int) = request("set_sort").put("index", index).send { }

    /** The cover with this id, as a JPEG file. Blocks while the file is made from the artwork.
     *
     * Returns null if a scan has not found the artwork yet, or it will not decode. */
    fun cover(id: Long): File? = request("cover").put("id", id).send { resp -> resp.optStringOrNull("path")?.let(::File) }

    /** The play queue, in tracks and in album runs. */
    fun queue(): Queue = request("queue").send { resp ->
        Queue(
            tracks = resp.getJSONArray("tracks").mapObjects { QueueTrack(it.getString("title"), it.getLong("album")) },
            runs = resp.getJSONArray("runs").mapObjects {
                Run(it.getInt("start"), it.getInt("end"), it.optObject("album")?.toAlbumHeader())
            },
        )
    }

    /** Replace the queue with the tracks of `albums`, and play from track `track` of the first. */
    fun play(albums: List<Long>, track: Int = 0) =
        request("play").put("albums", JSONArray(albums)).put("track", track).send { }

    /** Add the tracks of `albums` to the end of the queue, or only track `track` of the one album. */
    fun enqueue(albums: List<Long>, track: Int? = null) =
        request("enqueue").put("albums", JSONArray(albums)).put("track", track ?: JSONObject.NULL).send { }

    fun clear() = request("clear").send { }

    /** Shuffle the queue in place, by album or by track, and all of it or all but what plays. */
    fun shuffle(albums: Boolean, all: Boolean) = request("shuffle").put("albums", albums).put("all", all).send { }

    fun playback(): Playback = request("playback").send { resp -> resp.getJSONObject("playback").toPlayback() }

    /** Play the track at queue index `index`. */
    fun jumpTo(index: Int) = request("jump_to").put("index", index).send { }

    fun setPlaying(playing: Boolean) = request("set_playing").put("playing", playing).send { }

    fun togglePlaying() = request("toggle_playing").send { }

    fun next() = request("next").send { }

    fun prev() = request("prev").send { }

    fun seek(ms: Long) = request("seek").put("ms", ms).send { }

    fun setRepeat(repeat: Repeat) = request("set_repeat").put("repeat", repeat.wire).send { }

    private fun request(kind: String) = JSONObject().put("kind", kind)

    /** Send one request and decode the response with `decode`.
     *
     * Throws [CoreException] if the response is a `Response::Error` or cannot be decoded. */
    private fun <T> JSONObject.send(decode: (JSONObject) -> T): T {
        val reqKind = this.getString("kind")
        fun malformed(e: Exception) = CoreException("malformed response to $reqKind: ${e.message}")

        val rawReq = this.toString().toByteArray(Charsets.UTF_8)
        val rawResp = call(rawReq) ?: throw CoreException("native call returned null")
        val resp = try {
            JSONObject(String(rawResp, Charsets.UTF_8))
        } catch (e: JSONException) {
            throw malformed(e)
        }
        if (resp.optString("kind") == "error") throw CoreException(resp.optString("message"))
        try {
            return decode(resp)
        } catch (e: JSONException) {
            throw malformed(e)
        } catch (e: ClassCastException) {
            throw malformed(e)
        }
    }
}

private fun JSONObject.toAlbumSummary() = AlbumSummary(
    id = getLong("id"),
    title = getString("title"),
    artist = getString("artist"),
    genre = getString("genre"),
    year = optIntOrNull("year"),
    cover = optLongOrNull("cover"),
    accent = optIntOrNull("accent"),
    tracks = getInt("tracks"),
)

private fun JSONObject.toAlbumHeader() = AlbumHeader(
    id = getLong("id"),
    title = getString("title"),
    artist = getString("artist"),
    cover = optLongOrNull("cover"),
    accent = optIntOrNull("accent"),
)

private fun JSONObject.toPlayback() = Playback(
    current = optIntOrNull("current"),
    playing = getBoolean("playing"),
    positionMs = getLong("position_ms"),
    lengthMs = optLongOrNull("length_ms"),
    repeat = Repeat.entries.first { it.wire == getString("repeat") },
    title = getString("title"),
    album = optObject("album")?.toAlbumHeader(),
    coverFile = optStringOrNull("cover_file")?.let(::File),
)

// `optString` and friends read a JSON null as the string "null", or as zero.
private fun JSONObject.toScanProgress() = ScanProgress(
    stage = ScanStage.entries.first { it.wire == getString("stage") },
    done = getInt("done"),
    total = optIntOrNull("total"),
)

private fun JSONObject.optObject(name: String): JSONObject? = if (isNull(name)) null else getJSONObject(name)

private fun JSONObject.optStringOrNull(name: String): String? = if (isNull(name)) null else getString(name)

private fun JSONObject.optIntOrNull(name: String): Int? = if (isNull(name)) null else getInt(name)

private fun JSONObject.optLongOrNull(name: String): Long? = if (isNull(name)) null else getLong(name)

private fun JSONArray.toStrings(): List<String> = List(this.length()) { this.getString(it) }

private fun JSONArray.toLongs(): List<Long> = List(this.length()) { this.getLong(it) }

private fun <T> JSONArray.mapObjects(read: (JSONObject) -> T): List<T> = List(this.length()) { read(this.getJSONObject(it)) }

/** The core responded with a `Response::Error`, or with a response that could not be decoded. */
class CoreException(message: String) : Exception(message)

data class Version(val coreVersion: String, val arch: String, val profile: String)

/** A piece of I/O the core wants done on the music. Paths are names within the picked folder, `/` being the folder itself. */
sealed class Op {
    abstract val id: Long

    data class ReadDir(override val id: Long, val path: String) : Op()

    data class Open(override val id: Long, val path: String) : Op()
}

/** How an [Op] went. */
sealed class Answer {
    data class Listing(val entries: List<Entry>) : Answer()

    /** A file descriptor open for reading, which the core now owns and closes. */
    data class Opened(val fd: Int) : Answer()

    data class Failed(val message: String) : Answer()
}

/** One entry of a directory listing. `mtime` is in milliseconds since the epoch. */
data class Entry(val name: String, val dir: Boolean, val mtime: Long, val size: Long)

/** What changed. `covers` are the albums whose cover can now be had. `playback` is null if it did not change. */
data class Notices(
    val library: Boolean,
    val queue: Boolean,
    val scanning: Boolean,
    val scanProgress: ScanProgress?,
    val covers: List<Long>,
    val playback: Playback?,
)

/** How far a scan has come: `done` of `total` in its `stage`. `total` is null when the scan cannot tell, and while
 * listing only a guess, which `done` may pass. */
data class ScanProgress(val stage: ScanStage, val done: Int, val total: Int?)

enum class ScanStage(val wire: String) {
    /** Listing the folders. */
    LISTING("listing"),

    /** Reading the tags in the folders holding music. */
    READING("reading"),
}

/** `accent` is the cover's most distinct colour as 0xRRGGBB. */
data class AlbumSummary(
    val id: Long,
    val title: String,
    val artist: String,
    val genre: String,
    val year: Int?,
    val cover: Long?,
    val accent: Int?,
    val tracks: Int,
)

data class AlbumDetail(val summary: AlbumSummary, val trackTitles: List<String>)

data class AlbumHeader(val id: Long, val title: String, val artist: String, val cover: Long?, val accent: Int?)

data class Sorts(val labels: List<String>, val current: Int)

/** `album` is zero while a scan has not found the track's album. */
data class QueueTrack(val title: String, val album: Long)

/** Queue indices `start` until `end` are tracks of one album: one cover in the cover flow. */
data class Run(val start: Int, val end: Int, val album: AlbumHeader?)

/** The queue's tracks, and its runs of tracks from one album. What plays in it is [Playback.current]. */
data class Queue(val tracks: List<QueueTrack>, val runs: List<Run>) {
    /** The run holding queue index `track`, or -1 if none does. */
    fun runOf(track: Int?): Int = runs.indexOfFirst { track != null && track in it.start until it.end }

    companion object {
        val EMPTY = Queue(emptyList(), emptyList())
    }
}

/** What is playing. `current` is the queue index, null while the queue is empty.
 * `coverFile` is the cover as a JPEG, if one has been made. */
data class Playback(
    val current: Int?,
    val playing: Boolean,
    val positionMs: Long,
    val lengthMs: Long?,
    val repeat: Repeat,
    val title: String,
    val album: AlbumHeader?,
    val coverFile: File?,
) {
    companion object {
        val NONE = Playback(null, false, 0, null, Repeat.OFF, "", null, null)
    }
}

/** What happens when a track ends on its own. */
enum class Repeat(val wire: String) {
    OFF("off"),
    TRACK("track"),
    ALBUM("album"),
    PLAYLIST("playlist"),
    ;

    /** The next mode in the cycle the repeat button steps through. */
    fun cycled(): Repeat = entries[(ordinal + 1) % entries.size]
}
