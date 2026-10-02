package dev.netvideo.mobile.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class FormatTest {
    @Test
    fun formatsDurations() {
        assertEquals("0:00", Format.duration(0))
        assertEquals("4:05", Format.duration(245_000))
        assertEquals("1:02:03", Format.duration(3_723_999))
    }

    @Test
    fun namesResolutionsByWidthForLetterboxedFilms() {
        assertEquals("1080p", Format.resolution(1920, 800))
        assertEquals("2160p", Format.resolution(3840, 2160))
        assertEquals("720p", Format.resolution(1280, 720))
        assertEquals("480p", Format.resolution(640, 480))
        assertNull(Format.resolution(null, 1080))
    }

    @Test
    fun joinsKnownDetails() {
        assertEquals("1:00:00 · 1080p · H264", Format.details(3_600_000, 1920, 1080, "h264"))
        assertEquals("", Format.details(null, null, null, null))
    }
}
