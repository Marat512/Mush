# MUSH-A'-CLI-SPEC — Rust CLI прототип

**Версия:** 0.1.0  
**Дата:** 2026-09-29  
**Статус:** Draft — промежуточный шаг перед Kotlin Android  
**База:** MUSH-A'-SPEC v0.1.1  
**Цель:** Проверить handshake, CRDT sync, sensors, framing без Android toolchain.

---

## 1. Архитектура CLI

```
┌─────────────────────────────────────────┐
│              UI Layer (TUI)              │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  │
│  │  Chat   │  │Contacts │  │  Alert  │  │
│  │  pane   │  │  pane   │  │  pane   │  │
│  └────┬────┘  └────┬────┘  └────┬────┘  │
├───────┼────────────┼────────────┼───────┤
│       │            │            │       │
│  ┌────┴────────────┴────────────┴────┐  │
│  │         Mush Core (A'-CLI)        │  │
│  │  • Bundle framing (binary)        │  │
│  │  • Symmetric crypto (AES-256-GCM) │  │
│  │  • S1 ContactRateSensor           │  │
│  │  • S7 DecoyEidSensor              │  │
│  │  • InboxBuffer                    │  │
│  │  • CRDT Chat Store                │  │
│  └────────────────────────────────────┘  │
├─────────────────────────────────────────┤
│           Transport Layer                │
│  ┌─────────────────────────────────────┐│
│  │  TCP socket (127.0.0.1:7878)      ││
│  │  (Hotspot — НЕ в CLI)              ││
│  └─────────────────────────────────────┘│
└─────────────────────────────────────────┘
```

---

## 2. Компоненты

### 2.1. Node (одна нода = один процесс)

```rust
pub struct Node {
    pub identity: Identity,          // Ed25519 keypair
    pub contacts: HashMap<PubKey, Contact>,
    pub groups: HashMap<GroupID, AdHocGroup>,
    pub inbox: InboxBuffer,
    pub s1: ContactRateSensor,
    pub s7: DecoyEidSensor,
    pub transport: TcpTransport,
}
```

### 2.2. Запуск

```bash
# Нода 1 (создатель группы)
$ cargo run --bin mush-node -- --name Alice --port 7878
[INFO] Node started: 127.0.0.1:7878
[INFO] Identity: mush://aB3x9K2mNpQ...
[INFO] Invite link: mush://aB3x9K2mNpQ...&nonce=xyz789

# Нода 2 (присоединение)
$ cargo run --bin mush-node -- --name Bob --port 7879 --join mush://aB3x9K2mNpQ...&nonce=xyz789
[INFO] Connecting to 127.0.0.1:7878...
[INFO] Handshake complete. Session key established.
[INFO] Group joined: group-abc123
```

### 2.3. Команды TUI

```
/send <group> <text>     — отправить сообщение в группу
/join <invite-link>      — присоединиться к группе
/contacts                — список контактов
/sync <group>            — синхронизировать группу
/alert                   — показать alert'ы от сенсоров
/decoy                   — отправить на decoy (тест S7)
/flood <n>               — отправить N сообщений подряд (тест S1)
/quit                    — выход
```

---

## 3. Transport: TCP

### 3.1. Вместо Hotspot

| Hotspot (Android) | TCP (CLI) |
|-------------------|-----------|
| WiFi AP "Mush-XXXX" | `127.0.0.1:7878` |
| QR-код с IP+порт | `mush://pubkey&nonce` в stdout |
| DHCP + сканирование SSID | `--join` с IP из DNS/конфига |
| TCP socket через hotspot | TCP socket напрямую |

### 3.2. Bundle framing (идентично Android)

```
[4 bytes: length][1 byte: type][payload: CBOR]
```

Типы:
- `0x01` — text message
- `0x02` — group sync
- `0x03` — contact handshake
- `0x04` — decoy hit notification

---

## 4. Crypto (идентично Android)

- `ed25519-dalek` — identity keys, signatures
- `x25519-dalek` — ephemeral DH
- `aes-gcm` — symmetric encryption
- `hkdf` — key derivation
- `sha2` — hashing

**SNS handshake** — идентичен §3.1 MUSH-A'-SPEC.

---

## 5. CRDT (идентично Android)

- `AdHocGroup` — та же структура
- `ChatMessage` — ULID, Lamport clock, AES-GCM
- Sync protocol — `max_lamport_clock` exchange

---

## 6. Sensors (идентично Android)

### 6.1. S1 — ContactRateSensor

```
/flood 50
[INFO] Sent 50 messages to group-abc123
[WARN] S1 Alert: High rate from Alice (50 messages, median=0)
```

### 6.2. S7 — DecoyEidSensor

```
/decoy
[INFO] Sent bundle to decoy EID: decoy-xyz789
[WARN] S7 Alert: Decoy hit from Alice
```

---

## 7. Критерии приёмки CLI

| # | Критерий | Как проверить |
|---|----------|--------------|
| 1 | Две ноды обмениваются сообщениями | `cargo run` ×2, `/send` → сообщение видно |
| 2 | Handshake через `mush://` строку | Копировать-paste из stdout, `/join` |
| 3 | CRDT sync: 3 ноды, merge | Запустить 3 ноды, проверить идентичность истории |
| 4 | S1 детектирует flood | `/flood 50` → alert в UI |
| 5 | S7 детектирует decoy | `/decoy` → alert в UI |
| 6 | Backpressure: 100+ сообщений | `/flood 150` → старые удалены, буфер ≤ 100 |
| 7 | Нет крашей при 30 мин | `cargo test --release` + monkey commands |

---

## 8. Отличия от Android A'

| Что | CLI | Android |
|-----|-----|---------|
| **Transport** | TCP 127.0.0.1 | Hotspot WiFi AP |
| **Contact exchange** | `mush://` в stdout | QR-код |
| **UI** | TUI (ratatui) / stdin | Jetpack Compose |
| **Process model** | `cargo run` | APK на Android |
| **Storage** | SQLite или JSON файл | SQLCipher |
| **Permissions** | Нет | Android permissions |
| **Crypto libs** | Rust crates (dalek) | Rust crates через JNI или Tink |

**Не меняется:** protocol, framing, crypto, CRDT, sensors, invariants.

---

## 9. Следующий шаг

После прохождения критериев CLI → Kotlin Android Studio:

1. Тот же protocol (binary framing, SNS handshake, CRDT sync).
2. TCP → Hotspot API (`WifiManager.startLocalOnlyHotspot`).
3. `mush://` stdout → QR-код + invite-link.
4. TUI → Jetpack Compose (экраны: Splash, Onboarding, Chats, Chat, Contacts, Alerts).
5. `cargo test` → `JUnit` + `Espresso`.
6. `cargo run` → APK + два телефона.

Контракт: ядро (crypto, CRDT, sensors) — одно и то же. Только transport и UI меняются.