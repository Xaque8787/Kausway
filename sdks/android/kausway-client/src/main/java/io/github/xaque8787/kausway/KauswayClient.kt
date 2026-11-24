package io.github.xaque8787.kausway

import android.content.Context
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONObject

/** Use a dedicated application-private directory. Exclude it from device/cloud backup. */
data class KauswayConfig(
    val storageDirectory: File,
    val deviceName: String = "Android client",
    /** null: Iroh defaults; "none": direct LAN testing; otherwise custom HTTPS relay URL. */
    val relayUrl: String? = null,
    /** Discovery uses Mainline DHT by default. These settings do not change Iroh traffic relays. */
    val rendezvousDht: Boolean = true,
    /** HTTPS Pkarr gateway base URLs. HTTP is accepted only for explicit loopback fixtures. */
    val rendezvousGateways: List<String> = emptyList(),
    /** One discovery deadline, 1–30 seconds. Host connection has its own 30-second deadline. */
    val rendezvousTimeoutSeconds: Int = 10,
)

data class ProxyConnection(val proxyUrl: String)
enum class ConnectionState { IDLE, CONNECTED, DISCONNECTED, CLOSED, UNKNOWN }
enum class AuthorizationStatus { AUTHORIZED, DENIED, REVOKED, UNKNOWN }
enum class TransportPath { DIRECT, RELAY, UNKNOWN }
enum class TransportPolicy { AUTO, P2P_ONLY, RELAY_ONLY, UNKNOWN }
enum class PairingStage { RESOLVING, CONNECTING }
enum class KauswayErrorCode { CANCELLED, RENDEZVOUS_NOT_FOUND, RENDEZVOUS_TIMEOUT, RENDEZVOUS_EXPIRED, RENDEZVOUS_INVALID_RECORD, RENDEZVOUS_UNAVAILABLE, INVALID_INVITATION, AUTHORIZATION_DENIED, NOT_PAIRED, CLOSED, CONFIGURATION, STORAGE, TRANSPORT, INTERNAL }
class KauswayException(val code: KauswayErrorCode, message: String, val serverCode: String? = null) : Exception(message)
data class ConnectionStatus(
    val state: ConnectionState,
    val authorization: AuthorizationStatus,
    val path: TransportPath,
    val rttMs: Double?,
    /** Configured host preference. The observed path is reported separately. */
    val transportPolicy: TransportPolicy?,
    val policyEnforcement: String?,
    val proxyUrl: String?,
    val lastError: String?,
)

internal object NativeBridge {
    init { System.loadLibrary("kausway_client") }
    external fun initializeTls(context: Context)
    external fun invoke(handle: Long, operation: String, argument: String): String
}

/** One live client per private storage directory. Explicitly call shutdown when finished.
 * Native work runs off the UI thread. Lifecycle mutations finish even if the calling
 * coroutine is cancelled; stopProxy/shutdown cancel active media streams and upgrades.
 */
class KauswayClient private constructor(@Volatile private var handle: Long) {
    private val mutex = Mutex()
    companion object {
        suspend fun initialize(context: Context, config: KauswayConfig? = null): KauswayClient {
            var created: KauswayClient? = null
            try {
                return withContext(Dispatchers.IO) {
                    val effectiveConfig = config ?: KauswayConfig(File(context.noBackupFilesDir, "kausway-client"))
                    val directory = effectiveConfig.storageDirectory.canonicalFile
                    val privateRoot = context.applicationInfo.dataDir.let(::File).canonicalFile
                    if (!directory.path.startsWith(privateRoot.path + File.separator)) throw KauswayException(KauswayErrorCode.CONFIGURATION, "Storage must be a dedicated app-private subdirectory")
                    NativeBridge.initializeTls(context.applicationContext)
                    val args = JSONObject().put("dataDir", directory.path).put("deviceName", effectiveConfig.deviceName).put("relayUrl", effectiveConfig.relayUrl).put("rendezvousDht", effectiveConfig.rendezvousDht).put("rendezvousGateways", org.json.JSONArray(effectiveConfig.rendezvousGateways)).put("rendezvousTimeoutSeconds", effectiveConfig.rendezvousTimeoutSeconds)
                    KauswayClient(decode(NativeBridge.invoke(0, "initialize", args.toString())).getLong("handle")).also { created = it }
                }
            } catch (error: Throwable) {
                withContext(NonCancellable) { created?.shutdown() }
                throw error
            }
        }
        private fun decode(reply: String): JSONObject {
            val result = JSONObject(reply)
            if (!result.getBoolean("ok")) {
                val code = enumValues<KauswayErrorCode>().firstOrNull { it.name.equals(result.optString("code"), true) } ?: KauswayErrorCode.INTERNAL
                throw KauswayException(code, result.optString("message", "Native operation failed"), result.nullableString("serverCode"))
            }
            return result.getJSONObject("value")
        }
    }
    private suspend fun call(operation: String, argument: String = ""): JSONObject = withContext(Dispatchers.IO + NonCancellable) {
        mutex.withLock {
            if (handle == 0L) throw KauswayException(KauswayErrorCode.CLOSED, "Client is shut down")
            decode(NativeBridge.invoke(handle, operation, argument))
        }
    }
    /** Accepts the existing full invitation URL or a K1 short code. Never log either input. */
    suspend fun pair(invitationLink: String) { call("pair", invitationLink) }
    /** Can be polled while pair() is running; it does not wait behind lifecycle mutations. */
    suspend fun getPairingProgress(): PairingStage? = withContext(Dispatchers.IO) {
        val current = handle
        if (current == 0L) null else decode(NativeBridge.invoke(current, "pairingProgress", ""))
            .nullableString("stage")?.let { enumValue(it, PairingStage.CONNECTING) }
    }
    /** Interrupts discovery or an in-progress host connection without forgetting saved pairing. */
    suspend fun cancelPairing() = withContext(Dispatchers.IO + NonCancellable) {
        val current = handle
        if (current != 0L) decode(NativeBridge.invoke(current, "cancelPairing", ""))
        Unit
    }
    suspend fun startProxy(): ProxyConnection = ProxyConnection(call("startProxy").getString("proxy_url"))
    suspend fun stopProxy() { call("stopProxy") }
    /** Retains device identity. Does not revoke the device on the host. */
    suspend fun forgetPairing() { cancelPairing(); call("forgetPairing") }
    suspend fun notifyNetworkChange() { call("networkChange") }
    suspend fun getStatus(): ConnectionStatus {
        val s = call("status")
        return ConnectionStatus(
            enumValue(s.getString("state"), ConnectionState.UNKNOWN),
            enumValue(s.getString("authorization"), AuthorizationStatus.UNKNOWN),
            enumValue(s.getString("path"), TransportPath.UNKNOWN),
            if (s.isNull("rtt_ms")) null else s.getDouble("rtt_ms"),
            s.nullableString("transport_policy")?.let { enumValue(it, TransportPolicy.UNKNOWN) },
            s.nullableString("policy_enforcement"), s.nullableString("proxy_url"), s.nullableString("last_error"),
        )
    }
    suspend fun shutdown() = withContext(Dispatchers.IO + NonCancellable) {
        // Pairing holds the Kotlin mutation lock, so request cancellation before waiting for it.
        val current = handle
        if (current != 0L) NativeBridge.invoke(current, "cancelPairing", "")
        mutex.withLock {
            if (handle != 0L) {
                decode(NativeBridge.invoke(handle, "shutdown", ""))
                handle = 0
            }
        }
    }
}
private fun JSONObject.nullableString(key: String): String? = if (isNull(key)) null else optString(key).takeIf { it.isNotEmpty() }
private inline fun <reified T : Enum<T>> enumValue(value: String, fallback: T): T = enumValues<T>().firstOrNull { it.name.equals(value, true) } ?: fallback
