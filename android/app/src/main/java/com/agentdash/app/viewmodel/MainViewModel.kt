package com.agentdash.app.viewmodel

import android.app.Application
import android.content.Context
import android.net.Uri
import android.util.Log
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.agentdash.app.crypto.CryptoBox
import com.agentdash.app.model.*
import com.agentdash.app.network.RelayConnection
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.launch

class MainViewModel(application: Application) : AndroidViewModel(application) {

    companion object {
        private const val TAG = "MainViewModel"
        private const val PREFS_NAME = "agent_dash_config"
        private const val KEY_CONFIG = "pairing_config"
    }

    // -- Pairing state --

    private val _pairingConfig = MutableStateFlow<PairingConfig?>(null)
    val pairingConfig: StateFlow<PairingConfig?> = _pairingConfig.asStateFlow()

    val isPaired: Boolean get() = _pairingConfig.value != null

    // -- Connection --

    private var relayConnection: RelayConnection? = null

    private val _connectionState = MutableStateFlow(RelayConnection.State.DISCONNECTED)
    val connectionState: StateFlow<RelayConnection.State> = _connectionState.asStateFlow()

    private val _peerCount = MutableStateFlow(0)
    val peerCount: StateFlow<Int> = _peerCount.asStateFlow()

    // -- Sessions --

    private val _sessions = MutableStateFlow<List<DashSession>>(emptyList())
    val sessions: StateFlow<List<DashSession>> = _sessions.asStateFlow()

    // -- Chat messages per session --

    private val _chatMessages = MutableStateFlow<Map<String, List<ChatMessage>>>(emptyMap())
    val chatMessages: StateFlow<Map<String, List<ChatMessage>>> = _chatMessages.asStateFlow()

    // -- Pending permissions --

    data class PendingPermission(
        val sessionId: String,
        val requestId: String,
        val tool: String,
        val detail: String,
        val suggestions: List<com.google.gson.JsonElement>
    )

    private val _pendingPermissions = MutableStateFlow<List<PendingPermission>>(emptyList())
    val pendingPermissions: StateFlow<List<PendingPermission>> = _pendingPermissions.asStateFlow()

    // -- Currently watched session --

    private val _watchedSessionId = MutableStateFlow<String?>(null)
    val watchedSessionId: StateFlow<String?> = _watchedSessionId.asStateFlow()

    // -- Error messages --

    private val _toastMessage = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val toastMessage: SharedFlow<String> = _toastMessage.asSharedFlow()

    init {
        loadConfig()
        if (_pairingConfig.value != null) {
            connectToRelay()
        }
    }

    // -----------------------------------------------------------------------
    // Pairing
    // -----------------------------------------------------------------------

    /**
     * Parse an agentdash://pair URI and generate keys.
     * URI format: agentdash://pair?relay=<url>&secret=<b64>&pk=<b64>[&token=<token>]
     */
    fun handlePairingUri(uriString: String): Boolean {
        try {
            val uri = Uri.parse(uriString)
            if (uri.scheme != "agentdash" || uri.host != "pair") {
                _toastMessage.tryEmit("Invalid pairing URI")
                return false
            }

            val relayUrl = uri.getQueryParameter("relay")
                ?: throw IllegalArgumentException("Missing relay URL")
            val channelSecretB64 = uri.getQueryParameter("secret")
                ?: throw IllegalArgumentException("Missing channel secret")
            val daemonPkB64 = uri.getQueryParameter("pk")
                ?: throw IllegalArgumentException("Missing daemon public key")
            val serverToken = uri.getQueryParameter("token")

            // Derive channel_id from channel_secret.
            val channelSecret = CryptoBox.decodeBase64(channelSecretB64)
            val channelId = CryptoBox.deriveChannelId(channelSecret)

            // Generate phone's X25519 keypair.
            val (secretKey, publicKey) = CryptoBox.generateKeypair()

            val config = PairingConfig(
                relay_url = relayUrl,
                channel_id = channelId,
                channel_secret_b64 = channelSecretB64,
                daemon_public_key_b64 = daemonPkB64,
                phone_secret_key_b64 = CryptoBox.encodeBase64(secretKey),
                phone_public_key_b64 = CryptoBox.encodeBase64(publicKey),
                server_token = serverToken
            )

            _pairingConfig.value = config
            saveConfig(config)
            Log.d(TAG, "Pairing config saved. Phone public key: ${config.phone_public_key_b64}")

            return true
        } catch (e: Exception) {
            Log.e(TAG, "Pairing failed: ${e.message}")
            _toastMessage.tryEmit("Pairing failed: ${e.message}")
            return false
        }
    }

    fun unpair() {
        relayConnection?.disconnect()
        relayConnection = null
        _pairingConfig.value = null
        _sessions.value = emptyList()
        _chatMessages.value = emptyMap()
        _pendingPermissions.value = emptyList()
        _connectionState.value = RelayConnection.State.DISCONNECTED
        _peerCount.value = 0

        val prefs = getApplication<Application>()
            .getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        prefs.edit().remove(KEY_CONFIG).apply()
    }

    // -----------------------------------------------------------------------
    // Connection
    // -----------------------------------------------------------------------

    fun connectToRelay() {
        val config = _pairingConfig.value ?: return

        relayConnection?.disconnect()
        val conn = RelayConnection(config, viewModelScope)
        relayConnection = conn

        // Collect connection state.
        viewModelScope.launch {
            conn.connectionState.collect { state ->
                _connectionState.value = state
            }
        }

        // Collect peer count.
        viewModelScope.launch {
            conn.peerCount.collect { count ->
                _peerCount.value = count
            }
        }

        // Collect errors.
        viewModelScope.launch {
            conn.errors.collect { error ->
                _toastMessage.tryEmit(error)
            }
        }

        // Collect events and update state.
        viewModelScope.launch {
            conn.events.collect { event ->
                handleServerEvent(event)
            }
        }

        conn.connect()
    }

    fun disconnectRelay() {
        relayConnection?.disconnect()
        _connectionState.value = RelayConnection.State.DISCONNECTED
    }

    // -----------------------------------------------------------------------
    // Session / chat operations
    // -----------------------------------------------------------------------

    fun watchSession(sessionId: String) {
        // Unwatch previous session.
        _watchedSessionId.value?.let { prevId ->
            relayConnection?.send(ClientRequest.UnwatchSession(prevId))
        }

        _watchedSessionId.value = sessionId

        // Request recent messages.
        relayConnection?.send(
            ClientRequest.GetMessages(
                session_id = sessionId,
                format = "markdown",
                limit = 50
            )
        )

        // Watch for new messages.
        relayConnection?.send(
            ClientRequest.WatchSession(
                session_id = sessionId,
                format = "markdown"
            )
        )
    }

    fun unwatchSession() {
        _watchedSessionId.value?.let { sessionId ->
            relayConnection?.send(ClientRequest.UnwatchSession(sessionId))
        }
        _watchedSessionId.value = null
    }

    fun sendPrompt(sessionId: String, text: String) {
        relayConnection?.send(
            ClientRequest.SendPrompt(
                session_id = sessionId,
                text = text
            )
        )
    }

    fun respondToPermission(requestId: String, sessionId: String, decision: String) {
        relayConnection?.send(
            ClientRequest.PermissionResponse(
                request_id = requestId,
                session_id = sessionId,
                decision = decision
            )
        )

        // Optimistically remove the pending permission.
        _pendingPermissions.update { perms ->
            perms.filter { it.requestId != requestId }
        }
    }

    fun refreshState() {
        relayConnection?.send(ClientRequest.GetState)
    }

    // -----------------------------------------------------------------------
    // Event handling
    // -----------------------------------------------------------------------

    private fun handleServerEvent(event: ServerEvent) {
        when (event) {
            is ServerEvent.StateUpdate -> {
                _sessions.value = event.sessions.sortedWith(
                    compareBy<DashSession> { statusSortKey(it.status) }
                        .thenByDescending { it.last_status_change }
                )
            }

            is ServerEvent.PermissionPending -> {
                _pendingPermissions.update { perms ->
                    // Avoid duplicates.
                    if (perms.any { it.requestId == event.request_id }) {
                        perms
                    } else {
                        perms + PendingPermission(
                            sessionId = event.session_id,
                            requestId = event.request_id,
                            tool = event.tool,
                            detail = event.detail,
                            suggestions = event.suggestions
                        )
                    }
                }
            }

            is ServerEvent.PermissionResolved -> {
                _pendingPermissions.update { perms ->
                    perms.filter { it.requestId != event.request_id }
                }
            }

            is ServerEvent.Messages -> {
                _chatMessages.update { map ->
                    map + (event.session_id to event.messages)
                }
            }

            is ServerEvent.Message -> {
                _chatMessages.update { map ->
                    val existing = map[event.session_id] ?: emptyList()
                    map + (event.session_id to existing + event.message)
                }
            }

            is ServerEvent.PromptSent -> {
                Log.d(TAG, "Prompt sent to ${event.session_id}")
            }

            is ServerEvent.Error -> {
                Log.e(TAG, "Server error: ${event.message}")
                _toastMessage.tryEmit(event.message)
            }
        }
    }

    private fun statusSortKey(status: String): Int = when (status) {
        "needs_input" -> 0
        "working" -> 1
        "idle" -> 2
        "ended" -> 3
        else -> 4
    }

    // -----------------------------------------------------------------------
    // Config persistence
    // -----------------------------------------------------------------------

    private fun loadConfig() {
        val prefs = getApplication<Application>()
            .getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val json = prefs.getString(KEY_CONFIG, null)
        if (json != null) {
            try {
                _pairingConfig.value = ProtocolGson.gson.fromJson(json, PairingConfig::class.java)
            } catch (e: Exception) {
                Log.e(TAG, "Failed to load config: ${e.message}")
            }
        }
    }

    private fun saveConfig(config: PairingConfig) {
        val prefs = getApplication<Application>()
            .getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val json = ProtocolGson.gson.toJson(config)
        prefs.edit().putString(KEY_CONFIG, json).apply()
    }

    override fun onCleared() {
        super.onCleared()
        relayConnection?.disconnect()
    }
}
