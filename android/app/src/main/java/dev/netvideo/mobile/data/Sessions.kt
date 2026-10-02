package dev.netvideo.mobile.data

import android.content.Context
import android.security.NetworkSecurityPolicy
import uniffi.netvideo_mobile.MobileSession

/**
 * The one server this app talks to. Its URL is kept in plain preferences
 * (it is not a secret); the credentials live in [KeystoreVault].
 *
 * Building a session reads the Keystore, so call these off the main thread.
 */
object Sessions {
    private const val PREFS = "netvideo"
    private const val KEY_URL = "server_url"

    private var cached: MobileSession? = null

    /** The saved server's session, or `null` before the first pairing. */
    @Synchronized
    fun current(context: Context): MobileSession? {
        cached?.let { return it }
        val url = prefs(context).getString(KEY_URL, null) ?: return null
        return open(context, url).also { cached = it }
    }

    /**
     * A fresh session for `url`, not saved until [remember] is called.
     *
     * The Rust core opens its own sockets, which the manifest's cleartext
     * setting does not cover, so plain `http://` is refused here unless
     * this build permits cleartext (debug builds, for LAN testing).
     */
    fun open(context: Context, url: String): MobileSession {
        val cleartext = url.trim().startsWith("http://", ignoreCase = true)
        if (cleartext && !NetworkSecurityPolicy.getInstance().isCleartextTrafficPermitted) {
            throw IllegalArgumentException("Use the server's https:// address.")
        }
        return MobileSession(url, KeystoreVault(context.applicationContext))
    }

    /** Saves a newly paired session as the app's server. */
    @Synchronized
    fun remember(context: Context, session: MobileSession) {
        prefs(context).edit().putString(KEY_URL, session.url()).apply()
        cached = session
    }

    /** Unpairs and forgets the server. */
    @Synchronized
    fun forget(context: Context) {
        runCatching { current(context)?.forget() }
        prefs(context).edit().remove(KEY_URL).apply()
        cached = null
    }

    private fun prefs(context: Context) =
        context.applicationContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
