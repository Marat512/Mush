# MUSH Architecture — маппинг на Mycerus

**Версия:** 0.2.0
**Дата:** 2026-09-28
**Статус:** Архитектурное предложение (не нормативно)
**Гейт:** работы по MUSH не начинаются без явного двухкратного подтверждения владельца (см. MUSH-CONCEPT, шапка)
**База:** Mycerus MVP (этапы 1–19A) + ATTACK-MODEL v0.5.0 (MEB-SPEC v0.1.2, MGR-API-SPEC v0.2, GENOME-SPEC v0.1)

---

## 1. Принцип: пользователь видит ягоду, под землёй — грибница

| Человек видит | Под капотом (Mycerus) | Компонент MUSH |
|---------------|----------------------|---------------------|
| Контакты | SBP-handshake (люди) + QR/NFC | `MUSH Contacts` |
| Чаты | CRDT-история + bundle | `MUSH Chats` |
| Отправить сообщение | MEB + POT + store-carry-forward | `MUSH Core` |
| Доставлено | SRE receipt + подписанный ReportE2EDelivery (MGR-API-SPEC §4.12) | `MUSH Core` |
| Настройки приватности | Genome constraints | `MUSH Vault` |
| Резервная копия | SBP + споры | `MUSH Spores` |
| Сеть рядом | POT + CLA + MGR gradient | `MUSH Transport` |
| Групповой чат | Сервер-грибница (выделенный узел/узлы группы) | `MUSH Groups` |
| Мульти-устройство | Sync через споры (encrypted snapshot) | `MUSH Spores` |

**Пользователь никогда не видит:** BPv7, MEB, MGR, градиенты, Immune Block, Vironet. Он видит чаты, которые всегда работают.

**Терминология:** SNS-handshake — аутентификация **узлов** (challenge-response, ТЗ §9). SBP-handshake — handshake **людей** (добавление контакта, §2.2). Это разные уровни; не смешивать.

---

## 2. Компоненты MUSH

### 2.1. MUSH Core

**Задача:** Интерфейс между MUSH и Mycerus DTN. Формирует bundle'ы, получает их из сети, управляет lifecycle сообщения.

**Ложится на:** BPv7 Bundle Protocol + MEB (тип 200, MEB-SPEC v0.1.2) + Immune Block (тип 202, ATTACK-MODEL §6).

```rust
pub struct MUSHCore {
    mycerus: MycerusNode,           // BPv7 + MGR + SNS + POT
    meb_adapter: MEBAdapter,         // формирование MEB (MEB-SPEC v0.1.2 §4)
    message_queue: MessageQueue,     // исходящие, ожидающие контакта
    delivery_tracker: DeliveryTracker, // отслеживание ✓ / ✓✓ / ⏳
}

impl MUSHCore {
    /// Отправляет сообщение в сеть
    pub fn send(&mut self, msg: MUSHMessage, dst: PubKey) -> MessageID {
        let id = msg.id.clone();
        // 1. Шифруем payload (E2E, см. §3)
        let encrypted = self.onion.wrap(&msg, dst);
        // 2. Формируем BPv7 bundle с MEB (MEB-SPEC v0.1.2 §4)
        let bundle = self.build_bundle(encrypted, dst);
        // 3. Передаём в Mycerus для маршрутизации
        self.mycerus.submit(bundle);
        // 4. Возвращаем ID для отслеживания статуса
        id
    }

    /// Вызывается при получении bundle из сети
    pub fn on_bundle_received(&mut self, bundle: Bundle) {
        // 1. Проверяем destination — нам ли?
        if !bundle.is_for_me(self.my_key) { return; }
        // 2. Расшифровываем (E2E, см. §3)
        let msg = self.onion.unwrap(bundle.payload);
        // 3. Передаём в Chat Store
        self.chat_store.insert(msg);
        // 4. Отправляем SRE-receipt / ReportE2EDelivery (если запрошено)
        self.send_receipt(bundle.source, bundle.id);
    }
}
```

**Privacy-aware MEB hints — ОТКРЫТЫЙ ВОПРОС (не реализуемо в текущем контракте).**

MEB несёт `mgr_hint.destination_affinity`, `mgr_hint.path_cost`, `mgr_hint.visited` (MEB-SPEC v0.1.2 §4.2), а MGR/gradient routing требует их видимости для выбора next hop (MGR-API-SPEC v0.2). Идеи вроде замены `destination_affinity` на `privacy_zone` (грубая категория local/regional/global), сокрытия `visited` внутри onion-обёртки или непубликации `path_cost` **несовместимы с gradient routing**: маршрутизация с полностью скрытым destination в текущем протоколе невозможна. Это требует MEB-SPEC v0.2 + отдельного решения маршрутизации без видимого destination. До принятия такого решения MUSH использует MEB как есть (с видимыми mgr_hint) — см. честные ограничения §8.

### 2.2. MUSH Contacts — двухфазный SBP

**Задача:** Управление контактами с гарантией осознанного согласия.

**Ложится на:** SNS/SRE (узловой уровень) + QR/NFC handshake (прикладной уровень).

**Ключевой принцип:** В MUSH «знать адрес» и «иметь право отправить» — это одно и то же. Нельзя узнать адрес случайно, нельзя угадать, нельзя получить из базы. Только через физический обмен (QR, BLE-handshake, встреча). Это как обмен ключами — не действие, а согласие.

```rust
pub struct Contact {
    pubkey: Ed25519PublicKey,       // идентификатор (EID)
    display_name: String,            // пользовательское имя
    identicon: Identicon,            // аватар на основе pubkey
    trust_level: TrustLevel,         // unknown / known / verified
    added_at: Timestamp,
    last_seen: Option<Timestamp>,    // последний контакт через сеть
    // Нет телефона. Нет email. Нет username.
}

pub enum TrustLevel {
    Unknown,      // фаза 1: pubkey получен (QR/NFC), но handshake не завершён
    Known,        // фаза 2: SBP-handshake успешен, запись в адресный список
    Verified,     // fingerprint сравнен лично (Safety Number)
}
```

**Двухфазный SBP (Spore Bundle Protocol) для контактов:**

**Фаза 1 — Обмен идентичностями (out-of-band):**
```
Алиса генерирует QR: base64(pubkey + ephemeral_nonce + display_name_hash)
Боб сканирует → получает pubkey → формирует Contact(Unknown)
```
- Боб знает pubkey Алисы, но **не может отправить сообщение**.
- Pubkey не глобально маршрутизируем — MGR не знает, где искать Алису без beacon.

**Фаза 2 — Подтверждение контакта (SBP-handshake):**
```
Боб инициирует SBP-handshake с Алисой (challenge-response по Ed25519)
Алиса подтверждает → оба записывают друг друга в локальный адресный список
TrustLevel переходит в Known
```
- Без фазы 2 фаза 1 бесполезна — знание pubkey ≠ право отправки.
- SBP-handshake = взаимное согласие. Это защищает от «случайного» попадания в список.

**Gate на отправку (связь с §3 и §5.6):** `init_session` и отправка сообщений возможны **только для контактов в статусе Known** (фаза 2 пройдена). Публичный pre-key (§3) даёт возможность установить E2E-сессию, но **не право доставки**: первый bundle от контакта в статусе Unknown принимается в **silent inbox** (см. §5.6), не в основной чат.

**Приглашение через посредника:**
- Алиса хочет познакомить Боба с Верой.
- Алиса отправляет Бобу pubkey Веры (encrypted на pubkey Боба).
- Боб получает pubkey, но Contact = Unknown.
- Боб **должен сам инициировать SBP-handshake** с Верой.
- Вера **должна сама подтвердить** — никто не может добавить Веру без её согласия.

### 2.3. MUSH Chats

**Задача:** Хранение истории, отображение, CRDT-синхронизация.

**Ложится на:** Bundle Payload Block (тип 203 — зарезервирован, требует отдельной спецификации) + локальный Chat Store.

**Почему CRDT:**
- В DTN порядок доставки не гарантирован. Сообщение 2 может прийти раньше сообщения 1.
- Несколько устройств одного пользователя могут отправлять сообщения параллельно.
- CRDT (Conflict-free Replicated Data Type) позволяет сходиться к единой истории без центрального сервера.

```rust
pub struct ChatMessage {
    id: MessageID,                   // ULID (содержит wall-clock; в проде — см. §2.6)
    sender: PubKey,
    chat_id: ChatID,                 // hash(participant_pubkeys) для 1-to-1
    content: EncryptedContent,
    timestamp: LamportClock,         // logical clock, не wall-clock
    vector_clock: VectorClock,       // для CRDT merge
    parent: Option<MessageID>,       // для тредов/ответов
}

pub struct ChatStore {
    messages: CRDTMap<MessageID, ChatMessage>,
    read_cursors: Map<ChatID, MessageID>,  // последнее прочитанное
}

impl ChatStore {
    /// Слияние при получении новых сообщений (CRDT merge)
    pub fn merge(&mut self, remote: &ChatStore) {
        self.messages.merge(&remote.messages);  // LWW-Element-Set или OR-Map
    }
}
```

**CRDT-тип:** OR-Map (Observed-Removed Map) с LWW (Last-Write-Wins) для содержимого. Удаление сообщения = tombstone (не физическое удаление, а метка для скрытия).

### 2.4. MUSH Transport

**Задача:** Управление CLA (Channel Layer Adapters), абстракция над радио.

**Ложится на:** POT + CLA (WiFi-Direct / BLE / LAN / LoRa). Мобильные радиотранспорты — вне скоупа MVP Mycerus (ТЗ §3.2).

```rust
pub struct TransportManager {
    adapters: Vec<Box<dyn CLAAdapter>>,
    preferences: Vec<CLAType>,  // приоритет: WiFi-Direct > BLE > LAN
}

pub trait CLAAdapter {
    fn scan(&mut self) -> Vec<PeerEndpoint>;  // поиск соседей
    fn connect(&mut self, peer: &PeerEndpoint) -> Connection;
    fn disconnect(&mut self, conn: &Connection);
    fn energy_budget(&self) -> EnergyProfile;  // сколько mAh тратит
}
```

**Energy-aware switching:**
- Если батарея > 50% + зарядка — используем WiFi-Direct (быстро, дальность).
- Если батарея 20–50% — BLE (экономно, короткая дальность).
- Если батарея < 20% — только приём, без relay.

**MAC rotation (требование приватности):**
- Постоянный MAC = персистентный трекер. BLE/WiFi сниффер видит «узел X пришёл на митинг, потом в больницу».
- Ротация MAC каждые N минут (например, 15).
- При каждом новом MAC — lightweight Noise-handshake по известным ключам (pairing повторяется, но identity сохраняется).
- `mac_rotation_policy` — первоклассное требование Transport Manager.

```rust
pub struct MacRotationPolicy {
    pub rotation_interval: Duration,  // например, 15 минут
    pub noise_handshake: bool,        // обязательный re-handshake при rotation
}
```

**Trade-off:** ротация ломает BLE pairing (Android/iOS кэшируют MAC-LTK связь). Решение: identity поднимается на уровень выше — Noise-handshake по pubkey, не по MAC.

### 2.5. MUSH Vault

**Задача:** Encrypted local storage для сообщений, ключей, контактов.

**Ложится на:** SQLCipher / File-based encryption + Keychain/Keystore.

```rust
pub struct Vault {
    db: EncryptedDatabase,           // SQLCipher с ключом из Keychain
    master_key: ProtectedKey,        // никогда не в памяти в plaintext
}

impl Vault {
    /// Ключ шифрования БД — derived from hardware-backed keystore
    pub fn open(password: Option<String>) -> Result<Vault> {
        let hw_key = Keychain::get_or_create_key("mush_vault");
        let derived = if let Some(pwd) = password {
            Argon2::derive(hw_key, pwd)
        } else {
            hw_key  // если пароля нет — только hardware key
        };
        Vault::with_key(derived)
    }
}
```

**Plausible deniability:** опциональный hidden volume (как VeraCrypt). Пользователь может открыть Vault с "декоративным" паролем — покажется пустой чат. Реальные чаты — под другим паролем.

### 2.6. MUSH Spores

**Задача:** Backup, multi-device sync, эвакуация состояния.

**Ложится на:** SBP (Spore Bundle Protocol) + Mycerus store-carry-forward.

```rust
pub struct Spore {
    version: u32,
    created_at: Timestamp,
    device_id: DeviceID,             // уникальный ID устройства
    encrypted_snapshot: Bytes,       // зашифрованный dump Vault
    // Зашифрован на recovery_key, который знает только пользователь
}

pub struct SporeManager {
    recovery_key: ProtectedKey,      // никогда не передаётся по сети
}

impl SporeManager {
    /// Создать spore и оставить в сети
    pub fn spawn_spore(&mut self, vault: &Vault) -> SporeID {
        let snapshot = vault.export_encrypted(&self.recovery_key);
        let spore = Spore::new(snapshot);
        // Распространить как обычный bundle с высоким TTL и copies
        self.mycerus.submit_high_reliability(spore.to_bundle());
        spore.id
    }

    /// Подобрать spore из сети (на новом устройстве)
    pub fn germinate(&mut self, spore: &Spore) -> Vault {
        Vault::import_encrypted(&spore.encrypted_snapshot, &self.recovery_key)
    }
}
```

**Multi-device sync:**
- Устройство A создаёт spore → оставляет в сети (TTL = 7 дней, copies = spray_max).
- Устройство B при подключении к сети ищет spore'ы со своим pubkey → подбирает → germinate.
- После germination оба устройства синхронизируются через CRDT-merge чатов.

### 2.7. MUSH Onion

**Задача:** Защита метаданных (дорожная карта — после этапа 22 + MEB v2, см. §9). Relay-узлы не должны знать, кто отправитель и получатель, и не должны отличать настоящий трафик от фона. **В текущей сети onion не работает** — MGR требует видимого destination (§2.1).

**Ложится на:** Layered encryption поверх BPv7 bundle.

```rust
pub struct OnionWrapper {
    layers: Vec<OnionLayer>,         // один слой на хоп
}

pub struct OnionLayer {
    next_hop: PubKey,                // кому пересылать дальше
    encrypted_payload: Bytes,        // payload, зашифрованный для next_hop
}

impl OnionWrapper {
    /// Упаковка сообщения в onion
    pub fn wrap(&self, msg: &MUSHMessage, path: &[PubKey]) -> Result<Bytes, OnionError> {
        // 1. Шифруем сообщение для получателя (внутренний слой).
        //    Пустой путь — ошибка: без получателя нечего упаковывать.
        let recipient = path.last().ok_or(OnionError::EmptyPath)?;
        let mut current = Noise::encrypt(msg, recipient)?;
        // 2. Для каждого intermediate hop: оборачиваем в слой
        for hop in path.iter().rev().skip(1) {
            current = self.wrap_layer(current, hop)?;
        }
        Ok(current)
    }
}
```

**Паддинг:** все bundle'ы padding'ятся до ближайшего фиксированного размера (1 KB, 4 KB, 16 KB). Relay-узел не может определить, текст это или видео, по размеру.

**Cover traffic:** фоновая генерация dummy bundle'ов с тем же размером и структурой, что и реальные. Dummy'ы расходуются как обычные bundle'ы, но destination = случайный ключ (dead drop). Relay-узлы не отличают dummy от real. Только при питании/зарядке (см. §8).

```rust
pub struct CoverTraffic {
    rate: PoissonRate,               // lambda = N dummy per hour
    size_distribution: Vec<usize>,   // распределение размеров (как у реальных msg)
}
```

### 2.8. MUSH Groups — три режима группового чата

MUSH поддерживает три режима группового общения:

| Режим | Хост | История | Relay | Когда использовать |
|-------|------|---------|-------|-------------------|
| **1-on-1** | Нет | У обоих | Нет | Личные сообщения |
| **Ad-hoc группа** | Нет | Пока в радиусе | Только в радиусе | Спонтанные встречи (поход, протест, авария) |
| **Fruiting Gateway** | Есть | Постоянная | Через хост | Устойчивые сообщества |

**Ad-hoc группа** — временное плодовое тело (грибное кольцо): живёт, пока участники в радиусе связи. Без хоста, на полностью децентрализованном CRDT. После расформирования — история есть у каждого участника локально, но не синхронизируется.

**Fruiting Gateway** — постоянная грибница: требует хоста, даёт persistent историю и relay через store-carry-forward.

#### Три класса Fruiting Gateway

| Класс | Устройство | Участники | Uptime | Экономика |
|-------|-----------|-----------|--------|-----------|
| **Class A (Telephone)** | Телефон владельца | До 10 | Когда онлайн | Батарея владельца |
| **Class B (Dedicated)** | Raspberry Pi / старый телефон | До 100 | 24/7 | Электричество + интернет |
| **Class C (Community)** | Shared-ресурс (мостовой узел) | До 500 | 24/7 | Коллективное финансирование |

**Вытеснение на community-грибницу:** если хост Class A офлайн > TTL, участники могут голосовать (кворум) за перенос канала на ближайшую community-грибницу. Это не автоматическое — требует согласия. Иначе это захват чужого канала.

---

### 2.8.1. Ad-hoc группа

```rust
pub struct AdHocGroup {
    pub group_id: GroupID,           // hash(creator_pubkey || creation_timestamp)
    pub members: CRDTSet<PubKey>,    // участники (OR-Set)
    pub messages: CRDTMap<MessageID, GroupMessage>,
    pub group_key: SymmetricKey,     // общий ключ (обмен через QR при создании)
    pub ttl: Duration,               // время жизни группы без контакта
}
```

**Протокол:**
1. **Создание:** участник генерирует `group_id`, `group_key`, показывает QR-код.
2. **Присоединение:** другие сканируют QR — получают `group_id` + `group_key`.
3. **Сообщение:** шифруется на `group_key`, relay'ится всем участникам в радиусе WiFi-Direct/BLE.
4. **Синхронизация:** при встрече участников — CRDT merge историй.
5. **Расформирование:** если TTL истёк и нет контактов — группа "засыхает". История остаётся локально у каждого.

**Защита от спама:** все участники знают друг друга лично (QR-обмен). Нет анонимного входа. Нет relay за пределы радиуса — атакующий должен быть физически рядом.

---

### 2.8.2. Fruiting Gateway — сервер-грибница (состояние канала)

**Задача:** Групповые чаты через выделенную грибницу-хост.

**Ложится на:** CRDT-состояние + выделенный узел-грибница (§2.9).

**Ключевой принцип:** В MUSH нет "общего чата в воздухе". Хочешь канал — поднимай свою грибницу. Это Discord-модель, но с честной экономикой: не "сервер компании", а твоя грибница.

```rust
/// Состояние одного хостимого канала (хранится внутри FruitingGateway, §2.9)
pub struct ChannelState {
    owner: PubKey,                   // кто поднял грибницу
    channel_id: ChannelID,           // hash(owner_pubkey || creation_timestamp)
    members: CRDTSet<PubKey>,        // участники (OR-Set)
    group_key: SymmetricKey,         // текущий ключ канала
    key_history: Vec<(Timestamp, SymmetricKey)>,
    message_log: CRDTMap<MessageID, GroupMessage>,
    moderation_policy: ModerationPolicy,
}

pub struct GroupMessage {
    id: MessageID,
    sender: PubKey,
    encrypted_content: Bytes,        // encrypted with group_key
    timestamp: LamportClock,
    signature: Ed25519Signature,     // подпись отправителя
}

pub enum ModerationPolicy {
    Open,        // любой может читать, писать — только members
    Restricted,  // писать — только после approve владельца
    Private,     // читать/писать — только members
}
```

**Протокол:**
1. **Создание канала:**
   - Владелец поднимает грибницу (Fruiting Gateway) — выделенный узел Mycerus.
   - Генерирует `channel_id`, `group_key`, настраивает `moderation_policy`.
   - Грибница анонсирует себя в mesh (beacon с `channel_id` + `moderation_policy`).

2. **Добавление участника:**
   - Участник получает invite от владельца (QR или через общий контакт).
   - Invite-bundle: `{channel_id, group_key, gateway_pubkey}` encrypted на pubkey участника.
   - Участник **явно принимает** (accept-bundle) — без accept'а нет подписки.
   - Владелец добавляет pubkey в `members` (CRDT-операция).

3. **Отправка сообщения:**
   - Участник шифрует сообщение на `group_key`.
   - Отправляет bundle на `gateway_pubkey` (через MGR gradient).
   - Грибница-хост relay'ит сообщение всем участникам, которых видит.
   - Участники, не online, получат сообщение при следующем контакте с грибницей (store-carry-forward).

4. **Удаление участника:**
   - Владелец удаляет pubkey из `members`.
   - Генерирует новый `group_key` (rotation).
   - Рассылает новый ключ всем оставшимся членам.
   - Грибница перестаёт relay'ить bundle'ов удалённого участника.

5. **Синхронизация:**
   - Участник при контакте с грибницей запрашивает Merkle root сообщений.
   - При расхождении — запрос недостающих chunks.
   - CRDT merge: грибница — авторитетный snapshot, участники — deltas.

**DTN-оптимизация:**
- Грибница-хост = снапшот-сервер: держит полную историю.
- Участники синхронизируются при встрече с грибницей.
- Нет broadcast'а — только store-carry-forward через грибницу.

**Защита от злоупотреблений:**
- Грибница-хост модерирует relay (отказывает нарушителям), но не может подделать историю — сообщения подписаны отправителями.
- Владелец несёт реальные ресурсы: хранение, трафик, энергия. Это делает экономическую атаку (1000 фальшивых каналов) дорогой.

---

### 2.9. Fruiting Gateway — роль сервера-грибницы (узел)

**Fruiting Gateway** — это выделенный узел Mycerus, который:
- Хостит канал/группу (хранит CRDT-состояние, relay'ит сообщения).
- Служит точкой входа для мобильных узлов (плодовых тел).
- Может иметь bridge в интернет (опционально).
- Несёт реальные ресурсы: хранение, CPU, энергия.

```rust
pub struct FruitingGateway {
    // Mycerus node
    mycerus: MycerusNode,

    // Hosted channels (состояние каналов — §2.8.2, ChannelState)
    channels: HashMap<ChannelID, ChannelState>,

    // Client connections (mobile nodes)
    clients: HashMap<PubKey, ClientSession>,

    // Resource limits
    max_channels: usize,             // из конфига
    max_storage_per_channel: usize,  // в байтах
    max_clients: usize,              // одновременно
}

impl FruitingGateway {
    /// Приём нового клиента
    pub fn accept_client(&mut self, client_key: PubKey, channel_id: ChannelID) -> Result<()> {
        // 1. Проверить, не превышен ли лимит clients
        // 2. Проверить, есть ли client_key в members[channel_id]
        // 3. Создать ClientSession
        // 4. Начать синхронизацию (Merkle root → deltas)
    }

    /// Получение сообщения от участника
    pub fn on_message(&mut self, msg: GroupMessage, channel_id: ChannelID) {
        // 1. Проверить подпись отправителя
        // 2. Проверить, что отправитель в members[channel_id]
        // 3. Добавить в CRDT message_log
        // 4. Queue для relay всем online клиентам
        // 5. Store для offline клиентов (store-carry-forward)
    }
}
```

**Ресурсы и экономика:**
- Владелец платит: хранение (SSD), энергия (электричество), пропускная способность.
- Это не «бесплатный сервер компании» — это твой узел, твои ресурсы, твоя ответственность.
- Экономическая атака (1000 фальшивых каналов) требует 1000 физических узлов с электричеством.

**Модерация без цензуры:**
- Владелец может отказать в relay конкретному участнику (не принимать его bundle'ы).
- Но владелец не может подделать историю — сообщения подписаны отправителями, CRDT-состояние верифицируемо.
- Участник, недовольный модерацией, может fork'нуть канал на своей грибнице (CRDT-состояние переносится).

---

## 3. E2E шифрование: Double Ratchet для DTN

Signal Double Ratchet требует обоих участников online для key exchange. В DTN это невозможно.

**Адаптация — Offline Double Ratchet (требует отдельной спецификации до реализации):**

```rust
pub struct OfflineRatchet {
    root_key: RootKey,
    sending_chain: ChainKey,
    receiving_chain: ChainKey,
    // Pre-keys: каждый узел генерирует N одноразовых pre-keys
    // и публикует их в сеть (как часть beacon / contact-info)
    pre_keys: Vec<OneTimePreKey>,
}

impl OfflineRatchet {
    /// Инициация сессии (отправитель offline)
    /// GATE: только для контактов в статусе Known (фаза 2 SBP, §2.2)
    pub fn init_session(&mut self, recipient: &Contact) -> Result<Session> {
        // 0. Контакт обязан быть Known — иначе Err(NotKnown)
        if recipient.trust_level == TrustLevel::Unknown {
            return Err(SessionError::NotKnown);
        }
        // 1. Получаем один из pre-keys получателя из сети
        let pre_key = self.network.find_pre_key(recipient.pubkey);
        // 2. X3DH handshake с использованием pre-key
        let shared = x3dh(self.identity_key, pre_key);
        // 3. Инициализируем root_key и sending_chain
        Session::new(shared)
    }

    /// Отправка сообщения (получатель offline)
    pub fn encrypt(&mut self, session: &mut Session, plaintext: &str) -> EncryptedMessage {
        let msg_key = session.sending_chain.advance();
        let ciphertext = aes_gcm_encrypt(plaintext, &msg_key);
        // Сообщение содержит enough info для получателя, чтобы восстановить chain
        EncryptedMessage {
            ephemeral_dh: session.ephemeral_pubkey,
            chain_index: session.sending_chain.index,
            ciphertext,
        }
    }

    /// Расшифровка (получатель получил bundle)
    pub fn decrypt(&mut self, msg: &EncryptedMessage) -> String {
        // Восстанавливаем receiving_chain по ephemeral_dh
        let msg_key = self.receiving_chain.advance_to(msg.chain_index);
        aes_gcm_decrypt(&msg.ciphertext, &msg_key)
    }
}
```

**Ключи подписи и ключи согласия.** Для X25519/ECDH используется отдельный X25519-ключ, подписанный Ed25519-ключом идентичности (или конвертация Ed25519→X25519 по стандартной формуле). Ed25519-ключ сам по себе для ECDH не используется.

**MITM-оговорка (первый контакт):** обмен pubkey на фазе 1 SBP (§2.2) неаутентифицирован — активный MITM может подменить ключи. Доверие к fingerprint собеседника устанавливается out-of-band: сравнение fingerprint по QR при личной встрече (Safety Number). Это сравнение **и есть** доверие; без него первый контакт защищён только от пассивного наблюдателя.

**Pre-key distribution:**
- Каждый узел генерирует 100 одноразовых pre-keys.
- Pre-keys рассылаются в сеть как часть contact-info bundle (TTL = 30 дней, copies = spray_max).
- При исчерпании pre-keys — генерируются новые и рассылаются.

**Pre-keys и gate на Unknown:** публичный pre-key даёт возможность установить E2E-сессию, но **не право доставки**. Первый bundle от контакта в статусе Unknown принимается в **silent inbox** (§5.6), не в основной чат.

**Pre-keys и FSM:**
- При FSM `silent`, `degraded`, `quarantined` — публикация новых pre-keys прекращается.
- Старые pre-keys истекают и не обновляются — отправитель не может начать новый session.
- Это автоматически ограничивает коммуникацию без дополнительного механизма.

**Forward secrecy:** каждое сообщение использует новый msg_key из chain. Компрометация одного ключа не раскрывает прошлые сообщения.

---

## 4. Сравнение с существующими

| Мессенджер | Центр | Работает без сети | Метаданные | E2E по умолчанию | Server-less |
|------------|-------|-------------------|------------|------------------|-------------|
| Signal | ✅ серверы | ❌ | ⚠️ частично | ✅ | ❌ |
| Telegram | ✅ облако | ❌ | ❌ | ❌ (не по умолчанию) | ❌ |
| WhatsApp | ✅ Meta | ❌ | ❌ | ✅ | ❌ |
| Matrix | ✅ федерация | ❌ | ❌ | ✅ | ❌ |
| Session | ✅ Lokinet | ❌ | ⚠️ onion routing | ✅ | ❌ |
| Briar | ❌ P2P | ⚠️ только online P2P | ✅ | ✅ | ✅ |
| Berty | ❌ P2P | ⚠️ только online P2P | ✅ | ✅ | ✅ |
| **MUSH** | ❌ **нет** | ✅ **store-carry-forward** | ⚠️ **onion — дорожная карта; сейчас EID/размер/время видимы (§2.1)** | ✅ | ✅ |

**MUSH — первый мессенджер, который работает без сети и без серверов одновременно.**

---

## 5. Спам: структурно затруднён, CUSUM-детекция, иммунитет — последний рубеж

**MUSH не делает спам невозможным. MUSH делает его дорогим и осознанным. Если спам прорывается — срабатывают прикладные спам-сенсоры S1–S7 (ATTACK-MODEL §18) и иммунная FSM с тихим режимом.**

### 5.1. Формула: Структура → Сенсоры (CUSUM) → Иммунитет (FSM + тихий режим)

```
Структурные барьеры (QR + SBP-handshake + грибница)
        ↓
Если прорвался → Сенсоры S1–S7 (CUSUM-накопление, не порог)
        ↓
Тихий режим (silent) → FSM эскалация → Quarantine
        ↓
Пользовательское решение (ручной блок)
```

### 5.2. Шесть векторов спама

| # | Вектор | Как работает | Поверхность атаки |
|---|--------|-------------|-------------------|
| 1 | **Компрометация существующего контакта** | Контакт Боба захвачен. Атакующий отправляет спам всем, кто знает Боба. | Утечка ключа Боба, side-channel экстракция. |
| 2 | **Accept-bundle flooding** | После SBP-handshake атакующий отправляет легитимные accept-bundle'ы на десятки ключей, полученных из утечки. | Нарушение контракта: accept-bundle должен следовать за реальным физическим обменом. |
| 3 | **Channel invitation spam** | Владелец грибницы рассылает приглашения в канал всем, кого может достать. | Приглашение — это bundle. Массовая рассылка приглашений = спам. |
| 4 | **Fruiting Gateway abuse** | Владелец грибницы рассылает сообщения участникам своего канала. | Грибница = точка концентрации. Модератор злоупотребляет relay. |
| 5 | **Relay poisoning** | Скомпрометированный relay-узел модифицирует bundle'ы при пересылке. | Relay не должен трогать payload. |
| 6 | **Contact list poisoning** | Атакующий "засеивает" контакт-лист жертвы ложными accept-bundle'ами. | Нарушение контракта двухфазного SBP. |

### 5.3. Шесть спам-специфичных сенсоров (CUSUM) — прикладной уровень

> Сенсоры S1–S7 — **прикладной уровень** MUSH (спам в мессенджере), см. ATTACK-MODEL §18. **11 сетевых сенсоров** (SNSAnomaly, BeaconConsistency, GradientVelocity, E2ETracking, ResourcePressure, HandshakeRate, ContactQuality, QuorumConsistency, TimeConsistency, FitnessPlausibility, PoolDiversity) — сетевой уровень Mycerus, см. ATTACK-MODEL §7 и этапы 19–19A ТЗ.

Все сенсоры работают на **CUSUM** (Cumulative Sum) — накопление отклонения, а не пороговый срез. Это критично в DTN, где нет real-time потока.

```rust
/// CUSUM-детектор: накапливает отклонение от baseline
/// Параметры k, h — НЕ из генома: whitelist мутируемых полей GENOME-SPEC §8
/// их не содержит. Дефолты задаются ATTACK-MODEL; (k, h) — кандидат на
/// immune-блок генома (этап 22), сейчас — фиксированные значения.
pub struct CUSUMSensor {
    baseline_rate: f64,              // медиана msg/контакт от этого ключа
    k: f64,                          // slack (минимальное отклонение для накопления)
    h: f64,                          // threshold (когда CUSUM срабатывает)
    cusum_pos: f64,                  // текущее положительное накопление
    cusum_neg: f64,                  // текущее отрицательное накопление
}

impl CUSUMSensor {
    pub fn observe(&mut self, observed: f64) -> Option<Alarm> {
        let deviation = observed - self.baseline_rate;
        self.cusum_pos = (self.cusum_pos + deviation - self.k).max(0.0);
        self.cusum_neg = (self.cusum_neg - deviation - self.k).max(0.0);

        if self.cusum_pos > self.h {
            Some(Alarm::HighActivity { score: self.cusum_pos })
        } else if self.cusum_neg > self.h {
            Some(Alarm::LowActivity { score: self.cusum_neg })
        } else {
            None
        }
    }
}
```

| Сенсор | Что измеряет | CUSUM-параметр | Примечание |
|--------|-------------|----------------|------------|
| **ContactRateSensor (S1)** | Частота сообщений от одного контакта | `observed = msg_per_contact` | Бaseline пересчитывается при каждом контакте. Одиночные всплески не срабатывают — нужно накопление. |
| **ContentDiversitySensor (S2)** | Дубликаты сообщений | `observed = duplicate_ratio` | Одинаковые сообщения (copy-paste) накапливают отклонение быстрее, чем уникальные. |
| **AcceptRequestSensor (S3)** | Частота accept-bundle'ов | `observed = accepts_per_window` | Аномалия: ключ принимает больше контактов, чем исторически. |
| **InvitationRateSensor (S4)** | Частота приглашений в каналы | `observed = invites_per_window` | Применяется к Fruiting Gateway (кто рассылает). |
| **RecipientDiversitySensor (S5)** | Число уникальных получателей | `observed = unique_recipients_per_window` | Требует gossip-аттестаций (см. ниже). |
| **GribnitsaReputationSensor (S6)** | Репутация грибницы-хоста | `observed = complaint_ratio` | Участники канала голосуют "доверие/недоверие" грибнице. |

**Почему CUSUM:**
- В DTN bundle уже в пути — real-time блокировка невозможна.
- CUSUM реагирует на **накопленное отклонение**, а не на мгновенный скачок (защита от threshold gaming, атака B5 — ATTACK-MODEL).
- Параметры (k, h) фиксированы по ATTACK-MODEL; вынос их в геном (immune-блок генома) — кандидат на этап 22, не раньше.

### 5.4. DTN-специфика спам-детекции

| DTN-ограничение | Как адаптируем |
|-----------------|----------------|
| **Нет real-time** | Bundle уже в пути. Блокировка retroactive — следующие bundle'ы уже отфильтрованы. |
| **Локальная защита** | Сенсор на одном узле не знает о сенсоре на другом. Каждый узел принимает независимое решение. Глобальный вердикт — необязательная роскошь. |
| **Gossip-аттестации** | RecipientDiversitySensor требует: получатели сравнивают, от кого они получали спам. Это gossip между контактами. **Медленно**, **ненадёжно**, но **единственный способ** получить полную картину. |

### 5.5. Разделение evidence: Verifiable vs Reputational

> Каноническое правило — ATTACK-MODEL §2/§14. **Текущий режим — reputation + quorum; цель — verifiable-only.**

| Тип | Что это | Пример | Как используется |
|-----|---------|--------|------------------|
| **Verifiable** | Объективные факты, верифицируемые третьей стороной | Probe-бандлы с известным dst, подписанные onion/e2e-отчёты, accept-flooding с наблюдаемыми bundle'ами | Формирует `Threat`-утверждение с `evidence_hash`. |
| **Reputational** | Статистические обвинения, напрямую не проверяемые | CUSUM-накопление, «это спам» (кнопка «пожаловаться») | Не изолирует узел само по себе; накапливает strikes. |

**Правило силы доказательств (канон):**
- **`quarantined` — на verifiable evidence ИЛИ на кворуме N≥3 с reputational evidence** (ATTACK-MODEL §14). Не «только verifiable» — это целевая модель, текущая — reputation+quorum.
- `watch`/`degraded`/`silent` — допустимы на reputational evidence с кворумом N=2 (топологически независимые сенсоры).
- **Апелляции — только против verifiable quarantine** (ATTACK-MODEL §13.2): сенсор/узел оспаривает strike, предъявив доказательства независимости; кворум с повышенным порогом N=3.

### 5.6. Trust FSM — шесть состояний (канон — ATTACK-MODEL §8)

Канонический Trust FSM Mycerus (trusted → watch → silent → degraded → quarantined → probation) для DTN мягче классического: bundle уже в пути, резкая блокировка бесполезна.

```
trusted ──► watch ──► silent ──► degraded ──► quarantined ──► probation
              │          │           │                              │
              └──────────┴───────────┘                              │
                 (ручной сброс пользователем)                       │
                              ▲◀──────── истечение probation, чистая история
                              └────────────── (полное восстановление)
```

- **trusted**: полное участие.
- **watch**: один сенсор с rep>0 поднял тревогу. Логирование, без ограничений. TTL: часы. Пользователь не замечает.
- **silent**: мягкое состояние. Bundle'ы от контакта **принимаются**, но **не показываются** пользователю — лежат в теневом inbox (можно заглянуть и восстановить). В альтернативной конфигурации — приём с понижением приоритета. Сюда же попадает первый bundle от контакта **Unknown** (§2.2, §3).
- **degraded**: кворум N=2 топологически независимых сенсора. Каноническое действие: **пауза relay, игнор gradient'ов, снижение copies**. «Сообщения доходят медленнее» — это **наблюдаемое следствие** этих мер, а не определение состояния.
- **quarantined**: повторный кворум (новые сигналы) или verifiable evidence. Полная изоляция. Узел-источник — по **ключу**, не по payload (invariant Immune Block 202).
- **probation**: после истечения TTL quarantine. Ограниченное участие, усиленный мониторинг. При чистой истории — возврат в trusted.

**Bridge-hostage guard** (degraded/quarantined для local bridge) — см. ATTACK-MODEL §8.1: forced degraded + rerouting campaign (Immune Block subtype 4) вместо немедленной изоляции.

**Эскалация:**
- ContactRateSensor CUSUM срабатывает → `watch`.
- CUSUM продолжает накапливать → `silent` (тихий режим).
- Кворум сенсоров подтверждает → `degraded`.
- Повторный кворум или verifiable evidence → `quarantined`.

**Апелляция:**
- Пользователь может вручную перевести контакт из `silent` обратно в `trusted`.
- Это сбрасывает CUSUM-накопление для этого контакта на этом узле.
- Другие узлы не знают об апелляции — их CUSUM не сбрасывается.
- Против verifiable quarantine — формальная апелляция по ATTACK-MODEL §13.2 (кворум N=3).

### 5.7. Пользовательский UX при спаме

| Сценарий | Что видит пользователь |
|----------|----------------------|
| Контакт в `watch` | Ничего. Фоновое логирование. |
| Контакт в `silent` | **Явный индикатор:** "Контакт X: 3 сообщения скрыты (подозрение на аномалию). [Показать] [Блокировать]". При первом скрытии — уведомление: "MUSH временно скрыл сообщения от X. Проверить?". |
| Контакт в `degraded` | Красный индикатор (🔴 "контакт ограничен сетью: relay приостановлен"). Задержки доставки — следствие мер против узла, а не «медленный контакт». |
| Контакт в `quarantined` | "Контакт временно заблокирован сетью". Можно разблокировать вручную. |
| Контакт в `probation` | Индикатор восстановления: "Контакт X на испытательном сроке, мониторинг усилен". |
| Кнопка "пожаловаться" | Пользователь нажимает → генерируется reputational evidence (не quarantine). Увеличивает strike count. |
| **Опция "Не скрывать"** | В настройках: "При подозрении — только предупреждать, не скрывать сообщения". Тогда `silent` → `watch` + уведомление. |

**Shadow inbox cap:**
- Максимум 100 сообщений на контакт в теневом inbox.
- Максимум 7 дней хранения.
- При превышении — oldest-first eviction (удаление старых).
- Предотвращает молчаливое исчерпание памяти спам-сообщениями.

**Лог иммунной активности (для продвинутых):**
```
[14:32] ContactRateSensor (S1): Боб (key: abc123) — CUSUM превышен (score: 3.2)
[14:35] Контакт переведён в silent mode (теневой inbox)
[14:40] Пользователь заглянул в теневой inbox, восстановил контакт
[14:40] CUSUM сброшен для Боба на этом устройстве
```

### 5.8. Контрмеры (помимо FSM)

| Контрмера | Когда | Как |
|-----------|-------|-----|
| **Accept-bundle throttle** | AcceptRequestSensor (S3) срабатывает | Лимит на число accept'ов от одного ключа за окно (например, 1/час). |
| **Channel mute** | InvitationRateSensor (S4) срабатывает | Возможность "заглушить" канал (не показывать уведомления, но оставаться участником). |
| **Rate limiting** | ContactRateSensor (S1) срабатывает | Снижение приоритета bundle'ов от спамера в очереди relay. |

### 5.9. Граница: что иммунитет не делает

- **Не читает содержимое.** E2E запрещает. Все сенсоры — поведенческие.
- **Не блокирует первое сообщение.** Контакт в `silent` — это не блокировка, это скрытие. Пользователь всегда может заглянуть.
- **Не действует глобально.** Каждый узел решает локально. Глобальный вердикт — роскошь, не необходимость.
- **Не заменяет человеческое решение.** Пользователь всегда может вручную заблокировать или разблокировать.
- **Не бьёт по payload.** Санкции накладываются на ключ узла (Immune Block 202, ATTACK-MODEL §6), сообщения не помечаются.

---

## 6. Edge-кейсы и митигации

| Кейс | Описание | Митигация |
|------|----------|-----------|
| **Экономическая атака на грибницы** | Атакующий поднимает 1000 фальшивых грибниц с открытыми каналами, надеясь, что кто-то подпишется. | Грибница стоит реальных ресурсов (энергия, хранение). Канал рекомендуется только тем, кого ты уже знаешь. Спонтанная подписка — только через явный QR. |
| **Приглашения в группы** | Как пригласить нового участника? Это потенциальный вектор. | Приглашение — это bundle со специальным типом, которое участник должен **явно принять** (accept-bundle). Отклонённое приглашение не создаёт обязательств. |
| **Публичные каналы** | Если они есть — как их найти? | Каталог грибниц через web-of-trust. Не глобальный поиск, а «каналы, которые рекомендуют мои контакты». Это как в раннем интернете — ссылки, а не поисковик. |
| **Форк канала** | Участник недоволен модерацией и хочет свой канал с той же историей. | CRDT-состояние переносится. Участник fork'ает канал на своей грибнице с полной историей. Старые участники могут перейти или остаться. |
| **Потеря грибницы-хоста** | Владелец выключил узел, канал исчез. | Участники могут голосовать за новый хост (CRDT-операция). Или участник с полной историей (последний sync) поднимает новую грибницу. |
| **Позднее присоединение к ad-hoc группе** | Новый участник хочет войти после создания. | QR группы от любого текущего участника (содержит group_key + свободный sender_id от allocator'а) либо key rotation создателем. Схема координации nonce — MUSH-A'-SPEC §4.1. |

---

## 7. Что нужно доработать в Mycerus

Фактическое состояние Mycerus по иммунным этапам: Immune Block 202 (каркас: Threat/Quarantine/Revoke/Reroute, revocation-лист) — **реализован этапом 18** (ТЗ); подпись e2e-отчётов ReportE2EDelivery (Ed25519 над bundle_id, dst, delivered_at, receiver_key) — **реализована этапом 18**; сенсорный фреймворк и 11 сетевых сенсоров — **этап 19/19A** (фреймворк — 19A). Остальное immune plane — этапы 20–22 (response protocol, attack harness, immune memory).

| Доработка | Приоритет | Статус / этап | Описание |
|-----------|-----------|---------------|----------|
| **Onion wrapper поверх MEB** | 🔴 Высокий | После 22 + MEB v2 | Шифрование/сокрытие MEB-хинтов per-hop. **Открытый вопрос:** gradient routing требует видимого destination (§2.1) — требуется MEB-SPEC v0.2 и новое решение маршрутизации. |
| **Padding bundle** | 🔴 Высокий | После 22 + MEB v2 | Фиксированные размеры (1K/4K/16K) для всех payload'ов. |
| **Cover traffic generator** | 🟡 Средний | После 22 + MEB v2 | Фоновые dummy bundle'ы, неотличимые от реальных. Только при питании/зарядке. |
| **CRDT Chat Store** | 🔴 Высокий | MUSH-side | OR-Map + LWW для истории сообщений. Merge при получении. |
| **Fruiting Gateway protocol** | 🔴 Высокий | MUSH-side | Протокол хостинга каналов: приём клиентов, relay, CRDT-snapshot, модерация. |
| **Group CRDT protocol** | 🟡 Средний | MUSH-side | OR-Set для участников, key rotation, chunk-based sync, accept-bundle. |
| **Spore Bundle Protocol (SBP)** | 🟡 Средний | MUSH-side | Формат encrypted snapshot для backup, multi-device и двухфазного контакта. |
| **Offline Double Ratchet** | 🔴 Высокий | Требует отдельной спеки | X3DH + pre-keys + chain-based key derivation для DTN. |
| **Pre-key distribution** | 🟡 Средний | MUSH-side | Распространение одноразовых pre-keys через Mycerus mesh. Gate: Known-only (§3). |
| **Privacy-aware MEB hints** | 🟡 Средний | Открытый вопрос | Замена точных destination_affinity на privacy_zone — несовместима с gradient routing; требует MEB v2 (§2.1). |
| **ContactRateSensor (S1)** | 🟡 Средний | Прикладной, ATTACK-MODEL §18 | CUSUM: частота сообщений от одного контакта. Baseline адаптивный. |
| **ContentDiversitySensor (S2)** | 🟡 Средний | Прикладной, ATTACK-MODEL §18 | CUSUM: дубликаты сообщений. |
| **AcceptRequestSensor (S3)** | 🟡 Средний | Прикладной, ATTACK-MODEL §18 | CUSUM: частота accept-bundle'ов. |
| **InvitationRateSensor (S4)** | 🟡 Средний | Прикладной, ATTACK-MODEL §18 | CUSUM: приглашения в канал от Fruiting Gateway. |
| **RecipientDiversitySensor (S5)** | 🟡 Средний | Прикладной, ATTACK-MODEL §18 | CUSUM: число уникальных получателей. Требует gossip-аттестаций. |
| **GribnitsaReputationSensor (S6)** | 🟡 Средний | Прикладной, ATTACK-MODEL §18 | CUSUM: репутация грибницы. |
| **MUSH Payload Block (тип 203)** | 🟢 Низкий | Зарезервирован | Регистрация типа блока в BPv7 для MUSH сообщений; требует отдельной спецификации (payload_type enum — предварительно). |
| **Сенсорный фреймворк + 11 сетевых сенсоров** | 🔴 Высокий | Этап 19/19A (сетевой уровень, ATTACK-MODEL §7) | MAD-baseline + CUSUM-накопители. |

---

## 8. Честные ограничения

| Ограничение | Почему так | Как митигация |
|-------------|-----------|---------------|
| **Задержка** | Store-carry-forward = eventual delivery. Сообщение может прийти через секунды, часы или дни. | Позиционирование: "гарантированная доставка" — неверно; "высокая вероятность доставки до истечения TTL". Приоритетные сообщения (SOS) получают spray_max + highest priority. |
| **Группы = инфраструктура** | Нет чужого сервера = хочешь группу, поднимай свою грибницу (Fruiting Gateway). Это требует ресурсов и ответственности. | Грибница-хост = выделенный узел. Участники синхронизируются при контакте. Хост модерирует relay, но не может подделать историю (CRDT + подписи). Fork канала возможен. |
| **Мульти-устройство** | Нет облака = sync через споры. Новое устройство подбирает snapshot из сети. | Spore с высоким TTL (7 дней), явное "добавление устройства" через QR-код (как добавление контакта, но для себя). |
| **Метаданные** | Факт встречи двух узлов виден. Onion скрывает маршрут, но не факт контакта. **В текущей сети onion нет** — посредник видит EID источника/назначения, размер, время (§2.1). | Cover traffic делает факт контакта неопределённым (встреча могла быть для dummy). Полное сокрытие метаданных — после этапа 22 + MEB v2. |
| **Спам** | **Структурно затруднён + иммунитет.** Нет глобального адресного пространства, нет публичных ID. Сообщение только после SBP-handshake. Если спам прорывается (утечка ключей, скомпрометированный контакт) — сенсоры S1–S7 (ATTACK-MODEL §18) и Immune plane (этапы 19–22) детектируют аномалию и применяют точечную контрмеру по ключу узла. | Три уровня: структурный (адресация = разрешение), ресурсный (отправка стоит энергии), иммунный (сенсоры + FSM). |
| **Батарея** | BLE/WiFi-Direct в фоне = расход. | Energy-aware transport (см. 2.4). Relay-режим опционален. Cover traffic только при зарядке. |

---

## 9. Роадmap: от архитектуры к коду

**Синхронизация с Mycerus (иммунные этапы 19–22; см. ТЗ и ATTACK-MODEL §10):**

| MUSH | Mycerus | Что строим |
|-----------|---------|-----------|
| **Этап A'** | Вне скоупа MVP (ТЗ §3.2) | Демо ad-hoc группы: BPv7-lite + hotspot/QR + symmetric crypto + CRDT, без хоста. Параллельный трек; для иммунных функций опирается на этап 19A. Спека: MUSH-A'-SPEC.md |
| **Этап A** | База: этапы 1–19A | Core: Payload Block (тип 203, после спецификации), Offline Double Ratchet, status tracking (подписанный ReportE2EDelivery — готов, этап 18) |
| **Этап B** | Этап 20 (Response Protocol) | Vault + Chats + immune UX: SQLCipher, CRDT Chat Store, Identicon, Safety Number, silent inbox, апелляции (ATTACK-MODEL §13.2) |
| **Этап C** | После этапа 22 + MEB v2 | Onion + Transport: per-hop wrapper, padding, cover traffic, BLE adapter. Заблокировано открытым вопросом §2.1 (маршрутизация без видимого destination) |
| **Этап D** | Этап 21 (Attack harness) | Groups + Fruiting Gateway + harness-сценарии J/K/L (спам-векторы §5.2 как вакцинация) |
| **Этап E** | Этап 22 (Immune memory) | Resilient: интеграция immune memory, immune-блок генома (CUSUM k,h — кандидат), stress-test, audit |

### Этап A'. Ad-hoc группа (2 недели) — демо-прототип, вне скоупа MVP

**Зачем:** проверить базовый UX DTN-мессенджера до добавления сложности. Если два телефона в одной комнате не могут удобно переписываться — immune plane не спасёт.

- [ ] Ad-hoc группа: CRDT-чат без хоста, QR-обмен ключами (спека — MUSH-A'-SPEC.md)
- [ ] Hotspot transport для Android (fallback — BLE)
- [ ] Базовый UI: список сообщений, ввод, статус "доставлено"
- [ ] **Демо:** 2–4 телефона в одной комнате, групповой чат без интернета и сервера

**Чего НЕТ в этом этапе:** Fruiting Gateway, Immune plane, Vironet, Onion, Double Ratchet, Spores. Только plain BPv7 + symmetric key + CRDT. Это **не продукт и не часть MVP Mycerus** — ТЗ §3.2.

### Этап A. MUSH Core (4 недели) → база: этапы 1–19A
- [ ] MUSH Payload Block (тип 203) — после отдельной спецификации
- [ ] Offline Double Ratchet (X3DH + pre-keys) — после отдельной спецификации
- [ ] MEB по MEB-SPEC v0.1.2 §4 (без privacy-aware модификаций — см. §2.1)
- [ ] Message status tracking (⏳ / ✓ / ✓✓) через SRE receipts + подписанный ReportE2EDelivery (реализован этапом 18)
- [ ] **Демо:** два телефона, WiFi-Direct, текст, QR-контакты

### Этап B. MUSH Vault + Chats + immune UX (4 недели) → Mycerus 20
- [ ] SQLCipher store для сообщений
- [ ] CRDT Chat Store (OR-Map + LWW)
- [ ] Identicon generation
- [ ] Safety Number verification
- [ ] Silent inbox UX (§5.6/§5.7) + апелляции (ATTACK-MODEL §13.2)
- [ ] **Демо:** история сообщений, offline доставка, immune UX при спаме

### Этап C. MUSH Onion + Transport (4 недели) → после 22 + MEB v2
- [ ] Onion wrapper per-hop — **заблокировано открытым вопросом §2.1**
- [ ] Padding до фиксированных размеров
- [ ] Cover traffic generator (только при зарядке)
- [ ] BLE transport adapter
- [ ] Energy-aware switching
- [ ] **Демо:** 3+ hop'а через mesh, метаданные скрыты

### Этап D. MUSH Groups + Fruiting Gateway (4 недели) → Mycerus 21
- [ ] Fruiting Gateway protocol (хостинг каналов)
- [ ] Group CRDT protocol + accept-bundle
- [ ] Key rotation для групп
- [ ] Spore Bundle Protocol (SBP) для backup и multi-device
- [ ] Harness-сценарии J/K/L: векторы спама §5.2 как вакцинация (этап 21)
- [ ] **Демо:** групповой чат 5 человек на Fruiting Gateway, backup/restore

### Этап E. MUSH Resilient (4 недели) → Mycerus 22
- [ ] Интеграция Immune plane (тип 202) — CUSUM-сенсоры S1–S6 (прикладной уровень, ATTACK-MODEL §18), тихий режим; сетевые сенсоры — этапы 19–19A
- [ ] Интеграция Immune memory (этап 22); CUSUM (k, h) — кандидат на immune-блок генома
- [ ] Интеграция Vironet (адаптивная маршрутизация)
- [ ] Desktop/Web клиент
- [ ] Stress-test: 100+ узлов, 24 часа, эмуляция атак
- [ ] Security audit

---

*Mycerus — это мицелий. MUSH — это ягода. Пользователь видит ягоду. Под землёй — вся сеть, которая делает так, чтобы ягода всегда была.*
