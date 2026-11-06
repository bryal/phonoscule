package zone.jo.phonoscule

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LocalMinimumInteractiveComponentSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** Every artist, two to a row, to narrow the library to the albums of the `picked` ones, as its genre chips do: each
 * artist's chip toggles them, and All clears them, with none picked showing every album. */
@Composable
fun ArtistsScreen(picked: List<String>, onPick: (List<String>) -> Unit) {
    var artists by remember { mutableStateOf(emptyList<String>()) }
    LaunchedEffect(App.libraryVersion) { artists = withContext(Dispatchers.IO) { Core.artists() } }

    Column(modifier = Modifier.fillMaxSize()) {
        Text(
            text = if (picked.isEmpty()) "Artists" else "Artists (${picked.size})",
            style = MaterialTheme.typography.titleLarge,
            modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 12.dp),
        )
        HorizontalDivider()
        val grid = rememberLazyGridState()
        // As many chips as there are artists: rows padded out to Material's 48dp touch target would be twice as tall.
        CompositionLocalProvider(LocalMinimumInteractiveComponentSize provides Dp.Unspecified) {
            LazyVerticalGrid(
                columns = GridCells.Fixed(2),
                modifier = Modifier.scrollBar(grid),
                state = grid,
                contentPadding = PaddingValues(12.dp),
                horizontalArrangement = Arrangement.spacedBy(CHIP_GAP),
                verticalArrangement = Arrangement.spacedBy(CHIP_GAP),
            ) {
                item {
                    FilterToggle(
                        "All",
                        selected = picked.isEmpty(),
                        onClick = { onPick(emptyList()) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                items(artists, key = { it }) { artist ->
                    val selected = artist in picked
                    FilterToggle(
                        artist,
                        selected,
                        onClick = { onPick(if (selected) picked - artist else picked + artist) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        }
    }
}
