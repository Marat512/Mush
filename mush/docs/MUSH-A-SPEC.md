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
- `skipped`: `(ratchet_key, index) → (msg_key, created_at)`, **TTL 7 дней**, prune при export и lazily при decrypt
- `seen_ratchet_keys`: `key → last_seen`, **LRU cap 1000**, TTL 30 дней
- `ad`
- НЕ сериализуется: `spk_secret` (transient responder-инициализация; к моменту первого persist сессия уже обменялась сообщениями)

**Инварианты:**
- Размер сериализованного состояния < **8 KB** после любого трафика (тест: 300 сообщений, несколько ratchet-шагов, out-of-order → export < 8 KB). Рост = пропущены eviction-правила.
- Проверка корректности: twin-ghost тест — параллельная сессия без сериализации производит побайтово идентичные ciphertext'ы после restore.

## 4. Invariants протокола (зафиксированы как тесты)

- `MAX_CHAIN_GAP = 100` (отказ догонять больший разрыв индексов)
- `MAX_SKIPPED = 200` (eviction старейших, FIFO по вставке)
- `SEEN_CAP = 1000` (LRU)
- Skipped TTL = 7 суток; used_pre_keys TTL = 30 суток
- Респондер не шифрует до первого входящего (нет remote ratchet key)
- Повтор дешифровки того же сообщения → None

## 5. План интеграции (дни 7–10)

1. Runtime-флаг `CryptoMode::Legacy | Ratchet` на Node (по умолчанию Legacy — CLI-smoke и Android не ломаются).
2. Pre-key bundle-манифесты как payload framing (новый тип фрейма 0x05) + broadcast в Node.
3. 4 ноды localhost: обмен, разрыв TCP, восстановление сессии из persisted state, out-of-order после восстановления.
4. FS-проверка: компрометация persisted state после N сообщений не раскрывает первые сообщения (симуляция: извлекаем state, пытаемся дешифровать старые ciphertext'ы — нельзя).
