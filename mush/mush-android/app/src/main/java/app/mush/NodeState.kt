package app.mush

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** Состояние ноды, разделяемое между MushService (пишет) и UI (читает).
 * Колбэк Rust живёт в сервисе; сюда он кладёт только уже агрегированные
 * строки — Activity никогда не держит ссылки на нативный handle. */
object NodeState {
    data class State(
        val running: Boolean = false,
        val status: String = "Node not started",
        val pubkey: String = "",
        val port: Int = 0,
        val invite: String = "",
        val events: List<String> = emptyList(),
    )

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    private fun update(transform: (State) -> State) {
        _state.value = transform(_state.value)
    }

    fun started(pubkey: String, port: Int, invite: String) = update {
        it.copy(running = true, status = "Node started on port $port", pubkey = pubkey, port = port, invite = invite)
    }.also { android.util.Log.d("mush-ui", "started port=$port") }

    fun stopped(reason: String) = update {
        it.copy(running = false, status = reason, port = 0)
    }.also { android.util.Log.d("mush-ui", "stopped: $reason") }

    fun event(line: String) = update { it.copy(events = (it.events + line).takeLast(20)) }
        .also { android.util.Log.d("mush-ui", "event: $line") }
}
