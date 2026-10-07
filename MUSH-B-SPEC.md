# MUSH-B-SPEC — Этап B: Vault, Identicon, Safety Number

**Версия:** 0.1.0
**Дата:** 2026-10-05
**Статус:** Draft — развилки закрыты ДО кода. Реализация на эмуляторе (SQLCipher + Vault + Identicon + Safety Number UI), тестируется preview/CI; hardware-verification — при приезде Pixel/Samsung.
**База:** MUSH-A-SPEC v0.1 (X3DH + Double Ratchet в mush-core), MUSH-A'-ANDROID-SPEC v0.1.3
**Предшественники:** A'-CLI (принят), A'-Android (принят, MS0–MS4), Этап A (принят, дни 1–14)

---

## 0. Архитектурное решение: Legacy deprecated as of B (ЗАФИКСИРОВАНО)

**С этапа B `CryptoMode::Legacy` deprecated и удаляется в этапе C. CLI переводится на Ratchet.**

Обоснование:
- Сейчас каждая фича тестируется в двух режимах — CI-матрица удвоена. К этапу C будет 2–3 режима, и никто не вспомнит, зачем они.
- Legacy (SNS handshake + group_key payload) был прототипом A'; ratchet-режим покрывает его функциональность (проверено 4-нодной интеграцией).
- Миграционных потерь нет: A' не персистила legacy-сессии (`session_key` жил в памяти), persisted-состояния только ratchet (`RatchetSessionState` v2).

Следствия:
1. CLI-smoke (`mush/scripts/smoke.sh`) и все CLI-пути по умолчанию — Ratchet. Флаг `CryptoMode` остаётся до конца B для отката, default меняется на Ratchet отдельным коммитом в начале B.
2. `sns.rs` помечается `#[deprecated]`; удаление — первый коммит этапа C.
3. Тесты dual-mode схлопываются к ratchet-only; legacy-тесты переписываются или удаляются вместе с кодом в C.
4. Android уже на Ratchet (§13 A'-ANDROID-SPEC) — асимметрия CLI/Android исчезает.

## 1. Vault

### 1.1 SQLCipher: bundled (РЕШЕНО)

**`net.zetetic:sqlcipher-android` (AAR, bundled).** «System SQLCipher» на Android не существует — вариант «system» фиктивен; оставшиеся развилки: bundled AAR vs собственная сборка vs SQLite+应用层 шифрование.

- AAR покрывает arm64-v8a / armeabi-v7a / x86_64 — совпадает с ABI-набором `libmush_jni.so` (проверено в CI android-build).
- R8/ProGuard-правила идут с AAR; версия SQLCipher зафиксирована нами, не ОС.
- Цена: +8–10 MB к APK (сейчас ~18 MB debug — приемлемо; release minify вне scope B).
- Отвергнуто: собственная сборка SQLCipher (поддержка NDK-сборки чужого дерева — недельная трата без выигрыша); application-level шифрование полей поверх обычного SQLite (два слоя ключей, одинаковая цена, меньше совместимости).

### 1.2 Ключ Vault: Android Keystore, двухслойный обёртка (РЕШЕНО)

- **KEK** — AES-256, сгенерированный В Android Keystore (`KeyGenParameterSpec`, без user authentication на B; биометрия — этап C). Keystore выбирает hardware-backed при наличии TEE/StrongBox, иначе software-backed — приложение не контролирует, и это нормально (см. §8 риски).
- **DEK** — случайный 256-бит, шифрует SQLCipher-БД (PRAGMA key) и blob-файлы сессий. DEK хранится обёрнутым KEK в `filesDir/vault/dek.bin`.
- Без пароля пользователя на B (passphrase + Argon2id — этап C, вместе с биометрией).
- Отвергнуто «только Keystore напрямую как ключ БД»: SQLCipher требует сырой ключ в процессе — Keystore не отдаёт ключматериал наружу, поэтому нужен именно обёрнутый DEK (двухслойность обязательна, не опция).

### 1.3 Что хранится в Vault (scope B)

| Данные | Форма | Зачем |
|--------|-------|-------|
| Identity seed (Ed25519) | blob | переживает рестарт; иначе pubkey меняется при каждом запуске |
| Ratchet-сессии (pairwise) | `RatchetSessionState` v2 blob/table | reconnect без re-handshake |
| Pre-key store (SPK + OPK secrets + used-tombstones) | blob (новый versioned export в mush-core) | OPK одноразовые: потеря store после рестарта ломает published-бандлы |
| История чатов | SQLCipher `messages` | A' держала в памяти — терялась при свайпе |
| Контакты | SQLCipher `contacts` | иначе пропадают при рестарте |

Sensors/alerts/inbox — эфемерны (вне Vault, как сейчас).

**Размер NodeState CBOR — soft limit 1 MB, не invariant.** Первоначальная
оценка «< 64 KB» была на глаз с одним контактом: каждая ratchet-сессия ≈ 2–8 KB
(root + chains + skipped + seen), при 10+ контактах сессии дают 20–80 KB, плюс
100 OPK, плюс seed. 1 MB — с запасом, binder-транзакции к этому не привязаны
(экспорт — явный user-action в файл, не IPC). Превышение — повод для streaming
export в следующей версии формата, не для отказа.

### 1.2.1 Keystore failure modes (специфицировано до кода)

| Сценарий | Что происходит | UX-flow |
|----------|----------------|---------|
| Пользователь сменил PIN/паттерн устройства | Hardware-backed Keystore может инвалидировать ключ (зависит от TEE) | При отказе unwrap DEK: «Устройство заблокировало ключ хранилища. Восстановите из резервной копии (Backup) или создайте новую идентичность». Кнопки: «Импорт бэкапа» / «Новая идентичность» (второе — с явным предупреждением, что контакты не восстановятся без бэкапа) |
| Биометрия обновлена (новый палец/лицо) | На B DEK не привязан к biometric (KeyGenParameterSpec без user auth) — ключ НЕ должен инвалидироваться. Если вендор инвалидирует — это баг вендора: тот же flow, что при смене PIN | Диагностика в протоколе испытаний (§6): залогировать факт инвалидации на каждом устройстве |
| Устройство потеряно/сброшено | Keystore и DEK недоступны; у пользователя — exportState-CBOR (бэкап, §3 флаг 3) | Recovery = «Импорт бэкапа»: importState(bytes) → identity + pre-keys + сессии восстановлены. Без бэкапа — только новая идентичность (E2E-архитектура: сервера восстановления нет, это честное ограничение, показать в onboarding) |
| Приложение переустановлено | Keystore-entry Android < 6... на практике: Keystore НЕ app-scoped, ключ может пережить переустановку, а `filesDir` — нет → обёртка потеряна | Если unwrap DEK невозможен после reinstall — тот же recovery-flow. Проверить на §6: сохранился ли ключ после `adb uninstall` на каждом вендоре (зафиксировать факт, не кодить под него) |
| Keystore недоступен/без hardware (Xiaomi/Huawei) | `KeyStoreException`/`ProviderException` на генерации | Fallback: software-KEK (AES-256 в файле, защищённом правами FS) + честный баннер в UI «устройство без аппаратного хранилища — ключ в защищённом файле». Не молчаливая потеря, не краш (см. §8) |

Принцип: любой отказ unwrap DEK никогда не приводит к молчаливой потере
identity и никогда не крашит процесс — всегда один из двух путей:
«Импорт бэкапа» или «Новая идентичность» (с предупреждением).

### 1.4 Схема БД (v1)

```sql
PRAGMA user_version = 1;
CREATE TABLE messages(id TEXT, group_id BLOB, sender BLOB, lamport INTEGER,
                      text TEXT, received_at INTEGER, PRIMARY KEY(id, group_id));
CREATE TABLE contacts(pubkey BLOB PRIMARY KEY, name TEXT, added_at INTEGER);
CREATE TABLE kv(k TEXT PRIMARY KEY, v BLOB);  -- sessions, prekey store, identity seed
```

Миграции — только накат `user_version`, без downgrade.

## 2. Identicon (РЕШЕНО: Blockies-стиль 5×5, детерминированный)

- Сетка **5×5, зеркально симметричная по вертикали** (как Blockies/GitHub): 15 бит из хеша определяют заливку, fg/bg — два индекса палитры.
- Палитра: 16 цветов, фиксированная в спеке (hex-таблица в § приложении).
- Вход: SHA-256(pubkey)[..N] — детерминировано, одинаково на ВСЕХ устройствах, без платформенного RNG.
- Реализация в **двух местах с golden vectors**: `mush-core::crypto::identicon` (Rust, для CLI/TUI и Safety Number) и Kotlin (Compose) — общий набор тестовых pubkey с ожидаемыми сетками; расхождение ломает CI.
- Отвергнуто: github-identicon (7-полосный, меньше энтропии на площадь), свой «красивый» алгоритм (непереносимость — идентикон обязан совпадать у двух собеседников при сверке глазами).

## 3. Safety Number (РЕШЕНО: Signal-стиль 60 цифр, 5-группами)

- Вход: `SHA-256(min(IK_A,IK_B) || max(IK_A,IK_B))` — сортировка устраняет зависимость от порядка сторон (протокольное правило, не UI).
- Представление: 60 десятичных цифр, 12 групп по 5 (как Signal) — читаемость вслух, привычный паттерн.
- Вычисление в `mush-core` (`pub fn safety_number(ik_a, ik_b) -> String`), golden vectors; Android получает через JNI.
- Записывается также в MUSH-PROTOCOL.md (это протокол, не UI).

### 3.1 UX-протокол сверки (зафиксирован до кода UI)

| Шаг | Поведение |
|-----|-----------|
| Вход | Contact → «Сверить с <имя>» |
| Экран | Modal: safety number, 60 цифр, 12 групп по 5, monospace + идентиконы обеих сторон |
| Каналы | Кнопки «Скопировать» / «Поделиться» — для сверки по голосовому/третьему каналу |
| Совпадение | Кнопка «Совпадает» → trust_level = Verified (persist в Vault, переживает рестарт) |
| Несовпадение | Кнопка «Не совпадает» → trust_level = **VerificationFailed** (красный индикатор в списке, в sheet — «предыдущая сверка: не совпало»); отправка не блокируется, но с предупреждением; контакт остаётся подозрительным, пока пользователь явно не пересверит |

Сверка голосом/встречей — вне B (QR-SAS — этап C). Verified-флаг persist в
Vault (SQLCipher), переживает рестарт.

### 3.2 Семантика verification (зафиксировано до демо)

- **Verification односторонняя, локальная.** Alice верифицирует Bob'а → VERIFIED
  только в Vault Alice. Пир НЕ узнаёт о том, что его верифицировали: никакого
  handshake-фрейма «verification accepted» в B нет (симметричная verification —
  как в Signal — требует протокольной доработки, не этап B). На демо, если
  спросят «почему у Боба не горит verified» — это ожидаемо: verification не
  транслируется пиру.
- **Key change = новый контакт, не сброс старого.** Pubkey — первичный ключ
  контакта. Peer переустановил приложение без бэкапа → новый pubkey →
  появляется «unknown contact»; старый контакт (verified) остаётся со старым
  fingerprint и больше не отвечает. Связь — только заново через QR (в Signal
  это решает телефонная книга — у нас её нет). Это **ограничение модели,
  не баг**.
  Если peer переустановил приложение и потерял старый pubkey, у получателя
  появляется новый unknown contact. Автоматической связи со старым контактом
  нет. Recovery — только через повторный QR-обмен (и exportState-бэкап у
  переустановившей стороны, §3 флаг 3).
- **Re-handshake сохраняет verification.** Имя контакта — из свежего handshake,
  trust_level и verified_at — сохраняются (verification не сбрасывается при
  повторном подключении; тест: handshake → verify → re-handshake → Verified
  остался).
- TrustLevel: `unverified | verified | verification_failed`. VerificationFailed
  ставится только явным действием пользователя («не совпадает»), снимается
  только явной повторной сверкой.

## 4. Новая поверхность mush-core / JNI (scope B)

| Добавка | Где | Зачем |
|---------|-----|-------|
| `Identity::from_seed([u8;32])` + `Identity::seed()` | identity.rs | персист identity (сейчас только OsRng) |
| `PreKeyStore::export_state/import_state` (versioned) | crypto::ratchet | персист OPK/SPK (иначе рестарт ломает бандлы) |
| `Node::export_state/import_state` (NodeState v1: identity+prekeys+sessions+contacts) | transport.rs | атомарный снапшот persistence |
| `crypto::identicon` + golden vectors | новый модуль | §2 |
| `safety_number(ik_a, ik_b)` + golden vectors | crypto | §3 |
| JNI (handle-free, чистые функции от pubkey): `identiconSvg(pubkey)->String`, `identiconBytes(pubkey)->[u8;10]`, `safetyNumber(pkA, pkB)->String` | mush-jni | §2/§3; без ноды — UI может рисовать до старта ноды; **Rust — единственный источник правды для рендеринга** (SVG для UI, bytes — для golden-vector сверки) |
| JNI: `Native.exportState(handle)->byte[]` (CBOR NodeState), `Native.importState(handle, bytes)` — **только явный user-action** (Backup / Transfer), не на lifecycle; Kotlin пишет bytes в файл/Vault | mush-jni | persistence identity+prekeys+sessions (не Vault: chat history + contacts в SQLCipher — дни 4+, отдельные JNI-функции) |
| Kotlin: VaultManager (Keystore wrap, SQLCipher open, migrations) | app | §1 |

## 5. Тест-план (без телефонов)

1. **Rust unit:** identicon/safety-number golden vectors; PreKeyStore export/import roundtrip (опубликованные бандлы валидны после restore); Node export/import (identity сохраняется).
2. **JVM (Robolectric):** VaultManager wrap/unwrap без Keystore (mock-провайдер), SQLCipher open/migrate v1, identicon Kotlin == golden vectors.
3. **Эмулятор:** onboarding → отправить/получить → force-stop → запуск: тот же pubkey, сессии живы (reconnect без re-handshake), история на месте.
4. **CI:** rust tests + `./gradlew test` (Robolectric) + assembleDebug.

## 6. Hardware verification (Pixel/Samsung)

**Scope гейта (уточнён): применяется к Keystore-интеграции, НЕ к Vault-коду вообще.**
Vault ортогонален Ratchet: Vault хранит state, Ratchet шифрует сообщения.
Поэтому **пишется параллельно без телефонов**: Vault-скелет + SQLCipher-схема v1
+ миграция MushService на Vault-хранение тестируются на эмуляторе. Гейт
блокирует только **Keystore-обёртку DEK** (§1.2) — hardware-backed Keystore на
EMUI vs HyperOS vs AOSP ведёт себя по-разному, это проверяется только на
реальных устройствах. Прогон чеклиста — `scripts/hardware-check.sh`
(автоматический harness, не ad-hoc).

**Gap, закрываемый первым: ratchet-APK на железе не проверен.** Чеклист:

1. Install APK (с `setCryptoMode(1)`, §13 A'-ANDROID-SPEC) на оба устройства.
2. QR-контакт (owner=Pixel, client=Samsung), `/send` в обе стороны.
3. **Session ID совпадает на обеих сторонах**: `adb logcat -s mush` при join + `export_peer_session` short-id в debug-поле Contacts (добавить в B).
4. Keystore: Pixel — hardware-backed (проверить `KeyInfo.isInsideSecurityHardware` в debug-логе); Samsung/Xiaomi/Huawei — software-backed fallback + честное предупреждение в UI (не баг, см. §8).
5. Latency X3DH на Huawei Y5 2019: замер в debug-build (`SystemClock.elapsedRealtime` вокруг join), цель < 1.5 c; при превышении — прогресс-индикатор в UI onboarding-join.
6. Persistence на железе: restart приложения → pubkey тот же (только после реализации §4; до неё — зафиксировать в протоколе испытаний, что identity эфемерна).

**Не начинать Keystore-обёртку DEK до зелёного чеклиста №1–3. Vault-скелет,
SQLCipher и миграция — без гейта.**

## 7. Порядок работ (B)

| День | Работа |
|------|--------|
| 1 | §0: default=Ratchet в CLI (коммит-переключатель), legacy-тесты → ratchet ✅ |
| 2 | mush-core: identicon + safety_number + golden vectors (Rust) ✅ |
| 3 | mush-core: PreKeyStore/Node export-import + Identity::from_seed + JNI-экспорт ✅ |
| 3.5 | `scripts/hardware-check.sh` — автоматический harness §6 (два устройства, install, QR-проверка, session ID, /send, логи в файл) |
| 4–5 | Android: SQLCipher AAR, VaultManager-скелет (**plaintext DEK, Keystore — TODO после §6**), схема v1, миграция MushService на Vault-хранение |
| 6 | Kotlin identicon (golden == Rust через identiconBytes) + UI (Onboarding/Contacts) |
| 7 | Safety Number UI (**после persistence contacts — иначе Verified теряется при рестарте; UI раньше Vault = генератор багов**) |
| 8 | Эмуляторные интеграционные (restart-persistence) — **закрыт вручную** (restart-persistence проверен на эмуляторе при днях 4–7: NodeState, идентикон, Verified — каждый через force-stop). Автоматизация `connectedAndroidTest` **отложена в буфер 9–10** — явно, не молча |
| 9–10 | Буфер: hardware-чеклист §6 при приезде телефонов (**Keystore-обёртка DEK заменяет plaintext**), баги |

## 8. Риски

- **Keystore на кастомных прошивках** (Xiaomi/Huawei): `KeyStoreException`/`ProviderException` на генерации — catch и fallback на software-KEK (шифрованный файл, предупреждение в UI «устройство без аппаратного хранилища»). Это не баг, это честное понижение гарантии; зафиксировать в протоколе испытаний.
- **Двойная нативная нагрузка APK**: libmush_jni.so + SQLCipher-so; следить, чтобы ABI-наборы совпадали (CI android-build проверяет).
- **Robolectric и SQLCipher**: нативные либы в JVM-тестах — использовать `sqlcipher-android` test-схему (extract нативных либ в testOptions); если сломается — тесты БД только на эмуляторе (допустимо, отразить в CI).
- **Identity rotation**: до §4 pubkey менялся каждый запуск — после внедрения `from_seed` любые старые persisted-данные несовместимы; B-версия начинает с чистого Vault (прототип, не прод-миграция).

## Приложение A. Точный reference идентикона (pinned)

Reference — **stewartlord/identicon.js 2.3.3** (Blockies-семейство). Палитры нет:
цвета вычисляются. Реализация: `mush-core::crypto::identicon` (+ `render_svg`);
Kotlin-реализация обязана воспроизвести `to_canonical_bytes()` побайтово.

- Вход: `hash_hex` = lowercase hex от **SHA-256(pubkey)** (64 символа).
- Фон: RGB **[240, 240, 240]** — константа.
- Foreground: `hsl2rgb(hue, 0.7, 0.5)` (порт gist aemkei/1325937), где
  `hue = u32(последние 7 hex-символов) / 0xfffffff` (f64-деление).
- Сетка 5×5, 15 бит из **первых 15 hex-символов**: чётный разряд → fg, нечётный → bg.
  Раскладка: i 0..5 — центральный столбец (x=2, y=i); i 5..10 — x=1, y=i−5;
  i 10..15 — x=0, y=i−10; каждая ячейка зеркалится по x (cells[x] == cells[4−x]).
- Каноническая форма: `fg[3] ‖ bg[3] ‖ cells 25 бит → 4 байта` (row-major:
  i = y*5 + x, bit i в packed[i/8] |= 1 << (i%8)).

Safety number (MUSH-B-SPEC §3, реализация `mush-core::crypto::safety`):

- `fingerprint(IK) = SHA-256(IK ‖ "Mush-SafetyNumber-v1")` (domain separation вместо Signal-"0").
- `combined(A,B) = SHA-256(min(IK_A,IK_B) ‖ max(IK_A,IK_B))` — лексикографический
  порядок по сырым байтам Ed25519-ключей (симметрия: одна строка у обеих сторон).
- Строка: первые **30 байт** свёртки, каждый байт → две десятичные цифры
  `(b % 100) / 10, b % 10` (младшие две цифры десятичной записи байта) —
  формат **12 групп по 5 цифр** через пробел.

**Важно: изначальная ТЗ-формула `b / 10, b % 10` была багом** — для байтов
≥ 100 она даёт значения 10..25, т.е. не цифры. Реализованная формула —
правильная (семантика Signal `byte % 100`; для b < 100 обе формулы совпадают).
Это не «отступление от ТЗ», это исправление ТЗ; возврат к старой формуле
ломает формат-тесты и несовместим с golden vectors.

## Приложение B. Golden vectors

Пиннятся в `mush-core::crypto::vectors` (`IDENTICON_VECTORS`, `SAFETY_VECTORS`);
тесты Rust сверяют реализацию с этими константами. Kotlin сверяет через JNI тот
же модуль — расхождение на любом байте/цифре = красный CI. Публичные ключи
детерминированные: `SHA-256("mush-golden-pubkey-N")`, N = 1..5.

pubkey hex:

| # | pubkey |
|---|---|
| 1 | `8fd37bbd57ebef02482358078aaad2a9ccd857bd6222b95e7c130289614a388f` |
| 2 | `1c308a90a55310b4d72cb97171f24a67d8043dbeb6edb92404a88432a2c33136` |
| 3 | `9dae0ddf40a91639d7bcf231b37474408a75967c0273a58a31f12cf59a217502` |
| 4 | `59702033926001389fe19b29e1c5a0274ef78966dbf83439527e191c01f98492` |
| 5 | `e02cc7920c73048abd2badbae7667fa56933ea59622ae0a3e92e35923febbb5b` |

Identicon: pubkey → canonical bytes hex (10 байт = fg‖bg‖cells):

| # | canonical hex |
|---|---|
| 1 | `26d9a4f0f0f00ac40800` |
| 2 | `c726d9f0f0f0ced54a00` |
| 3 | `75d926f0f0f0ae56b201` |
| 4 | `8b26d9f0f0f0fb935801` |
| 5 | `26d956f0f0f03bc6b801` |

Safety number (пары 1↔2 … 5↔1; симметричны):

| Пара | safety number |
|---|---|
| 1↔2 | `38354 02615 51790 34113 62233 84069 47915 67857 95563 21487 78107 73510` |
| 2↔3 | `94149 10767 66406 43728 95388 75322 35469 49165 35491 80060 46679 80641` |
| 3↔4 | `31450 21100 60382 25608 51848 05540 59190 32750 50610 81377 98173 83874` |
| 4↔5 | `12046 67920 10371 39090 62166 71967 53335 28087 84348 98733 03293 88329` |
| 5↔1 | `79044 91996 63810 58886 35362 11518 65248 80213 69549 97730 38444 51149` |
