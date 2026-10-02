package dev.netvideo.mobile.data

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import uniffi.netvideo_mobile.SecretVault

/**
 * Keeps the Rust core's secrets (device private keys and tokens) encrypted
 * with an AES-GCM key that never leaves the Android Keystore. Only the
 * ciphertext reaches app storage, and each value is bound to its key name,
 * so values cannot be swapped between entries.
 */
class KeystoreVault(context: Context) : SecretVault {
    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    override fun read(key: String): String? {
        val stored = prefs.getString(key, null) ?: return null
        val bytes = Base64.decode(stored, Base64.NO_WRAP)
        require(bytes.size > IV_BYTES) { "vault entry is truncated" }
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(TAG_BITS, bytes, 0, IV_BYTES))
        cipher.updateAAD(key.toByteArray(Charsets.UTF_8))
        val plain = cipher.doFinal(bytes, IV_BYTES, bytes.size - IV_BYTES)
        return String(plain, Charsets.UTF_8)
    }

    override fun write(key: String, value: String) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        // The Keystore picks a fresh random IV for every encryption.
        cipher.init(Cipher.ENCRYPT_MODE, secretKey())
        cipher.updateAAD(key.toByteArray(Charsets.UTF_8))
        val sealed = cipher.iv + cipher.doFinal(value.toByteArray(Charsets.UTF_8))
        check(cipher.iv.size == IV_BYTES) { "unexpected IV size" }
        val saved = prefs.edit().putString(key, Base64.encodeToString(sealed, Base64.NO_WRAP)).commit()
        check(saved) { "could not save the vault entry" }
    }

    override fun delete(key: String) {
        check(prefs.edit().remove(key).commit()) { "could not delete the vault entry" }
    }

    private companion object {
        const val PREFS = "netvideo_vault"
        const val ALIAS = "netvideo_vault_key"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
        const val TAG_BITS = 128

        @Synchronized
        fun secretKey(): SecretKey {
            val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
            (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
            val spec = KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
            val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
            generator.init(spec)
            return generator.generateKey()
        }
    }
}
