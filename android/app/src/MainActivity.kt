package zone.jo.phonoscule

import android.content.ComponentName
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.ListenableFuture
import kotlinx.coroutines.launch

class MainActivity : ComponentActivity() {
    private val chooseFolder = this.registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree ->
        if (tree != null) App.adopt(tree)
    }

    /** Bound to the playback service while the activity shows, which keeps it running to be promoted to the
     * foreground once something plays. */
    private var controller: ListenableFuture<MediaController>? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        this.enableEdgeToEdge()
        this.setContent {
            MaterialTheme(colorScheme = darkColorScheme(background = Color.Black, surface = Color.Black)) {
                Surface(modifier = Modifier.fillMaxSize()) {
                    Box(modifier = Modifier.safeDrawingPadding()) {
                        Screens(onChooseFolder = { chooseFolder.launch(null) })
                    }
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        val token = SessionToken(this, ComponentName(this, PlaybackService::class.java))
        controller = MediaController.Builder(this, token).buildAsync()
    }

    override fun onStop() {
        controller?.let(MediaController::releaseFuture)
        controller = null
        super.onStop()
    }

    override fun onResume() {
        super.onResume()
        App.resume()
    }

    override fun onPause() {
        App.pause()
        super.onPause()
    }
}

/** Three pages side by side, the artists, the library and the player, with an album opened over them. Swiping moves
 * between the pages, and Back from either side returns to the library. Back closes an open album.
 *
 * Opens on the player when there is something to play. Playing an album closes it, so Back from the player returns to
 * the library. */
@Composable
private fun Screens(onChooseFolder: () -> Unit) {
    var album by rememberSaveable { mutableStateOf<Long?>(null) }
    var artists by rememberSaveable { mutableStateOf(emptyList<String>()) }
    val first = if (App.storage != null && App.playback.current != null) PLAYER else LIBRARY
    val pages = rememberPagerState(initialPage = first) { PAGES }
    val scope = rememberCoroutineScope()
    val show = { page: Int -> scope.launch { pages.animateScrollToPage(page) } }
    val openAlbum = album
    BackHandler(enabled = openAlbum != null || pages.currentPage != LIBRARY) {
        if (openAlbum != null) album = null else show(LIBRARY)
    }
    val onLibrary = openAlbum == null && pages.currentPage == LIBRARY
    // A focused search field would otherwise keep the keyboard up over another page.
    val focus = LocalFocusManager.current
    LaunchedEffect(onLibrary) { if (!onLibrary) focus.clearFocus() }
    Box {
        // Kept under an open album, hidden, so that the pages come back as they were left: scrolled, searched and
        // filtered. Every page is kept while another shows, for the same reason.
        val hidden = Modifier.graphicsLayer { alpha = 0f }.clearAndSetSemantics {}
        Box(modifier = if (openAlbum != null) hidden else Modifier) {
            HorizontalPager(
                state = pages,
                beyondViewportPageCount = PAGES - 1,
                userScrollEnabled = App.storage != null,
            ) { page ->
                when (page) {
                    ARTISTS -> ArtistsScreen(artists, onPick = { artists = it })
                    LIBRARY -> LibraryScreen(
                        covered = !onLibrary,
                        artists = artists,
                        onClearArtists = { artists = emptyList() },
                        onOpenAlbum = { album = it },
                        onOpenPlayer = { show(PLAYER) },
                        onChooseFolder = onChooseFolder,
                    )
                    else -> PlayerScreen(onBack = { show(LIBRARY) })
                }
            }
        }
        if (openAlbum != null) {
            // Takes every touch, so none reaches the pages beneath.
            val takeTouches = Modifier.pointerInput(Unit) {
                awaitPointerEventScope { while (true) awaitPointerEvent() }
            }
            Box(modifier = Modifier.fillMaxSize().then(takeTouches)) {
                AlbumScreen(openAlbum, onBack = { album = null }, onPlayed = {
                    album = null
                    scope.launch { pages.scrollToPage(PLAYER) }
                })
            }
        }
    }
}

private const val ARTISTS = 0
private const val LIBRARY = 1
private const val PLAYER = 2
private const val PAGES = 3
