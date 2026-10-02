package dev.netvideo.mobile.data

/**
 * When to resume a video and when to call it watched. Pure functions, so
 * they are unit tested without a device.
 */
object Resume {
    /** Positions closer than this to the start are not worth resuming. */
    const val MIN_RESUME_MS = 10_000L

    /** Within this of the end, a video counts as watched. */
    const val END_MARGIN_MS = 120_000L

    /** How often progress is saved while playing. */
    const val SAVE_INTERVAL_MS = 15_000L

    /**
     * The position to start from: the saved one, unless the video was
     * watched, barely started, or the position is past the end.
     */
    fun startPosition(savedMs: Long?, watched: Boolean, durationMs: Long?): Long {
        if (savedMs == null || watched || savedMs < MIN_RESUME_MS) return 0
        if (durationMs != null && durationMs > 0 && savedMs >= durationMs - END_MARGIN_MS / 4) return 0
        return savedMs
    }

    /**
     * Whether stopping at `positionMs` means the video was watched: within
     * [END_MARGIN_MS] of the end, or past 95% for short videos.
     */
    fun isWatched(positionMs: Long, durationMs: Long): Boolean {
        if (durationMs <= 0) return false
        val margin = minOf(END_MARGIN_MS, durationMs / 20)
        return positionMs >= durationMs - margin
    }
}
