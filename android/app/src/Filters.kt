package zone.jo.phonoscule

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/** What the library can be narrowed by, besides its search. */
enum class Filter(val title: String) {
    GENRES("Genres"),
    ARTISTS("Artists"),
    ;

    /** Every value some album has, sorted. Blocks on the core. */
    fun options(): List<String> = when (this) {
        GENRES -> Core.genres()
        ARTISTS -> Core.artists()
    }
}

/** Every value of `filter`, two to a row, to narrow the library to the albums of the `picked` ones: each value's chip
 * toggles it, and All clears them, with none picked showing every album. */
@Composable
fun FilterScreen(filter: Filter, picked: List<String>, onPick: (List<String>) -> Unit, onBack: () -> Unit) {
    var options by remember { mutableStateOf(emptyList<String>()) }
    LaunchedEffect(filter, App.libraryVersion) { options = withContext(Dispatchers.IO) { filter.options() } }

    Column(modifier = Modifier.fillMaxSize()) {
        Row(modifier = Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Library") }
            Text(
                text = if (picked.isEmpty()) filter.title else "${filter.title} (${picked.size})",
                style = MaterialTheme.typography.titleMedium,
            )
        }
        HorizontalDivider()
        val grid = rememberLazyGridState()
        // As many chips as there are values: rows padded out to Material's 48dp touch target would be twice as tall.
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
                items(options, key = { it }) { option ->
                    val selected = option in picked
                    FilterToggle(
                        option,
                        selected,
                        onClick = { onPick(if (selected) picked - option else picked + option) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        }
    }
}
