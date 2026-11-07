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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.ListenableFuture
import java.io.Serializable

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
            MaterialTheme(
                colorScheme = darkColorScheme(background = Color.Black, surface = Color.Black),
                typography = IOSEVKA_TYPOGRAPHY,
            ) {
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

/** The library, with the player, an album or one of its filters opened over it. Back closes whatever is on top.
 *
 * Opens on the player when there is something to play. Playing an album closes it, so Back from the player returns to
 * the library. */
@Composable
private fun Screens(onChooseFolder: () -> Unit) {
    var overlay by rememberSaveable {
        mutableStateOf<Overlay?>(if (App.storage != null && App.playback.current != null) Overlay.Player else null)
    }
    var picks by rememberSaveable { mutableStateOf(Picks()) }
    val covered = overlay != null
    BackHandler(enabled = covered) { overlay = null }
    // A focused search field would otherwise keep the keyboard up over the screen on top.
    val focus = LocalFocusManager.current
    LaunchedEffect(covered) { if (covered) focus.clearFocus() }
    Box {
        // Kept under whatever is open, hidden, so that it comes back as it was left: scrolled, searched and filtered.
        val hidden = Modifier.graphicsLayer { alpha = 0f }.clearAndSetSemantics {}
        Box(modifier = if (covered) hidden else Modifier) {
            LibraryScreen(
                covered = covered,
                picks = picks,
                onPick = { picks = it },
                onOpenFilter = { overlay = Overlay.Picker(it) },
                onOpenAlbum = { overlay = Overlay.Album(it) },
                onOpenPlayer = { overlay = Overlay.Player },
                onChooseFolder = onChooseFolder,
            )
        }
        val open = overlay
        if (open != null) {
            // Takes every touch, so none reaches the library beneath.
            val takeTouches = Modifier.pointerInput(Unit) {
                awaitPointerEventScope { while (true) awaitPointerEvent() }
            }
            Box(modifier = Modifier.fillMaxSize().then(takeTouches)) {
                when (open) {
                    Overlay.Player -> PlayerScreen()
                    is Overlay.Album -> AlbumScreen(
                        open.id,
                        onBack = { overlay = null },
                        onPlayed = { overlay = Overlay.Player },
                    )
                    is Overlay.Picker -> FilterScreen(
                        open.filter,
                        picked = picks[open.filter],
                        onPick = { picks = picks.with(open.filter, it) },
                        onBack = { overlay = null },
                    )
                }
            }
        }
    }
}

/** What is open over the library. */
private sealed interface Overlay : Serializable {
    data object Player : Overlay

    data class Album(val id: Long) : Overlay

    /** A filter's picker. */
    data class Picker(val filter: Filter) : Overlay
}

/** The desktop player's font, Iosevka, in every style, its regular weight standing in for the heavier ones as on the
 * desktop. What it lacks, such as Japanese, falls back to the system's font. */
private val IOSEVKA_TYPOGRAPHY = Typography().let { t ->
    val iosevka = FontFamily(Font(R.font.iosevka))
    fun TextStyle.inIosevka() = this.copy(fontFamily = iosevka)
    t.copy(
        displayLarge = t.displayLarge.inIosevka(),
        displayMedium = t.displayMedium.inIosevka(),
        displaySmall = t.displaySmall.inIosevka(),
        headlineLarge = t.headlineLarge.inIosevka(),
        headlineMedium = t.headlineMedium.inIosevka(),
        headlineSmall = t.headlineSmall.inIosevka(),
        titleLarge = t.titleLarge.inIosevka(),
        titleMedium = t.titleMedium.inIosevka(),
        titleSmall = t.titleSmall.inIosevka(),
        bodyLarge = t.bodyLarge.inIosevka(),
        bodyMedium = t.bodyMedium.inIosevka(),
        bodySmall = t.bodySmall.inIosevka(),
        labelLarge = t.labelLarge.inIosevka(),
        labelMedium = t.labelMedium.inIosevka(),
        labelSmall = t.labelSmall.inIosevka(),
    )
}
