package dev.netvideo.mobile.ui

import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.netvideo.mobile.data.Sessions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.netvideo_mobile.MobileSession
import uniffi.netvideo_mobile.parsePairingLink

/**
 * Pairs this phone with a server, by scanning `netvideo-server pair --qr`
 * or by typing the address and code. A scanned code also pins the server's
 * key, so an impostor answering at that address is refused. The server is
 * checked before the code is sent, so a mistyped address does not burn it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PairScreen(onPaired: (MobileSession) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var url by rememberSaveable { mutableStateOf("https://") }
    var code by rememberSaveable { mutableStateOf("") }
    var name by rememberSaveable { mutableStateOf(Build.MODEL ?: "Android") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    fun pair(serverUrl: String, pairingCode: String, fingerprint: String?) {
        busy = true
        error = null
        scope.launch {
            val result = withContext(Dispatchers.IO) {
                runCatching {
                    val session = Sessions.open(context, serverUrl)
                    session.checkServer()
                    session.pair(pairingCode, name.trim().ifEmpty { "Android" }, fingerprint)
                    Sessions.remember(context, session)
                    session
                }
            }
            busy = false
            result.onSuccess(onPaired).onFailure { error = it.userMessage() }
        }
    }

    val scanner = rememberLauncherForActivityResult(ScanContract()) { scan ->
        val text = scan.contents ?: return@rememberLauncherForActivityResult
        runCatching { parsePairingLink(text) }
            .onSuccess { link ->
                url = link.url
                code = link.code
                pair(link.url, link.code, link.keyFingerprint)
            }
            .onFailure { error = it.userMessage() }
    }

    Scaffold(topBar = { TopAppBar(title = { Text("Pair with your server") }) }) { insets ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(insets)
                .padding(16.dp)
                .verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Text(
                "On the server, run `netvideo-server pair --viewer --qr --url <address>` " +
                    "and scan the code it shows (drop `--viewer` to make this phone an admin).",
                style = MaterialTheme.typography.bodyMedium,
            )
            Button(
                enabled = !busy,
                onClick = {
                    scanner.launch(
                        ScanOptions()
                            .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                            .setPrompt("Scan the code printed by netvideo-server pair --qr")
                            .setBeepEnabled(false)
                            .setOrientationLocked(false),
                    )
                },
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Scan QR code") }
            Text(
                "Or type the address and the 6-digit code:",
                style = MaterialTheme.typography.bodySmall,
            )
            OutlinedTextField(
                value = url,
                onValueChange = { url = it },
                label = { Text("Server address") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = code,
                onValueChange = { code = it.filter(Char::isDigit).take(6) },
                label = { Text("Pairing code") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                modifier = Modifier.fillMaxWidth(),
            )
            OutlinedTextField(
                value = name,
                onValueChange = { name = it.take(64) },
                label = { Text("Name for this phone") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            if (busy) {
                CircularProgressIndicator(Modifier.align(Alignment.End))
            } else {
                Button(
                    enabled = url.length > "https://".length && code.length == 6,
                    onClick = { pair(url.trim(), code, null) },
                    modifier = Modifier.align(Alignment.End),
                ) { Text("Pair") }
            }
        }
    }
}
