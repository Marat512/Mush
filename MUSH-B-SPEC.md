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
- UI B: экран контакта → «Сверить Safety Number»: 60 цифр + идентиконы обеих сторон. Сверка голосом/встречей — вне B (QR-SAS — этап C).
- Записывается также в MUSH-PROTOCOL.md (это протокол, не UI).

## 4. Новая поверхность mush-core / JNI (scope B)

| Добавка | Где | Зачем |
|---------|-----|-------|
| `Identity::from_seed([u8;32])` + `Identity::seed()` | identity.rs | персист identity (сейчас только OsRng) |
| `PreKeyStore::export_state/import_state` (versioned) | crypto::ratchet | персист OPK/SPK (иначе рестарт ломает бандлы) |
| `Node::export_node_state/import_node_state` (identity+prekeys+sessions) | transport.rs | атомарный снапшот Vault |
| `crypto::identicon` + golden vectors | новый модуль | §2 |
| `safety_number(ik_a, ik_b)` + golden vectors | crypto | §3 |
| JNI: `Native.exportState(handle)->byte[]`, `Native.importState(handle, bytes)` | mush-jni | Vault roundtrip |
| Kotlin: VaultManager (Keystore wrap, SQLCipher open, migrations) | app | §1 |

## 5. Тест-план (без телефонов)

1. **Rust unit:** identicon/safety-number golden vectors; PreKeyStore export/import roundtrip (опубликованные бандлы валидны после restore); Node export/import (identity сохраняется).
2. **JVM (Robolectric):** VaultManager wrap/unwrap без Keystore (mock-провайдер), SQLCipher open/migrate v1, identicon Kotlin == golden vectors.
3. **Эмулятор:** onboarding → отправить/получить → force-stop → запуск: тот же pubkey, сессии живы (reconnect без re-handshake), история на месте.
4. **CI:** rust tests + `./gradlew test` (Robolectric) + assembleDebug.

## 6. Hardware verification (Pixel/Samsung — ПЕРЕД Stage B кодом)

**Gap, закрываемый первым: ratchet-APK на железе не проверен.** Чеклист:

1. Install APK (с `setCryptoMode(1)`, §13 A'-ANDROID-SPEC) на оба устройства.
2. QR-контакт (owner=Pixel, client=Samsung), `/send` в обе стороны.
3. **Session ID совпадает на обеих сторонах**: `adb logcat -s mush` при join + `export_peer_session` short-id в debug-поле Contacts (добавить в B).
4. Keystore: Pixel — hardware-backed (проверить `KeyInfo.isInsideSecurityHardware` в debug-логе); Samsung/Xiaomi/Huawei — software-backed fallback + честное предупреждение в UI (не баг, см. §8).
5. Latency X3DH на Huawei Y5 2019: замер в debug-build (`SystemClock.elapsedRealtime` вокруг join), цель < 1.5 c; при превышении — прогресс-индикатор в UI onboarding-join.
6. Persistence на железе: restart приложения → pubkey тот же (только после реализации §4; до неё — зафиксировать в протоколе испытаний, что identity эфемерна).

**Не начинать Stage B код до зелёного чеклиста №1–3.**

## 7. Порядок работ (B)

| День | Работа |
|------|--------|
| 1 | §0: default=Ratchet в CLI (коммит-переключатель), legacy-тесты → ratchet |
| 2 | mush-core: identicon + safety_number + golden vectors (Rust) |
| 3 | mush-core: PreKeyStore/Node export-import + Identity::from_seed |
| 4–5 | Android: SQLCipher AAR, VaultManager (Keystore wrap), схема v1 |
| 6 | Kotlin identicon (golden == Rust) + UI (Onboarding/Contacts) |
| 7 | Safety Number UI + JNI export/import |
| 8 | Эмуляторные интеграционные (restart-persistence) |
| 9–10 | Буфер: hardware-чеклист §6 при приезде телефонов, баги |

## 8. Риски

- **Keystore на кастомных прошивках** (Xiaomi/Huawei): `KeyStoreException`/`ProviderException` на генерации — catch и fallback на software-KEK (шифрованный файл, предупреждение в UI «устройство без аппаратного хранилища»). Это не баг, это честное понижение гарантии; зафиксировать в протоколе испытаний.
- **Двойная нативная нагрузка APK**: libmush_jni.so + SQLCipher-so; следить, чтобы ABI-наборы совпадали (CI android-build проверяет).
- **Robolectric и SQLCipher**: нативные либы в JVM-тестах — использовать `sqlcipher-android` test-схему (extract нативных либ в testOptions); если сломается — тесты БД только на эмуляторе (допустимо, отразить в CI).
- **Identity rotation**: до §4 pubkey менялся каждый запуск — после внедрения `from_seed` любые старые persisted-данные несовместимы; B-версия начинает с чистого Vault (прототип, не прод-миграция).

## Приложение A. Палитра идентикона (16 цветов)

Фиксируется в коде `mush-core::crypto::identicon` как константа; hex-значения в этом разделе обязаны совпадать с кодом (тест-пин). Заполнение при принятии B-кода.

## Приложение B. Golden vectors

Наборы (pubkey → identicon grid, (IK_A, IK_B) → safety number) генерируются при первой реализации и пиннятся в обеих реализациях (Rust + Kotlin). Любое изменение алгоритма ломает CI — намеренно.
