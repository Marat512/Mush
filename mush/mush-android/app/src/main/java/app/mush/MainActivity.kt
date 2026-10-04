package app.mush

import android.content.Context
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.AccountCircle
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Notifications
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import app.mush.ui.AlertsScreen
import app.mush.ui.ChatsScreen
import app.mush.ui.ChatScreen
import app.mush.ui.ContactsScreen
import app.mush.ui.MushViewModel
import app.mush.ui.NodeCommand
import app.mush.ui.NodePhase
import app.mush.ui.OnboardingScreen
import app.mush.ui.QrScanSheet
import app.mush.ui.QrShowSheet
import app.mush.ui.SplashScreen

/** MUSH A' Android: Splash → Onboarding (первый запуск) → Chats/Contacts/Alerts.
 *  QR — ModalBottomSheet поверх экранов, не route (§8.1). Нода живёт в
 *  [MushService]; этот Activity только запрашивает permissions и стартует
 *  сервис. */
class MainActivity : ComponentActivity() {
    companion object {
        private const val PREFS = "mush_prefs"
        private const val KEY_NAME = "name"
    }

    private val permissionLauncher =
        registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            // Отказ — не блокер: LAN fallback (§10, критерий #3).
            startService()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface {
                    AppNav(
                        hasName = name().isNotBlank(),
                        requestPermissions = {
                            permissionLauncher.launch(Permissions.requestSet())
                        },
                        startService = { startService() },
                        saveName = { name(it) },
                    )
                }
            }
        }
    }

    private fun name(): String =
        getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY_NAME, "") ?: ""

    private fun name(value: String) {
        getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(KEY_NAME, value).apply()
    }

    private fun startService() {
        startForegroundService(Intent(this, MushService::class.java))
    }
}

@Composable
fun AppNav(
    hasName: Boolean,
    requestPermissions: () -> Unit,
    startService: () -> Unit,
    saveName: (String) -> Unit,
) {
    val nav = rememberNavController()
    var showQr by rememberSaveable { mutableStateOf(false) }
    var scanQr by rememberSaveable { mutableStateOf(false) }
    val viewModel: MushViewModel = viewModel()

    // Автостарт ноды при входе в main-экраны: сервис не переживает
    // force-stop/reinstall, иначе UI показывал бы "Node not started" вечно.
    val autoStart: () -> Unit = {
        if (viewModel.state.value.phase == NodePhase.Stopped) startService()
    }

    NavHost(navController = nav, startDestination = "splash") {
        composable("splash") {
            SplashScreen(onTimeout = {
                nav.navigate(if (hasName) "chats" else "onboarding") {
                    popUpTo("splash") { inclusive = true }
                }
            })
        }
        composable("onboarding") {
            OnboardingScreen(onDone = { name ->
                saveName(name)
                requestPermissions()
                nav.navigate("chats") { popUpTo("onboarding") { inclusive = true } }
            })
        }
        composable("chats") { androidx.compose.runtime.LaunchedEffect(Unit) { autoStart() }; MainScaffold(viewModel, nav, { showQr = true }, { scanQr = true }) }
        composable("chat") {
            ChatScreen(viewModel, onBack = { nav.popBackStack() })
        }
        composable("contacts") { MainScaffold(viewModel, nav, { showQr = true }, { scanQr = true }, tab = "contacts") }
        composable("alerts") { MainScaffold(viewModel, nav, { showQr = true }, { scanQr = true }, tab = "alerts") }
    }

    if (showQr) {
        QrShowSheet(invite = viewModel.state.value.invite.ifEmpty { "mush://pending" }, onDismiss = { showQr = false })
    }
    if (scanQr) {
        QrScanSheet(
            onInvite = { invite ->
                scanQr = false
                viewModel.send(NodeCommand.Join(invite))
                nav.navigate("chat")
            },
            onDismiss = { scanQr = false },
        )
    }
}

@Composable
fun MainScaffold(
    viewModel: MushViewModel,
    nav: androidx.navigation.NavHostController,
    onShowQr: () -> Unit,
    onScanQr: () -> Unit,
    tab: String = "chats",
) {
    val backStack by nav.currentBackStackEntryAsState()
    val current = backStack?.destination?.route ?: tab

    Scaffold(
        bottomBar = {
            NavigationBar {
                NavigationBarItem(
                    selected = current == "chats",
                    onClick = { nav.navigate("chats") },
                    icon = { Icon(Icons.Filled.Home, contentDescription = null) },
                    label = { Text("Chats") },
                )
                NavigationBarItem(
                    selected = current == "contacts",
                    onClick = { nav.navigate("contacts") },
                    icon = { Icon(Icons.Filled.AccountCircle, contentDescription = null) },
                    label = { Text("Contacts") },
                )
                NavigationBarItem(
                    selected = current == "alerts",
                    onClick = { nav.navigate("alerts") },
                    icon = { Icon(Icons.Filled.Notifications, contentDescription = null) },
                    label = { Text("Alerts") },
                )
            }
        },
    ) { padding ->
        androidx.compose.foundation.layout.Box(Modifier.padding(padding)) {
            when (tab) {
                "contacts" -> ContactsScreen(viewModel, onShowQr = onShowQr)
                "alerts" -> AlertsScreen(viewModel)
                else -> ChatsScreen(
                    viewModel,
                    onOpenChat = { nav.navigate("chat") },
                    onScanQr = onScanQr,
                )
            }
        }
    }
}
