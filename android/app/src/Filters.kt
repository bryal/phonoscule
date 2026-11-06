package zone.jo.phonoscule

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
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
 * toggles it, and All clears them, with none picked showing every album.
 *
 * A tap on a value not picked picks it alone and goes back; a long press adds it to those picked and stays. A tap or a
 * long press on a picked value unpicks it. All, tapped, clears them and goes back; long pressed, it stays. */
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
        LazyVerticalGrid(
            columns = GridCells.Fixed(2),
            modifier = Modifier.scrollBar(grid),
            state = grid,
            contentPadding = PaddingValues(12.dp),
            horizontalArrangement = Arrangement.spacedBy(CHIP_GAP),
            verticalArrangement = Arrangement.spacedBy(CHIP_GAP),
        ) {
            item {
                PickerChip(
                    "All",
                    selected = picked.isEmpty(),
                    onClick = {
                        onPick(emptyList())
                        onBack()
                    },
                    onLongClick = { onPick(emptyList()) },
                )
            }
            items(options, key = { it }) { option ->
                val selected = option in picked
                PickerChip(
                    option,
                    selected,
                    onClick = {
                        if (selected) {
                            onPick(picked - option)
                        } else {
                            onPick(listOf(option))
                            onBack()
                        }
                    },
                    onLongClick = { onPick(if (selected) picked - option else picked + option) },
                )
            }
        }
    }
}

/** A value to pick, drawn as Material's filter chip, which has no long press: filled while picked, outlined while
 * not. It fills its width, and is compact, as a page of them is many. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun PickerChip(label: String, selected: Boolean, onClick: () -> Unit, onLongClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Surface(
        shape = FilterChipDefaults.shape,
        color = if (selected) colors.secondaryContainer else Color.Transparent,
        contentColor = if (selected) colors.onSecondaryContainer else colors.onSurfaceVariant,
        border = if (selected) null else BorderStroke(1.dp, colors.outlineVariant),
        modifier = Modifier.fillMaxWidth().height(CHIP_HEIGHT),
    ) {
        Box(
            contentAlignment = Alignment.CenterStart,
            modifier = Modifier
                .fillMaxSize()
                .combinedClickable(role = Role.Checkbox, onClick = onClick, onLongClick = onLongClick)
                .padding(horizontal = 12.dp),
        ) {
            Text(label, style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

private val CHIP_HEIGHT = 28.dp
