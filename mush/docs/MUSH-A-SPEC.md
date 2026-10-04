# MUSH-A-SPEC — Этап A: E2E-криптография (X3DH + Double Ratchet)

**Версия:** 0.1.0
**Дата:** 2026-10-04
**Статус:** Draft — архитектурные решения зафиксированы до реализации persistence/integration
**Место:** `mush-core::crypto::ratchet` рядом с legacy `crypto::session` (sns.rs). Переключение — runtime-конфигом на уровне Node; legacy-протокол не тронут до интеграции.

---

## 1. Pre-key модель (РЕШЕНО)

**Одноразовые OPK + semi-static SPK (модель Signal).** Не semi-static-only.

Обоснование:
- OPK в X3DH даёт устойчивость past sessions при компрометации SPK (DH4).
- OPK тратится на **новую сессию**, а не на «пару сообщений»: ratchet живёт сам. Новый bundle нужен только новому контакту.
- DTN-стоимость доставки амортизируется **пакетным манифестом**: один подписанный bundle несёт 50–100 OPK → одна доставка на десятки будущих сессий. Трафик предсказуем без ослабления FS.

Следствия:
- `PreKeyStore.replenish(N)` — пакетное пополнение; публикация — манифестом.
- Одноразовость контролируется двумя механизмами: OPK удаляется из `published` при использовании (anti-replay активный) + tombstone-множество `used_pre_keys: BTreeMap<opk_id, used_at>` с TTL 30 дней (детекция позднего reuse/атаки; ids монотонны, переиспользования id не бывает).
- SPK ротируется по эпохам (в A' — при рестарте ноды; в B — по расписанию). Ротация SPK не ломает установленные сессии (ratchet-keys независимы).

## 2. Отклонения от libsignal (явные, байт-несовместимость)

Архитектура — Signal (X3DH + DR), байтовая совместимость — НЕТ. Зафиксировано:

| Аспект | libsignal | mush |
|--------|-----------|------|
| KDF домены | HKDF с их info-байтами | `mush-x3dh-sk`, `mush-dr-ck/mk/root/chain` |
| Подпись SPK | XEdDSA (подпись на X25519) | Ed25519-подпись доменного сообщения `mush-spk‖id‖pk` |
| SK KDF | их KDF-цепочка | HKDF-SHA256(DH1..4, salt=`mush-x3dh-sk`, info=AD) |
| DR chain step | HMAC(ck, 0x01/0x02) | HKDF(ck, `mush-dr-ck` / `mush-dr-mk`) |
| Распространение pre-keys | сервер | DTN bundle-манифесты (TTL + spray) |
| AD | IK_A‖IK_B | IK_A‖IK_B (совпадает) |

Защита от «зашитой ошибки в KDF»: **golden vectors** на KDF/DH-примитивах закоммичены в тестах — любое изменение domain-строки или структуры KDF ломает CI.

## 3. Контракт session persistence

Сериализуется (`RatchetSessionState`, version=1, несовпадение version → import возвращает None, НЕ молчаливая потеря):

- `root_key`, `dhs` (текущий DH-секрет), `dhr`
- `send_chain`, `recv_chain` (chain key + index)
- `prev_send_len` (pn)
- `skipped`: `(ratchet_key, index) → (msg_key, created_at_logical)`, TTL по **логическому счётчику сессии** (события encrypt/decrypt), не по часам устройства; prune — явный `prune_expired()` + eviction по размеру
- `seen_ratchet_keys`: `key → last_seen_logical`, **LRU cap 1000**, TTL 100_000 событий
- `ad`
- НЕ сериализуется: `spk_secret` (transient responder-инициализация; к моменту первого persist сессия уже обменялась сообщениями)

**Инварианты:**
- Размер сериализованного состояния < **8 KB** после любого трафика (тест: 300 сообщений, несколько ratchet-шагов, out-of-order → export < 8 KB). Рост = пропущены eviction-правила.
- `export_state(&self)` — чистая, два подряд вызова дают идентичный результат; просрочка чистится только явным `prune_expired(&mut self)`.
- Проверка корректности: twin-ghost тест — функциональное продолжение обмена с нетронутым пиром после restore (побайтовое сравнение ciphertext'ов невозможно: случайные nonce).

## 4. Invariants протокола (зафиксированы как тесты)

- `MAX_CHAIN_GAP = 100` (отказ догонять больший разрыв индексов)
- `MAX_SKIPPED = 200` (eviction старейших)
- `SEEN_CAP = 1000` (LRU)
- Skipped/seen TTL — **по логическому счётчику сессии** (события encrypt/decrypt), не по часам: устройства в поле могут иметь сбитое время. SKIPPED_TTL_LOGICAL = 10_000 событий, SEEN_TTL_LOGICAL = 100_000
- `used_pre_keys` (PreKeyStore, анти-replay OPK) — на wall-clock узла: ошибка часа лишь меняет срок хранения tombstone, безопасность не ломается
- **export_state — идемпотентен** (`&self`, без мутаций): prune — отдельный явный вызов `prune_expired(&mut self)`. Два подряд export дают одинаковый state
- Респондер не шифрует до первого входящего (нет remote ratchet key)
- Повтор дешифровки того же сообщения → None

## 4.1 Распространение bundle (РЕШЕНО до интеграции)

- **До первого контакта** bundle едет ВНУТРИ invite-строки (поле `pk=`: spk_id, spk, подпись r/s, opk_id, opk). Канал обнаружения A' — и есть invite/QR; отдельный канал не нужен. Сканирующий одним QR получает группу, адрес и бандл.
- **Mesh-фаза** (после A'): bundle-манифесты (пакет OPK) — отдельный payload, фрейм 0x05, broadcast по установленным соединениям. TTL манифеста 30 дней, spray-копии — вне A'.
- **Лимиты размера**: `MANIFEST_MAX_KEYS = 100`, `MANIFEST_MAX_BYTES = 16_384` (CBOR). Тест: полный манифест сериализуется под лимит. Одиночный bundle в invite (~300 байт base64) в QR влезает.
- **Tombstone на принимающей стороне**: потребитель бандлов (initiator) ведёт `BundleConsumer.used: (spk_id, opk_id) → logical_at` — повторный старт сессии с тем же OPK отклоняется до ухода в сеть (дублирует responder-side защиту; ловит баги инициатора локально).

## 5. План интеграции (дни 7–10)

1. `transport.rs` — **флаг режима, не переписывание**: `Node.crypto_mode: CryptoMode::{Legacy, Ratchet}` (default Legacy — CLI-smoke и Android не трогаются). CI гоняет оба режима тестами. invite-строка получает поле `pk=` (bundle).
2. Handshake в Ratchet-режиме: X3dhInit вместо legacy SNS (новый тип первого фрейма), bundle из invite; responder — PreKeyStore.
3. 4 ноды localhost: обмен, разрыв TCP, восстановление сессии из persisted state, out-of-order после восстановления.
4. FS-проверка: компрометация persisted state после N сообщений не раскрывает первые сообщения.
