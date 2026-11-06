package zone.jo.phonoscule

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.grid.LazyGridState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/** A square cover, standing in its accent colour until the pixels have loaded. */
@Composable
fun CoverImage(cover: Long?, accent: Int?, edge: Dp, modifier: Modifier = Modifier) {
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

/** A bar floating over what it serves, as the desktop player's do: glass, the `accent` darkened and translucent, if a
 * little less so than the desktop's. */
fun glass(accent: Color) = Color(accent.red * 0.18f, accent.green * 0.18f, accent.blue * 0.18f, 0.8f)

/** The glass's edge where it meets what it floats over, catching the light. */
@Composable
fun GlassEdge(accent: Color) {
    val edge = Color(accent.red * 0.7f, accent.green * 0.7f, accent.blue * 0.7f, 0.35f)
    Box(modifier = Modifier.fillMaxWidth().height(1.dp).background(edge))
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

/** How far apart chips stand, in a row or a grid. */
val CHIP_GAP = 6.dp

private val CHIP_HEIGHT = 28.dp
private val CHIP_ICON = 16.dp
private val CHIP_TEXT_MAX = 220.dp

private val SCROLL_BAR_WIDTH = 3.dp
private val SCROLL_BAR_MARGIN = 3.dp
private val SCROLL_BAR_MIN = 24.dp
private const val SCROLL_BAR_ALPHA = 0.22f
