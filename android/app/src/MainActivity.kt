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
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.media3.session.MediaController
import androidx.media3.session.SessionToken
import com.google.common.util.concurrent.ListenableFuture

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

/** The library, with an album or the player opened over it. Back closes whatever is on top. */
@Composable
private fun Screens(onChooseFolder: () -> Unit) {
    var album by rememberSaveable { mutableStateOf<Long?>(null) }
    var player by rememberSaveable { mutableStateOf(false) }
    BackHandler(enabled = player || album != null) {
        if (player) player = false else album = null
    }
    val openAlbum = album
    when {
        player -> PlayerScreen(onBack = { player = false })
        openAlbum != null -> AlbumScreen(openAlbum, onBack = { album = null }, onPlayed = { player = true })
        else -> LibraryScreen(
            onOpenAlbum = { album = it },
            onOpenPlayer = { player = true },
            onChooseFolder = onChooseFolder,
        )
    }
}
