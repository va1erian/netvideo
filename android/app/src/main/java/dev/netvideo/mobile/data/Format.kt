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
        // Wide films are letterboxed (1920x800 is still 1080p): go by width.
        val lines = maxOf(height, width * 9 / 16)
        return when {
            lines >= 2000 -> "2160p"
            lines >= 1400 -> "1440p"
            lines >= 1000 -> "1080p"
            lines >= 700 -> "720p"
            else -> "${height}p"
        }
    }

    /** One line of details: duration, resolution and codec, when known. */
    fun details(durationMs: Long?, width: Long?, height: Long?, codec: String?): String =
        listOfNotNull(durationMs?.let(::duration), resolution(width, height), codec?.uppercase())
            .joinToString(" · ")
}
