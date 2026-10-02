package dev.netvideo.mobile.data

/** Display formatting for the browse list. Pure, so it is unit tested. */
object Format {
    /** `1:02:03` or `4:05`. */
    fun duration(ms: Long): String {
        val total = (ms / 1000).coerceAtLeast(0)
        val hours = total / 3600
        val minutes = total % 3600 / 60
        val seconds = total % 60
        return if (hours > 0) {
            "%d:%02d:%02d".format(hours, minutes, seconds)
        } else {
            "%d:%02d".format(minutes, seconds)
        }
    }

    /** `2160p`, `1080p`, ... from the frame height, or `null` when unknown. */
    fun resolution(width: Long?, height: Long?): String? {
        if (width == null || height == null || width <= 0 || height <= 0) return null
        // Letterboxed films (1920x800) are still 1080p, and portrait clips
        // (1080x1920) too: rate the short side, or the long side at 16:9.
        val short = minOf(width, height)
        val long = maxOf(width, height)
        val lines = maxOf(short, long * 9 / 16)
        return when {
            lines >= 2000 -> "2160p"
            lines >= 1400 -> "1440p"
            lines >= 1000 -> "1080p"
            lines >= 700 -> "720p"
            else -> "${short}p"
        }
    }

    /** One line of details: duration, resolution and codec, when known. */
    fun details(durationMs: Long?, width: Long?, height: Long?, codec: String?): String =
        listOfNotNull(durationMs?.let(::duration), resolution(width, height), codec?.uppercase())
            .joinToString(" · ")
}
