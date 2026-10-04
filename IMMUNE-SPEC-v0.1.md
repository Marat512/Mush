# IMMUNE-SPEC-v0.1 — Спецификация Immune Plane для MyceRus DTN

**Версия:** 0.1.0  
**Дата:** 2026-09-29  
**Статус:** Draft — формализация ATTACK-MODEL v0.3.0  
**Зависимости:** BPv7 (RFC 9171), GENOME-SPEC-v0.x, MEB-SPEC-v0.1.1

---

## 1. Область применения и термины

### 1.1. Область

Этот документ формализует Immune plane (иммунную плоскость) сети MyceRus — DTN-маршрутизатора с эволюционирующей политикой протокола. Immune plane отвечает за:
- Детекцию аномалий (сенсоры).
- Принятие решений о контрмерах (кворум, FSM).
- Применение контрмер (penicillin — точечное ограничение, HR — гиперчувствительный ответ).
- Распространение информации об угрозах (Immune Block тип 202).
- Восстановление после ошибок (appeals, partition reconciliation).
- Защиту новых узлов (bootstrap, genesis-якоря).

### 1.2. Термины

| Термин | Определение |
|--------|-------------|
| **Immune plane** | Подсистема MyceRus, отвечающая за обнаружение и реакцию на атаки. Независима от Data plane, Control plane и Evolution plane. |
| **Sensor** | Модуль детекции аномалий. Принимает события (bundle, контакт, handshake), выдаёт SensorEvent. |
| **SensorEvent** | Структурированное событие: `{ sensor_name, anomaly_type, target_key, score, evidence }`. |
| **Evidence** | Данные, подтверждающие аномалию. Два типа: **verifiable** (объективные факты) и **reputational** (субъективное мнение). |
| **Quorum** | Минимальное число независимых сенсоров, подтвердивших аномалию, для активации контрмеры. |
| **Topological independence** | Два сенсора считаются независимыми, если их множества соседей за окно имеют пересечение < `shared_neighbor_threshold`. |
| **FSM (Finite State Machine)** | Конечный автомат доверия: `trusted → watch → silent → degraded → quarantined`, с `probation` и `bridge-hostage guard`. |
| **Immune Block** | Extension Block типа 202 в BPv7. Содержит Threat, Quarantine, Revoke или Reroute. |
| **CUSUM** | Cumulative Sum — алгоритм накопления отклонения от baseline. Используется в спам-сенсорах. |
| **Silent mode** | Состояние FSM: bundle'ы принимаются, но не показываются пользователю (теневой inbox). |
| **Genesis anchor** | Предустановленный доверенный ключ для bootstrap-режима нового узла. |
| **Appeal** | Апелляция против quarantine: counter-evidence + запрос на пересмотр. |

---

## 2. Immune Block (тип 202) — CBOR-схема

### 2.1. Регистрация

Immune Block — Extension Block в BPv7 с типом `202`. Регистрируется в IANA под Experimental Use (по аналогии с Bundle Security Protocol).

### 2.2. Общая структура

```cddl
ImmuneBlock = {
    block_type: 202,
    block_number: uint,
    block_flags: BlockFlags,
    block_data_length: uint,
    block_data: ImmunePayload
}

ImmunePayload = {
    subtype: uint,        ; 1=Threat, 2=Quarantine, 3=Revoke, 4=Reroute
    version: uint,        ; 1
    timestamp: uint,      ; unix timestamp или logical epoch
    ttl: uint,            ; seconds, 0 = бессрочно (только для Revoke)
    issuer_key: bytes,    ; Ed25519 public key (32 bytes)
    signature: bytes,     ; Ed25519 signature (64 bytes)
    payload: SubtypePayload
}

SubtypePayload = ThreatPayload / QuarantinePayload / RevokePayload / ReroutePayload
```

### 2.3. Подтип 1: Threat

```cddl
ThreatPayload = {
    target_key: bytes,        ; Ed25519 public key атакующего
    threat_type: text,        ; "identity_spoof" | "routing_anomaly" | "resource_flood" | "false_alarm" | "fan_out_spam" | "behavioral_spam" | ...
    evidence_hash: bytes,     ; SHA-256 хэш evidence (32 bytes)
    evidence_size: uint,      ; размер evidence в байтах
    score: float,             ; anomaly_score ≥ 0
    sensor_name: text         ; имя сенсора, поднявшего тревогу
}
```

### 2.4. Подтип 2: Quarantine

```cddl
QuarantinePayload = {
    target_key: bytes,
    quorum_signatures: [+ bytes],  ; массив подписей сенсоров, подтвердивших кворум
    quorum_sensors: [+ bytes],     ; массив pubkey'ей сенсоров
    reason: text,
    duration: uint                 ; seconds
}
```

**Правило обработки:**
- Принимается только если ≥ `quorum_threshold` подписей валидны.
- Подписи проверяются по pubkey'ям из `quorum_sensors`.
- Сенсоры должны иметь `rep > min_sensor_rep`.

### 2.5. Подтип 3: Revoke

```cddl
RevokePayload = {
    revoked_key: bytes,
    reason: text,
    replacement_key: bytes / null,  ; опционально: новый ключ для rotation
    issued_by: bytes,               ; pubkey эмитента
}
```

**Правило обработки:**
- Revoke имеет бесконечный TTL (не истекает).
- Применяется только при наличии `issued_by` с положительной репутацией.
- Если `replacement_key` не null — старый ключ заменяется на новый в контакт-листе.

### 2.6. Подтип 4: Reroute

```cddl
ReroutePayload = {
    target_key: bytes,          ; ключ, который reroute'ят
    excluded_paths: [+ bytes],  ; pubkey'и узлов, которых избегать
    suggested_paths: [+ bytes], ; опциональные альтернативные пути
    campaign_id: bytes          ; уникальный ID кампании rerouting
}
```

**Правило обработки:**
- Reroute не наказывает target_key, а перенаправляет трафик в обход excluded_paths.
- Используется при bridge-hostage guard: вместо quarantine моста — rerouting campaign.
- `campaign_id` предотвращает повторную обработку одной кампании.

### 2.7. Правила обработки Immune Block

| Шаг | Правило |
|-----|---------|
| **1. Rate limiting** | Не более `immune_block_per_sensor_limit` (10) блоков от одного сенсора за контакт. Превышение → отбрасывание без проверки подписи. |
| **2. Signature verification** | Проверка Ed25519 подписи. Невалидная → отбрасывание. |
| **3. Issuer reputation** | Если `rep(issuer) ≤ 0` → блок ставится в низкоприоритетную очередь. |
| **4. TTL check** | Если `timestamp + ttl < now` → отбрасывание (кроме Revoke). |
| **5. Duplicate check** | `(issuer_key, target_key, timestamp)` проверяется на дубликат в окне `immune_dedup_window`. Дубликат → отбрасывание. |
| **6. FSM update** | Применение к локальному FSM: `watch` (1 сенсор), `degraded` (кворум), `quarantined` (повторный кворум + bridge-guard). |
| **7. Propagation** | Immune Block relay'ится дальше с пониженным приоритетом (чтобы не перегружать сеть). |

---

## 3. Sensor API — protobuf-схема

### 3.1. SensorEvent

```protobuf
syntax = "proto3";

message SensorEvent {
    string sensor_name = 1;
    AnomalyType anomaly_type = 2;
    bytes target_key = 3;      // Ed25519 public key или null
    double score = 4;          // anomaly_score ≥ 0
    Evidence evidence = 5;
    uint64 timestamp = 6;
}

enum AnomalyType {
    UNKNOWN = 0;
    IDENTITY_SPOOF = 1;
    ROUTING_ANOMALY = 2;
    RESOURCE_FLOOD = 3;
    FALSE_ALARM = 4;
    SPAM_ACTIVITY = 5;
    BEHAVIORAL_ANOMALY = 6;
    FITNESS_POISONING = 7;
    POOL_CAPTURE = 8;
    COLLUSION_PATTERN = 9;
}

message Evidence {
    oneof evidence_type {
        RateDeviation rate_deviation = 1;
        DuplicateRatio duplicate_ratio = 2;
        FanOutCount fan_out_count = 3;
        ComplaintRatio complaint_ratio = 4;
        HashOnly hash_only = 5;
    }
}

message RateDeviation {
    double baseline = 1;
    double observed = 2;
}

message DuplicateRatio {
    double baseline = 1;
    double observed = 2;
}

message FanOutCount {
    double baseline = 1;
    uint32 observed = 2;
    bytes attestations_hash = 3;
}

message ComplaintRatio {
    double baseline = 1;
    double observed = 2;
    uint32 participants = 3;
}

message HashOnly {
    bytes evidence_hash = 1;
    uint32 evidence_size = 2;
}
```

### 3.2. Sensor trait (Rust)

```rust
pub trait Sensor {
    fn name(&self) -> &'static str;
    
    /// Обработка события bundle. Возвращает 0+ SensorEvent.
    fn on_bundle(&mut self, event: &BundleEvent) -> Vec<SensorEvent>;
    
    /// Обработка события контакта (POT).
    fn on_contact(&mut self, event: &ContactEvent) -> Vec<SensorEvent>;
    
    /// Обработка события handshake (SNS).
    fn on_handshake(&mut self, event: &HandshakeEvent) -> Vec<SensorEvent>;
    
    /// Периодический tick (для CUSUM-накопления и garbage collection).
    fn tick(&mut self, now: Timestamp) -> Vec<SensorEvent>;
}
```

### 3.3. CUSUM-фреймворк

```rust
pub struct CUSUMSensor {
    baseline_rate: f64,
    k: f64,           // slack (из генома: cusum_slack)
    h: f64,           // threshold (из генома: cusum_threshold)
    cusum_pos: f64,
    cusum_neg: f64,
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
    
    pub fn reset(&mut self) {
        self.cusum_pos = 0.0;
        self.cusum_neg = 0.0;
    }
}
```

### 3.4. Differential Privacy для сенсоров

Сенсоры анализируют поведение, но метаданные тоже чувствительны. Каждый сенсор добавляет Laplace noise к `score` перед публикацией `SensorEvent`:

```rust
pub struct PrivateSensor {
    base_sensor: Box<dyn Sensor>,
    epsilon: f64,  // privacy budget (invariant, не мутирует эволюцией)
}

impl Sensor for PrivateSensor {
    fn on_bundle(&mut self, event: &BundleEvent) -> Vec<SensorEvent> {
        let mut events = self.base_sensor.on_bundle(event);
        for e in &mut events {
            let noise = laplace_noise(1.0 / self.epsilon);
            e.score = (e.score + noise).max(0.0);
        }
        events
    }
}
```

**Параметр `epsilon`:**
- Часть генома, но является **invariant** — эволюция не может его мутировать.
- Рекомендуемое значение: `epsilon = 1.0`.
- Trade-off: DP снижает точность → более высокие пороги CUSUM → медленнее detection. Vironet адаптирует `cusum_h`, но не `epsilon`.

---

## 4. QuorumEngine — алгоритм

### 4.1. Структура

```rust
pub struct QuorumEngine {
    pub threshold: u32,                    // минимум 2 (invariant)
    pub min_sensor_rep: f64,               // минимальная репутация для участия
    pub shared_neighbor_threshold: f64,    // max пересечение соседей (0.5)
    pub neighbor_window: usize,            // число последних контактов для окна
    pub sparse_fallback_k: usize,          // минимум соседей для fallback
    pub sensor_min_age: Duration,          // минимальный возраст сенсора для fallback (антиспам)
}
```

### 4.2. Алгоритм evaluate

```rust
impl QuorumEngine {
    pub fn evaluate(&self, events: &[SensorEvent]) -> QuorumResult {
        // 1. Фильтрация по репутации
        let valid_events: Vec<_> = events.iter()
            .filter(|e| sensor_reputation(e.sensor_name) > self.min_sensor_rep)
            .collect();
        
        // 2. Группировка по target_key
        let mut groups: HashMap<_, Vec<_>> = HashMap::new();
        for e in &valid_events {
            if let Some(target) = e.target_key {
                groups.entry(target).or_default().push(e);
            }
        }
        
        // 3. Для каждой группы — проверка топологической независимости
        for (target, group_events) in groups {
            if group_events.len() < self.threshold {
                continue;
            }
            
            let mut independent_count = 0;
            let mut used_sensors = Vec::new();
            
            for event in group_events {
                let sensor_neighbors = get_neighbors(event.sensor_name, self.neighbor_window);
                
                let is_independent = used_sensors.iter().all(|used| {
                    let used_neighbors = get_neighbors(used, self.neighbor_window);
                    
                    // Fallback для разреженных трасс (только для зрелых сенсоров)
                    if sensor_neighbors.len() < self.sparse_fallback_k 
                        && used_neighbors.len() < self.sparse_fallback_k
                        && sensor_age(event.sensor_name) > self.sensor_min_age
                        && sensor_age(used) > self.sensor_min_age {
                        return true; // считаем независимыми
                    }
                    
                    let shared = sensor_neighbors.intersection(&used_neighbors).count();
                    let normalized = shared as f64 / 
                        sensor_neighbors.len().min(used_neighbors.len()) as f64;
                    
                    normalized < self.shared_neighbor_threshold
                });
                
                if is_independent {
                    independent_count += 1;
                    used_sensors.push(event.sensor_name);
                    
                    if independent_count >= self.threshold {
                        return QuorumResult::Confirmed {
                            target: target.clone(),
                            sensors: used_sensors,
                        };
                    }
                }
            }
        }
        
        QuorumResult::Denied
    }
}
```

### 4.3. Результат

```rust
pub enum QuorumResult {
    Confirmed {
        target: Ed25519PublicKey,
        sensors: Vec<String>,
    },
    Denied,
}
```

---

## 5. Trust FSM — шесть состояний

### 5.0. Invariants (не мутируют эволюцией)

| Параметр | Минимум | Максимум | Почему invariant |
|----------|---------|----------|------------------|
| `quorum_threshold` | 2 | — | Один сенсор не может решать за всех |
| `quarantine_max_scope` | — | 0.1 | Не более 10% сети в quarantine |
| `sensor_penalty_ttl` | 3600 сек | — | Минимальное время «наказания» за ложную тревогу |
| `sensor_epsilon` | 0.5 | — | Privacy budget differential privacy |

Эти параметры **не являются частью генома** — они hardcoded в ядро. Vironet не может их мутировать даже теоретически.

### 5.1. Состояния

```
trusted ──► watch ──► silent ──► degraded ──► quarantined
              │          │           │            │
              │          │           │            │ probation
              │          │           │            ▼
              │          │           │      probation ──► trusted
              │          │           │      (TTL=d)       (clean history)
              │          │           │
              │          │           │ bridge-hostage guard
              │          │           │ (local bridge + 2 sensors)
              │          │           │
              │          │           └──► rerouting campaign
              │          │                (forced degraded, not quarantined)
              │          │
              └──────────┴───────────────────► trusted
                      (manual reset by user)
```

### 5.2. Определения состояний

| Состояние | Relay | Gradient | SNS | User-visible |
|-----------|-------|----------|-----|--------------|
| **trusted** | Да | Да | Да | Зелёный индикатор |
| **watch** | Да | Да | Да | Нет индикатора (фоновое логирование) |
| **silent** | Да | Да | Да | 🟡 "Необычная активность". Bundle'ы в теневом inbox. |
| **degraded** | Нет | Игнорируются | Да | 🔴 "Контакт ограничен" |
| **quarantined** | Нет | Игнорируются | Отказ | Красный индикатор, "заблокирован" |
| **probation** | Да (ограниченно) | Да | Да | Жёлтый индикатор, "на испытании" |

### 5.3. Переходы

| From | To | Триггер | TTL |
|------|-----|---------|-----|
| trusted | watch | 1 сенсор, CUSUM сработал | часы |
| watch | silent | CUSUM накопил дальше | часы |
| silent | degraded | Кворум сенсоров | часы–сутки |
| degraded | quarantined | Повторный кворум + bridge-guard OK | сутки |
| degraded | probation | Истечение TTL | d (сутки) |
| quarantined | probation | Истечение TTL | d |
| probation | trusted | Истечение TTL + clean history | — |
| any | trusted | Ручной сброс пользователем | — |

### 5.4. Bridge-hostage guard

```rust
pub fn apply_quarantine_with_guard(
    fsm: &mut FSMTable,
    target: Ed25519PublicKey,
    quorum: &QuorumResult,
    local_bridge_estimate: &BridgeEstimate,
) -> FSMState {
    if local_bridge_estimate.is_local_bridge(&target) {
        // Local bridge: delayed quarantine + rerouting campaign
        fsm.insert(target, FSMState::Degraded);
        
        // Запуск rerouting campaign
        let reroute = ImmuneBlock::Reroute(ReroutePayload {
            target_key: target.clone(),
            excluded_paths: vec![target.clone()],
            campaign_id: generate_campaign_id(),
            ..Default::default()
        });
        
        broadcast_reroute(reroute);
        FSMState::Degraded // bridge остаётся в degraded, не quarantined
    } else {
        // Not a bridge: quarantine OK
        fsm.insert(target, FSMState::Quarantined);
        FSMState::Quarantined
    }
}
```

### 5.5. Immune Shock Detector (cytokine storm guard)

Если 20% сети одновременно в quarantine из-за ложной тревоги — сеть коллапсирует. Immune Shock Detector — глобальный предохранитель:

```rust
pub struct ImmuneShockDetector {
    quarantine_rate_threshold: f64,  // 0.05 (5% за 1 контакт)
    global_pause_duration: Duration, // 1 час
}

impl ImmuneShockDetector {
    pub fn check(&mut self, fsm_table: &FSMTable, recent_changes: usize) -> Option<ShockResponse> {
        let total_nodes = fsm_table.len();
        let quarantined = fsm_table.count_quarantined();
        let rate = (quarantined as f64 / total_nodes as f64) / recent_changes as f64;
        
        if rate > self.quarantine_rate_threshold {
            Some(ShockResponse {
                action: ShockAction::GlobalPause,
                duration: self.global_pause_duration,
                reset_cusum: true,
                freeze_new_quarantines: true,
            })
        } else {
            None
        }
    }
}
```

**При срабатывании:**
1. Новые quarantine блокируются на `global_pause_duration`.
2. Все CUSUM-накопители сбрасываются.
3. Существующие quarantine не отменяются (иначе восстановятся атакующие).
4. Событие записывается в `known_threats` как «immune_shock».

**Инвариант:** `quarantine_max_scope ≤ 0.1` — не более 10% узлов в quarantine.

### 5.6. Silent mode

```rust
pub struct SilentInbox {
    pub target: Ed25519PublicKey,
    pub bundles: Vec<Bundle>,  // bundle'ы, принятые, но не показаны
    pub cusum_score: f64,      // текущий CUSUM-накопитель
    pub entered_at: Timestamp,
}

impl SilentInbox {
    pub fn add_bundle(&mut self, bundle: Bundle) {
        self.bundles.push(bundle);
    }
    
    pub fn restore(&mut self) -> Vec<Bundle> {
        // Пользователь решил восстановить контакт
        // Возвращаем все накопленные bundle'ы
        std::mem::take(&mut self.bundles)
    }
}
```

### 5.6. Appeals

См. §16 ATTACK-MODEL. AppealBundle содержит counter-evidence. При успешной апелляции — переход из `quarantined` в `probation` (не `trusted`).

---

## 6. Bootstrap-режим — web-of-trust + genesis-якоря

### 6.1. Два пути bootstrap

**Путь 1 (дефолт): Web-of-trust maturation**
- Новый узел принимает контакты, но в `watch` (только логирование, без relay).
- После `N` контактов с разными ключами (diversity guard) — maturation в `trusted`.
- Не требует genesis-якорей, полностью децентрализован.
- Медленнее, но устойчив к компрометации.

**Путь 2 (опциональный): Genesis-якорь**
- Предустановленный доверенный ключ из out-of-band (QR, NFC, predistribution).
- При первом контакте с узлом из якоря — `watch` (не `trusted`).
- Якорь даёт направление, не абсолютное доверие.
- Если якорь скомпрометирован — worst case: maturation медленнее, не ловушка.

```rust
pub struct GenesisAnchor {
    pub pubkey: Ed25519PublicKey,
    pub signature: Signature,         // подпись creator'а или администратора
    pub ttl: Duration,
    pub max_trust_level: FSMState,    // watch (не trusted)
}
```

### 6.2. Cap на действия

| Действие | Ограничение bootstrap | После maturation |
|----------|----------------------|-----------------|
| Relay | Только от якорных узлов (если якорь есть), иначе только passive | Полный |
| SNS-handshake | Инициация да, приём от незнакомых нет | Полный |
| Sensor | Нет | Да |
| Vironet voting | Нет | Да |
| Quarantine | Нет | Да |

### 6.3. MaturationTracker

```rust
pub struct MaturationTracker {
    pub contacts_observed: usize,
    pub contacts_initiated: usize,
    pub bundles_relayed: usize,
    pub maturation_threshold: usize,  // например, 50
}

impl MaturationTracker {
    pub fn is_mature(&self) -> bool {
        self.contacts_observed >= self.maturation_threshold
    }
    
    pub fn can_act_as_sensor(&self) -> bool {
        self.contacts_observed >= self.maturation_threshold / 2
    }
}
```

---

## 7. Partition reconciliation

### 7.1. Алгоритм

```rust
pub fn reconcile_partition(
    local_fsm: &FSMTable,
    remote_fsm: &FSMTable,
    local_revocations: &RevocationList,
    remote_revocations: &RevocationList,
) -> ReconciliationResult {
    // 1. FSM: union-with-expiry
    let merged_fsm = merge_fsm(local_fsm, remote_fsm, |local, remote| {
        if local.state == remote.state { return local; }
        
        // Quarantine с ненулевым TTL побеждает
        match (local.state, remote.state) {
            (FSMState::Quarantined, _) if local.ttl_remaining() > 0 => local,
            (_, FSMState::Quarantined) if remote.ttl_remaining() > 0 => remote,
            _ => stricter_state(local.state, remote.state),
        }
    });
    
    // 2. Revocations: last-writer-wins
    let merged_revocations = merge_revocations(local_revocations, remote_revocations, |local, remote| {
        if local.timestamp > remote.timestamp { local } else { remote }
    });
    
    ReconciliationResult { fsm: merged_fsm, revocations: merged_revocations }
}
```

### 7.2. Порядок strictness

```rust
fn stricter_state(a: FSMState, b: FSMState) -> FSMState {
    use FSMState::*;
    match (a, b) {
        (Quarantined, _) | (_, Quarantined) => Quarantined,
        (Degraded, _) | (_, Degraded) => Degraded,
        (Silent, _) | (_, Silent) => Silent,
        (Watch, _) | (_, Watch) => Watch,
        _ => Trusted,
    }
}
```

---

## 8. Attack harness — сценарии валидации

### 8.1. Сценарии J, K, L (спам)

См. §12.9 ATTACK-MODEL. Кратко:

**J. ContactRateSensor + CUSUM:** 10 узлов, захват контакта, 50 сообщений за 2 часа. Detection после ~10 сообщений. Ложные срабатывания на ЧС = 0.

**K. Accept-bundle flooding:** 20 узлов, 15 accept'ов за 10 минут. >80% ложных accept'ов отброшены.

**L. Gossip-аттестации:** 8 узлов, clique, fan-out. Консенсус gossip за < 5 контактов.

### 8.2. Шесть новых атак

| ID | Атака | Цель валидации |
|----|-------|---------------|
| **A5** | Replay beacon | `BeaconFreshnessSensor` отбрасывает stale beacon'ы. 0 принятых stale. |
| **B5** | Sybil flooding | `NodeDensitySensor` + rate limiting. Gradient-table не переполняется. |
| **B6** | Gradient oscillation | `GradientVelocitySensor` детектирует oscillation. Cost стабилизируется. |
| **C6** | Storage exhaustion | `StoragePressureSensor` + eviction. Fill-ratio < threshold. |
| **D6** | Collusion ring | `CollusionPatternSensor` выявляет closed loop. Кворум отклонён. |
| **F5** | Adaptive genome | `GenomeBehaviorSensor` + constraints floor'ы. Адаптивный геном провалит attack harness. |

---

## 9. Критерии приёмки — девять метрик

| # | Метрика | Целевое значение | Как измеряется |
|---|---------|-----------------|----------------|
| **M1** | **False positive rate** | < 5% на 3 трассах без атаки | Число ложных срабатываний / общее число контактов |
| **M2** | **Detection latency** | < 10 сообщений (CUSUM) или < 3 контактов (пороговые) | Число bundle'ев/контактов от начала атаки до первого срабатывания |
| **M3** | **Immune overhead** | < 10% CPU/time per contact | Время обработки immune plane / общее время контакта |
| **M4** | **False-collusion rate** | < 1% | Процент ложных кворумов, не отклонённых топологической проверкой |
| **M5** | **Quarantine slot utilization** | < 10% | Доля узлов в quarantine одновременно |
| **M6** | **Revocation list size** | < 10% памяти immune plane | Размер revocation-листа / выделенная память |
| **M7** | **Bootstrap time** | < 48 часов | Время от первого контакта до `is_mature()` |
| **M8** | **Attack resilience** | ≥ 0.7 | Доставляемость под атакой с immune / доставляемость без атаки |
| **M9** | **Recovery time** | < 2× baseline | Время восстановления после ложного quarantine (до возврата в trusted) |
| **M10** | **Immune debt** | < 5% от трафика | Накопленный ущерб от ложных срабатываний за 1000 контактов: задержанные легитимные bundle'ы + время на appeals |

**Формула immune debt:**
```
immune_debt = Σ(задержанных_легитимных_bundle'ов × время_задержки) +
              Σ(время_на_appeals) +
              Σ(потерянных_контактов_из_за_quarantine)
```

Цель Vironet: минимизировать не только `false_positive_rate`, но и **immune debt**.

---

## 10. Зависимости и блокеры

| Зависимость | Статус | Влияние |
|-------------|--------|---------|
| `creator_signature` проверка ядром | Долг этапов 9–10 | Блокер. Без него Threat-bundle'ы не доверенные. |
| Ed25519 key rotation | Новое | Необходимо для A3 (impersonation recovery). |
| Immune Block тип 202 (Extension Block) | Новое | Формат Threat/Quarantine/Revoke/Reroute. |
| Local bridge estimate в MGR | Новое | Bridge-hostage guard в FSM. |
| CUSUM sensor framework | Новое | S1–S6 (спам-сенсоры). |
| Partition reconciliation protocol | Новое | §14 (восстановление после разделения). |
| Genesis anchor format + verification | Новое | §15 (bootstrap-режим). |
| AppealBundle format + processing | Новое | §16 (апелляции). |

---

*IMMUNE-SPEC-v0.1 — формализация ATTACK-MODEL v0.3.0. После реализации этапов 17–20 мигрирует в RFC-подобный документ с IANA-регистрацией типа 202.*