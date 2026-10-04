# MUSH-A'-SPEC — Минимальный прототип (этап A')

**Версия:** 0.1.1  
**Дата:** 2026-09-29  
**Статус:** Draft — спецификация первого прототипа  
**Цель:** Два телефона, одна комната, три дня до демо. Проверить гипотезу: «люди готовы сканировать QR вместо регистрации».

---

## Изменения v0.1.0 → v0.1.1

- **§2:** «BPv7 lite» → Bundle framing (простой binary, не претендует на RFC 9171). Interop с Mycerus — этап A.
- **§3:** SNS-handshake формализован — Ed25519 для подписи, X25519 для DH, nonce 16 байт, tie-break по pubkey.
- **§5.1:** Hotspot IP не хардкодится. Owner IP зашивается в QR при создании группы.
- **§5.2:** BLE вырезан из A' (Android 12+ permissions + GATT MTU + fragment'ация — мёртвый груз для 3-дневного прототипа).
- **§6.1:** ContactRateSensor — честно назван threshold-based для A'. CUSUM отложен до этапа E.
- **§1, §7:** Shadow inbox вырезан. Заменён на «backpressure на входящих» (обычное ограничение буфера, не FSM).
- **§8:** Критерий #5 переформулирован — «50 сообщений подряд от одного контакта → alert» (тестируется на 2 телефонах).

---

## 1. Что входит в A'

A' — это не продукт, это проверка гипотезы. Минимальный набор для демо:

| Компонент | Что делает | Чего НЕТ |
|-----------|-----------|----------|
| **Hotspot transport** | Один телефон поднимает точку доступа, остальные коннектятся | WiFi-Direct P2P, BLE (сложно, отложено) |
| **QR + invite-link** | Обмен контактами через QR (офлайн) или ссылку (онлайн) | Ретрансляция, NFC |
| **CRDT-чат** | Групповой чат без хоста, синхронизация при встрече | Fruiting Gateway, история после расформирования |
| **Symmetric encryption** | AES-256-GCM с ключом, полученным через QR | Double Ratchet, Onion, pre-keys |
| **ContactRateSensor (S1)** | Threshold-based: `msg_count > median × 5` | CUSUM, тихий режим, immune plane целиком |
| **DecoyEidSensor (S7)** | Honeypot: фейковые EID в beacon'ах | Остальные сенсоры S2–S6 |
| **Backpressure на входящих** | Ограничение буфера: 100 сообщений / 7 дней, oldest-first eviction | Теневой inbox, FSM, silent mode |

---

## 2. Архитектура A'

```
┌─────────────────────────────────────────┐
│           ПОЛЬЗОВАТЕЛЬСКИЙ СЛОЙ         │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  │
│  │  Chat   │  │Contacts │  │ Settings│  │
│  │  UI     │  │  UI     │  │  UI     │  │
│  └────┬────┘  └────┬────┘  └────┬────┘  │
├───────┼────────────┼────────────┼───────┤
│       │            │            │       │
│  ┌────┴────┐  ┌────┴────┐  ┌────┴────┐  │
│  │  CRDT   │  │ Contact │  │  Config │  │
│  │  Chat   │  │ Manager │  │  Store  │  │
│  │  Store  │  │         │  │         │  │
│  └────┬────┘  └────┬────┘  └────┬────┘  │
├───────┼────────────┼────────────┼───────┤
│       │            │            │       │
│  ┌────┴────────────┴────────────┴────┐  │
│  │         Mush Core (A')            │  │
│  │  • Bundle framing (binary)        │  │
│  │  • Symmetric crypto (AES-256-GCM) │  │
│  │  • S1 ContactRateSensor           │  │
│  │  • S7 DecoyEidSensor              │  │
│  │  • Backpressure on inbox          │  │
│  └────────────────────────────────────┘  │
├─────────────────────────────────────────┤
│           Transport Layer               │
│  ┌─────────────────────────────────────┐│
│  │  Hotspot (WiFi AP) — primary       ││
│  │  (BLE — НЕ в A')                   ││
│  └─────────────────────────────────────┘│
└─────────────────────────────────────────┘
```

**Bundle framing (не BPv7 lite):** A' использует простой binary format:

```
[4 bytes: length][1 byte: type][payload: CBOR]
```

Это не BPv7. Это Mush wire format для A'. Претензии на RFC 9171 — этап A, когда потребуется interop с Mycerus. Для 2-телефонного демо BPv7 даёт canonical CBOR, EID parsing и 8+ полей primary block — избыточно.

---

## 3. Onboarding flow (A')

```
1. Скачивание
   ↓
2. Генерация ключа (Ed25519, 2 сек)
   ↓
3. Имя + идентикон
   ↓
4. Готово — можно получать invite или показать QR
   ↓
5. Добавление контакта:
      a) Invite-link (основной): ссылка с pubkey + nonce → клик → SNS-handshake
      b) QR (fallback): сканирование → pubkey exchange → SNS-handshake
   ↓
6. Пишем сообщения
```

### 3.1. SNS-handshake в A' — формализация

**Ключи:**

- Каждый узел генерирует Ed25519 keypair при первом запуске.
- Для DH Ed25519 pubkey конвертируется в X25519 через бирациональное отображение (стандарт, libsodium `crypto_sign_ed25519_pk_to_curve25519`).
- Ed25519 private key конвертируется в X25519 private аналогично.

**Что передаётся в invite-link / QR:**

```
pubkey_ed25519: 32 bytes (base64)
nonce: 16 bytes random (base64)
```

**Протокол handshake:**

```
A (инициатор) → B:
  ephemeral_A: X25519 public (32 bytes)
  nonce_A: 16 bytes random
  sig_A: Ed25519_sign(ephemeral_A || nonce_A || "SNS1") by A's identity key

B → A:
  ephemeral_B: X25519 public (32 bytes)
  nonce_B: 16 bytes random
  sig_B: Ed25519_sign(ephemeral_B || nonce_B || nonce_A || "SNS1") by B's identity key

Оба:
  shared = X25519(ephemeral_A_priv, ephemeral_B_pub) = X25519(ephemeral_B_priv, ephemeral_A_pub)
  session_key = HKDF-SHA256(shared, salt=nonce_A || nonce_B, info="Mush-SNS-v1")
```

**Проверки:**

1. `sig_A` верифицируется по `pubkey_ed25519_A` (получен из QR/invite).
2. `sig_B` верифицируется по `pubkey_ed25519_B` (получен из handshake).
3. `nonce_A` из handshake совпадает с nonce из invite-link/QR (защита от replay старого invite).
4. Nonce не встречался ранее на этом узле (nonce-кэш с TTL 24 часа).

**Tie-break при одновременном сканировании:**

- Если оба узла показывают QR и оба сканируют — инициатором становится тот, чей `pubkey_ed25519` меньше лексикографически.
- Меньший pubkey → отправляет первый handshake.
- Больший pubkey → ждёт и отвечает.
- Это детерминировано и исключает race condition.

**Что НЕ в handshake A':**

- **Double Ratchet** — симметричный session_key, без forward secrecy.
- **Pre-keys** — нет offline delivery в A'.
- **Повторный handshake для rekey** — нет, один сеанс на контакт.

---

## 4. CRDT-чат (A')

### 4.1. Ad-hoc группа

```rust
pub struct AdHocGroup {
    pub group_id: GroupID,           // hash(creator_pubkey || creation_timestamp)
    pub members: Vec<PubKey>,        // участники (известны при создании)
    pub messages: Vec<ChatMessage>,  // упорядоченные по Lamport clock
    pub group_key: SymmetricKey,     // общий ключ (обмен через QR при создании)
}
```

**Протокол:**

1. Создатель генерирует `group_id`, `group_key`, показывает QR-код.
2. Участники сканируют QR → получают `group_id` + `group_key`.
3. Сообщение шифруется на `group_key` (AES-256-GCM).
4. При встрече участников — sync: merge сообщений по Lamport clock, дедупликация по `MessageID`.

### 4.2. ChatMessage

```rust
pub struct ChatMessage {
    pub id: MessageID,               // ULID
    pub sender: PubKey,              // Ed25519 pubkey
    pub content: Vec<u8>,            // AES-256-GCM encrypted
    pub timestamp: LamportClock,     // logical clock (не wall-clock!)
    pub nonce: [u8; 12],             // GCM nonce
}
```

**Sync-протокол:**

1. Узел A отправляет узлу B свой `max_lamport_clock`.
2. B отправляет все сообщения с clock > `max_lamport_clock(A)`.
3. A merge'ит, обновляет `max_lamport_clock`.
4. Дедупликация по `MessageID`.

### 4.3. Граница

- Нет Fruiting Gateway — группа живёт только пока участники в радиусе.
- Нет истории после расформирования — каждый участник хранит локально.
- Нет Onion, cover traffic, padding — всё в plaintext метаданных (sender, timestamp).
- Это нормально для A' — приватность добавляется на этапе C.

---

## 5. Transport (A')

### 5.1. Hotspot-режим

```
Телефон A (Group Owner):
  Поднимает WiFi AP "Mush-XXXX"
    где XXXX = group_id[0..4] в hex (НЕ pubkey — иначе коллизия при двух группах)
  Свой IP определяет сам при подключении клиента (может быть 192.168.43.1,
    192.168.1.1, 192.168.232.1 — зависит от вендора)
  IP + порт зашиваются в QR при создании группы

QR при создании группы:
  { group_id, group_key, owner_ip, owner_port, owner_pubkey }

Телефон B (Client):
  Сканирует SSID "Mush-*"
  Коннектится, получает IP через DHCP
  Из QR берёт owner_ip:owner_port
  Отправляет bundle на owner_ip:owner_port
```

**Почему hotspot, а не WiFi-Direct:**

- WiFi-Direct P2P Group Owner negotiation — боль на Android (разное поведение на разных вендорах).
- Hotspot работает на всех Android с API 26+ без root.
- Для сценария «одна комната» это доказывает весь UX с нулевым риском транспорта.

**Owner IP — не хардкодить.** На Samsung — 192.168.43.1, на Xiaomi — 192.168.1.1, на Huawei — 192.168.232.1. Owner при создании группы определяет свой IP (через NetworkInterface API), зашивает его в QR. Клиент читает IP из QR, не угадывает.

**Fallback: обычный Wi-Fi (если hotspot недоступен).** `startLocalOnlyHotspot` требует runtime permissions (`NEARBY_WIFI_DEVICES` на API 33+, `ACCESS_FINE_LOCATION` до 33). Если пользователь отклонил permission или вендор заблокировал hotspot (кастомные прошивки Xiaomi/Huawei) — A' не умирает, а переключается на LAN-режим: оба телефона в одной существующей Wi-Fi сети, owner публикует `IP:port` через QR как обычно, клиент коннектится по нему. Протокол, handshake и CRDT идентичны — меняется только способ доставки IP до клиента (тот же QR). Это не «mesh без инфраструктуры», но доказывает весь UX-слой (QR-обмен, handshake, чат, синхронизация). Критерии приёмки §8 проверяются в первую очередь в hotspot-режиме; LAN-режим — задокументированный запасной путь, а не скрытый.

**Протокол:** TCP socket, простой binary framing:

```
[4 bytes: length][1 byte: type][payload: CBOR]
```

**Типы bundle в A':**

- `0x01` — text message
- `0x02` — group sync (Lamport clock + deltas)
- `0x03` — contact handshake
- `0x04` — decoy hit notification (debug)

### 5.2. BLE — НЕ в A'

BLE вырезан из A'. Причины:

- Android 12+ требует runtime permissions `BLUETOOTH_SCAN` + `BLUETOOTH_CONNECT` + location.
- GATT MTU negotiation — не везде 512 байт.
- Fragment'ация bundle'ов по MTU — отдельный код.
- Для 3-дневного прототипа, где hotspot работает — это мёртвый груз.

BLE вернётся на этапе C, когда hotspot-демо уже доказан.

---

## 6. Сенсоры (A')

### 6.1. ContactRateSensor (S1) — threshold-based

В A' это честно threshold-based, не CUSUM. CUSUM требует накопления и окна — откладывается до этапа E.

```rust
pub struct ContactRateSensor {
    message_counts: HashMap<PubKey, VecDeque<usize>>,  // msg/контакт
    window: usize,                                      // 10 контактов
    baseline: HashMap<PubKey, f64>,                     // медиана
}

impl ContactRateSensor {
    pub fn on_message(&mut self, from: PubKey, msg_count: usize) -> Option<Alert> {
        let counts = self.message_counts.entry(from).or_default();
        counts.push_back(msg_count);
        if counts.len() > self.window {
            counts.pop_front();
        }
        
        let median = counts.iter().cloned().collect::<Vec<_>>().median();
        self.baseline.insert(from, median);
        
        // Threshold-based для A'. CUSUM — этап E.
        if msg_count > median * 5.0 {
            Some(Alert::HighRate { from, msg_count, median })
        } else {
            None
        }
    }
}
```

**Почему threshold, а не CUSUM:**

- CUSUM требует `baseline_rate` + `k` + `h` + накопление `cusum_pos/neg`.
- В A' нет FSM, нет долгого состояния — CUSUM бессмыслен.
- Threshold-based проверяет гипотезу («детектирует ли сенсор спам вообще»), а не production-поведение.
- Переход на CUSUM — этап E, без изменения API (поле `evidence_tier` уже заложено).

**Что делает при alert:**

- Логирует в UI: «Контакт X отправил много сообщений».
- Не блокирует, не скрывает — только уведомление.
- Пользователь решает: заблокировать, проигнорировать, или посмотреть.

### 6.2. DecoyEidSensor (S7) — honeypot

```rust
pub struct DecoyEidSensor {
    decoy_eids: Vec<EID>,              // генерируются локально
    rotation_period: usize,            // ротация каждые N контактов
    contact_count: usize,
}

impl DecoyEidSensor {
    pub fn on_bundle(&mut self, bundle: &Bundle) -> Option<Alert> {
        if self.decoy_eids.contains(&bundle.destination) {
            Some(Alert::DecoyHit {
                source: bundle.source.clone(),
                decoy: bundle.destination.clone(),
            })
        } else {
            None
        }
    }
}
```

**Как decoy попадают в сеть:**

- Decoy EIDs генерируются локально: `decoy = hash(random_nonce || node_pubkey || epoch)`.
- Публикуются в SSID/hotspot name как дополнительная строка (debug-режим).
- В debug-меню — кнопка «отправить на decoy» (для проверки критерия #6).
- В реальной сети decoy попадут через beacon'ы — но beacon'ов в A' нет.

**Что делает при alert:**

- Немедленное уведомление: «Обнаружена подозрительная активность от X».
- Пользователь решает: заблокировать или проигнорировать.
- Логируется для обучения (если Vironet подключён позже).

### 6.3. Backpressure на входящих (не shadow inbox)

Shadow inbox вырезан из A'. Он подразумевает FSM (silent state), которого в A' нет. Без FSM «теневой inbox» — это либо бессмысленный буфер, либо первая версия silent mode. Оба варианта не проверяют гипотезу.

**Что вместо:** обычный backpressure на входящем буфере.

```rust
pub struct InboxBuffer {
    max_messages: usize,      // 100
    max_age: Duration,        // 7 дней
    messages: VecDeque<IncomingMessage>,
}

impl InboxBuffer {
    pub fn push(&mut self, msg: IncomingMessage) {
        // Eviction: oldest-first по age
        self.evict_old();
        
        // Eviction: oldest-first по количеству
        if self.messages.len() >= self.max_messages {
            self.messages.pop_front();
        }
        
        self.messages.push_back(msg);
    }
    
    fn evict_old(&mut self) {
        let cutoff = now() - self.max_age;
        while self.messages.front().map(|m| m.timestamp < cutoff).unwrap_or(false) {
            self.messages.pop_front();
        }
    }
}
```

**Отличие от shadow inbox:**

- Backpressure — это ограничение ресурсов, не политика безопасности.
- Сообщения показываются пользователю (нет скрытия).
- Нет триггера от сенсоров.
- Нет ручного «восстановления» — старые просто удаляются.
- Это честно для A'. Silent mode и FSM — этап E.

---

## 7. UI (A')

### 7.1. Экраны

| Экран | Что показывает |
|-------|---------------|
| **Splash** | Логотип Mush, генерация ключа (2 сек) |
| **Onboarding** | Имя, аватар (идентикон), кнопка «Показать QR» |
| **Chats** | Список групп. Кнопка «+» → создать группу / присоединиться по QR |
| **Chat** | Сообщения (входящие/исходящие). Статус: ✓ = отправлено, ✓✓ = получено sync |
| **Contacts** | Список контактов. Кнопка «Поделиться контактом» (invite-link / QR) |
| **Alerts** | Уведомления от сенсоров (S1, S7). Можно проигнорировать или заблокировать |
| **Debug (скрытый)** | Кнопка «Отправить на decoy» (для теста критерия #6) |

### 7.2. Invite-link

```
https://mush.app/i#pubkey=<base64>&nonce=<base64>
```

**При клике:**

- Если приложение установлено — открывается, автоматически подхватывает pubkey + nonce.
- Если нет — открывается страница со ссылками на APK / TestFlight / F-Droid.
- После установки — приложение читает intent / deep link, подхватывает invite.

---

## 8. Критерии приёмки A'

| # | Критерий | Как проверить |
|---|----------|--------------|
| 1 | Два телефона в одной комнате обмениваются сообщениями | Демо: 2 Android, hotspot, текст |
| 2 | QR-контакт работает без интернета | Демо: airplane mode, QR scan, handshake |
| 3 | Invite-link работает с автоподхватом | Демо: клик ссылки → установка → контакт добавлен |
| 4 | Групповой чат 4 человека синхронизируется | Демо: 4 телефона, сообщения видны всем после sync |
| 5 | S1 детектирует аномалию (ручной тест) | 50 сообщений подряд от одного контакта → alert в UI |
| 6 | S7 детектирует decoy hit (debug-кнопка) | Debug: «отправить на decoy» → alert |
| 7 | Backpressure работает | Проверка: 100+ сообщений → oldest удаляются |
| 8 | Нет крашей при 30 мин использования | Monkey test: случайные тапы 30 мин |

**Критерий #5** — переформулирован. В v0.1.0 было «50 сообщений за 10 контактов → alert», но на 2 телефонах это один контакт, не 10. Медиана по окну 10 не работает на 1 значении. Новый критерий: «50 сообщений подряд от одного контакта» — проверяет то же самое, но реально тестируется на 2 телефонах.

---

## 9. Что НЕ входит, но заложено как extension point

### 9.1. MessageSource trait

```rust
pub trait MessageSource {
    fn send_message(&mut self, msg: &ChatMessage) -> Result<MsgId>;
    fn receive_messages(&mut self) -> Vec<ChatMessage>;
    fn status(&self) -> ConnectionStatus;
}

// A': одна реализация — HotspotMessageSource
// Этап C+: добавляем BleMessageSource, DtnMessageSource
```

### 9.2. SensorEvent с evidence_tier

```rust
pub struct SensorEvent {
    pub sensor_name: String,
    pub target: PubKey,
    pub score: f64,
    pub evidence_tier: EvidenceTier,  // заложен, но A' использует только Hard
}

pub enum EvidenceTier {
    Hard,         // verifiable, объективный факт (decoy hit, rate spike)
    Soft,         // behavioral, требует CUSUM (добавится на этапе E)
    Reputational, // complaint, голосование (добавится на этапе D)
}
```

**A':** `evidence_tier` поле существует в структуре, но сенсоры всегда возвращают `Hard`. Это позволяет добавить `Soft`/`Reputational` позже без изменения API.

### 9.3. Инварианты (записать сейчас)

```rust
/// Invariants — hardcoded в ядро, не в геном, не мутируют эволюцией
pub mod invariants {
    pub const QUORUM_THRESHOLD_MIN: u32 = 2;           // quorum ≥ 2
    pub const QUARANTINE_MAX_SCOPE: f64 = 0.1;         // ≤ 10% сети в quarantine
    pub const SENSOR_PENALTY_TTL_MIN: u64 = 3600;      // минимум 1 час
    pub const SENSOR_EPSILON_MIN: f64 = 0.5;           // DP privacy budget
}
```

**A':** кворум и quarantine не используются (нет FSM), но константы определены. При переходе к этапу E — включаются без изменения кода.

### 9.4. Что НЕ входит в A' (и почему)

| Компонент | Почему не в A' | Когда |
|-----------|---------------|-------|
| **Double Ratchet** | Сложнее symmetric, не критично для демо | Этап A |
| **Onion + cover traffic** | Нет mesh → нет multi-hop | Этап C |
| **Fruiting Gateway** | Нет persistent групп в A' | Этап D |
| **Immune plane (тип 202)** | Нет FSM, quarantine, appeals | Этап E |
| **Vironet** | Нет эволюции в однокомнатном демо | Этап E |
| **MAC rotation** | Hotspot AP не требует rotation | Этап C |
| **Partition reconciliation** | Нет partition в одной комнате | Этап E |
| **Pre-keys** | Нет offline delivery в A' | Этап A |
| **BLE** | Android 12+ permissions + GATT MTU | Этап C |
| **Shadow inbox** | Бессмыслен без FSM | Этап E |
| **BPv7** | Избыточен для 2-телефонного демо | Этап A |

---

## 10. Прототип: Rust CLI → Kotlin Android

**Решение:** Сначала Rust CLI для проверки логики ядра. Затем Kotlin Android Studio для реального A'.

**Почему Rust CLI сначала:**

| Риск | Android сразу | Rust CLI сначала |
|------|--------------|------------------|
| **Android toolchain** | Studio, SDK, эмулятор — час настройки | `cargo run` — 10 сек |
| **Отладка handshake** | Логи через Logcat, устройство/эмулятор | `println!` в терминале |
| **CRDT sync** | Сложно на двух эмуляторах | Два терминала на одной машине |
| **Sensors** | Нет данных для теста | Симуляция: 50 сообщений → alert |
| **CI** | Нет | GitHub Actions: `cargo test` |

**Что входит в Rust CLI:**

| Компонент | CLI-версия | Аналог Android |
|-----------|-----------|----------------|
| **Transport** | TCP socket (порт 7878) | Hotspot AP |
| **Contact exchange** | `mush://<base64_pubkey>` в stdout | QR-код |
| **UI** | TUI (`ratatui`) или stdin/stdout | Jetpack Compose |
| **Crypto** | `ed25519-dalek`, `x25519-dalek`, `aes-gcm` | Те же библиотеки через JNI |
| **Bundle framing** | `[length][type][CBOR payload]` | Идентично |
| **CRDT sync** | TCP merge | WiFi sync |
| **Sensors** | `ContactRateSensor`, `DecoyEidSensor` | Идентично |
| **Backpressure** | `InboxBuffer` | Идентично |

**Что НЕ входит в Rust CLI (только Android):**

- WiFi AP / Hotspot API (заменён на TCP)
- QR-код сканирование (заменён на `mush://` строка)
- Android permissions, intents, deep links
- Идентикон, Safety Number, Identicon
- Jetpack Compose UI

**Roadmap:**

```
Сейчас:
  Rust CLI — проверка handshake, CRDT, sensors, framing
  cargo test — unit tests для crypto, CRDT merge, sensor logic
  cargo run --bin mush-node — две ноды на одной машине

После проверки (критерии #1–#7 проходят в CLI):
  Kotlin Android Studio — тот же protocol, нативный UI
  Hotspot API — вместо TCP
  QR-камера — вместо stdout
  APK → два телефона → демо
```

**Контракт:** Rust CLI и Kotlin Android делят одну логику — protocol не меняется. Только transport layer (TCP ↔ Hotspot) и UI (TUI ↔ Compose).

---

## 11. Следующий шаг после A'

**Если A' проходит критерии:**

- **Этап A:** Double Ratchet, pre-keys, persistent 1-on-1 чаты, BPv7 interop.
- **Этап B:** SQLCipher, Vault, Identicon, Safety Number.
- **Этап C:** Onion, cover traffic, BLE transport, MAC rotation.
- **Этап D:** Fruiting Gateway, Group CRDT, SBP.
- **Этап E:** Immune plane (тип 202), Vironet, CUSUM, silent mode, stress-test.

**Если A' не проходит** (люди не сканируют QR, UX непонятен, sync не работает) — переделываем onboarding и transport, не добавляя сложности.