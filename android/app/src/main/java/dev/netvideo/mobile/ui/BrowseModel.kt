package dev.netvideo.mobile.ui

import androidx.compose.foundation.lazy.LazyListState
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
 * the folder stack, a cache of each visited folder's listing and its scroll
 * position.
 *
 * One load runs at a time, for the open folder; navigating cancels it. All
 * state is written on the main thread, after the blocking fetch returns.
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
    private var singleRoot by mutableStateOf(false)
    private val scrolls = mutableMapOf<String, LazyListState>()
    private var job: Job? = null

    /** The open folder's listing, when loaded. */
    val listing: Listing? get() = stack.lastOrNull()?.let { listings[it.id] }

    val canGoBack: Boolean get() = stack.size > (if (singleRoot) 1 else 0)

    /** The scroll position of the open folder (or the roots). */
    fun scroll(): LazyListState = scrolls.getOrPut(stack.lastOrNull()?.id ?: ROOTS) { LazyListState() }

    /** Loads the roots, unless they are loaded already. */
    fun start() {
        if (roots == null && job?.isActive != true) reload()
    }

    fun open(folder: FolderRef) {
        navigate(stack + folder)
    }

    fun back() {
        if (canGoBack) navigate(stack.dropLast(1))
    }

    /** Reloads the open folder (or the roots) from the start. */
    fun reload() {
        val folder = stack.lastOrNull()
        if (folder == null) {
            fetch {
                val loaded = session.roots()
                val only = loaded.singleOrNull()
                Fetched(only?.id, only?.let { session.folder(it.id, null, PAGE) }, false, loaded)
            }
        } else {
            load(folder.id, null)
        }
    }

    /** Fetches the next page of the open folder, if there is one. */
    fun loadMore() {
        val folder = stack.lastOrNull() ?: return
        val cursor = listings[folder.id]?.nextCursor ?: return
        if (job?.isActive != true) load(folder.id, cursor)
    }

    /** Shows progress saved by the player without refetching the folder. */
    fun updateProgress(videoId: String, progress: Progress) {
        listings = listings.mapValues { (_, listing) ->
            listing.copy(
                videos = listing.videos.map { if (it.id == videoId) it.copy(progress = progress) else it },
            )
        }
    }

    /** Leaves the open folder: its load and error do not follow the user. */
    private fun navigate(to: List<FolderRef>) {
        job?.cancel()
        loading = false
        error = null
        stack = to
        val folder = to.lastOrNull()
        if (folder != null && folder.id !in listings) load(folder.id, null)
    }

    private fun load(folderId: String, cursor: String?) = fetch {
        Fetched(folderId, session.folder(folderId, cursor, PAGE), append = cursor != null)
    }

    private class Fetched(
        val folderId: String?,
        val page: FolderPage?,
        val append: Boolean,
        val roots: List<FolderRef>? = null,
    )

    /** Runs a blocking fetch off the main thread, then applies it here. */
    private fun fetch(block: () -> Fetched) {
        job?.cancel()
        loading = true
        error = null
        job = scope.launch {
            // A cancelled job never resumes here, so stale results are dropped.
            val result = withContext(Dispatchers.IO) { runCatching(block) }
            loading = false
            result.onFailure { error = it }.onSuccess(::show)
        }
    }

    private fun show(fetched: Fetched) {
        fetched.roots?.let { loaded ->
            roots = loaded
            singleRoot = loaded.size == 1
            if (singleRoot) stack = loaded
        }
        val id = fetched.folderId ?: return
        val page = fetched.page ?: return
        val previous = listings[id]
        // A first page replaces the cached listing; a later page extends it.
        val listing = if (fetched.append && previous != null) {
            Listing(previous.folders + page.folders, previous.videos + page.videos, page.nextCursor)
        } else {
            Listing(page.folders, page.videos, page.nextCursor)
        }
        listings = listings + (id to listing)
    }

    private companion object {
        const val PAGE = 100u
        const val ROOTS = ""
    }
}
