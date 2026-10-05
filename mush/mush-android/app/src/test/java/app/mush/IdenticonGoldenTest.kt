package app.mush

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.AssumptionViolatedException
import org.junit.BeforeClass
import org.junit.Test
import java.io.File
import java.util.Base64

/** Golden vectors Rust↔Kotlin (MUSH-B-SPEC §2/§3, приложение B) — день 6.
 *  Грузит ДЕСКТОПНУЮ сборку libmush_jni (env MUSH_JNI_LIB или fallback рядом
 *  с workspace) и сверяет JNI-вывод с векторами, зафиксированными в
 *  `mush-core/src/crypto/vectors.rs` (константы ниже — копия оттуда, источник
 *  правды — vectors.rs; при смене векторов править ОБА места).
 *
 *  Либа не найдена -> AssumptionViolated (skip, не fail): локальный
 *  `gradlew test` без cargo-сборки не красится. В CI либа собирается
 *  (`cargo build --release -p mush-jni`, job android-build) — там тест
 *  обязан пройти. */
class IdenticonGoldenTest {

    companion object {
        // GOLDEN_PUBKEY_N = SHA-256("mush-golden-pubkey-N"), см. vectors.rs.
        private val PUBKEYS: List<ByteArray> = listOf(
            hex("8fd37bbd57ebef02482358078aaad2a9ccd857bd6222b95e7c130289614a388f"),
            hex("1c308a90a55310b4d72cb97171f24a67d8043dbeb6edb92404a88432a2c33136"),
            hex("9dae0ddf40a91639d7bcf231b37474408a75967c0273a58a31f12cf59a217502"),
            hex("59702033926001389fe19b29e1c5a0274ef78966dbf83439527e191c01f98492"),
            hex("e02cc7920c73048abd2badbae7667fa56933ea59622ae0a3e92e35923febbb5b"),
        )

        private fun bytes(vararg v: Int): ByteArray = ByteArray(v.size) { i -> v[i].toByte() }

        // CANON_N = Identicon::for_pubkey(PUBKEY_N).to_canonical_bytes() (10 байт).
        private val CANONICAL: List<ByteArray> = listOf(
            bytes(0x26, 0xd9, 0xa4, 0xf0, 0xf0, 0xf0, 0x0a, 0xc4, 0x08, 0x00),
            bytes(0xc7, 0x26, 0xd9, 0xf0, 0xf0, 0xf0, 0xce, 0xd5, 0x4a, 0x00),
            bytes(0x75, 0xd9, 0x26, 0xf0, 0xf0, 0xf0, 0xae, 0x56, 0xb2, 0x01),
            bytes(0x8b, 0x26, 0xd9, 0xf0, 0xf0, 0xf0, 0xfb, 0x93, 0x58, 0x01),
            bytes(0x26, 0xd9, 0x56, 0xf0, 0xf0, 0xf0, 0x3b, 0xc6, 0xb8, 0x01),
        )

        // SAFETY_N = safety_number(PUBKEY_N, PUBKEY_N+1) — 12 групп по 5 цифр.
        private val SAFETY = listOf(
            "38354 02615 51790 34113 62233 84069 47915 67857 95563 21487 78107 73510",
            "94149 10767 66406 43728 95388 75322 35469 49165 35491 80060 46679 80641",
            "31450 21100 60382 25608 51848 05540 59190 32750 50610 81377 98173 83874",
            "12046 67920 10371 39090 62166 71967 53335 28087 84348 98733 03293 88329",
            "79044 91996 63810 58886 35362 11518 65248 80213 69549 97730 38444 51149",
        )

        private fun hex(s: String): ByteArray =
            ByteArray(s.length / 2) { i -> s.substring(2 * i, 2 * i + 2).toInt(16).toByte() }

        private fun b64(raw: ByteArray): String = Base64.getEncoder().encodeToString(raw)

        @JvmStatic
        @BeforeClass
        fun loadNative() {
            val candidates = listOfNotNull(
                System.getenv("MUSH_JNI_LIB"),
                "../target/release/mush_jni.dll", // Windows, user.dir = mush-android
                "../target/release/libmush_jni.so", // Linux/macOS
                "../../target/release/mush_jni.dll",
                "../../target/release/libmush_jni.so",
            )
            val lib = candidates.map { File(it) }.firstOrNull { it.isFile }
                ?: throw AssumptionViolatedException(
                    "libmush_jni не найден (MUSH_JNI_LIB и fallback-пути) — " +
                        "собери: cargo build --release -p mush_jni. Тест skip.",
                )
            System.load(lib.absolutePath)
        }
    }

    @Test
    fun identiconBytesMatchGoldenVectors() {
        PUBKEYS.forEachIndexed { i, pk ->
            val got = Native.identiconBytes(b64(pk))
            assertArrayEquals(
                "identiconBytes != вектору #$i (источник: vectors.rs IDENTICON_VECTORS)",
                CANONICAL[i],
                got,
            )
        }
    }

    @Test
    fun safetyNumberMatchesGoldenVectors() {
        // Пары (1,2) (2,3) (3,4) (4,5) (5,1) — как SAFETY_VECTORS в vectors.rs.
        val pairs = listOf(0 to 1, 1 to 2, 2 to 3, 3 to 4, 4 to 0)
        pairs.forEachIndexed { i, (a, b) ->
            // Симметрия: оба порядка аргументов — одна строка.
            assertEquals(
                "safetyNumber != вектору #$i",
                SAFETY[i],
                Native.safetyNumber(b64(PUBKEYS[a]), b64(PUBKEYS[b])),
            )
            assertEquals(
                "safetyNumber(a,b) != safetyNumber(b,a) для пары #$i",
                Native.safetyNumber(b64(PUBKEYS[a]), b64(PUBKEYS[b])),
                Native.safetyNumber(b64(PUBKEYS[b]), b64(PUBKEYS[a])),
            )
        }
    }
}
