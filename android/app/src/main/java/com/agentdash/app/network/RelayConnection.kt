package com.agentdash.app.network

import android.util.Log
import com.agentdash.app.crypto.CryptoBox
import com.agentdash.app.model.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import okhttp3.*

/**
 * Manages the encrypted WebSocket connection to the relay server.
 *
 * This class acts as the phone-side counterpart of the daemon's relay_connector.
 * It connects to the relay, authenticates, and handles encrypted bidirectional
 * messaging using NaCl crypto_box (X25519 + XSalsa20-Poly1305).
 */
class RelayConnection(
    private val config: PairingConfig,
    private val scope: CoroutineScope
) {
    companion object {
        private const val TAG = "RelayConnection"
        private const val INITIAL_BACKOFF_MS = 1000L
        private const val MAX_BACKOFF_MS = 30_000L
    }

    enum class State {
        DISCONNECTED,
        CONNECTING,
        AUTHENTICATING,
        CONNECTED,
        RECONNECTING
    }

    private val _connectionState = MutableStateFlow(State.DISCONNECTED)
    val connectionState: StateFlow<State> = _connectionState.asStateFlow()

    private val _peerCount = MutableStateFlow(0)
    val peerCount: StateFlow<Int> = _peerCount.asStateFlow()

    private val _events = MutableSharedFlow<ServerEvent>(extraBufferCapacity = 64)
    val events: SharedFlow<ServerEvent> = _events.asSharedFlow()

    private val _errors = MutableSharedFlow<String>(extraBufferCapacity = 16)
    val errors: SharedFlow<String> = _errors.asSharedFlow()

    private val client = OkHttpClient.Builder()
        .pingInterval(java.time.Duration.ofSeconds(30))
        .build()

    private val cryptoBox: CryptoBox = CryptoBox.create(
        mySecretKey = CryptoBox.decodeBase64(config.phone_secret_key_b64),
        theirPublicKey = CryptoBox.decodeBase64(config.daemon_public_key_b64)
    )

    private var webSocket: WebSocket? = null
    private var connectJob: Job? = null
    private var shouldReconnect = false

    /**
     * Start the connection with auto-reconnect.
     */
    fun connect() {
        shouldReconnect = true
        connectJob?.cancel()
        connectJob = scope.launch {
            var backoff = INITIAL_BACKOFF_MS
            while (shouldReconnect && isActive) {
                _connectionState.value = if (backoff > INITIAL_BACKOFF_MS) {
                    State.RECONNECTING
                } else {
                    State.CONNECTING
                }

                try {
                    doConnect()
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Log.e(TAG, "Connection error: ${e.message}")
                    _errors.tryEmit("Connection error: ${e.message}")
                }

                if (!shouldReconnect) break

                _connectionState.value = State.RECONNECTING
                Log.d(TAG, "Reconnecting in ${backoff}ms...")
                delay(backoff)
                backoff = (backoff * 2).coerceAtMost(MAX_BACKOFF_MS)
            }
        }
    }

    /**
     * Disconnect and stop auto-reconnect.
     */
    fun disconnect() {
        shouldReconnect = false
        connectJob?.cancel()
        connectJob = null
        webSocket?.close(1000, "Client disconnect")
        webSocket = null
        _connectionState.value = State.DISCONNECTED
        _peerCount.value = 0
    }

    /**
     * Send an encrypted ClientRequest to the daemon through the relay.
     */
    fun send(request: ClientRequest) {
        val ws = webSocket ?: run {
            Log.w(TAG, "Cannot send: not connected")
            return
        }

        try {
            val json = ProtocolGson.gson.toJson(request, ClientRequest::class.java)
            val (ciphertext, nonce) = cryptoBox.encryptString(json)

            val relayMsg = RelayMessage.Encrypted(
                channel_id = config.channel_id,
                ciphertext = ciphertext,
                nonce = nonce
            )
            val msgJson = ProtocolGson.gson.toJson(relayMsg, RelayMessage::class.java)
            ws.send(msgJson)
        } catch (e: Exception) {
            Log.e(TAG, "Send error: ${e.message}")
            _errors.tryEmit("Send error: ${e.message}")
        }
    }

    private suspend fun doConnect() {
        val connected = CompletableDeferred<Unit>()
        val closed = CompletableDeferred<Unit>()

        val request = Request.Builder()
            .url(config.relay_url)
            .build()

        val listener = object : WebSocketListener() {
            private var authenticated = false

            override fun onOpen(webSocket: WebSocket, response: Response) {
                Log.d(TAG, "WebSocket opened, authenticating...")
                _connectionState.value = State.AUTHENTICATING

                val auth = RelayMessage.Auth(
                    channel_id = config.channel_id,
                    public_key = config.phone_public_key_b64,
                    server_token = config.server_token
                )
                val json = ProtocolGson.gson.toJson(auth, RelayMessage::class.java)
                webSocket.send(json)
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                try {
                    val msg = ProtocolGson.gson.fromJson(text, RelayMessage::class.java)
                    handleRelayMessage(msg, connected)
                } catch (e: Exception) {
                    Log.e(TAG, "Failed to parse relay message: ${e.message}")
                }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                Log.d(TAG, "WebSocket closing: $code $reason")
                webSocket.close(code, reason)
            }

            override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                Log.d(TAG, "WebSocket closed: $code $reason")
                _connectionState.value = State.DISCONNECTED
                closed.complete(Unit)
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                Log.e(TAG, "WebSocket failure: ${t.message}")
                _connectionState.value = State.DISCONNECTED
                if (!connected.isCompleted) {
                    connected.completeExceptionally(t)
                }
                closed.complete(Unit)
            }
        }

        webSocket = client.newWebSocket(request, listener)

        // Wait for auth or failure.
        connected.await()

        // Subscribe to daemon events.
        send(ClientRequest.Subscribe)
        send(ClientRequest.GetState)

        // Wait until the connection closes.
        closed.await()
    }

    private fun handleRelayMessage(msg: RelayMessage, authDeferred: CompletableDeferred<Unit>) {
        when (msg) {
            is RelayMessage.AuthOk -> {
                Log.d(TAG, "Authenticated (${msg.peer_count} peers)")
                _connectionState.value = State.CONNECTED
                _peerCount.value = msg.peer_count
                if (!authDeferred.isCompleted) {
                    authDeferred.complete(Unit)
                }
            }

            is RelayMessage.AuthError -> {
                Log.e(TAG, "Auth error: ${msg.message}")
                _errors.tryEmit("Auth error: ${msg.message}")
                if (!authDeferred.isCompleted) {
                    authDeferred.completeExceptionally(Exception("Auth error: ${msg.message}"))
                }
            }

            is RelayMessage.Encrypted -> {
                try {
                    val plaintext = cryptoBox.decryptString(msg.ciphertext, msg.nonce)
                    val event = ProtocolGson.gson.fromJson(plaintext, ServerEvent::class.java)
                    _events.tryEmit(event)
                } catch (e: Exception) {
                    Log.e(TAG, "Failed to decrypt/parse message: ${e.message}")
                }
            }

            is RelayMessage.PeerChange -> {
                Log.d(TAG, "Peer count: ${msg.peer_count}")
                _peerCount.value = msg.peer_count
            }

            is RelayMessage.SyncResponse -> {
                for (buffered in msg.messages) {
                    try {
                        val plaintext = cryptoBox.decryptString(
                            buffered.ciphertext,
                            buffered.nonce
                        )
                        val event = ProtocolGson.gson.fromJson(plaintext, ServerEvent::class.java)
                        _events.tryEmit(event)
                    } catch (e: Exception) {
                        Log.e(TAG, "Failed to decrypt buffered message: ${e.message}")
                    }
                }
            }

            else -> {
                Log.d(TAG, "Unhandled relay message: $msg")
            }
        }
    }

    /**
     * Request sync of messages missed while offline.
     */
    fun requestSync(sinceSeq: Long) {
        val ws = webSocket ?: return
        val syncMsg = RelayMessage.Sync(
            channel_id = config.channel_id,
            since_seq = sinceSeq
        )
        val json = ProtocolGson.gson.toJson(syncMsg, RelayMessage::class.java)
        ws.send(json)
    }
}
