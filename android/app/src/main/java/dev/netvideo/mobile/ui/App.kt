package dev.netvideo.mobile.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import dev.netvideo.mobile.data.Sessions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.netvideo_mobile.MobileSession
import uniffi.netvideo_mobile.VideoSummary

/** Where the app is: loading, pairing, browsing, or playing over browsing. */
@Composable
fun NetvideoApp() {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var loaded by remember { mutableStateOf(false) }
    var session by remember { mutableStateOf<MobileSession?>(null) }
    var playing by remember { mutableStateOf<VideoSummary?>(null) }

    LaunchedEffect(Unit) {
        session = withContext(Dispatchers.IO) {
            runCatching { Sessions.current(context) }.getOrNull()?.takeIf { it.isPaired() }
        }
        loaded = true
    }

    val current = session
    // One model per session, so a new pairing starts from a clean slate.
    val model = remember(current) { current?.let { BrowseModel(it, scope) } }
    val video = playing
    when {
        !loaded -> Box(Modifier.fillMaxSize()) {
            CircularProgressIndicator(Modifier.align(Alignment.Center))
        }
        current == null || model == null -> PairScreen(onPaired = { session = it })
        video != null -> PlayerScreen(current, video) { progress ->
            progress?.let { model.updateProgress(video.id, it) }
            playing = null
        }
        else -> BrowseScreen(
            model = model,
            onPlay = { playing = it },
            onUnpair = {
                scope.launch {
                    withContext(Dispatchers.IO) { Sessions.forget(context) }
                    session = null
                }
            },
        )
    }
}
