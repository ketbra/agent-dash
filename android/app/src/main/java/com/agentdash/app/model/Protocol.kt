package com.agentdash.app.model

import com.google.gson.*
import com.google.gson.annotations.SerializedName
import java.lang.reflect.Type

// ---------------------------------------------------------------------------
// Relay messages (phone <-> relay server, over WebSocket)
// ---------------------------------------------------------------------------

sealed class RelayMessage {
    data class Auth(
        val channel_id: String,
        val public_key: String,
        val server_token: String? = null
    ) : RelayMessage()

    data class AuthOk(val peer_count: Int) : RelayMessage()
    data class AuthError(val message: String) : RelayMessage()

    data class Encrypted(
        val channel_id: String,
        val ciphertext: String,
        val nonce: String
    ) : RelayMessage()

    data class PeerChange(
        val channel_id: String,
        val peer_count: Int
    ) : RelayMessage()

    data class Sync(
        val channel_id: String,
        val since_seq: Long
    ) : RelayMessage()

    data class SyncResponse(
        val channel_id: String,
        val messages: List<BufferedMessage>
    ) : RelayMessage()
}

data class BufferedMessage(
    val seq: Long,
    val ciphertext: String,
    val nonce: String,
    val timestamp: Long
)

// ---------------------------------------------------------------------------
// Client requests (phone -> daemon, encrypted through relay)
// ---------------------------------------------------------------------------

sealed class ClientRequest {
    data object Subscribe : ClientRequest()
    data object GetState : ClientRequest()

    data class PermissionResponse(
        val request_id: String,
        val session_id: String,
        val decision: String,
        val suggestion: JsonElement? = null
    ) : ClientRequest()

    data class GetMessages(
        val session_id: String,
        val format: String? = null,
        val limit: Int? = null
    ) : ClientRequest()

    data class WatchSession(
        val session_id: String,
        val format: String? = null
    ) : ClientRequest()

    data class UnwatchSession(
        val session_id: String
    ) : ClientRequest()

    data class SendPrompt(
        val session_id: String,
        val text: String
    ) : ClientRequest()
}

// ---------------------------------------------------------------------------
// Server events (daemon -> phone, encrypted through relay)
// ---------------------------------------------------------------------------

sealed class ServerEvent {
    data class StateUpdate(val sessions: List<DashSession>) : ServerEvent()

    data class PermissionPending(
        val session_id: String,
        val request_id: String,
        val tool: String,
        val detail: String,
        val suggestions: List<JsonElement> = emptyList()
    ) : ServerEvent()

    data class PermissionResolved(
        val request_id: String,
        val resolved_by: String
    ) : ServerEvent()

    data class Messages(
        val session_id: String,
        val messages: List<ChatMessage>
    ) : ServerEvent()

    data class Message(
        val session_id: String,
        val message: ChatMessage
    ) : ServerEvent()

    data class PromptSent(val session_id: String) : ServerEvent()
    data class Error(val message: String) : ServerEvent()
}

// ---------------------------------------------------------------------------
// Session & message types
// ---------------------------------------------------------------------------

data class DashSession(
    val session_id: String,
    val project_name: String,
    val branch: String,
    val status: String,
    val last_status_change: Long,
    val jsonl_path: String? = null,
    val input_reason: DashInputReason? = null,
    val active_tool: DashActiveTool? = null
)

data class DashInputReason(
    val type: String,
    val tool: String? = null,
    val command: String? = null,
    val detail: String? = null,
    val text: String? = null
)

data class DashActiveTool(
    val name: String,
    val detail: String,
    val icon: String
)

data class ChatMessage(
    val role: String,
    val content: ChatContent
)

sealed class ChatContent {
    data class Structured(val blocks: List<ContentBlock>) : ChatContent()
    data class Rendered(val text: String) : ChatContent()
}

sealed class ContentBlock {
    data class Text(val text: String) : ContentBlock()
    data class ToolUse(
        val name: String,
        val detail: String,
        val input: JsonElement? = null
    ) : ContentBlock()

    data class ToolResult(
        val name: String,
        val output: String? = null
    ) : ContentBlock()
}

// ---------------------------------------------------------------------------
// Pairing config (stored locally on phone)
// ---------------------------------------------------------------------------

data class PairingConfig(
    val relay_url: String,
    val channel_id: String,
    val channel_secret_b64: String,
    val daemon_public_key_b64: String,
    val phone_secret_key_b64: String,
    val phone_public_key_b64: String,
    val server_token: String? = null
)

// ---------------------------------------------------------------------------
// Gson serialization adapters
// ---------------------------------------------------------------------------

object ProtocolGson {
    val gson: Gson = GsonBuilder()
        .registerTypeAdapter(RelayMessage::class.java, RelayMessageAdapter())
        .registerTypeAdapter(ClientRequest::class.java, ClientRequestSerializer())
        .registerTypeAdapter(ServerEvent::class.java, ServerEventAdapter())
        .registerTypeAdapter(ChatContent::class.java, ChatContentAdapter())
        .registerTypeAdapter(ContentBlock::class.java, ContentBlockAdapter())
        .create()
}

// -- RelayMessage adapter (tagged by "type") --

class RelayMessageAdapter : JsonDeserializer<RelayMessage>, JsonSerializer<RelayMessage> {
    override fun deserialize(
        json: JsonElement, typeOfT: Type, context: JsonDeserializationContext
    ): RelayMessage {
        val obj = json.asJsonObject
        return when (obj.get("type").asString) {
            "auth" -> RelayMessage.Auth(
                channel_id = obj.get("channel_id").asString,
                public_key = obj.get("public_key").asString,
                server_token = obj.get("server_token")?.takeIf { !it.isJsonNull }?.asString
            )
            "auth_ok" -> RelayMessage.AuthOk(obj.get("peer_count").asInt)
            "auth_error" -> RelayMessage.AuthError(obj.get("message").asString)
            "encrypted" -> RelayMessage.Encrypted(
                channel_id = obj.get("channel_id").asString,
                ciphertext = obj.get("ciphertext").asString,
                nonce = obj.get("nonce").asString
            )
            "peer_change" -> RelayMessage.PeerChange(
                channel_id = obj.get("channel_id").asString,
                peer_count = obj.get("peer_count").asInt
            )
            "sync" -> RelayMessage.Sync(
                channel_id = obj.get("channel_id").asString,
                since_seq = obj.get("since_seq").asLong
            )
            "sync_response" -> RelayMessage.SyncResponse(
                channel_id = obj.get("channel_id").asString,
                messages = context.deserialize(
                    obj.get("messages"),
                    object : com.google.gson.reflect.TypeToken<List<BufferedMessage>>() {}.type
                )
            )
            else -> throw JsonParseException("Unknown relay message type: ${obj.get("type")}")
        }
    }

    override fun serialize(
        src: RelayMessage, typeOfSrc: Type, context: JsonSerializationContext
    ): JsonElement {
        val obj = JsonObject()
        when (src) {
            is RelayMessage.Auth -> {
                obj.addProperty("type", "auth")
                obj.addProperty("channel_id", src.channel_id)
                obj.addProperty("public_key", src.public_key)
                src.server_token?.let { obj.addProperty("server_token", it) }
            }
            is RelayMessage.AuthOk -> {
                obj.addProperty("type", "auth_ok")
                obj.addProperty("peer_count", src.peer_count)
            }
            is RelayMessage.AuthError -> {
                obj.addProperty("type", "auth_error")
                obj.addProperty("message", src.message)
            }
            is RelayMessage.Encrypted -> {
                obj.addProperty("type", "encrypted")
                obj.addProperty("channel_id", src.channel_id)
                obj.addProperty("ciphertext", src.ciphertext)
                obj.addProperty("nonce", src.nonce)
            }
            is RelayMessage.PeerChange -> {
                obj.addProperty("type", "peer_change")
                obj.addProperty("channel_id", src.channel_id)
                obj.addProperty("peer_count", src.peer_count)
            }
            is RelayMessage.Sync -> {
                obj.addProperty("type", "sync")
                obj.addProperty("channel_id", src.channel_id)
                obj.addProperty("since_seq", src.since_seq)
            }
            is RelayMessage.SyncResponse -> {
                obj.addProperty("type", "sync_response")
                obj.addProperty("channel_id", src.channel_id)
                obj.add("messages", context.serialize(src.messages))
            }
        }
        return obj
    }
}

// -- ClientRequest serializer (tagged by "method") --

class ClientRequestSerializer : JsonSerializer<ClientRequest> {
    override fun serialize(
        src: ClientRequest, typeOfSrc: Type, context: JsonSerializationContext
    ): JsonElement {
        val obj = JsonObject()
        when (src) {
            is ClientRequest.Subscribe -> obj.addProperty("method", "subscribe")
            is ClientRequest.GetState -> obj.addProperty("method", "get_state")
            is ClientRequest.PermissionResponse -> {
                obj.addProperty("method", "permission_response")
                obj.addProperty("request_id", src.request_id)
                obj.addProperty("session_id", src.session_id)
                obj.addProperty("decision", src.decision)
                src.suggestion?.let { obj.add("suggestion", it) }
            }
            is ClientRequest.GetMessages -> {
                obj.addProperty("method", "get_messages")
                obj.addProperty("session_id", src.session_id)
                src.format?.let { obj.addProperty("format", it) }
                src.limit?.let { obj.addProperty("limit", it) }
            }
            is ClientRequest.WatchSession -> {
                obj.addProperty("method", "watch_session")
                obj.addProperty("session_id", src.session_id)
                src.format?.let { obj.addProperty("format", it) }
            }
            is ClientRequest.UnwatchSession -> {
                obj.addProperty("method", "unwatch_session")
                obj.addProperty("session_id", src.session_id)
            }
            is ClientRequest.SendPrompt -> {
                obj.addProperty("method", "send_prompt")
                obj.addProperty("session_id", src.session_id)
                obj.addProperty("text", src.text)
            }
        }
        return obj
    }
}

// -- ServerEvent adapter (tagged by "event") --

class ServerEventAdapter : JsonDeserializer<ServerEvent> {
    override fun deserialize(
        json: JsonElement, typeOfT: Type, context: JsonDeserializationContext
    ): ServerEvent {
        val obj = json.asJsonObject
        return when (obj.get("event").asString) {
            "state_update" -> ServerEvent.StateUpdate(
                sessions = context.deserialize(
                    obj.get("sessions"),
                    object : com.google.gson.reflect.TypeToken<List<DashSession>>() {}.type
                )
            )
            "permission_pending" -> ServerEvent.PermissionPending(
                session_id = obj.get("session_id").asString,
                request_id = obj.get("request_id").asString,
                tool = obj.get("tool").asString,
                detail = obj.get("detail").asString,
                suggestions = obj.get("suggestions")?.let {
                    context.deserialize(
                        it,
                        object : com.google.gson.reflect.TypeToken<List<JsonElement>>() {}.type
                    )
                } ?: emptyList()
            )
            "permission_resolved" -> ServerEvent.PermissionResolved(
                request_id = obj.get("request_id").asString,
                resolved_by = obj.get("resolved_by").asString
            )
            "messages" -> ServerEvent.Messages(
                session_id = obj.get("session_id").asString,
                messages = context.deserialize(
                    obj.get("messages"),
                    object : com.google.gson.reflect.TypeToken<List<ChatMessage>>() {}.type
                )
            )
            "message" -> ServerEvent.Message(
                session_id = obj.get("session_id").asString,
                message = context.deserialize(obj.get("message"), ChatMessage::class.java)
            )
            "prompt_sent" -> ServerEvent.PromptSent(
                session_id = obj.get("session_id").asString
            )
            "error" -> ServerEvent.Error(
                message = obj.get("message").asString
            )
            else -> throw JsonParseException("Unknown server event: ${obj.get("event")}")
        }
    }
}

// -- ChatContent adapter (untagged: array = Structured, string = Rendered) --

class ChatContentAdapter : JsonDeserializer<ChatContent> {
    override fun deserialize(
        json: JsonElement, typeOfT: Type, context: JsonDeserializationContext
    ): ChatContent {
        return when {
            json.isJsonArray -> {
                val blocks = json.asJsonArray.map { elem ->
                    context.deserialize<ContentBlock>(elem, ContentBlock::class.java)
                }
                ChatContent.Structured(blocks)
            }
            json.isJsonPrimitive && json.asJsonPrimitive.isString -> {
                ChatContent.Rendered(json.asString)
            }
            else -> ChatContent.Rendered(json.toString())
        }
    }
}

// -- ContentBlock adapter (tagged by "type") --

class ContentBlockAdapter : JsonDeserializer<ContentBlock> {
    override fun deserialize(
        json: JsonElement, typeOfT: Type, context: JsonDeserializationContext
    ): ContentBlock {
        val obj = json.asJsonObject
        return when (obj.get("type").asString) {
            "text" -> ContentBlock.Text(obj.get("text").asString)
            "tool_use" -> ContentBlock.ToolUse(
                name = obj.get("name").asString,
                detail = obj.get("detail").asString,
                input = obj.get("input")
            )
            "tool_result" -> ContentBlock.ToolResult(
                name = obj.get("name").asString,
                output = obj.get("output")?.takeIf { !it.isJsonNull }?.asString
            )
            else -> ContentBlock.Text(text = json.toString())
        }
    }
}
