# MUSH-BRIDGE-ARCH — справочник по адаптерам сторонних сетей

**Версия:** 0.1.0  
**Дата:** 2026-09-29  
**Статус:** Справочник — архитектурный reference для B-статуса (подключение адаптеров). Не входит в основной роадмап.  
**Главный принцип:** Mush — первичен. Адаптеры — опциональны, изолированы, выключены по умолчанию. MyceRus не знает об их существовании.

**Когда используется:** когда Mush достиг стадии зрелости (статус B) и пользователь решает подключить сторонние мессенджеры для удобства. Не для A', не для MVP.

---

## 1. Архитектурный инвариант

```
              зависит от            не зависит от
    MUSH ──────────────────▶ MyceRus ──✗─▶ MUSH
         
    Adapter ──IPC──▶ MUSH ──▶ MyceRus
         
    Adapter ──✗─▶ MyceRus (напрямую недоступен)
    Adapter ──✗─▶ Mush Vault (ключи недоступны)
    Adapter ──✗─▶ Mush Contacts (социальный граф недоступен)
```

| Уровень | Что видит | Что НЕ видит |
|---------|-----------|-------------|
| **MyceRus** | Bundle (source EID, destination EID, payload) | Откуда пришёл payload, что внутри, есть ли адаптеры |
| **Mush Core** | UnifiedMessage, контакты, ключи | Сторонние сети (Telegram, Signal и т.д.) |
| **Adapter** | ForeignMessage (отправить/получить), свои credentials | Mush Vault, Mush Contacts, MyceRus bundle'ы, другие адаптеры |

---

## 2. Adapter API — push-based, без BPv7

### 2.1. Trait

```rust
/// Адаптер сторонней сети. Живёт в отдельном процессе, общается с Mush Core через IPC.
pub trait NetworkAdapter {
    /// Имя сети: "telegram", "signal", "whatsapp", "matrix", "email"...
    fn name(&self) -> &'static str;
    
    /// Тип сети по доверию
    fn trust_level(&self) -> TrustLevel;
    
    /// Аутентификация в сторонней сети
    /// Telegram: token + phone verification
    /// Signal: linking device через QR
    /// WhatsApp: pairing через WhatsApp Web QR
    fn authenticate(&mut self, creds: Credentials) -> Result<Session>;
    
    /// Push-based входящие: регистрируем callback, адаптер вызывает при получении сообщения
    fn on_incoming(&mut self, cb: Box<dyn Fn(ForeignMessage)>);
    
    /// Отправить сообщение в стороннюю сеть
    fn send(&mut self, msg: &ForeignMessage) -> Result<MsgId>;
    
    /// Статус подключения
    fn status(&self) -> AdapterStatus;
    
    /// Graceful shutdown: закрыть сессии, очистить кэши
    fn shutdown(&mut self);
}

pub enum TrustLevel {
    Trusted,      // Mush-to-Mush через MyceRus
    Untrusted,    // Сторонняя сеть (Telegram, Signal и т.д.)
    Mixed,        // Мост Mush ↔ сторонняя сеть (если будет)
}

pub struct ForeignMessage {
    pub network: String,        // "telegram", "signal"...
    pub native_id: String,      // ID сообщения в сторонней сети
    pub sender: ForeignSender,  // кто отправил (в формате сторонней сети)
    pub recipients: Vec<ForeignRecipient>,
    pub content: ForeignContent, // текст, файл, голос...
    pub timestamp: Timestamp,   // wall-clock (сторонней сети)
    pub metadata: ForeignMetadata, // всё, что даёт сторонняя сеть
}

pub struct ForeignSender {
    pub native_id: String,      // Telegram user_id, Signal uuid...
    pub display_name: String,   // имя в сторонней сети
    pub handle: Option<String>, // @username, +phone...
}
```

### 2.2. Чего НЕТ в Adapter

| Чего нет | Почему |
|----------|--------|
| **BPv7 Bundle** | Конвертация ForeignMessage ↔ UnifiedMessage — работа Mush Core |
| **Шифрование Mush** | Adapter использует только криптографию сторонней сети (MTProto, Signal Protocol) |
| **Доступ к Vault** | Adapter не имеет доступа к Mush identity keys |
| **Доступ к Contacts** | Adapter не видит Mush-социальный граф |
| **Poll API** | Только push/callback. Polling = бан API + пропущенные сообщения |

---

## 3. IPC-архитектура: отдельные процессы

### 3.1. Почему не Cargo features

Cargo features компилируют всё в один бинарник:
- Компрометация одного адаптера = компрометация всего Mush.
- `unsafe`-код в одной библиотеке → доступ ко всей памяти.
- Нет way изолировать права: WhatsApp adapter может читать память Signal adapter.

### 3.2. Правильно: процессы + IPC

```
┌─────────────────────────────────────────────┐
│                  Mush Core                    │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐   │
│  │  Vault   │  │ Contacts │  │ Unified  │   │
│  │ (keys)   │  │ (graph)  │  │ Inbox    │   │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘   │
│       │             │             │          │
│  ┌────┴─────────────┴─────────────┴────┐    │
│  │         MuShOrchestrator            │    │
│  │  (маршрутизация, dedup, fallback)   │    │
│  └────┬──────────────────────────┬─────┘    │
│       │          IPC             │          │
│       ▼                          ▼          │
│  ┌──────────┐              ┌──────────┐     │
│  │  UNIX    │              │  UNIX    │     │
│  │  socket  │              │  socket  │     │
│  └────┬─────┘              └────┬─────┘     │
└───────┼─────────────────────────┼───────────┘
        │                         │
┌───────┴──────────┐    ┌─────────┴──────────┐
│  bridge-telegram │    │   bridge-signal    │
│  (отдельный      │    │   (отдельный       │
│   процесс)       │    │    процесс)        │
│                  │    │                    │
│  • teloxide/tdlib│    │  • libsignal       │
│  • MTProto       │    │  • Signal Protocol │
│  • НЕ знает про  │    │  • НЕ знает про    │
│    Mush Vault    │    │    Mush Vault      │
└──────────────────┘    └────────────────────┘
```

### 3.3. IPC протокол

```rust
/// IPC между MuShOrchestrator и Adapter
pub enum AdapterRequest {
    Send(ForeignMessage),
    GetStatus,
    Shutdown,
}

pub enum AdapterResponse {
    Sent(MsgId),
    Received(ForeignMessage),
    Status(AdapterStatus),
    Error(String),
}

/// Adapter → MuShOrchestrator (push)
pub enum AdapterEvent {
    Incoming(ForeignMessage),
    StatusChanged(AdapterStatus),
    Error { code: u32, message: String },
}
```

**Транспорт IPC:**
- **UNIX socket** (Linux/Android) — быстро, надёжно, стандартно.
- **Named pipes** (Windows) — аналог.
- **gRPC/JSON-RPC** — если нужна совместимость с другими языками (Python bridge для WhatsApp Web).

### 3.4. Sandbox процессов

| Ограничение | Как |
|-------------|-----|
| **No filesystem access** | Adapter работает в chroot с tmpfs, нет доступа к Vault |
| **No network except target** | seccomp: разрешены только исходящие соединения к серверам Telegram/Signal |
| **Memory limits** | cgroups: max 256MB RAM per adapter |
| **No introspection** | Adapter не может ptrace Mush Core |
| **Credential isolation** | Credentials сторонней сети хранятся в keyring адаптера, не в Mush Vault |

---

## 4. Mush перенаправляет — не оркестрирует

**Точное определение:** Mush — единая точка входа. Пользователь пишет сообщение в одном UI. Mush решает, через какой канал его отправить. Это **перенаправление** (routing), не **оркестрация** (orchestration).

**Различие:**
- **Оркестрация** — активное управление жизненным циклом, state-машины, retry-логика, coordination между компонентами.
- **Перенаправление** — пассивное: «сообщение для Васи, Вася доступен через Telegram → отправить через Telegram-адаптер». Никакого active coordination.

### 4.1. MuShOrchestrator — маршрутизация и dedup

### 4.1. UnifiedMessage

```rust
/// Внутренний формат Mush. Adapter не видит это.
pub struct UnifiedMessage {
    pub id: MsgId,                    // ULID
    pub source: MessageSource,        // Mush | Adapter(name)
    pub destination: Destination,     // Contact | Group
    pub content: Content,             // текст, файл, голос...
    pub encryption: EncryptionType,   // None | Symmetric | Ratchet
    pub dedup_key: Hash,              // hash(content + timestamp_approx + sender)
    pub delivery_attempts: Vec<DeliveryAttempt>,
}

pub enum MessageSource {
    Mush { sender: PubKey },           // от Mush-контакта через MyceRus
    Adapter { network: String, native_id: String }, // от сторонней сети
}

pub struct DeliveryAttempt {
    pub channel: Channel,              // MyceRus | Adapter(name)
    pub status: DeliveryStatus,        // Pending | Sent | Failed | Delivered
    pub timestamp: Timestamp,
}

pub enum Channel {
    MyceRus,                           // DTN, offline-capable
    Adapter(String),                   // "telegram", "signal"...
}
```

### 4.2. Dedup-стратегия для гибридной доставки (Mode 3)

**Проблема:** сообщение ушло через Signal, связь пропала, Mush не знает дошло ли, отправляет через MyceRus, получатель получает два сообщения.

**Решение:**

```rust
pub struct HybridDelivery {
    pub primary: Channel,              // предпочтительный канал (быстрый, но ненадёжный)
    pub fallback: Channel,             // резервный (MyceRus, медленный, но надёжный)
    pub dedup_key: Hash,               // hash(content + timestamp_approx + sender_pubkey)
    pub primary_ttl: Duration,         // сколько ждать ACK от primary
    pub fallback_trigger: FallbackTrigger,
}

pub enum FallbackTrigger {
    OnAckTimeout(Duration),            // запустить fallback если нет ACK за N минут
    OnExplicitFailure,                 // запустить fallback при явной ошибке primary
    Manual,                            // пользователь выбирает
}
```

**Процесс:**
1. MuShOrchestrator отправляет через primary (Signal).
2. Ждёт ACK `primary_ttl` (например, 5 минут).
3. Если ACK нет — отправляет через fallback (MyceRus) с тем же `dedup_key`.
4. Получатель: видит сообщение с `dedup_key`, проверяет — уже видел? → отбрасывает дубль.
5. Получатель отправляет ACK на оба канала (если оба дошли).

**Dedup key:**
```rust
fn compute_dedup_key(content: &[u8], timestamp: Timestamp, sender: &PubKey) -> Hash {
    // Округляем timestamp до 5-минутного интервала — tolerance для clock skew
    let ts_bucket = timestamp / 300_000; // 5 min in millis
    blake3::hash(&[content, &ts_bucket.to_le_bytes(), sender.as_bytes()].concat())
}
```

### 4.3. Маршрутизация

```rust
impl MuShOrchestrator {
    pub fn route(&mut self, msg: &UnifiedMessage) -> Vec<Channel> {
        match msg.destination {
            Destination::MuShContact(_) => {
                // Проверяем, доступен ли контакт через MyceRus (offline)
                if self.myceRus.can_route(&msg.destination) {
                    vec![Channel::MyceRus]
                } else {
                    // Нет пути через DTN — спрашиваем пользователя или используем fallback
                    vec![] // пользователь выбирает адаптер вручную
                }
            }
            Destination::ForeignContact { network, .. } => {
                // Контакт только в сторонней сети
                vec![Channel::Adapter(network)]
            }
            Destination::HybridContact { mush_path, foreign_path } => {
                // Контакт доступен и так, и так
                vec![
                    Channel::Adapter(foreign_path.network), // primary (быстрый)
                    Channel::MyceRus,                       // fallback (надёжный)
                ]
            }
        }
    }
}
```

---

## 5. UI-разделение и политика включения

### 5.1. Визуальное разделение

```
┌──────────────────────────────────┐
│  🔒 Mush (MyceRus)               │  ← E2E, offline, без сервера
│  ├─ Алиса [✓✓]                   │     Зелёный индикатор = trusted
│  ├─ Боб [⏳]                     │
│  └─ Поход [ad-hoc]               │
├──────────────────────────────────┤
│  🌐 Адаптеры (требуют интернет)  │  ← Другая модель угроз
│  ├─ Telegram: Вася [✓]           │     Жёлтый индикатор = untrusted
│  ├─ Signal: Петя [✓]             │
│  └─ WhatsApp: Мама [✓]           │
└──────────────────────────────────┘
```

### 5.2. Политика включения

| Правило | Почему |
|---------|--------|
| **Адаптеры выключены по умолчанию** | Mush-first, не Beeper с DTN |
| **Явное предупреждение при включении** | «Вы теряете модель угроз Mush: метаданные уходят на серверы Telegram» |
| **Нет автоматической синхронизации контактов** | Социальный граф Mush ≠ социальный граф Telegram |
| **Нельзя отправить Mush-сообщение через адаптер без явного действия** | Защита от случайной утечки |
| **Адаптер видет только свои контакты** | WhatsApp adapter не знает про Signal контакты |

### 5.3. Предупреждение при первом включении

```
┌─────────────────────────────────────────┐
│  ⚠️ Внимание                             │
│                                          │
│  Включение Telegram-адаптера означает:   │
│  • Ваши сообщения проходят через серверы │
│    Telegram                               │
│  • Telegram знает, кто с кем общается    │
│  • Это НЕ E2E в смысле Mush              │
│                                          │
│  Mush-контакты остаются защищёнными.     │
│                                          │
│  [Включить]  [Отмена]                    │
└─────────────────────────────────────────┘
```

---

## 6. Безопасность адаптеров

### 6.1. Анонимитет и социальный граф

**Проблема:** если у пользователя есть Mush-контакт А и Telegram-контакт Б, и оба адаптера в одном приложении — при компрометации одного адаптера атакующий видит весь социальный граф.

**Решения:**
1. **Процессная изоляция** — адаптер не имеет доступа к памяти Mush Core.
2. **Контактная изоляция** — адаптер знает только свои контакты, не Mush Contacts.
3. **Нет shared state** — адаптер не может прочитать `UnifiedMessage.id` или `dedup_key` другого адаптера.

### 6.2. Юридические риски

| Сеть | Риск | Митигация |
|------|------|-----------|
| **WhatsApp** | Meta банит неофициальные клиенты | Адаптер — отдельный процесс, легко отключить; нет reverse engineering API |
| **Signal** | Signal Foundation враждебна к форкам | Используем официальный libsignal, не форкаем |
| **Telegram** | TDLib — LGPL, требует раскрытия модификаций | Используем как есть, не модифицируем |
| **Apple iMessage** | Закрытый протокол, юридические барьеры | Не поддерживаем (пока) |

### 6.3. Maintenance

| Риск | Митигация |
|------|-----------|
| Протоколы меняются, ломают адаптер | Адаптер — отдельный процесс, можно обновить независимо от Mush |
| Уязвимость в библиотеке адаптера | Sandbox: network-only, no filesystem, memory limit |
| Адаптер умирает (сеть закрыла API) | Mush Core не зависит от адаптера, удаляем — работаем дальше |

---

## 7. Backlog для B-статуса (не в MVP, зафиксировано для перехода)

### 7.1. Outbound API: typed methods, не ForeignMessage

Сейчас в Adapter:
```rust
fn send(&mut self, msg: &ForeignMessage) -> Result<MsgId>;
```

**Проблема:** Mush Core знает формат ForeignMessage — это просачивание стороннего протокола.

**Решение для B:**
```rust
fn send_text(&mut self, recipient: &ForeignRecipient, text: &str) -> Result<MsgId>;
fn send_media(&mut self, recipient: &ForeignRecipient, media: MediaAttachment) -> Result<MsgId>;
fn reply_to(&mut self, msg_id: &MsgId, text: &str) -> Result<MsgId>;
```

Mush Core не знает про ForeignMessage — он передаёт typed args. Adapter сам упаковывает в native format.

### 7.2. Dedup: через nonce, не через хэш контента

Сейчас:
```rust
dedup_key = hash(content + timestamp_bucket + sender)
```

**Проблема:** два «ок» подряд в пределах 5 минут → второе отброшено.

**Решение для B:** каждое исходящее сообщение получает `nonce: [u8; 16]` от Mush Core. Dedup по `nonce`, не по content. Не зависит от содержимого.

### 7.3. iOS: без bridges

**Платформенное ограничение:**
- iOS — один процесс, App Store заблокирует linked-device паттерн.
- UNIX sockets между процессами запрещены sandbox'ом.
- Background execution жёстко лимитирован.

**Решение для B:**
- iOS-версия Mush — без адаптеров вообще (дефолт).
- Или: адаптеры внутри одного процесса (компромисс безопасности, но принудительный).
- Или: WebView с ограниченными правами (если App Store разрешит — не гарантировано).
- Явно прописать в документации: «Android — полная изоляция процессов, iOS — ограниченная или невозможна».

### 7.4. AdapterEvent::Crashed + retry policy

Сейчас:
```rust
pub enum AdapterEvent {
    Incoming(ForeignMessage),
    StatusChanged(AdapterStatus),
    Error { code: u32, message: String },
}
```

**Проблема:** падение bridge'а оставляет сообщения в `Pending` навсегда.

**Решение для B:**
```rust
pub enum AdapterEvent {
    Incoming(ForeignMessage),
    StatusChanged(AdapterStatus),
    Error { code: u32, message: String },
    Crashed { reason: String, backtrace: Option<String> },
}

pub struct RetryPolicy {
    pub max_attempts: u32,       // например, 3
    pub backoff: Duration,       // экспоненциальный (1s, 2s, 4s)
    pub fallback_channel: Option<Channel>, // fallback на MyceRus после исчерпания
}
```

При `Crashed`: MuShOrchestrator пробует restart adapter → retry отправки → fallback на другой канал.

---

## 8. Что зафиксировано

| Пункт | Статус |
|-------|--------|
| Adapter API — без BPv7, без криптографии Mush | ✅ |
| Push-based (callback), не poll | ✅ |
| Отдельные процессы через IPC (UNIX socket) | ✅ |
| Dedup-стратегия для гибридной доставки | ✅ |
| UI-разделение + выключено по умолчанию + предупреждение | ✅ |
| Key isolation: адаптер не имеет доступа к Mush Vault | ✅ |
| Контактная изоляция: адаптер не видит Mush Contacts | ✅ |
| Sandbox: network-only, memory limit, no filesystem | ✅ |

---

## 8. Не в этом документе (после A')

| Компонент | Когда |
|-----------|-------|
| Конкретные адаптеры (telegram, signal, whatsapp) | После этапа D |
| IPC-реализация (gRPC/JSON-RPC schema) | После этапа C |
| Sandbox (seccomp, cgroups) | После этапа E |
| UI для управления адаптерами | После этапа B |

---

*MUSH-BRIDGE-ARCH — адаптеры сторонних сетей. Mush-first, не Beeper с DTN.*