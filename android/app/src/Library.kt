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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.InputChip
import androidx.compose.material3.LocalMinimumInteractiveComponentSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
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
                OutlinedTextField(
                    value = query,
                    onValueChange = { query = it },
                    modifier = Modifier.weight(1f).onFocusChanged { searching = it.isFocused },
                    singleLine = true,
                    placeholder = { Text("Search albums") },
                    leadingIcon = { Icon(Icons.Default.Search, contentDescription = null) },
                    trailingIcon = {
                        if (query.isNotEmpty()) {
                            IconButton(onClick = { query = "" }) {
                                Icon(Icons.Default.Clear, contentDescription = "Clear")
                            }
                        }
                    },
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

/** A thin bar down the right edge of the grid, always shown, for where the view is in the whole of it and how much of it
 * shows. It runs between the grid's content padding, and takes rows to be of equal height. */
fun Modifier.scrollBar(state: LazyGridState): Modifier = this.drawWithContent {
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
    val top = maxOf(margin, info.beforeContentPadding.toFloat())
    val track = size.height - maxOf(margin, info.afterContentPadding.toFloat()) - top
    val thumb = (track * viewport / content).coerceAtLeast(SCROLL_BAR_MIN.toPx()).coerceAtMost(track)
    drawRoundRect(
        color = Color.White.copy(alpha = SCROLL_BAR_ALPHA),
        topLeft = Offset(size.width - margin - width, top + (track - thumb) * fraction),
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

/** `filter`'s row: its name, at the same place in every row, which opens its picker, then a chip for each value picked
 * of it, scrolling sideways when they do not fit, which unpicks that value. */
@Composable
private fun FilterRow(filter: Filter, picked: List<String>, onOpen: () -> Unit, onPick: (List<String>) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        AssistChip(
            onClick = onOpen,
            label = { Text(filter.title) },
            trailingIcon = { Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, contentDescription = null) },
            modifier = Modifier.width(FILTER_NAME_WIDTH),
        )
        if (picked.isEmpty()) {
            Text("All", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        } else {
            LazyRow(modifier = Modifier.weight(1f), horizontalArrangement = Arrangement.spacedBy(CHIP_GAP)) {
                items(picked, key = { it }) { value ->
                    InputChip(
                        selected = true,
                        onClick = { onPick(picked - value) },
                        label = { Text(value, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                        trailingIcon = { Icon(Icons.Default.Clear, contentDescription = "Unpick $value") },
                    )
                }
            }
        }
    }
}

/** A compact chip that turns a filter on and off. Put it where the minimum touch target is turned off, or Material pads
 * it out to one. */
@Composable
fun FilterToggle(label: String, selected: Boolean, onClick: () -> Unit, modifier: Modifier = Modifier) {
    FilterChip(
        selected = selected,
        onClick = onClick,
        label = {
            Text(label, style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        },
        modifier = modifier.height(CHIP_HEIGHT),
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

/** A bar floating over the library, as the desktop player's are: glass, the `accent` darkened and translucent. */
private fun glass(accent: Color) = Color(accent.red * 0.18f, accent.green * 0.18f, accent.blue * 0.18f, 0.68f)

/** The glass's edge where it meets what it floats over, catching the light. */
@Composable
private fun GlassEdge(accent: Color) {
    val edge = Color(accent.red * 0.7f, accent.green * 0.7f, accent.blue * 0.7f, 0.35f)
    Box(modifier = Modifier.fillMaxWidth().height(1.dp).background(edge))
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

private val CHIP_HEIGHT = 28.dp
val CHIP_GAP = 6.dp
private val FILTER_NAME_WIDTH = 104.dp

private val SCROLL_BAR_WIDTH = 3.dp
private val SCROLL_BAR_MARGIN = 3.dp
private val SCROLL_BAR_MIN = 24.dp
private const val SCROLL_BAR_ALPHA = 0.22f
