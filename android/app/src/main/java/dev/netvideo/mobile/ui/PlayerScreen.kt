package dev.netvideo.mobile.ui

import androidx.activity.compose.BackHandler
import androidx.activity.compose.LocalActivity
import androidx.annotation.OptIn
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.media3.common.MediaItem
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.datasource.ResolvingDataSource
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.source.DefaultMediaSourceFactory
import androidx.media3.ui.PlayerView
import dev.netvideo.mobile.data.Resume
import java.io.IOException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.netvideo_mobile.MobileSession
import uniffi.netvideo_mobile.Progress
import uniffi.netvideo_mobile.VideoSummary

/**
 * Saves progress after the screen is gone, so leaving never loses it. One
 * at a time, in order, so an older position never lands after a newer one.
 */
private val saver = CoroutineScope(SupervisorJob() + Dispatchers.IO.limitedParallelism(1))

/**
 * Plays a video's file directly (M2 has no transcoding yet), resuming from
 * this device's saved position and saving it while playing, on pause and on
 * exit. [onClose] receives the last saved progress for the list.
 */
@OptIn(UnstableApi::class)
@Composable
fun PlayerScreen(session: MobileSession, video: VideoSummary, onClose: (Progress?) -> Unit) {
    val context = LocalContext.current
    var error by remember { mutableStateOf<PlaybackException?>(null) }
    var last by remember { mutableStateOf<Progress?>(null) }
    val player = remember {
        // The header is resolved per request, on the loader thread, so the
        // token refreshes when due and never appears in the URL.
        val http = DefaultHttpDataSource.Factory().setAllowCrossProtocolRedirects(false)
        val authed = ResolvingDataSource.Factory(http) { spec ->
            val header = try {
                session.authorization()
            } catch (failure: Exception) {
                throw IOException(failure.userMessage(), failure)
            }
            spec.withAdditionalHeaders(mapOf("Authorization" to header))
        }
        ExoPlayer.Builder(context)
            .setMediaSourceFactory(DefaultMediaSourceFactory(authed))
            .build()
            .apply {
                val start = Resume.startPosition(
                    video.progress?.positionMs,
                    video.progress?.watched ?: false,
                    video.durationMs,
                )
                setMediaItem(MediaItem.fromUri(session.fileUrl(video.id)), start)
                prepare()
                playWhenReady = true
            }
    }

    fun save() {
        val duration = player.duration.takeIf { it > 0 } ?: video.durationMs ?: 0
        val position = player.currentPosition.coerceAtLeast(0)
        if (position == 0L && last == null) return
        val watched = Resume.isWatched(position, duration) || player.playbackState == Player.STATE_ENDED
        val previous = last
        if (previous != null && previous.positionMs == position && previous.watched == watched) return
        val progress = Progress(position, watched, System.currentTimeMillis() / 1000)
        last = progress
        saver.launch { runCatching { session.saveProgress(video.id, position, watched) } }
    }

    fun close() {
        save()
        onClose(last)
    }

    BackHandler(onBack = ::close)
    LaunchedEffect(player) {
        while (true) {
            delay(Resume.SAVE_INTERVAL_MS)
            if (player.isPlaying) save()
        }
    }
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    // Keyed on the player alone: it is released here, so nothing may outlive it.
    DisposableEffect(player) {
        val listener = object : Player.Listener {
            override fun onPlayerError(failure: PlaybackException) {
                error = failure
            }

            override fun onPlaybackStateChanged(state: Int) {
                if (state == Player.STATE_ENDED) save()
            }
        }
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) {
                player.pause()
                save()
            }
        }
        player.addListener(listener)
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            player.removeListener(listener)
            player.release()
        }
    }
    FullScreen()

    Box(Modifier.fillMaxSize().background(Color.Black)) {
        AndroidView(
            factory = { PlayerView(it).apply { this.player = player; keepScreenOn = true } },
            modifier = Modifier.fillMaxSize(),
        )
        error?.let { failure ->
            Column(
                modifier = Modifier.align(Alignment.Center).padding(24.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(playbackMessage(failure), color = Color.White)
                Button(onClick = ::close) { Text("Back") }
            }
        }
    }
}

/** Why playback failed, in words. Unsupported formats need M3's transcoding. */
@OptIn(UnstableApi::class)
private fun playbackMessage(failure: PlaybackException): String = when (failure.errorCode) {
    PlaybackException.ERROR_CODE_DECODER_INIT_FAILED,
    PlaybackException.ERROR_CODE_DECODING_FORMAT_UNSUPPORTED,
    PlaybackException.ERROR_CODE_PARSING_CONTAINER_UNSUPPORTED,
    -> "This phone cannot play this file's format yet."
    PlaybackException.ERROR_CODE_IO_BAD_HTTP_STATUS ->
        "The server refused the video (${failure.cause?.message ?: failure.errorCodeName})."
    PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED,
    PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_TIMEOUT,
    -> "Lost the connection to the server."
    else -> failure.cause?.message ?: failure.errorCodeName
}

/** Hides the system bars while the player is shown. */
@Composable
private fun FullScreen() {
    val activity = LocalActivity.current ?: return
    DisposableEffect(activity) {
        val controller = WindowCompat.getInsetsController(activity.window, activity.window.decorView)
        controller.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        controller.hide(WindowInsetsCompat.Type.systemBars())
        onDispose { controller.show(WindowInsetsCompat.Type.systemBars()) }
    }
}
