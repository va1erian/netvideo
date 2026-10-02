package dev.netvideo.mobile.ui

import uniffi.netvideo_mobile.MobileException

/** A sentence for the user. uniffi leaves `message` empty, so map by type. */
fun Throwable.userMessage(): String = when (this) {
    is MobileException.NotPaired -> "This phone is not paired yet."
    is MobileException.PairingCode -> "That pairing code is wrong, expired or already used."
    is MobileException.Unauthorized -> "This phone was unpaired from the server. Pair it again."
    is MobileException.ServerMismatch ->
        "This server is not the one the QR code named. Check the address before trusting it."
    is MobileException.Network -> "Cannot reach the server (${detail})."
    is MobileException.InvalidUrl -> detail
    is MobileException.Server -> detail
    is MobileException.Other -> detail
    else -> message ?: javaClass.simpleName
}

/** Whether the only way forward is pairing again. */
fun Throwable.needsPairing(): Boolean =
    this is MobileException.NotPaired || this is MobileException.Unauthorized
