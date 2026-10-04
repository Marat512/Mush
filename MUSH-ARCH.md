# MUSH-ARCH — техническая архитектура

**Версия:** 0.2.1  
**Дата:** 2026-09-29  
**Статус:** Техническая спецификация — компоненты, протоколы, роадмап  
**База:** MUSH.md (манифест) + ATTACK-MODEL v0.3.0 + Mycerus MVP (этапы 1–16)  
**Связанные документы:**
- MUSH.md — манифест и пользовательское руководство
- MUSH-PROTOCOL.md — формальные спецификации протоколов
- MUSH-A'-SPEC.md — спецификация первого прототипа
- MUSH-A'-CLI-SPEC.md — Rust CLI прототип (промежуточный шаг)
- MUSH-BRIDGE-ARCH.md — справочник по адаптерам сторонних сетей (B-статус)

---

## 1. Принцип: пользователь видит ягоду, под землёй — грибница

| Человек видит | Под капотом (Mycerus) | Компонент Mush |
|---------------|----------------------|---------------------|
| Контакты | SNS + QR-handshake | `Mush Contacts` |
| Чаты | CRDT-история + bundle | `Mush Chats` |
| Отправить сообщение | MEB + POT + store-carry-forward | `Mush Core` |
| Доставлено | SRE receipt + ReportE2EDelivery | `Mush Core` |
| Настройки приватности | Genome constraints | `Mush Vault` |
| Резервная копия | SBP + споры | `Mush Spores` |
| Сеть рядом | POT + CLA + MGR gradient | `Mush Transport` |
| Групповой чат | Сервер-грибница (выделенный узел/узлы группы) | `Mush Groups` |
| Мульти-устройство | Sync через споры (encrypted snapshot) | `Mush Spores` |

**Пользователь никогда не видит:** BPv7, MEB, MGR, градиенты, Immune Block, Vironet. Он видит чаты, которые всегда работают.

---

## 2. Компоненты Mush

### 2.1. Mush Core

**Задача:** Интерфейс между Mush и Mycerus DTN. Формирует bundle'ы, получает их из сети, управляет lifecycle сообщения.

**Ложится на:** BPv7 Bundle Protocol + MEB (тип 200) + Immune Block (тип 202).

```rust
pub struct MuShCore {
    mycerus: MycerusNode,           // BPv7 + MGR + SNS + POT
    meb_adapter: MEBAdapter,         // формирование MEB с privacy-aware hints
    message_queue: MessageQueue,     // исходящие, ожидающие контакта
    delivery_tracker: DeliveryTracker, // отслеживание ✓ / ✓✓ / ⏳
}

impl MuShCore {
    /// Отправляет сообщение в сеть
    pub fn send(&mut self, msg: MuShMessage, dst: PubKey) -> MessageID {
        // 1. Шифруем payload через Mush Onion
        let encrypted = self.onion.wrap(msg, dst);
        // 2. Формируем BPv7 bundle с MEB
        let bundle = self.build_bundle(encrypted, dst);
        // 3. Передаём в Mycerus для маршрутизации
        self.mycerus.submit(bundle);
        // 4. Возвращаем ID для отслеживания статуса
        msg.id
    }
    
    /// Вызывается при получении bundle из сети
    pub fn on_bundle_received(&mut self, bundle: Bundle) {
        // 1. Проверяем destination — нам ли?
        if !bundle.is_for_me(self.my_key) { return; }
        // 2. Расшифровываем через Onion
        let msg = self.onion.unwrap(bundle.payload);
        // 3. Передаём в Chat Store
        self.chat_store.insert(msg);
        // 4. Отправляем SRE-receipt (если запрошено)
        self.send_receipt(bundle.source, bundle.id);
    }
}
```

**Privacy-aware MEB hints:** MEB обычно несёт `destination_affinity`, `visited`, `path_cost`. Для Mush эти поля — утечка метаданных. Поэтому:
- `destination_affinity` заменяется на `privacy_zone` (грубая категория: local / regional / global), не конкретный destination.
- `visited` хранится внутри Onion-обёртки, не в MEB.
- `path_cost` вычисляется динамически по gradient, но не публикуется в bundle.

### 2.2. Mush Contacts — двухфазный SBP

**Задача:** Управление контактами с гарантией осознанного согласия.

**Ложится на:** SNS/SRE + QR/NFC handshake.

**Ключевой принцип:** В MyceRus «знать адрес» и «иметь право отправить» — это одно и то же. Нельзя узнать адрес случайно, нельзя угадать, нельзя получить из базы. Только через физический обмен (QR, BLE-handshake, встреча). Это как обмен ключами — не действие, а согласие.

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
    Known,        // фаза 2: SNS-handshake успешен, запись в адресный список
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

**Фаза 2 — Подтверждение контакта (SNS-handshake):**
```
Боб инициирует SNS-handshake с Алисой (challenge-response по Ed25519)
Алиса подтверждает → оба записывают друг друга в локальный адресный список
TrustLevel переходит в Known
```
- Без фазы 2 фаза 1 бесполезна — знание pubkey ≠ право отправки.
- SNS-handshake = взаимное согласие. Это защищает от «случайного» попадания в список.

**Приглашение через посредника:**
- Алиса хочет познакомить Боба с Верой.
- Алиса отправляет Бобу pubkey Веры (encrypted на pubkey Боба).
- Боб получает pubkey, но Contact = Unknown.
- Боб **должен сам инициировать SNS-handshake** с Верой.
- Вера **должна сама подтвердить** — никто не может добавить Веру без её согласия.

### 2.3. Mush Chats

**Задача:** Хранение истории, отображение, CRDT-синхронизация.

**Ложится на:** Bundle Payload Block (тип 203) + локальный Chat Store.

**Почему CRDT:**
- В DTN порядок доставки не гарантирован. Сообщение 2 может прийти раньше сообщения 1.
- Несколько устройств одного пользователя могут отправлять сообщения параллельно.
- CRDT (Conflict-free Replicated Data Type) позволяет сходиться к единой истории без центрального сервера.

```rust
pub struct ChatMessage {
    id: MessageID,                   // ULID (временная сортировка + уникальность)
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

### 2.4. Mush Transport

**Задача:** Управление CLA (Channel Layer Adapters), абстракция над радио.

**Ложится на:** POT + CLA (WiFi-Direct / BLE / LAN / LoRa).

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

### 2.5. Mush Vault

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

### 2.6. Mush Spores

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

### 2.7. Mush Onion

**Задача:** Защита метаданных. Relay-узлы не должны знать, кто отправитель и получатель, и не должны отличать настоящий трафик от фона.

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
    pub fn wrap(&self, msg: &MuShMessage, path: &[PubKey]) -> Bytes {
        // 1. Шифруем сообщение для получателя (внутренний слой)
        let inner = Noise::encrypt(msg, &path.last().unwrap());
        // 2. Для каждого intermediate hop: оборачиваем в слой
        let mut current = inner;
        for hop in path.iter().rev().skip(1) {
            current = self.wrap_layer(current, hop);
        }
        current
    }
}
```

**Паддинг:** все bundle'ы padding'ятся до ближайшего фиксированного размера (1 KB, 4 KB, 16 KB). Relay-узел не может определить, текст это или видео, по размеру.

**Cover traffic:** фоновая генерация dummy bundle'ов с тем же размером и структурой, что и реальные. Dummy'ы расходуются как обычные bundle'ы, но destination = случайный ключ (dead drop). Relay-узлы не отличают dummy от real.

```rust
pub struct CoverTraffic {
    rate: PoissonRate,               // lambda = N dummy per hour
    size_distribution: Vec<usize>,   // распределение размеров (как у реальных msg)
}
```

### 2.8. Mush Groups — три режима группового чата

MyceRus поддерживает три режима группового общения:

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

### 2.8.2. Fruiting Gateway — сервер-грибница

**Задача:** Групповые чаты через выделенную грибницу-хост.

**Ложится на:** CRDT-состояние + выделенный узел-грибница.

**Ключевой принцип:** В MyceRus нет "общего чата в воздухе". Хочешь канал — поднимай свою грибницу. Это Discord-модель, но с честной экономикой: не "сервер компании", а твоя грибница.

```rust
pub struct FruitingGateway {
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

### 2.9. Fruiting Gateway — роль сервера-грибницы

**Fruiting Gateway** — это выделенный узел Mycerus, который:
- Хостит канал/группу (хранит CRDT-состояние, relay'ит сообщения).
- Служит точкой входа для мобильных узлов (плодовых тел).
- Может иметь bridge в интернет (опционально).
- Несёт реальные ресурсы: хранение, CPU, энергия.

```rust
pub struct FruitingGateway {
    // Mycerus node
    mycerus: MycerusNode,
    
    // Hosted channels
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

**Адаптация — Offline Double Ratchet:**

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
    pub fn init_session(&mut self, recipient_pubkey: &PubKey) -> Session {
        // 1. Получаем один из pre-keys получателя из сети
        let pre_key = self.network.find_pre_key(recipient_pubkey);
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

**Pre-key distribution:**
- Каждый узел генерирует 100 одноразовых pre-keys.
- Pre-keys рассылаются в сеть как часть contact-info bundle (TTL = 30 дней, copies = spray_max).
- При исчерпании pre-keys — генерируются новые и рассылаются.

**Pre-keys и FSM:**
- При FSM `degraded`, `silent`, `quarantined` — публикация новых pre-keys прекращается.
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
| **Mush** | ❌ **нет** | ✅ **store-carry-forward** | ✅ **onion + padding + cover** | ✅ | ✅ |

**Mush — первый мессенджер, который работает без сети и без серверов одновременно.**

---

## 5. Спам: структурно затруднён, CUSUM-детекция, иммунитет — последний рубеж

**MyceRus не делает спам невозможным. MyceRus делает его дорогим и осознанным. Если спам прорывается — срабатывает иммунитет с CUSUM-детекцией и тихим режимом.**

### 5.1. Формула: Структура → Сенсоры (CUSUM) → Иммунитет (FSM + тихий режим)

```
Структурные барьеры (QR + SNS-handshake + грибница)
        ↓
Если прорвался → Сенсоры (CUSUM-накопление, не порог)
        ↓
Тихий режим (silent) → FSM эскалация → Quarantine
        ↓
Пользовательское решение (ручной блок)
```

### 5.2. Шесть векторов спама

| # | Вектор | Как работает | Поверхность атаки |
|---|--------|-------------|-------------------|
| 1 | **Компрометация существующего контакта** | Контакт Боба захвачен. Атакующий отправляет спам всем, кто знает Боба. | Утечка ключа Боба, side-channel экстракция. |
| 2 | **Accept-bundle flooding** | После SNS-handshake атакующий отправляет легитимные accept-bundle'ы на десятки ключей, полученных из утечки. | Нарушение контракта: accept-bundle должен следовать за реальным физическим обменом. |
| 3 | **Channel invitation spam** | Владелец грибницы рассылает приглашения в канал всем, кого может достать. | Приглашение — это bundle. Массовая рассылка приглашений = спам. |
| 4 | **Fruiting Gateway abuse** | Владелец грибницы рассылает сообщения участникам своего канала. | Грибница = точка концентрации. Модератор злоупотребляет relay. |
| 5 | **Relay poisoning** | Скомпрометированный relay-узел модифицирует bundle'ы при пересылке. | Relay не должен трогать payload. |
| 6 | **Contact list poisoning** | Атакующий "засеивает" контакт-лист жертвы ложными accept-bundle'ами. | Нарушение контракта двухфазного SBP. |

### 5.3. Шесть спам-специфичных сенсоров (CUSUM)

Все сенсоры работают на **CUSUM** (Cumulative Sum) — накопление отклонения, а не пороговый срез. Это критично в DTN, где нет real-time потока.

```rust
/// CUSUM-детектор: накапливает отклонение от baseline
/// Параметры из Vironet-genoma (могут мутировать под среду)
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
| **ContactRateSensor** | Частота сообщений от одного контакта | `observed = msg_per_contact` | Бaseline пересчитывается при каждом контакте. Одиночные всплески не срабатывают — нужно накопление. |
| **ContentDiversitySensor** | Дубликаты сообщений | `observed = duplicate_ratio` | Одинаковые сообщения (copy-paste) накапливают отклонение быстрее, чем уникальные. |
| **AcceptRequestSensor** | Частота accept-bundle'ов | `observed = accepts_per_window` | Аномалия: ключ принимает больше контактов, чем исторически. |
| **InvitationRateSensor** | Частота приглашений в каналы | `observed = invites_per_window` | Применяется к Fruiting Gateway (кто рассылает). |
| **RecipientDiversitySensor** | Число уникальных получателей | `observed = unique_recipients_per_window` | Требует gossip-аттестаций (см. ниже). |
| **GribnitsaReputationSensor** | Репутация грибницы-хоста | `observed = complaint_ratio` | Участники канала голосуют "доверие/недоверие" грибнице. |

**Почему CUSUM:**
- В DTN bundle уже в пути — real-time блокировка невозможна.
- CUSUM реагирует на **накопленное отклонение**, а не на мгновенный скачок.
- Параметры (k, h) могут быть эволюционными (Vironet мутирует их под среду).

### 5.4. DTN-специфика спам-детекции

| DTN-ограничение | Как адаптируем |
|-----------------|----------------|
| **Нет real-time** | Bundle уже в пути. Блокировка retroactive — следующие bundle'ы уже отфильтрованы. |
| **Локальная защита** | Сенсор на одном узле не знает о сенсоре на другом. Каждый узел принимает независимое решение. Глобальный вердикт — необязательная роскошь. |
| **Gossip-аттестации** | RecipientDiversitySensor требует: получатели сравнивают, от кого они получали спам. Это gossip между контактами. **Медленно**, **ненадёжно**, но **единственный способ** получить полную картину. |

### 5.5. Разделение evidence: Verifiable vs Reputational

| Тип | Что это | Пример | Как используется |
|-----|---------|--------|----------------|
| **Verifiable** | Объективные факты, верифицируемые любым узлом | Частота сообщений, дубликаты, accept-flooding | Формирует `Threat`-bundle с `evidence_hash`. Quarantine только на verifiable evidence. |
| **Reputational** | Субъективное мнение пользователя | "Это спам" (кнопка "пожаловаться") | Не вызывает quarantine. Накапливает strike в таблице репутации. При ≥N strike'ов — сенсор теряет вес. |

**Принцип:** Quarantine — это тяжёлое решение. Оно требует **verifiable evidence**. "Мне не нравится" — это reputational evidence, оно не может изолировать узел.

### 5.6. Тихий режим (Silent Mode) в FSM

Традиционный FSM: `trusted → watch → degraded → quarantined`. В DTN это слишком резко — bundle уже в пути.

**Расширенный FSM для спам-контактов:**

```
trusted ──► watch ──► silent ──► degraded ──► quarantined
              │          │           │
              └──────────┴───────────┘
                 (ручной сброс пользователем)
```

- **watch**: логирование, без ограничений. Пользователь не замечает.
- **silent**: bundle'ы от контакта **принимаются**, но **не показываются** пользователю. Они лежат в теневом inbox. Пользователь может заглянуть и восстановить.
- **degraded**: relay-приоритет снижен. Сообщения доходят, но медленнее.
- **quarantined**: полный отказ в relay. Уже принятые bundle'ы остаются.

**Эскалация:**
- ContactRateSensor CUSUM срабатывает → `watch`.
- CUSUM продолжает накапливать → `silent` (тихий режим).
- Кворум сенсоров подтверждает → `degraded`.
- Повторный кворум + bridge-guard проверка → `quarantined`.

**Апелляция:**
- Пользователь может вручную перевести контакт из `silent` обратно в `trusted`.
- Это сбрасывает CUSUM-накопление для этого контакта на этом узле.
- Другие узлы не знают об апелляции — их CUSUM не сбрасывается.

### 5.7. Пользовательский UX при спаме

| Сценарий | Что видит пользователь |
|----------|----------------------|
| Контакт в `watch` | Ничего. Фоновое логирование. |
| Контакт в `silent` | **Явный индикатор:** "Контакт X: 3 сообщения скрыты (подозрение на аномалию). [Показать] [Блокировать]". При первом скрытии — уведомление: "Mush временно скрыл сообщения от X. Проверить?". |
| Контакт в `degraded` | Красный индикатор (🔴 "контакт ограничен"). Сообщения приходят с задержкой. |
| Контакт в `quarantined` | "Контакт временно заблокирован сетью". Можно разблокировать вручную. |
| Кнопка "пожаловаться" | Пользователь нажимает → генерируется reputational evidence (не quarantine). Увеличивает strike count. |
| **Опция "Не скрывать"** | В настройках: "При подозрении — только предупреждать, не скрывать сообщения". Тогда `silent` → `watch` + уведомление. |

**Shadow inbox cap:**
- Максимум 100 сообщений на контакт в теневом inbox.
- Максимум 7 дней хранения.
- При превышении — oldest-first eviction (удаление старых).
- Предотвращает молчаливое исчерпание памяти спам-сообщениями.

**Лог иммунной активности (для продвинутых):**
```
[14:32] ContactRateSensor: Боб (key: abc123) — CUSUM превышен (score: 3.2)
[14:35] Контакт переведён в silent mode (теневой inbox)
[14:40] Пользователь заглянул в теневой inbox, восстановил контакт
[14:40] CUSUM сброшен для Боба на этом устройстве
```

### 5.8. Контрмеры (помимо FSM)

| Контрмера | Когда | Как |
|-----------|-------|-----|
| **Accept-bundle throttle** | AcceptRequestSensor срабатывает | Лимит на число accept'ов от одного ключа за окно (например, 1/час). |
| **Channel mute** | InvitationRateSensor срабатывает | Возможность "заглушить" канал (не показывать уведомления, но оставаться участником). |
| **Rate limiting** | ContactRateSensor срабатывает | Снижение приоритета bundle'ов от спамера в очереди relay. |

### 5.9. Граница: что иммунитет не делает

- **Не читает содержимое.** E2E запрещает. Все сенсоры — поведенческие.
- **Не блокирует первое сообщение.** Контакт в `silent` — это не блокировка, это скрытие. Пользователь всегда может заглянуть.
- **Не действует глобально.** Каждый узел решает локально. Глобальный вердикт — роскошь, не необходимость.
- **Не заменяет человеческое решение.** Пользователь всегда может вручную заблокировать или разблокировать.

---

## 6. Edge-кейсы и митигации

| Кейс | Описание | Митигация |
|------|----------|-----------|
| **Экономическая атака на грибницы** | Атакующий поднимает 1000 фальшивых грибниц с открытыми каналами, надеясь, что кто-то подпишется. | Грибница стоит реальных ресурсов (энергия, хранение). Канал рекомендуется только тем, кого ты уже знаешь. Спонтанная подписка — только через явный QR. |
| **Приглашения в группы** | Как пригласить нового участника? Это потенциальный вектор. | Приглашение — это bundle со специальным типом, которое участник должен **явно принять** (accept-bundle). Отклонённое приглашение не создаёт обязательств. |
| **Публичные каналы** | Если они есть — как их найти? | Каталог грибниц через web-of-trust. Не глобальный поиск, а «каналы, которые рекомендуют мои контакты». Это как в раннем интернете — ссылки, а не поисковик. |
| **Форк канала** | Участник недоволен модерацией и хочет свой канал с той же историей. | CRDT-состояние переносится. Участник fork'ает канал на своей грибнице с полной историей. Старые участники могут перейти или остаться. |
| **Потеря грибницы-хоста** | Владелец выключил узел, канал исчез. | Участники могут голосовать за новый хост (CRDT-операция). Или участник с полной историей (последний sync) поднимает новую грибницу. |

---

## 7. Что нужно доработать в Mycerus

| Доработка | Приоритет | Статус | Описание |
|-----------|-----------|--------|----------|
| **Onion wrapper поверх MEB** | 🔴 Высокий | Новое | Шифрование MEB-хинтов per-hop. Relay-узлы не видят destination. |
| **Padding bundle** | 🔴 Высокий | Новое | Фиксированные размеры (1K/4K/16K) для всех payload'ов. |
| **Cover traffic generator** | 🟡 Средний | Новое | Фоновые dummy bundle'ы, неотличимые от реальных. |
| **CRDT Chat Store** | 🔴 Высокий | Новое | OR-Map + LWW для истории сообщений. Merge при получении. |
| **Fruiting Gateway protocol** | 🔴 Высокий | Новое | Протокол хостинга каналов: приём клиентов, relay, CRDT-snapshot, модерация. |
| **Group CRDT protocol** | 🟡 Средний | Новое | OR-Set для участников, key rotation, chunk-based sync, accept-bundle. |
| **Spore Bundle Protocol (SBP)** | 🟡 Средний | Новое | Формат encrypted snapshot для backup, multi-device и двухфазного контакта. |
| **Offline Double Ratchet** | 🔴 Высокий | Новое | X3DH + pre-keys + chain-based key derivation для DTN. |
| **Pre-key distribution** | 🟡 Средний | Новое | Распространение одноразовых pre-keys через Mycerus mesh. |
| **Privacy-aware MEB hints** | 🟡 Средний | Новое | Замена точных destination_affinity на privacy_zone. |
| **ContactRateSensor (S1)** | 🟡 Средний | Новое | CUSUM: частота сообщений от одного контакта. Baseline адаптивный. |
| **ContentDiversitySensor (S2)** | 🟡 Средний | Новое | CUSUM: дубликаты сообщений. Copy-paste накапливает быстрее уникальных. |
| **AcceptRequestSensor (S3)** | 🟡 Средний | Новое | CUSUM: частота accept-bundle'ов. Аномалия = больше контактов, чем исторически. |
| **InvitationRateSensor (S4)** | 🟡 Средний | Новое | CUSUM: приглашения в канал от Fruiting Gateway. |
| **RecipientDiversitySensor (S5)** | 🟡 Средний | Новое | CUSUM: число уникальных получателей. Требует gossip-аттестаций. |
| **GribnitsaReputationSensor (S6)** | 🟡 Средний | Новое | CUSUM: репутация грибницы. Участники голосуют доверие/недоверие. |
| **Mush Payload Block (тип 203)** | 🟢 Низкий | Новое | Регистрация типа блока в BPv7 для Mush сообщений. |

---

## 8. Честные ограничения

| Ограничение | Почему так | Как митигация |
|-------------|-----------|---------------|
| **Задержка** | Store-carry-forward = eventual delivery. Сообщение может прийти через секунды, часы или дни. | Позиционирование: "гарантированная доставка", не "мгновенная". Приоритетные сообщения (SOS) получают spray_max + highest priority. |
| **Группы = инфраструктура** | Нет чужого сервера = хочешь группу, поднимай свою грибницу (Fruiting Gateway). Это требует ресурсов и ответственности. | Грибница-хост = выделенный узел. Участники синхронизируются при контакте. Хост модерирует relay, но не может подделать историю (CRDT + подписи). Fork канала возможен. |
| **Мульти-устройство** | Нет облака = sync через споры. Новое устройство подбирает snapshot из сети. | Spore с высоким TTL (7 дней), явное "добавление устройства" через QR-код (как добавление контакта, но для себя). |
| **Метаданные** | Факт встречи двух узлов виден. Onion скрывает маршрут, но не факт контакта. | Cover traffic делает факт контакта неопределённым (встреча могла быть для dummy). |
| **Спам** | **Структурно затруднён + иммунитет.** Нет глобального адресного пространства, нет публичных ID. Сообщение только по известному адресу после SNS-handshake. Если спам прорывается (утечка ключей, скомпрометированный контакт) — Immune plane детектирует аномалию (fan-out, поведение) и применяет точечную контрмеру. | Три уровня: структурный (адресация = разрешение), ресурсный (отправка стоит энергии), иммунный (сенсоры + FSM). |
| **Батарея** | BLE/WiFi-Direct в фоне = расход. | Energy-aware transport (см. 2.4). Relay-режим опционален. Cover traffic только при зарядке. |

---

## 9. Роадmap: от архитектуры к коду

**Синхронизация с Mycerus (этапы 17–20):**

| Mush | Mycerus | Что строим |
|-----------|---------|-----------|
| **Этап A'** | — | Ad-hoc группа: BPv7 + WiFi-Direct + CRDT, без хоста |
| **Этап A** | Этап 17 (Sensors) | Core: Payload Block, Double Ratchet, MEB hints, status tracking |
| **Этап B** | Этап 18 (Response Protocol) | Vault + Chats: SQLCipher, CRDT Chat Store, Identicon, Safety Number |
| **Этап C** | Этап 18 (Response Protocol) | Onion + Transport: per-hop wrapper, padding, cover traffic, BLE adapter |
| **Этап D** | Этап 19 (Attack harness) | Groups + Fruiting Gateway: хостинг, group CRDT, SBP, backup |
| **Этап E** | Этап 20 (Immune memory) | Resilient: Immune plane integration, Vironet, stress-test, audit |

### Этап A'. Ad-hoc группа (2 недели) — первый прототип

**Зачем:** проверить базовый UX DTN-мессенджера до добавления сложности. Если два телефона в одной комнате не могут удобно переписываться — immune plane не спасёт.

**Промежуточный шаг — Rust CLI:**
- Сначала: Rust CLI прототип (`cargo run`) — проверка handshake, CRDT sync, sensors, framing.
- Затем: Kotlin Android Studio — тот же protocol, нативный UI, Hotspot API, QR-камера.
- Контракт: protocol не меняется, только transport (TCP → Hotspot) и UI (TUI → Compose).

**Rust CLI (MUSH-A'-CLI-SPEC.md):**
- [ ] TCP transport (вместо Hotspot)
- [ ] `mush://` invite string в stdout (вместо QR)
- [ ] TUI или stdin/stdout UI
- [ ] SNS handshake, CRDT sync, sensors — идентично Android
- [ ] `cargo test` для crypto, CRDT, sensor logic

**Kotlin Android (MUSH-A'-SPEC.md):**
- [ ] Ad-hoc группа: CRDT-чат без хоста, QR-обмен ключами
- [ ] Hotspot transport для Android
- [ ] Jetpack Compose UI: список сообщений, ввод, статус "доставлено"
- [ ] **Демо:** 2–4 телефона в одной комнате, групповой чат без интернета и сервера

**Чего НЕТ в этом этапе:** Fruiting Gateway, Immune plane, Vironet, Onion, Double Ratchet, Spores. Только bundle framing + symmetric key + CRDT.

### Этап A. Mush Core (4 недели) → Mycerus 17
- [ ] Mush Payload Block (тип 203) в BPv7
- [ ] Offline Double Ratchet (X3DH + pre-keys)
- [ ] Privacy-aware MEB hints (privacy_zone вместо destination_affinity)
- [ ] Message status tracking (⏳ / ✓ / ✓✓) через SRE receipts
- [ ] **Демо:** два телефона, WiFi-Direct, текст, QR-контакты

### Этап B. Mush Vault + Chats (4 недели) → Mycerus 18
- [ ] SQLCipher store для сообщений
- [ ] CRDT Chat Store (OR-Map + LWW)
- [ ] Identicon generation
- [ ] Safety Number verification
- [ ] **Демо:** история сообщений, offline доставка, read receipts

### Этап C. Mush Onion + Transport (4 недели) → Mycerus 18
- [ ] Onion wrapper per-hop
- [ ] Padding до фиксированных размеров
- [ ] Cover traffic generator
- [ ] BLE transport adapter
- [ ] Energy-aware switching
- [ ] **Демо:** 3+ hop'а через mesh, метаданные скрыты

### Этап D. Mush Groups + Fruiting Gateway (4 недели) → Mycerus 19
- [ ] Fruiting Gateway protocol (хостинг каналов)
- [ ] Group CRDT protocol + accept-bundle
- [ ] Key rotation для групп
- [ ] Spore Bundle Protocol (SBP) для backup и multi-device
- [ ] **Демо:** групповой чат 5 человек на Fruiting Gateway, backup/restore

### Этап E. Mush Resilient (4 недели) → Mycerus 20
- [ ] Интеграция Immune plane (тип 202) — CUSUM-сенсоры S1–S6, тихий режим
- [ ] Интеграция Vironet (адаптивная маршрутизация)
- [ ] Desktop/Web клиент
- [ ] Stress-test: 100+ узлов, 24 часа, эмуляция атак (включая 6 новых атак A5, B5–B6, C6, D6, F5)
- [ ] Security audit

---

*Mycerus — это мицелий. Mush — это ягода. Пользователь видит ягоду. Под землёй — вся сеть, которая делает так, чтобы ягода всегда была.*
