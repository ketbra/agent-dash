package com.agentdash.app.ui

import android.widget.Toast
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import com.agentdash.app.ui.chat.ChatScreen
import com.agentdash.app.ui.pair.PairScreen
import com.agentdash.app.ui.sessions.SessionListScreen
import com.agentdash.app.viewmodel.MainViewModel
import kotlinx.coroutines.flow.collectLatest

object Routes {
    const val PAIR = "pair"
    const val SESSIONS = "sessions"
    const val CHAT = "chat/{sessionId}"

    fun chat(sessionId: String) = "chat/$sessionId"
}

@Composable
fun AgentDashNavigation(viewModel: MainViewModel = viewModel()) {
    val navController = rememberNavController()
    val context = LocalContext.current

    val pairingConfig by viewModel.pairingConfig.collectAsState()
    val connectionState by viewModel.connectionState.collectAsState()
    val peerCount by viewModel.peerCount.collectAsState()
    val sessions by viewModel.sessions.collectAsState()
    val chatMessages by viewModel.chatMessages.collectAsState()
    val pendingPermissions by viewModel.pendingPermissions.collectAsState()

    // Show toast messages
    LaunchedEffect(Unit) {
        viewModel.toastMessage.collectLatest { message ->
            Toast.makeText(context, message, Toast.LENGTH_SHORT).show()
        }
    }

    val startDestination = if (pairingConfig != null) Routes.SESSIONS else Routes.PAIR

    NavHost(
        navController = navController,
        startDestination = startDestination
    ) {
        composable(Routes.PAIR) {
            PairScreen(
                pairingConfig = pairingConfig,
                onPairingUri = { uri ->
                    val success = viewModel.handlePairingUri(uri)
                    if (success) {
                        // Stay on pair screen to show key exchange instructions
                    }
                    success
                },
                onConnect = {
                    viewModel.connectToRelay()
                    navController.navigate(Routes.SESSIONS) {
                        popUpTo(Routes.PAIR) { inclusive = true }
                    }
                },
                onUnpair = {
                    viewModel.unpair()
                }
            )
        }

        composable(Routes.SESSIONS) {
            SessionListScreen(
                sessions = sessions,
                pendingPermissions = pendingPermissions,
                connectionState = connectionState,
                peerCount = peerCount,
                onSessionClick = { sessionId ->
                    viewModel.watchSession(sessionId)
                    navController.navigate(Routes.chat(sessionId))
                },
                onRefresh = {
                    viewModel.refreshState()
                },
                onSettingsClick = {
                    navController.navigate(Routes.PAIR)
                },
                onPermissionResponse = { requestId, sessionId, decision ->
                    viewModel.respondToPermission(requestId, sessionId, decision)
                }
            )
        }

        composable(
            Routes.CHAT,
            arguments = listOf(navArgument("sessionId") { type = NavType.StringType })
        ) { backStackEntry ->
            val sessionId = backStackEntry.arguments?.getString("sessionId") ?: return@composable
            val session = sessions.find { it.session_id == sessionId }
            val messages = chatMessages[sessionId] ?: emptyList()
            val sessionPermissions = pendingPermissions.filter { it.sessionId == sessionId }

            ChatScreen(
                sessionId = sessionId,
                session = session,
                messages = messages,
                pendingPermissions = sessionPermissions,
                onBack = {
                    viewModel.unwatchSession()
                    navController.popBackStack()
                },
                onSendPrompt = { text ->
                    viewModel.sendPrompt(sessionId, text)
                },
                onPermissionResponse = { requestId, sid, decision ->
                    viewModel.respondToPermission(requestId, sid, decision)
                }
            )
        }
    }
}
