package dev.netvideo.mobile.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ResumeTest {
    private val hour = 3_600_000L

    @Test
    fun resumesASavedPosition() {
        assertEquals(600_000L, Resume.startPosition(600_000L, false, hour))
    }

    @Test
    fun startsOverWhenThereIsNothingWorthResuming() {
        assertEquals(0L, Resume.startPosition(null, false, hour))
        assertEquals(0L, Resume.startPosition(5_000L, false, hour))
        assertEquals(0L, Resume.startPosition(600_000L, true, hour))
        assertEquals(0L, Resume.startPosition(hour - 1_000L, false, hour))
    }

    @Test
    fun resumesWhenTheDurationIsUnknown() {
        assertEquals(600_000L, Resume.startPosition(600_000L, false, null))
    }

    @Test
    fun watchedNearTheEnd() {
        assertTrue(Resume.isWatched(hour - 60_000L, hour))
        assertFalse(Resume.isWatched(hour / 2, hour))
        // Short clips use 5% of their length instead of two minutes.
        assertFalse(Resume.isWatched(50_000L, 60_000L))
        assertTrue(Resume.isWatched(58_000L, 60_000L))
        assertFalse(Resume.isWatched(0L, 0L))
    }
}
