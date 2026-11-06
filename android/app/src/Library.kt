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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/** The library: a wall of covers to search, filter by genre and sort, with what is playing along the bottom.
 *
 * Back clears the search and the genre, and leaves the search field, before it leaves the app. Pass `covered` while
 * another screen is open over this one, which then has Back to itself. */
@Composable
fun LibraryScreen(covered: Boolean, onOpenAlbum: (Long) -> Unit, onOpenPlayer: () -> Unit, onChooseFolder: () -> Unit) {
    if (App.storage == null) {
        ChooseFolder(onChooseFolder)
        return
    }

    var query by rememberSaveable { mutableStateOf("") }
    var genre by rememberSaveable { mutableStateOf<String?>(null) }
    var albums by remember { mutableStateOf(emptyList<AlbumSummary>()) }
    var genres by remember { mutableStateOf(emptyList<String>()) }
    var searching by remember { mutableStateOf(false) }

    val focus = LocalFocusManager.current
    BackHandler(enabled = !covered && (query.isNotEmpty() || genre != null || searching)) {
        query = ""
        genre = null
        focus.clearFocus()
    }

    LaunchedEffect(query, genre, App.libraryVersion) {
        val (found, all) = withContext(Dispatchers.IO) { Core.albums(query, genre) to Core.genres() }
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

        GenreChips(genres, genre, onPick = { genre = it })

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
                            text = if (query.isEmpty() && genre == null) {
                                "No albums here yet. Pull down to scan."
                            } else {
                                "Nothing matches."
                            },
                            modifier = Modifier.padding(24.dp),
                        )
                    }
                }
            } else {
                LazyVerticalGrid(
                    columns = GridCells.Adaptive(140.dp),
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

/** Chips that limit the wall to one genre. Shows nothing while no album has a genre. */
@Composable
private fun GenreChips(genres: List<String>, picked: String?, onPick: (String?) -> Unit) {
    if (genres.isEmpty()) return
    LazyRow(
        contentPadding = PaddingValues(horizontal = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        item { FilterChip(selected = picked == null, onClick = { onPick(null) }, label = { Text("All") }) }
        items(genres, key = { it }) { genre ->
            FilterChip(selected = picked == genre, onClick = { onPick(genre) }, label = { Text(genre) })
        }
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
