package com.agentdash.app

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import com.agentdash.app.ui.AgentDashNavigation
import com.agentdash.app.ui.theme.AgentDashTheme
import com.agentdash.app.viewmodel.MainViewModel

class MainActivity : ComponentActivity() {
    private val viewModel: MainViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        // Handle deep link if launched via agentdash:// URI
        handleDeepLink(intent)

        setContent {
            AgentDashTheme {
                AgentDashNavigation(viewModel = viewModel)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleDeepLink(intent)
    }

    private fun handleDeepLink(intent: Intent?) {
        val uri = intent?.data ?: return
        if (uri.scheme == "agentdash" && uri.host == "pair") {
            viewModel.handlePairingUri(uri.toString())
        }
    }
}
