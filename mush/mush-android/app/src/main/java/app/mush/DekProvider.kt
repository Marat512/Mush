package app.mush

/** Ошибки обёртки DEK (MUSH-B-SPEC §1.2, ревью-пункт 3): Result.failure
 * с этими значениями до границы VaultManager — не исключения-крэши.
 * Sealed-наследники Exception, чтобы Result.failure принимал их напрямую. */
sealed class DekError(message: String) : Exception(message) {
    /** Файл dek.bin порчен (длина != 32, мусор). */
    data object CORRUPTED : DekError("dek.bin corrupted")

    /** Keystore недоступен (ProviderException и т.п.) — будущий
     * Keystore-провайдер; plaintext-провайдер не производит. */
    data object UNAVAILABLE : DekError("keystore unavailable")

    /** Распакованный ключ невалидной длины. */
    data object INVALID_LENGTH : DekError("dek has invalid length")
}

/** Обёртка DEK (MUSH-B-SPEC §1.2). Keystore-обёртка — гейт hardware-verify §6;
 * до неё plaintext — намеренный временный шаг, см. [PlaintextDekProvider]. */
interface DekProvider {
    /** Обёрнуть сырой 32-байтовый DEK для хранения в dek.bin. */
    fun wrap(dek: ByteArray): Result<ByteArray>

    /** Распаковать DEK из dek.bin. Порча/недоступность — Result.failure. */
    fun unwrap(wrapped: ByteArray): Result<ByteArray>
}

/** Временный plaintext-провайдер (MUSH-B-SPEC §7, дни 4–5).
 *
 * TODO: заменить на Keystore-wrapped провайдера ПОСЛЕ hardware-verify §6 —
 * гейт применяется именно к Keystore-интеграции, не к Vault-коду вообще.
 * Plaintext здесь — намеренный временный шаг: Vault-скелет пишется
 * параллельно с ожиданием телефонов (Pixel/Samsung). */
class PlaintextDekProvider : DekProvider {
    override fun wrap(dek: ByteArray): Result<ByteArray> = Result.success(dek.copyOf())

    override fun unwrap(wrapped: ByteArray): Result<ByteArray> =
        if (wrapped.size == 32) {
            Result.success(wrapped.copyOf())
        } else {
            Result.failure(DekError.CORRUPTED)
        }
}
