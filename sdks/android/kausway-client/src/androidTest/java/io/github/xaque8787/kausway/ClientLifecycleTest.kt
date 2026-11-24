package io.github.xaque8787.kausway

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.runBlocking
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import android.util.Log
import java.io.File
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.HttpURLConnection
import java.net.SocketTimeoutException
import java.net.URL

@RunWith(AndroidJUnit4::class)
class ClientLifecycleTest {
    private suspend fun <T> stage(name: String, action: suspend () -> T): T = try {
        action()
    } catch (error: Exception) {
        val detail = if (error is KauswayException) "${error.code}: ${error.message}"
            else "${error.javaClass.simpleName}: ${error.message}"
        throw AssertionError("Android SDK stage failed: $name ($detail)", error)
    }

    private fun verifyRunnerUdp(routes: String) {
        var reachable = false
        val failures = mutableListOf<String>()
        // 1200 bytes is the QUIC Initial minimum, unlike a small ping or TCP/ADB check.
        val marker = "kausway-emulator-udp-v1:${java.util.UUID.randomUUID()}".toByteArray()
        val payload = ByteArray(1200).also { marker.copyInto(it) }
        for (route in routes.split(',').take(8)) {
            try {
                val host = route.substringBeforeLast(':')
                val port = route.substringAfterLast(':').toInt()
                DatagramSocket().use { socket ->
                    socket.soTimeout = 1500
                    socket.connect(InetAddress.getByName(host), port)
                    for (attempt in 1..3) {
                        socket.send(DatagramPacket(payload, payload.size))
                        val reply = DatagramPacket(ByteArray(1201), 1201)
                        try {
                            socket.receive(reply)
                        } catch (error: SocketTimeoutException) {
                            if (attempt == 3) throw error
                            continue
                        }
                        check(reply.length == payload.size && reply.data.copyOf(reply.length).contentEquals(payload)) {
                            "UDP probe returned unexpected bytes"
                        }
                        break
                    }
                }
                reachable = true
                Log.i("KauswayFixture", "1200-byte UDP round trip succeeded: $route")
            } catch (error: Exception) {
                val failure = "$route: ${error.javaClass.simpleName}"
                failures.add(failure)
                Log.i("KauswayFixture", "UDP route failed: $failure")
            }
        }
        check(reachable) { "No bidirectional 1200-byte UDP route from emulator to runner: ${failures.joinToString()}" }
    }

    @Test fun nativeLifecycleAndStorage() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val storage = File(context.noBackupFilesDir, "test-${System.nanoTime()}")
        val client = KauswayClient.initialize(context, KauswayConfig(storage, relayUrl = "none"))
        try {
            assertEquals(ConnectionState.IDLE, client.getStatus().state)
            try { client.pair("invalid"); fail("Expected invalid invitation") }
            catch (e: KauswayException) { assertEquals(KauswayErrorCode.INVALID_INVITATION, e.code) }
            try { client.startProxy(); fail("Expected not paired") }
            catch (e: KauswayException) { assertEquals(KauswayErrorCode.NOT_PAIRED, e.code) }
            client.notifyNetworkChange()
            client.stopProxy(); client.stopProxy(); client.forgetPairing()
            assertTrue(File(storage, "client.key").isFile)
        } finally { client.shutdown(); client.shutdown(); storage.deleteRecursively() }
    }
    /** Optional real host fixture provided by scripts/android/emulator-e2e.sh. */
    @Test fun realHostRangeAndReconnect() = runBlocking {
        val invite = InstrumentationRegistry.getArguments().getString("kauswayInvite")
        org.junit.Assume.assumeTrue("No real host fixture supplied", invite != null)
        InstrumentationRegistry.getArguments().getString("kauswayUdpProbe")?.let { routes ->
            stage("runner UDP preflight") { verifyRunnerUdp(routes) }
        }
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val storage = File(context.noBackupFilesDir, "pair-${System.nanoTime()}")
        val client = KauswayClient.initialize(context, KauswayConfig(storage, relayUrl = "none"))
        try {
            stage("pair invitation") { client.pair(requireNotNull(invite)) }
            val connection = stage("start proxy after pairing") { client.startProxy() }
            assertEquals(connection, stage("reuse running proxy") { client.startProxy() })
            val http = URL(connection.proxyUrl + "movie.bin").openConnection() as HttpURLConnection
            http.setRequestProperty("Range", "bytes=10-19")
            http.connectTimeout = 15000; http.readTimeout = 15000
            try {
                assertEquals(206, http.responseCode)
                assertEquals(10, http.inputStream.use { it.readBytes().size })
            } finally { http.disconnect() }
            assertEquals(AuthorizationStatus.AUTHORIZED, client.getStatus().authorization)
            client.notifyNetworkChange()
            client.stopProxy()
            assertNotEquals(connection.proxyUrl, stage("restart proxy after stop") { client.startProxy() }.proxyUrl)
            client.forgetPairing()
        } finally { client.shutdown(); storage.deleteRecursively() }
    }
}
