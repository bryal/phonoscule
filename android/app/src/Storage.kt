package zone.jo.phonoscule

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.DocumentsContract
import android.util.Log
import java.io.FileNotFoundException
import java.util.concurrent.ConcurrentHashMap
import kotlin.concurrent.thread

/** Read access to the music folder through the Storage Access Framework.
 *
 * The core names files by path within the folder: `/` is the folder itself, and `/Artist/Album/01.opus` a file two
 * directories down. SAF names them by document ID, which only the provider knows how to make, so the IDs are learned
 * from listings and remembered. A path the core asks for before any listing showed it, as when a restored queue starts
 * playing before the first scan, is found by listing its way down from the folder.
 *
 * Get one with [adopt] when the user has just picked a folder, and with [restore] to get it back later. */
class Storage private constructor(private val resolver: ContentResolver, val tree: Uri) {
    private val ids = ConcurrentHashMap<String, Listed>().apply {
        this[ROOT] = Listed(DocumentsContract.getTreeDocumentId(tree), Entry("", true, 0, 0))
    }

    /** Every entry of the directory at `path`.
     *
     * Throws [FileNotFoundException] if there is no such directory, and SecurityException if the grant has been revoked. */
    fun list(path: String): List<Entry> {
        val dir = this.find(path)
        if (!dir.entry.dir) throw FileNotFoundException("$path is not a directory")
        return this.children(path, dir.docId)
    }

    /** The file at `path`, opened for reading. The caller owns the returned file descriptor.
     *
     * Throws [FileNotFoundException] if there is no such file, and SecurityException if the grant has been revoked. */
    fun open(path: String): Int {
        val uri = DocumentsContract.buildDocumentUriUsingTree(tree, this.find(path).docId)
        val fd = resolver.openFileDescriptor(uri, "r") ?: throw FileNotFoundException("could not open $path")
        return fd.detachFd()
    }

    /** The document at `path`, listing its parent to learn of it if it is not known yet. */
    private fun find(path: String): Listed {
        ids[path]?.let { return it }
        val parent = path.substringBeforeLast('/').ifEmpty { ROOT }
        if (parent == path) throw FileNotFoundException("no such document: $path")
        this.list(parent)
        return ids[path] ?: throw FileNotFoundException("no such document: $path")
    }

    // One query for the whole directory. DocumentFile.listFiles() would query once per child,
    // which takes tens of seconds over a large library.
    private fun children(path: String, docId: String): List<Entry> {
        val uri = DocumentsContract.buildChildDocumentsUriUsingTree(tree, docId)
        val columns = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
            DocumentsContract.Document.COLUMN_LAST_MODIFIED,
            DocumentsContract.Document.COLUMN_SIZE,
        )
        val cursor = resolver.query(uri, columns, null, null, null) ?: throw FileNotFoundException("could not list $path")
        return cursor.use { rows ->
            // A provider that fetches over the network answers with what it has so far, and says it is still loading.
            // Scanning a partial listing would look exactly like the missing music having been deleted.
            if (rows.extras?.getBoolean(DocumentsContract.EXTRA_LOADING) == true) {
                throw FileNotFoundException("$path is still loading, so try again in a moment")
            }
            val id = rows.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DOCUMENT_ID)
            val name = rows.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_DISPLAY_NAME)
            val mime = rows.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_MIME_TYPE)
            val modified = rows.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_LAST_MODIFIED)
            val size = rows.getColumnIndexOrThrow(DocumentsContract.Document.COLUMN_SIZE)
            val entries = mutableListOf<Entry>()
            while (rows.moveToNext()) {
                val entry = Entry(
                    name = rows.getString(name),
                    dir = rows.getString(mime) == DocumentsContract.Document.MIME_TYPE_DIR,
                    mtime = if (rows.isNull(modified)) 0 else rows.getLong(modified),
                    size = if (rows.isNull(size)) 0 else rows.getLong(size),
                )
                entries.add(entry)
                ids[join(path, entry.name)] = Listed(rows.getString(id), entry)
            }
            entries
        }
    }

    /** A document a listing has shown. */
    private data class Listed(val docId: String, val entry: Entry)

    companion object {
        private const val ROOT = "/"
        private const val PREFS = "phonoscule"
        private const val KEY_FOLDER = "music_folder"

        private fun join(dir: String, name: String) = if (dir == ROOT) "/$name" else "$dir/$name"

        /** Adopt the folder the user just picked, in place of any adopted before.
         *
         * The folder is remembered for later launches, see [restore].
         * Throws SecurityException if the provider refuses a lasting grant,
         * in which case the folder adopted before stays adopted. Call it off the main thread. */
        fun adopt(context: Context, tree: Uri): Storage {
            val resolver = context.contentResolver
            resolver.takePersistableUriPermission(tree, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            prefs(context).edit().putString(KEY_FOLDER, tree.toString()).apply()
            for (old in resolver.persistedUriPermissions) {
                if (old.uri != tree) resolver.releasePersistableUriPermission(old.uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            return Storage(resolver, tree)
        }

        /** The folder adopted last.
         *
         * Returns null if none was adopted, or if its grant has since been revoked. */
        fun restore(context: Context): Storage? {
            val saved = prefs(context).getString(KEY_FOLDER, null) ?: return null
            val uri = Uri.parse(saved)
            val resolver = context.contentResolver
            val granted = resolver.persistedUriPermissions.any { it.uri == uri && it.isReadPermission }
            return if (granted) Storage(resolver, uri) else null
        }

        private fun prefs(context: Context) = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    }
}

/** The threads that do the core's I/O on the music: each takes an [Op] from the core, performs it on [storage], and
 * answers. They live as long as the process, as the core does. */
class Workers(private val storage: () -> Storage?) {
    fun start() {
        repeat(COUNT) { n ->
            thread(name = "phonoscule-io-$n", isDaemon = true) {
                while (true) this.serve(Core.nextOp())
            }
        }
    }

    private fun serve(op: Op) {
        val answer = try {
            val storage = storage() ?: throw FileNotFoundException("no music folder has been picked")
            when (op) {
                is Op.ReadDir -> Answer.Listing(storage.list(op.path))
                is Op.Open -> Answer.Opened(storage.open(op.path))
            }
        } catch (e: Exception) {
            Log.w(TAG, "could not ${op.javaClass.simpleName} ${op.pathOf()}: $e")
            Answer.Failed(e.message ?: e.toString())
        }
        Core.answer(op.id, answer)
    }

    private fun Op.pathOf() = when (this) {
        is Op.ReadDir -> path
        is Op.Open -> path
    }

    companion object {
        /** As many as the scan reads directories at once, so that it never waits on a free worker. Provider calls are
         * binder round trips, mostly waiting rather than working. */
        private val COUNT = Runtime.getRuntime().availableProcessors().coerceAtLeast(4)
        private const val TAG = "phonoscule"
    }
}
