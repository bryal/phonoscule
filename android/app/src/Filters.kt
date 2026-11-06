package zone.jo.phonoscule

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
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
import androidx.compose.ui.graphics.vector.ImageVector
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
                CompactChip(
                    "All",
                    filled = picked.isEmpty(),
                    onClick = {
                        onPick(emptyList())
                        onBack()
                    },
                    onLongClick = { onPick(emptyList()) },
                    role = Role.Checkbox,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            items(options, key = { it }) { option ->
                val selected = option in picked
                CompactChip(
                    option,
                    filled = selected,
                    onClick = {
                        if (selected) {
                            onPick(picked - option)
                        } else {
                            onPick(listOf(option))
                            onBack()
                        }
                    },
                    onLongClick = { onPick(if (selected) picked - option else picked + option) },
                    role = Role.Checkbox,
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}

/** A chip as Material draws one, filled or outlined, but compact: 28dp tall, padded 8dp, and its text a size smaller.
 * Material's own pad their contents twice as much, which clips a label in a narrow chip, and have no long press.
 *
 * The leading icon is tinted as the chip's accent; the trailing one, as its text, says `trailingDescription`. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun CompactChip(
    label: String,
    filled: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    onLongClick: (() -> Unit)? = null,
    role: Role = Role.Button,
    leadingIcon: ImageVector? = null,
    trailingIcon: ImageVector? = null,
    trailingDescription: String? = null,
) {
    val colors = MaterialTheme.colorScheme
    Surface(
        shape = FilterChipDefaults.shape,
        color = if (filled) colors.secondaryContainer else Color.Transparent,
        contentColor = if (filled) colors.onSecondaryContainer else colors.onSurfaceVariant,
        border = if (filled) null else BorderStroke(1.dp, colors.outlineVariant),
        modifier = modifier.height(CHIP_HEIGHT),
    ) {
        Row(
            modifier = Modifier
                .fillMaxSize()
                .combinedClickable(role = role, onClick = onClick, onLongClick = onLongClick)
                .padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            if (leadingIcon != null) {
                Icon(leadingIcon, contentDescription = null, tint = colors.primary, modifier = Modifier.size(CHIP_ICON))
            }
            Text(
                label,
                style = MaterialTheme.typography.labelMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                // Not a weight: in a row that scrolls, a chip has no width of its own to share out.
                modifier = Modifier.widthIn(max = CHIP_TEXT_MAX),
            )
            if (trailingIcon != null) {
                Icon(trailingIcon, contentDescription = trailingDescription, modifier = Modifier.size(CHIP_ICON))
            }
        }
    }
}

private val CHIP_HEIGHT = 28.dp
private val CHIP_ICON = 16.dp
private val CHIP_TEXT_MAX = 220.dp
