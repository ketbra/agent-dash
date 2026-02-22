package com.agentdash.app.ui.sessions

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.agentdash.app.model.DashSession
import com.agentdash.app.network.RelayConnection
import com.agentdash.app.viewmodel.MainViewModel

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SessionListScreen(
    sessions: List<DashSession>,
    pendingPermissions: List<MainViewModel.PendingPermission>,
    connectionState: RelayConnection.State,
    peerCount: Int,
    onSessionClick: (String) -> Unit,
    onRefresh: () -> Unit,
    onSettingsClick: () -> Unit,
    onPermissionResponse: (requestId: String, sessionId: String, decision: String) -> Unit
) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Sessions") },
                colors = TopAppBarDefaults.topAppBarColors(
                    containerColor = MaterialTheme.colorScheme.primaryContainer,
                    titleContentColor = MaterialTheme.colorScheme.onPrimaryContainer
                ),
                actions = {
                    // Connection indicator
                    ConnectionBadge(connectionState, peerCount)

                    IconButton(onClick = onRefresh) {
                        Icon(Icons.Default.Refresh, contentDescription = "Refresh")
                    }
                    IconButton(onClick = onSettingsClick) {
                        Icon(Icons.Default.Settings, contentDescription = "Settings")
                    }
                }
            )
        }
    ) { padding ->
        if (sessions.isEmpty() && connectionState == RelayConnection.State.CONNECTED) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(padding),
                contentAlignment = Alignment.Center
            ) {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(8.dp)
                ) {
                    Icon(
                        Icons.Default.Terminal,
                        contentDescription = null,
                        modifier = Modifier.size(48.dp),
                        tint = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Text(
                        "No active sessions",
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                }
            }
        } else {
            LazyColumn(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(padding),
                contentPadding = PaddingValues(12.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp)
            ) {
                // Show pending permissions that need attention at the top
                val globalPermissions = pendingPermissions.filter { perm ->
                    sessions.none { it.session_id == perm.sessionId }
                }
                if (globalPermissions.isNotEmpty()) {
                    item {
                        Text(
                            "Pending Permissions",
                            style = MaterialTheme.typography.titleSmall,
                            modifier = Modifier.padding(horizontal = 4.dp, vertical = 4.dp)
                        )
                    }
                    items(globalPermissions, key = { it.requestId }) { perm ->
                        PermissionCard(
                            permission = perm,
                            onResponse = { decision ->
                                onPermissionResponse(perm.requestId, perm.sessionId, decision)
                            }
                        )
                    }
                }

                // Sessions
                items(sessions, key = { it.session_id }) { session ->
                    val sessionPermissions = pendingPermissions.filter {
                        it.sessionId == session.session_id
                    }
                    SessionCard(
                        session = session,
                        permissions = sessionPermissions,
                        onClick = { onSessionClick(session.session_id) },
                        onPermissionResponse = { requestId, decision ->
                            onPermissionResponse(requestId, session.session_id, decision)
                        }
                    )
                }

                // Connection status at bottom
                if (connectionState != RelayConnection.State.CONNECTED) {
                    item {
                        ConnectionStatusBar(connectionState)
                    }
                }
            }
        }
    }
}

@Composable
private fun ConnectionBadge(state: RelayConnection.State, peerCount: Int) {
    val (color, icon) = when (state) {
        RelayConnection.State.CONNECTED -> Pair(Color(0xFF4CAF50), Icons.Default.Cloud)
        RelayConnection.State.CONNECTING,
        RelayConnection.State.AUTHENTICATING -> Pair(Color(0xFFFFC107), Icons.Default.CloudSync)
        RelayConnection.State.RECONNECTING -> Pair(Color(0xFFFF9800), Icons.Default.CloudSync)
        RelayConnection.State.DISCONNECTED -> Pair(Color(0xFFF44336), Icons.Default.CloudOff)
    }

    Row(
        verticalAlignment = Alignment.CenterVertically,
        modifier = Modifier.padding(end = 4.dp)
    ) {
        Icon(
            icon,
            contentDescription = state.name,
            tint = color,
            modifier = Modifier.size(20.dp)
        )
        if (state == RelayConnection.State.CONNECTED && peerCount > 0) {
            Text(
                " $peerCount",
                style = MaterialTheme.typography.labelSmall,
                color = color
            )
        }
    }
}

@Composable
private fun ConnectionStatusBar(state: RelayConnection.State) {
    val message = when (state) {
        RelayConnection.State.DISCONNECTED -> "Disconnected from relay"
        RelayConnection.State.CONNECTING -> "Connecting..."
        RelayConnection.State.AUTHENTICATING -> "Authenticating..."
        RelayConnection.State.RECONNECTING -> "Reconnecting..."
        RelayConnection.State.CONNECTED -> "Connected"
    }

    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(
            containerColor = when (state) {
                RelayConnection.State.DISCONNECTED -> MaterialTheme.colorScheme.errorContainer
                else -> MaterialTheme.colorScheme.surfaceVariant
            }
        )
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.Center
        ) {
            if (state != RelayConnection.State.DISCONNECTED) {
                CircularProgressIndicator(
                    modifier = Modifier.size(16.dp),
                    strokeWidth = 2.dp
                )
                Spacer(modifier = Modifier.width(8.dp))
            }
            Text(message, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable
private fun SessionCard(
    session: DashSession,
    permissions: List<MainViewModel.PendingPermission>,
    onClick: () -> Unit,
    onPermissionResponse: (requestId: String, decision: String) -> Unit
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
        colors = CardDefaults.cardColors(
            containerColor = if (session.status == "needs_input") {
                MaterialTheme.colorScheme.tertiaryContainer.copy(alpha = 0.3f)
            } else {
                MaterialTheme.colorScheme.surface
            }
        ),
        elevation = CardDefaults.cardElevation(defaultElevation = 1.dp)
    ) {
        Column(
            modifier = Modifier.padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp)
        ) {
            // Header row: project name + status
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text(
                    text = session.project_name,
                    style = MaterialTheme.typography.titleMedium,
                    modifier = Modifier.weight(1f)
                )
                StatusChip(session.status)
            }

            // Branch + session ID
            Row(
                horizontalArrangement = Arrangement.spacedBy(12.dp)
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(
                        Icons.Default.AccountTree,
                        contentDescription = null,
                        modifier = Modifier.size(14.dp),
                        tint = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                    Spacer(modifier = Modifier.width(4.dp))
                    Text(
                        session.branch,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                }

                Text(
                    session.session_id.take(8),
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                    color = MaterialTheme.colorScheme.onSurfaceVariant
                )
            }

            // Active tool
            session.active_tool?.let { tool ->
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(top = 2.dp)
                ) {
                    Icon(
                        toolIcon(tool.name),
                        contentDescription = null,
                        modifier = Modifier.size(14.dp),
                        tint = MaterialTheme.colorScheme.primary
                    )
                    Spacer(modifier = Modifier.width(6.dp))
                    Text(
                        "${tool.name}: ${tool.detail}",
                        style = MaterialTheme.typography.bodySmall,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        color = MaterialTheme.colorScheme.primary
                    )
                }
            }

            // Inline permission requests
            permissions.forEach { perm ->
                HorizontalDivider(modifier = Modifier.padding(vertical = 4.dp))
                PermissionInline(
                    permission = perm,
                    onResponse = { decision ->
                        onPermissionResponse(perm.requestId, decision)
                    }
                )
            }
        }
    }
}

@Composable
private fun StatusChip(status: String) {
    val (label, color) = when (status) {
        "needs_input" -> Pair("Needs Input", Color(0xFFE65100))
        "working" -> Pair("Working", Color(0xFF2E7D32))
        "idle" -> Pair("Idle", Color(0xFF9E9E9E))
        "ended" -> Pair("Ended", Color(0xFF616161))
        else -> Pair(status, Color(0xFF9E9E9E))
    }

    Surface(
        shape = MaterialTheme.shapes.small,
        color = color.copy(alpha = 0.15f)
    ) {
        Text(
            text = label,
            modifier = Modifier.padding(horizontal = 8.dp, vertical = 2.dp),
            style = MaterialTheme.typography.labelSmall,
            color = color
        )
    }
}

@Composable
private fun PermissionInline(
    permission: MainViewModel.PendingPermission,
    onResponse: (String) -> Unit
) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(
                Icons.Default.Security,
                contentDescription = null,
                modifier = Modifier.size(16.dp),
                tint = MaterialTheme.colorScheme.tertiary
            )
            Spacer(modifier = Modifier.width(6.dp))
            Text(
                "${permission.tool}: ${permission.detail}",
                style = MaterialTheme.typography.bodySmall,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                color = MaterialTheme.colorScheme.tertiary
            )
        }

        Row(
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            FilledTonalButton(
                onClick = { onResponse("allow") },
                modifier = Modifier.weight(1f),
                colors = ButtonDefaults.filledTonalButtonColors(
                    containerColor = Color(0xFF4CAF50).copy(alpha = 0.15f),
                    contentColor = Color(0xFF2E7D32)
                ),
                contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp)
            ) {
                Text("Allow", style = MaterialTheme.typography.labelMedium)
            }

            FilledTonalButton(
                onClick = { onResponse("deny") },
                modifier = Modifier.weight(1f),
                colors = ButtonDefaults.filledTonalButtonColors(
                    containerColor = Color(0xFFF44336).copy(alpha = 0.15f),
                    contentColor = Color(0xFFC62828)
                ),
                contentPadding = PaddingValues(horizontal = 12.dp, vertical = 6.dp)
            ) {
                Text("Deny", style = MaterialTheme.typography.labelMedium)
            }
        }
    }
}

@Composable
private fun PermissionCard(
    permission: MainViewModel.PendingPermission,
    onResponse: (String) -> Unit
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.cardColors(
            containerColor = MaterialTheme.colorScheme.tertiaryContainer.copy(alpha = 0.5f)
        )
    ) {
        Column(
            modifier = Modifier.padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            PermissionInline(permission = permission, onResponse = onResponse)
        }
    }
}

private fun toolIcon(toolName: String) = when (toolName) {
    "Bash" -> Icons.Default.Terminal
    "Read" -> Icons.Default.Description
    "Edit" -> Icons.Default.Edit
    "Write" -> Icons.Default.NoteAdd
    "Grep" -> Icons.Default.Search
    "Glob" -> Icons.Default.FolderOpen
    "WebFetch" -> Icons.Default.Language
    "WebSearch" -> Icons.Default.TravelExplore
    "Task" -> Icons.Default.PlayArrow
    else -> Icons.Default.Build
}
