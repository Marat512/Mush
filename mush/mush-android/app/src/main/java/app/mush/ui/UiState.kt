package app.mush.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

// ---- UiState (A'-ANDROID-SPEC §8.2): единый, без JNI-типов ----

enum class NodePhase { Starting, Running, Stopped, Error }

sealed interface ConnectionState {
    data object None : ConnectionState
    data class Hotspot(val ip: String, val port: Int) : ConnectionState
    data class Lan(val ip: String, val port: Int) : ConnectionState
}

data class ChatMessage(
    val id: String,
    val senderKey: String,
    val senderName: String,
    val text: String,
    val ts: Long, // lamport
    val mine: Boolean,
)

data class Contact(
    val pubkey: String,
    val name: String,
    val connected: Boolean,
    /** Доверие из Vault (MUSH-B-SPEC §3.2): unverified | verified |
     *  verification_failed. КРАСНЫЙ индикатор у FAILED, галочка у VERIFIED. */
    val trust: UiTrust = UiTrust.UNVERIFIED,
)

/** UI-модель trust (зеркало Vault TrustLevel без Android-зависимостей слоя). */
enum class UiTrust { UNVERIFIED, VERIFIED, FAILED }

data class AlertItem(
    val id: String,
    val sensor: String, // "S1" / "S7"
    val targetPubkey: String,
    val reason: String,
)

data class UiState(
    val phase: NodePhase = NodePhase.Stopped,
    val statusLine: String = "Node not started",
    val myPubkey: String = "",
    val myName: String = "",
    val invite: String = "",
    val connection: ConnectionState = ConnectionState.None,
    val messages: List<ChatMessage> = emptyList(),
    val contacts: List<Contact> = emptyList(),
    val alerts: List<AlertItem> = emptyList(),
)

/** Команды UI -> сервис. Service присваивает исполнитель в onCreate. */
sealed interface NodeCommand {
    data class SendMessage(val text: String) : NodeCommand
    data class Join(val invite: String) : NodeCommand
    data class DismissAlert(val id: String) : NodeCommand
    /** Этап B (§3.1/§3.2): результат сверки safety number — Verified
     *  («Совпадает») или Failed («Не совпадает», persist в Vault, красный
     *  индикатор). Отправка не блокируется (A'-модель доверия). */
    data class SetTrust(val pubkey: String, val trust: UiTrust) : NodeCommand
    /** Этап B (флаг 4): Onboarding записал имя — поднимаем listener/сеть.
     *  Identity (handle) существует к этому моменту, см. MushService.initNode. */
    data object StartNetworking : NodeCommand
    // Debug hooks (§10, критерии 6/7): триггерят S1/S7 вручную.
    data class DebugFlood(val n: Int) : NodeCommand
    data object DebugDecoy : NodeCommand
}

/** Хранилище состояния: пишет MushService (колбэк Rust), читает UI.
 *  Точка схождения service<->UI; ViewModel — тонкая обёртка для Compose. */
object MushStore {
    private val _state = MutableStateFlow(UiState())
    val state: StateFlow<UiState> = _state.asStateFlow()

    @Volatile
    var commandSink: ((NodeCommand) -> Unit)? = null

    fun update(transform: (UiState) -> UiState) {
        _state.update(transform)
    }

    fun send(cmd: NodeCommand) {
        commandSink?.invoke(cmd)
    }
}

class MushViewModel : ViewModel() {
    val state: StateFlow<UiState> = MushStore.state

    fun send(cmd: NodeCommand) {
        viewModelScope.launch { MushStore.send(cmd) }
    }
}
