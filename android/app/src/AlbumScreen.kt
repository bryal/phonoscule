package zone.jo.phonoscule

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.PlaylistAdd
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** One album: its cover and tags, and its tracks to play from or enqueue one at a time. */
@Composable
fun AlbumScreen(id: Long, onBack: () -> Unit, onPlayed: () -> Unit) {
    var album by remember { mutableStateOf<AlbumDetail?>(null) }
    var loaded by remember { mutableStateOf(false) }
    LaunchedEffect(id, App.libraryVersion) {
        album = withContext(Dispatchers.IO) { Core.album(id) }
        loaded = true
    }
    val detail = album
    if (detail == null) {
        // A rescan can take an album away while it is open.
        if (loaded) {
            Column(modifier = Modifier.padding(8.dp)) {
                IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
                Text("This album is no longer in the library.", modifier = Modifier.padding(16.dp))
            }
        }
        return
    }
    val summary = detail.summary
    val accent = accentColor(summary.accent)

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(Brush.verticalGradient(listOf(accent.copy(alpha = 0.35f), Color.Black), endY = 1600f)),
    ) {
        LazyColumn(modifier = Modifier.fillMaxSize()) {
            item {
                Column(modifier = Modifier.fillMaxWidth().padding(horizontal = 24.dp, vertical = 8.dp)) {
                    IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back") }
                    CoverImage(
                        summary.cover,
                        summary.accent,
                        edge = 360.dp,
                        modifier = Modifier.fillMaxWidth(0.8f).align(Alignment.CenterHorizontally),
                    )
                    Text(summary.title, style = MaterialTheme.typography.headlineSmall, modifier = Modifier.padding(top = 16.dp))
                    Text(summary.artist, style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(
                        text = listOfNotNull(summary.year?.toString(), summary.genre.ifEmpty { null }).joinToString("  -  "),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Row(
                        modifier = Modifier.padding(vertical = 16.dp),
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        Button(onClick = {
                            App.command { Core.play(listOf(id)) }
                            onPlayed()
                        }) {
                            Icon(Icons.Default.PlayArrow, contentDescription = null)
                            Text("Play", modifier = Modifier.padding(start = 4.dp))
                        }
                        OutlinedButton(onClick = { App.command { Core.enqueue(listOf(id)) } }) {
                            Icon(Icons.AutoMirrored.Filled.PlaylistAdd, contentDescription = null)
                            Text("Enqueue", modifier = Modifier.padding(start = 4.dp))
                        }
                    }
                }
            }
            itemsIndexed(detail.trackTitles) { ix, title ->
                Row(
                    modifier = Modifier
                        .fillMaxWidth()
                        .clickable {
                            App.command { Core.play(listOf(id), track = ix) }
                            onPlayed()
                        }
                        .padding(start = 24.dp, end = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        "${ix + 1}",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.width(32.dp),
                    )
                    Text(
                        title,
                        style = MaterialTheme.typography.bodyLarge,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f).padding(vertical = 14.dp),
                    )
                    IconButton(onClick = { App.command { Core.enqueue(listOf(id), track = ix) } }) {
                        Icon(Icons.AutoMirrored.Filled.PlaylistAdd, contentDescription = "Enqueue this track")
                    }
                }
            }
        }
    }
}
