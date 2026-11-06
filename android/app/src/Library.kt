package zone.jo.phonoscule

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.staggeredgrid.LazyHorizontalStaggeredGrid
import androidx.compose.foundation.lazy.staggeredgrid.StaggeredGridCells
import androidx.compose.foundation.lazy.staggeredgrid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalMinimumInteractiveComponentSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/** The library: a wall of covers to search, filter by genre and sort, with what is playing along the bottom.
 *
 * Back clears the search and the genres, and leaves the search field, before it leaves the app. Pass `covered` while
 * another screen is open over this one, which then has Back to itself. */
@Composable
fun LibraryScreen(covered: Boolean, onOpenAlbum: (Long) -> Unit, onOpenPlayer: () -> Unit, onChooseFolder: () -> Unit) {
    if (App.storage == null) {
        ChooseFolder(onChooseFolder)
        return
    }

    var query by rememberSaveable { mutableStateOf("") }
    var picked by rememberSaveable { mutableStateOf(emptyList<String>()) }
    var albums by remember { mutableStateOf(emptyList<AlbumSummary>()) }
    var genres by remember { mutableStateOf(emptyList<String>()) }
    var searching by remember { mutableStateOf(false) }

    val focus = LocalFocusManager.current
    BackHandler(enabled = !covered && (query.isNotEmpty() || picked.isNotEmpty() || searching)) {
        query = ""
        picked = emptyList()
        focus.clearFocus()
    }

    LaunchedEffect(query, picked, App.libraryVersion) {
        val (found, all) = withContext(Dispatchers.IO) { Core.albums(query, picked) to Core.genres() }
        albums = found
        genres = all
    }

    Column(modifier = Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 8.dp, bottom = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            OutlinedTextField(
                value = query,
                onValueChange = { query = it },
                modifier = Modifier.weight(1f).onFocusChanged { searching = it.isFocused },
                singleLine = true,
                placeholder = { Text("Search albums") },
                leadingIcon = { Icon(Icons.Default.Search, contentDescription = null) },
                trailingIcon = {
                    if (query.isNotEmpty()) {
                        IconButton(onClick = { query = "" }) { Icon(Icons.Default.Clear, contentDescription = "Clear") }
                    }
                },
            )
            LibraryMenu(albums, onChooseFolder)
        }

        GenreChips(genres, picked, onPick = { picked = it })

        HorizontalDivider()

        // Spinning from the pull until the core says it is scanning, so the indicator does not retract in between.
        var pulled by remember { mutableStateOf(false) }
        LaunchedEffect(App.scanning) { pulled = false }
        LaunchedEffect(pulled) {
            if (pulled) {
                delay(PULL_GRACE_MS)
                pulled = false
            }
        }
        // Asked before the scan starts, as its notification is hidden for good if posted without.
        val context = LocalContext.current
        val askToNotify = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { App.scan() }
        PullToRefreshBox(
            isRefreshing = pulled || App.scanning,
            onRefresh = {
                pulled = true
                if (Build.VERSION.SDK_INT >= 33 &&
                    context.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
                ) {
                    askToNotify.launch(Manifest.permission.POST_NOTIFICATIONS)
                } else {
                    App.scan()
                }
            },
            modifier = Modifier.weight(1f),
        ) {
            if (albums.isEmpty()) {
                // Scrollable, or there would be nothing to pull.
                Column(modifier = Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
                    if (!App.scanning) {
                        Text(
                            text = if (query.isEmpty() && picked.isEmpty()) {
                                "No albums here yet. Pull down to scan."
                            } else {
                                "Nothing matches."
                            },
                            modifier = Modifier.padding(24.dp),
                        )
                    }
                }
            } else {
                val grid = rememberLazyGridState()
                LazyVerticalGrid(
                    columns = GridCells.Adaptive(140.dp),
                    modifier = Modifier.scrollBar(grid),
                    state = grid,
                    contentPadding = PaddingValues(12.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    items(albums, key = { it.id }) { album -> AlbumCard(album, onClick = { onOpenAlbum(album.id) }) }
                }
            }
        }

        NowPlayingBar(onOpenPlayer)
    }
}

/** A thin bar down the right edge of the grid, always shown, for where the view is in the whole of it and how much of it
 * shows. Rows are taken to be of equal height, as the album cards are. */
private fun Modifier.scrollBar(state: LazyGridState): Modifier = this.drawWithContent {
    drawContent()
    val info = state.layoutInfo
    val items = info.visibleItemsInfo
    if (items.isEmpty()) return@drawWithContent
    val columns = items.maxOf { it.column } + 1
    val rows = (info.totalItemsCount + columns - 1) / columns
    val stride = items.first().size.height + info.mainAxisItemSpacing
    val content = rows * stride - info.mainAxisItemSpacing + info.beforeContentPadding + info.afterContentPadding
    val viewport = info.viewportSize.height
    if (content <= viewport) return@drawWithContent
    val scrolled = state.firstVisibleItemIndex / columns * stride + state.firstVisibleItemScrollOffset
    val fraction = (scrolled.toFloat() / (content - viewport)).coerceIn(0f, 1f)

    val margin = SCROLL_BAR_MARGIN.toPx()
    val width = SCROLL_BAR_WIDTH.toPx()
    val track = size.height - 2 * margin
    val thumb = (track * viewport / content).coerceAtLeast(SCROLL_BAR_MIN.toPx()).coerceAtMost(track)
    drawRoundRect(
        color = Color.White.copy(alpha = SCROLL_BAR_ALPHA),
        topLeft = Offset(size.width - margin - width, margin + (track - thumb) * fraction),
        size = Size(width, thumb),
        cornerRadius = CornerRadius(width / 2),
    )
}

@Composable
private fun LibraryMenu(albums: List<AlbumSummary>, onChooseFolder: () -> Unit) {
    var open by remember { mutableStateOf(false) }
    var sorting by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }) { Icon(Icons.Default.MoreVert, contentDescription = "More") }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            DropdownMenuItem(text = { Text("Play all shown") }, onClick = {
                open = false
                App.command { Core.play(albums.map { it.id }) }
            })
            DropdownMenuItem(text = { Text("Enqueue all shown") }, onClick = {
                open = false
                App.command { Core.enqueue(albums.map { it.id }) }
            })
            DropdownMenuItem(text = { Text("Order...") }, onClick = {
                open = false
                sorting = true
            })
            DropdownMenuItem(text = { Text("Change folder") }, onClick = {
                open = false
                onChooseFolder()
            })
        }
        SortMenu(expanded = sorting, onDismiss = { sorting = false })
    }
}

@Composable
private fun SortMenu(expanded: Boolean, onDismiss: () -> Unit) {
    var sorts by remember { mutableStateOf<Sorts?>(null) }
    LaunchedEffect(expanded) {
        if (expanded) sorts = withContext(Dispatchers.IO) { Core.sorts() }
    }
    DropdownMenu(expanded = expanded && sorts != null, onDismissRequest = onDismiss) {
        sorts?.labels?.forEachIndexed { ix, label ->
            DropdownMenuItem(
                text = { Text(if (ix == sorts?.current) "$label  *" else label) },
                onClick = {
                    onDismiss()
                    App.command { Core.setSort(ix) }
                },
            )
        }
    }
}

/** Chips that limit the wall to the albums of the `picked` genres, in two rows to scroll across. Each genre's chip
 * toggles it; All clears them, and with none picked every album shows. Shows nothing while no album has a genre. */
@Composable
private fun GenreChips(genres: List<String>, picked: List<String>, onPick: (List<String>) -> Unit) {
    if (genres.isEmpty()) return
    // Material pads every chip out to a 48dp touch target, which would make two rows of them a third of a phone.
    CompositionLocalProvider(LocalMinimumInteractiveComponentSize provides Dp.Unspecified) {
        LazyHorizontalStaggeredGrid(
            rows = StaggeredGridCells.Fixed(2),
            modifier = Modifier.height(CHIP_HEIGHT * 2 + CHIP_GAP * 3),
            contentPadding = PaddingValues(horizontal = 12.dp, vertical = CHIP_GAP),
            verticalArrangement = Arrangement.spacedBy(CHIP_GAP),
            horizontalItemSpacing = CHIP_GAP,
        ) {
            item { GenreChip("All", selected = picked.isEmpty(), onClick = { onPick(emptyList()) }) }
            items(genres, key = { it }) { genre ->
                val selected = genre in picked
                GenreChip(genre, selected, onClick = { onPick(if (selected) picked - genre else picked + genre) })
            }
        }
    }
}

@Composable
private fun GenreChip(label: String, selected: Boolean, onClick: () -> Unit) {
    FilterChip(
        selected = selected,
        onClick = onClick,
        label = { Text(label, style = MaterialTheme.typography.labelMedium) },
        modifier = Modifier.height(CHIP_HEIGHT),
    )
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun AlbumCard(album: AlbumSummary, onClick: () -> Unit) {
    Column(
        modifier = Modifier.combinedClickable(
            onClick = onClick,
            onLongClick = { App.command { Core.enqueue(listOf(album.id)) } },
        ),
    ) {
        CoverImage(album.cover, album.accent, edge = 180.dp, modifier = Modifier.fillMaxWidth())
        Text(
            text = album.title,
            style = MaterialTheme.typography.titleSmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(top = 6.dp),
        )
        Text(
            text = listOfNotNull(album.artist, album.year?.toString()).joinToString("  -  "),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** A square cover, standing in its accent colour until the pixels have loaded. */
@Composable
fun CoverImage(cover: Long?, accent: Int?, edge: androidx.compose.ui.unit.Dp, modifier: Modifier = Modifier) {
    val px = with(LocalDensity.current) { edge.roundToPx() }
    val bitmap = rememberCover(cover, px)
    val shape = RoundedCornerShape(4.dp)
    Box(modifier = modifier.aspectRatio(1f).clip(shape).background(accentColor(accent).copy(alpha = 0.5f))) {
        if (bitmap != null) {
            Image(
                bitmap = bitmap.asImageBitmap(),
                contentDescription = null,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize(),
            )
        }
    }
}

/** An album's accent as a colour, or a dim grey before it is known. */
fun accentColor(accent: Int?): Color = accent?.let { Color(0xff000000 or it.toLong()) } ?: Color(0xff2a2a30)

/** What is playing, along the bottom of the library. Tapping it opens the player. Shows nothing while the queue is
 * empty. */
@Composable
private fun NowPlayingBar(onOpenPlayer: () -> Unit) {
    val playback = App.playback
    if (playback.current == null) return
    HorizontalDivider()
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(accentColor(playback.album?.accent).copy(alpha = 0.15f))
            .clickable(onClick = onOpenPlayer)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        CoverImage(playback.album?.cover, playback.album?.accent, edge = 48.dp, modifier = Modifier.size(48.dp))
        Column(modifier = Modifier.weight(1f).padding(horizontal = 12.dp)) {
            Text(playback.title, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(
                text = playback.album?.artist.orEmpty(),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        IconButton(onClick = { App.command { Core.togglePlaying() } }) {
            Icon(
                if (playback.playing) Icons.Default.Pause else Icons.Default.PlayArrow,
                contentDescription = if (playback.playing) "Pause" else "Play",
            )
        }
    }
}

@Composable
private fun ChooseFolder(onChooseFolder: () -> Unit) {
    Column(
        modifier = Modifier.fillMaxSize().padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("Phonoscule", style = MaterialTheme.typography.headlineMedium)
        Text("Pick the folder your music is in. Opus and WAV files are played, and cover.jpg and its kin are shown.")
        Button(onClick = onChooseFolder) { Text("Choose music folder") }
        val core = remember { runCatching { Core.version() } }
        Text(
            text = core.fold(
                { version -> "core v${version.coreVersion} ${version.arch} ${version.profile}" },
                { error -> "core error: ${error.message}" },
            ),
            style = MaterialTheme.typography.labelSmall,
        )
    }
}

/** How long the pull-to-refresh indicator spins on a pull alone, should the core never say it started scanning. */
private const val PULL_GRACE_MS = 1000L

private val CHIP_HEIGHT = 28.dp
private val CHIP_GAP = 6.dp

private val SCROLL_BAR_WIDTH = 3.dp
private val SCROLL_BAR_MARGIN = 3.dp
private val SCROLL_BAR_MIN = 24.dp
private const val SCROLL_BAR_ALPHA = 0.22f
