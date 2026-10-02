package dev.netvideo.mobile.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.composables.icons.lucide.ArrowLeft
import com.composables.icons.lucide.CircleCheck
import com.composables.icons.lucide.EllipsisVertical
import com.composables.icons.lucide.Film
import com.composables.icons.lucide.Folder
import com.composables.icons.lucide.Lucide
import dev.netvideo.mobile.data.Format
import uniffi.netvideo_mobile.FolderRef
import uniffi.netvideo_mobile.VideoSummary

/** Folders first, then videos, exactly as they sit on the server's disk. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun BrowseScreen(model: BrowseModel, onPlay: (VideoSummary) -> Unit, onUnpair: () -> Unit) {
    LaunchedEffect(model) { model.start() }
    BackHandler(enabled = model.canGoBack) { model.back() }
    var menu by remember { mutableStateOf(false) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Text(
                        model.stack.lastOrNull()?.name ?: "netvideo",
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                },
                navigationIcon = {
                    if (model.canGoBack) {
                        IconButton(onClick = model::back) { Icon(Lucide.ArrowLeft, "Back") }
                    }
                },
                actions = {
                    IconButton(onClick = { menu = true }) { Icon(Lucide.EllipsisVertical, "More") }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        DropdownMenuItem(
                            text = { Text("Refresh") },
                            onClick = {
                                menu = false
                                model.reload()
                            },
                        )
                        DropdownMenuItem(
                            text = { Text("Unpair this phone") },
                            onClick = {
                                menu = false
                                onUnpair()
                            },
                        )
                    }
                },
            )
        },
    ) { insets ->
        Box(Modifier.fillMaxSize().padding(insets)) {
            val error = model.error
            val listing = model.listing
            val roots = model.roots
            when {
                error != null -> ErrorPane(error, onRetry = model::reload, onUnpair = onUnpair)
                model.stack.isEmpty() && roots != null -> FolderList(roots, emptyList(), model, onPlay)
                listing != null -> FolderList(listing.folders, listing.videos, model, onPlay)
                else -> CircularProgressIndicator(Modifier.align(Alignment.Center))
            }
        }
    }
}

@Composable
private fun FolderList(
    folders: List<FolderRef>,
    videos: List<VideoSummary>,
    model: BrowseModel,
    onPlay: (VideoSummary) -> Unit,
) {
    val state = model.scroll()
    val nearEnd by remember(state) {
        derivedStateOf {
            val last = state.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0
            last >= state.layoutInfo.totalItemsCount - 10
        }
    }
    LaunchedEffect(nearEnd, folders.size + videos.size, model.loading) {
        if (nearEnd) model.loadMore()
    }
    if (folders.isEmpty() && videos.isEmpty() && !model.loading) {
        Text("This folder has no videos.", Modifier.padding(24.dp))
        return
    }
    LazyColumn(state = state, modifier = Modifier.fillMaxSize()) {
        items(folders, key = { "f" + it.id }) { folder ->
            ListItem(
                headlineContent = { Text(folder.name) },
                leadingContent = { Icon(Lucide.Folder, contentDescription = null) },
                modifier = Modifier.clickable { model.open(folder) },
            )
        }
        items(videos, key = { "v" + it.id }) { video -> VideoRow(video, onClick = { onPlay(video) }) }
        if (model.loading) {
            item {
                Box(Modifier.fillMaxWidth().padding(16.dp), contentAlignment = Alignment.Center) {
                    CircularProgressIndicator()
                }
            }
        }
    }
}

@Composable
private fun VideoRow(video: VideoSummary, onClick: () -> Unit) {
    val progress = video.progress
    val duration = video.durationMs
    ListItem(
        headlineContent = { Text(video.name, maxLines = 2, overflow = TextOverflow.Ellipsis) },
        supportingContent = {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                val details = Format.details(duration, video.width, video.height, video.videoCodec)
                if (details.isNotEmpty()) Text(details, style = MaterialTheme.typography.bodySmall)
                if (progress != null && !progress.watched && duration != null && duration > 0) {
                    LinearProgressIndicator(
                        progress = { (progress.positionMs.toFloat() / duration).coerceIn(0f, 1f) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        },
        leadingContent = { Icon(Lucide.Film, contentDescription = null) },
        trailingContent = {
            if (progress?.watched == true) Icon(Lucide.CircleCheck, contentDescription = "Watched")
        },
        modifier = Modifier.clickable(onClick = onClick),
    )
}

@Composable
private fun ErrorPane(error: Throwable, onRetry: () -> Unit, onUnpair: () -> Unit) {
    Column(
        modifier = Modifier.fillMaxSize().padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(error.userMessage(), style = MaterialTheme.typography.bodyLarge)
        if (error.needsPairing()) {
            Button(onClick = onUnpair) { Text("Pair again") }
        } else {
            Button(onClick = onRetry) { Text("Retry") }
        }
    }
}
