package zone.jo.phonoscule

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowLeft
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalMinimumInteractiveComponentSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.pulltorefresh.PullToRefreshDefaults
import androidx.compose.material3.pulltorefresh.rememberPullToRefreshState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/** The library: a wall of covers to search, narrow by genre and artist, and sort, with what is playing along the
 * bottom.
 *
 * Shows only the albums of the picked `genres` and `artists`, all of them while none are picked. A row for each filter
 * names what is picked of it: its name opens its picker with [onOpenFilter], and what is picked clears it.
 *
 * Back clears the search, the genres and the artists, and leaves the search field, before it leaves the app. Pass
 * `covered` while another screen is open over this one, which then has Back to itself. */
@Composable
fun LibraryScreen(
    covered: Boolean,
    genres: List<String>,
    artists: List<String>,
    onPickGenres: (List<String>) -> Unit,
    onPickArtists: (List<String>) -> Unit,
    onOpenFilter: (Filter) -> Unit,
    onOpenAlbum: (Long) -> Unit,
    onOpenPlayer: () -> Unit,
    onChooseFolder: () -> Unit,
) {
    if (App.storage == null) {
        ChooseFolder(onChooseFolder)
        return
    }

    var query by rememberSaveable { mutableStateOf("") }
    var albums by remember { mutableStateOf(emptyList<AlbumSummary>()) }
    var searching by remember { mutableStateOf(false) }

    val focus = LocalFocusManager.current
    val filtered = query.isNotEmpty() || genres.isNotEmpty() || artists.isNotEmpty()
    BackHandler(enabled = !covered && (filtered || searching)) {
        query = ""
        onPickGenres(emptyList())
        onPickArtists(emptyList())
        focus.clearFocus()
    }

    LaunchedEffect(query, genres, artists, App.libraryVersion) {
        albums = withContext(Dispatchers.IO) { Core.albums(query, genres, artists) }
    }

    // The wall runs the full height, under the bars that float over it, and clears them by their heights.
    val accent = accentColor(App.playback.album?.accent)
    val playing = App.playback.current != null
    var headerPx by remember { mutableIntStateOf(0) }
    var footerPx by remember { mutableIntStateOf(0) }
    val header = with(LocalDensity.current) { headerPx.toDp() }
    val footer = with(LocalDensity.current) { (if (playing) footerPx else 0).toDp() }

    Box(modifier = Modifier.fillMaxSize()) {
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
        val pull = rememberPullToRefreshState()
        val refreshing = pulled || App.scanning
        PullToRefreshBox(
            isRefreshing = refreshing,
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
            modifier = Modifier.fillMaxSize(),
            state = pull,
            indicator = {
                PullToRefreshDefaults.Indicator(
                    state = pull,
                    isRefreshing = refreshing,
                    modifier = Modifier.align(Alignment.TopCenter).padding(top = header),
                )
            },
        ) {
            if (albums.isEmpty()) {
                // Scrollable, or there would be nothing to pull.
                Column(
                    modifier = Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .padding(top = header, bottom = footer),
                ) {
                    if (!App.scanning) {
                        Text(
                            text = if (!filtered) {
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
                    contentPadding = PaddingValues(
                        start = 12.dp,
                        end = 12.dp,
                        top = header + 12.dp,
                        bottom = footer + 12.dp,
                    ),
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp),
                ) {
                    items(albums, key = { it.id }) { album -> AlbumCard(album, onClick = { onOpenAlbum(album.id) }) }
                }
            }
        }

        Column(
            modifier = Modifier
                .align(Alignment.TopCenter)
                .fillMaxWidth()
                .background(glass(accent))
                .onSizeChanged { headerPx = it.height },
        ) {
            Row(
                modifier = Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 8.dp, bottom = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                SearchField(
                    query,
                    onQuery = { query = it },
                    modifier = Modifier.weight(1f).onFocusChanged { searching = it.isFocused },
                )
                LibraryMenu(albums, onChooseFolder)
            }

            // Compact rows: padded out to Material's 48dp touch target, two of them would take a sixth of the screen.
            CompositionLocalProvider(LocalMinimumInteractiveComponentSize provides Dp.Unspecified) {
                Column(
                    modifier = Modifier.padding(horizontal = 12.dp, vertical = CHIP_GAP),
                    verticalArrangement = Arrangement.spacedBy(CHIP_GAP),
                ) {
                    FilterRow(Filter.GENRES, genres, onOpen = { onOpenFilter(Filter.GENRES) }, onPick = onPickGenres)
                    FilterRow(
                        Filter.ARTISTS,
                        artists,
                        onOpen = { onOpenFilter(Filter.ARTISTS) },
                        onPick = onPickArtists,
                    )
                }
            }

            GlassEdge(accent)
        }

        if (playing) {
            NowPlayingBar(
                onOpenPlayer,
                modifier = Modifier.align(Alignment.BottomCenter).onSizeChanged { footerPx = it.height },
            )
        }
    }
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

/** The library's search: an outlined field, shorter than Material's 56dp. */
@Composable
private fun SearchField(query: String, onQuery: (String) -> Unit, modifier: Modifier = Modifier) {
    val interaction = remember { MutableInteractionSource() }
    BasicTextField(
        value = query,
        onValueChange = onQuery,
        modifier = modifier,
        singleLine = true,
        textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
        cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
        interactionSource = interaction,
        decorationBox = { field ->
            OutlinedTextFieldDefaults.DecorationBox(
                value = query,
                innerTextField = field,
                enabled = true,
                singleLine = true,
                visualTransformation = VisualTransformation.None,
                interactionSource = interaction,
                placeholder = { Text("Search albums") },
                leadingIcon = { Icon(Icons.Default.Search, contentDescription = null) },
                trailingIcon = {
                    if (query.isNotEmpty()) {
                        IconButton(onClick = { onQuery("") }) {
                            Icon(Icons.Default.Clear, contentDescription = "Clear")
                        }
                    }
                },
                contentPadding = OutlinedTextFieldDefaults.contentPadding(top = 12.dp, bottom = 12.dp),
                container = {
                    OutlinedTextFieldDefaults.Container(
                        enabled = true,
                        isError = false,
                        interactionSource = interaction,
                    )
                },
            )
        },
    )
}

/** `filter`'s row: a chip for each value picked of it, which unpicks that value; then at the right, for the thumb, and
 * at the same place in every row, its name, which opens its picker. The chips run alphabetically leftwards from the
 * name, and scroll that way when they do not fit. */
@Composable
private fun FilterRow(filter: Filter, picked: List<String>, onOpen: () -> Unit, onPick: (List<String>) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (picked.isEmpty()) {
            Text(
                "All",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = TextAlign.End,
                modifier = Modifier.weight(1f),
            )
        } else {
            LazyRow(
                modifier = Modifier.weight(1f),
                reverseLayout = true,
                horizontalArrangement = Arrangement.spacedBy(CHIP_GAP, Alignment.End),
            ) {
                items(picked.sortedWith(String.CASE_INSENSITIVE_ORDER), key = { it }) { value ->
                    CompactChip(
                        value,
                        filled = true,
                        onClick = { onPick(picked - value) },
                        trailingIcon = Icons.Default.Clear,
                        trailingDescription = "Unpick $value",
                    )
                }
            }
        }
        CompactChip(
            filter.title,
            filled = false,
            onClick = onOpen,
            leadingIcon = Icons.AutoMirrored.Filled.KeyboardArrowLeft,
            modifier = Modifier.width(FILTER_NAME_WIDTH),
        )
    }
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

/** What is playing, along the bottom of the library, on the same glass as its header. Tapping it opens the player.
 * Shows nothing while the queue is empty. */
@Composable
private fun NowPlayingBar(onOpenPlayer: () -> Unit, modifier: Modifier = Modifier) {
    val playback = App.playback
    if (playback.current == null) return
    val accent = accentColor(playback.album?.accent)
    Column(modifier = modifier.fillMaxWidth().background(glass(accent))) {
        GlassEdge(accent)
        NowPlaying(playback, onOpenPlayer)
    }
}

@Composable
private fun NowPlaying(playback: Playback, onOpenPlayer: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
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

private val FILTER_NAME_WIDTH = 84.dp
