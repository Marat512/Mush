package app.mush

/** Обёртка DEK (MUSH-B-SPEC §1.2). Keystore-обёртка — гейт hardware-verify §6;
 * до неё plaintext — намеренный временный шаг, см. [PlaintextDekProvider]. */
interface DekProvider {
    /** Обёрнуть сырой 32-байтовый DEK для хранения в dek.bin. */
    fun wrap(dek: ByteArray): ByteArray

    /** Распаковать DEK из dek.bin. Бросает исключение при порче файла. */
    fun unwrap(wrapped: ByteArray): ByteArray
}

/** Временный plaintext-провайдер (MUSH-B-SPEC §7, дни 4–5).
 *
 * TODO: заменить на Keystore-wrapped провайдера ПОСЛЕ hardware-verify §6 —
 * гейт применяется именно к Keystore-интеграции, не к Vault-коду вообще.
 * Plaintext здесь — намеренный временный шаг: Vault-скелет пишется
 * параллельно с ожиданием телефонов (Pixel/Samsung). */
class PlaintextDekProvider : DekProvider {
    override fun wrap(dek: ByteArray): ByteArray = dek.copyOf()
    override fun unwrap(wrapped: ByteArray): ByteArray = wrapped.copyOf()
}
