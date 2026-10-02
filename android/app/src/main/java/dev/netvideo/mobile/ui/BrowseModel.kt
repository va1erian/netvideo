package dev.netvideo.mobile.ui

import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.netvideo_mobile.FolderPage
import uniffi.netvideo_mobile.FolderRef
import uniffi.netvideo_mobile.MobileSession
import uniffi.netvideo_mobile.Progress
import uniffi.netvideo_mobile.VideoSummary

/** What one folder shows: everything loaded so far, and where to go on. */
data class Listing(
    val folders: List<FolderRef>,
    val videos: List<VideoSummary>,
    val nextCursor: String?,
)

/**
 * Browse state, hoisted above the screens so it survives playing a video:
 * the folder stack and a cache of each visited folder's listing.
 *
 * With a single library root the stack starts inside it, so the user never
 * sees a one-item list.
 */
@Stable
class BrowseModel(private val session: MobileSession, private val scope: CoroutineScope) {
    /** Open folders, outermost first; empty shows the roots. */
    var stack by mutableStateOf<List<FolderRef>>(emptyList())
        private set
    var roots by mutableStateOf<List<FolderRef>?>(null)
        private set
    var loading by mutableStateOf(false)
        private set
    var error by mutableStateOf<Throwable?>(null)
        private set

    private var listings by mutableStateOf<Map<String, Listing>>(emptyMap())
    private var job: Job? = null
    private var singleRoot = false

    /** The open folder's listing, when loaded. */
    val listing: Listing? get() = stack.lastOrNull()?.let { listings[it.id] }

    val canGoBack: Boolean get() = stack.size > (if (singleRoot) 1 else 0)

    /** Loads the roots, unless they are loaded already. */
    fun start() {
        if (roots == null) reload()
    }

    fun open(folder: FolderRef) {
        stack = stack + folder
        if (folder.id !in listings) load(folder.id, null)
    }

    fun back() {
        if (!canGoBack) return
        stack = stack.dropLast(1)
        // A load cancelled by navigating away left nothing cached.
        val folder = stack.lastOrNull()
        if (folder != null && folder.id !in listings) load(folder.id, null)
    }

    /** Reloads the open folder (or the roots) from the start. */
    fun reload() {
        val folder = stack.lastOrNull()
        if (folder == null) loadRoots() else load(folder.id, null)
    }

    /** Fetches the next page of the open folder, if there is one. */
    fun loadMore() {
        val folder = stack.lastOrNull() ?: return
        val cursor = listings[folder.id]?.nextCursor ?: return
        if (!loading) load(folder.id, cursor)
    }

    /** Shows progress saved by the player without refetching the folder. */
    fun updateProgress(videoId: String, progress: Progress) {
        listings = listings.mapValues { (_, listing) ->
            listing.copy(
                videos = listing.videos.map { if (it.id == videoId) it.copy(progress = progress) else it },
            )
        }
    }

    private fun loadRoots() = run {
        val loaded = session.roots()
        roots = loaded
        singleRoot = loaded.size == 1
        if (singleRoot) {
            stack = loaded
            Fetched(loaded[0].id, session.folder(loaded[0].id, null, PAGE), append = false)
        } else {
            null
        }
    }

    private fun load(folderId: String, cursor: String?) = run {
        Fetched(folderId, session.folder(folderId, cursor, PAGE), append = cursor != null)
    }

    private class Fetched(val folderId: String, val page: FolderPage, val append: Boolean)

    /** Runs a blocking fetch off the main thread, then merges its page. */
    private fun run(fetch: () -> Fetched?) {
        job?.cancel()
        loading = true
        error = null
        job = scope.launch {
            val result = withContext(Dispatchers.IO) { runCatching(fetch) }
            loading = false
            result.onFailure { error = it }
            result.getOrNull()?.let(::merge)
        }
    }

    /** A first page replaces the cached listing; a later page extends it. */
    private fun merge(fetched: Fetched) {
        val page = fetched.page
        val previous = listings[fetched.folderId]
        val listing = if (fetched.append && previous != null) {
            Listing(previous.folders + page.folders, previous.videos + page.videos, page.nextCursor)
        } else {
            Listing(page.folders, page.videos, page.nextCursor)
        }
        listings = listings + (fetched.folderId to listing)
    }

    private companion object {
        const val PAGE = 100u
    }
}
