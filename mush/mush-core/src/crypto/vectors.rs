//! Golden vectors (MUSH-B-SPEC приложение B): зафиксированные тестовые
//! векторы для cross-language сверки. JNI отдаёт их Kotlin, и Kotlin-реализация
//! идентикона/safety number обязана воспроизвести ожидаемые значения
//! побайтово (canonically). Публичные ключи — детерминированные
//! (SHA-256 от известных строк), НЕ случайные: векторы воспроизводимы
//! без ГПСЧ.
//!
//! ВНИМАНИЕ: значения в этом файле — часть wire/UX-контракта. Менять их
//! можно только вместе с изменением спеки и Kotlin-стороны.

/// Идентикон-вектор: pubkey и ожидаемые канонические байты (10 = 3 fg + 3 bg + 4 cells).
pub struct IdenticonVector {
    pub pubkey: [u8; 32],
    pub canonical: [u8; 10],
}

/// Safety number-вектор: пара identity-ключей и ожидаемая строка.
pub struct SafetyVector {
    pub key_a: [u8; 32],
    pub key_b: [u8; 32],
    pub number: &'static str,
}

/// Детерминированный pubkey для векторов: SHA-256("mush-golden-pubkey-N").
pub fn golden_pubkey(n: u32) -> [u8; 32] {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("mush-golden-pubkey-{n}").as_bytes());
    digest.into()
}

/// Золотой вектор идентикона: canonical = Identicon::for_pubkey(pubkey).to_canonical_bytes().
pub static IDENTICON_VECTORS: &[IdenticonVector] = &[
    IdenticonVector {
        pubkey: GOLDEN_PUBKEY_1,
        canonical: PLACEHOLDER_CANON_1,
    },
    IdenticonVector {
        pubkey: GOLDEN_PUBKEY_2,
        canonical: PLACEHOLDER_CANON_2,
    },
    IdenticonVector {
        pubkey: GOLDEN_PUBKEY_3,
        canonical: PLACEHOLDER_CANON_3,
    },
    IdenticonVector {
        pubkey: GOLDEN_PUBKEY_4,
        canonical: PLACEHOLDER_CANON_4,
    },
    IdenticonVector {
        pubkey: GOLDEN_PUBKEY_5,
        canonical: PLACEHOLDER_CANON_5,
    },
];

/// Золотые векторы safety number.
pub static SAFETY_VECTORS: &[SafetyVector] = &[
    SafetyVector {
        key_a: GOLDEN_PUBKEY_1,
        key_b: GOLDEN_PUBKEY_2,
        number: PLACEHOLDER_SAFETY_12,
    },
    SafetyVector {
        key_a: GOLDEN_PUBKEY_2,
        key_b: GOLDEN_PUBKEY_3,
        number: PLACEHOLDER_SAFETY_23,
    },
    SafetyVector {
        key_a: GOLDEN_PUBKEY_3,
        key_b: GOLDEN_PUBKEY_4,
        number: PLACEHOLDER_SAFETY_34,
    },
    SafetyVector {
        key_a: GOLDEN_PUBKEY_4,
        key_b: GOLDEN_PUBKEY_5,
        number: PLACEHOLDER_SAFETY_45,
    },
    SafetyVector {
        key_a: GOLDEN_PUBKEY_5,
        key_b: GOLDEN_PUBKEY_1,
        number: PLACEHOLDER_SAFETY_51,
    },
];

// Детерминированные публичные ключи векторов (SHA-256 от известных строк).
const GOLDEN_PUBKEY_1: [u8; 32] = [
    0x8f, 0xd3, 0x7b, 0xbd, 0x57, 0xeb, 0xef, 0x02, 0x48, 0x23, 0x58, 0x07, 0x8a, 0xaa, 0xd2, 0xa9,
    0xcc, 0xd8, 0x57, 0xbd, 0x62, 0x22, 0xb9, 0x5e, 0x7c, 0x13, 0x02, 0x89, 0x61, 0x4a, 0x38, 0x8f,
];
const GOLDEN_PUBKEY_2: [u8; 32] = [
    0x1c, 0x30, 0x8a, 0x90, 0xa5, 0x53, 0x10, 0xb4, 0xd7, 0x2c, 0xb9, 0x71, 0x71, 0xf2, 0x4a, 0x67,
    0xd8, 0x04, 0x3d, 0xbe, 0xb6, 0xed, 0xb9, 0x24, 0x04, 0xa8, 0x84, 0x32, 0xa2, 0xc3, 0x31, 0x36,
];
const GOLDEN_PUBKEY_3: [u8; 32] = [
    0x9d, 0xae, 0x0d, 0xdf, 0x40, 0xa9, 0x16, 0x39, 0xd7, 0xbc, 0xf2, 0x31, 0xb3, 0x74, 0x74, 0x40,
    0x8a, 0x75, 0x96, 0x7c, 0x02, 0x73, 0xa5, 0x8a, 0x31, 0xf1, 0x2c, 0xf5, 0x9a, 0x21, 0x75, 0x02,
];
const GOLDEN_PUBKEY_4: [u8; 32] = [
    0x59, 0x70, 0x20, 0x33, 0x92, 0x60, 0x01, 0x38, 0x9f, 0xe1, 0x9b, 0x29, 0xe1, 0xc5, 0xa0, 0x27,
    0x4e, 0xf7, 0x89, 0x66, 0xdb, 0xf8, 0x34, 0x39, 0x52, 0x7e, 0x19, 0x1c, 0x01, 0xf9, 0x84, 0x92,
];
const GOLDEN_PUBKEY_5: [u8; 32] = [
    0xe0, 0x2c, 0xc7, 0x92, 0x0c, 0x73, 0x04, 0x8a, 0xbd, 0x2b, 0xad, 0xba, 0xe7, 0x66, 0x7f, 0xa5,
    0x69, 0x33, 0xea, 0x59, 0x62, 0x2a, 0xe0, 0xa3, 0xe9, 0x2e, 0x35, 0x92, 0x3f, 0xeb, 0xbb, 0x5b,
];

// Ожидаемые канонические байты идентиконов (MUSH-B-SPEC приложение B).
const PLACEHOLDER_CANON_1: [u8; 10] = [0x26, 0xd9, 0xa4, 0xf0, 0xf0, 0xf0, 0x0a, 0xc4, 0x08, 0x00];
const PLACEHOLDER_CANON_2: [u8; 10] = [0xc7, 0x26, 0xd9, 0xf0, 0xf0, 0xf0, 0xce, 0xd5, 0x4a, 0x00];
const PLACEHOLDER_CANON_3: [u8; 10] = [0x75, 0xd9, 0x26, 0xf0, 0xf0, 0xf0, 0xae, 0x56, 0xb2, 0x01];
const PLACEHOLDER_CANON_4: [u8; 10] = [0x8b, 0x26, 0xd9, 0xf0, 0xf0, 0xf0, 0xfb, 0x93, 0x58, 0x01];
const PLACEHOLDER_CANON_5: [u8; 10] = [0x26, 0xd9, 0x56, 0xf0, 0xf0, 0xf0, 0x3b, 0xc6, 0xb8, 0x01];

// Ожидаемые строки safety number (MUSH-B-SPEC приложение B).
const PLACEHOLDER_SAFETY_12: &str =
    "38354 02615 51790 34113 62233 84069 47915 67857 95563 21487 78107 73510";
const PLACEHOLDER_SAFETY_23: &str =
    "94149 10767 66406 43728 95388 75322 35469 49165 35491 80060 46679 80641";
const PLACEHOLDER_SAFETY_34: &str =
    "31450 21100 60382 25608 51848 05540 59190 32750 50610 81377 98173 83874";
const PLACEHOLDER_SAFETY_45: &str =
    "12046 67920 10371 39090 62166 71967 53335 28087 84348 98733 03293 88329";
const PLACEHOLDER_SAFETY_51: &str =
    "79044 91996 63810 58886 35362 11518 65248 80213 69549 97730 38444 51149";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::identicon::Identicon;
    use crate::crypto::safety::safety_number;

    /// Свёрка реализации идентикона с зафиксированными векторами: реализация
    /// обязана воспроизвести canonical побайтово (Kotlin сверяет те же векторы).
    #[test]
    fn identicon_vectors_match_implementation() {
        assert_eq!(IDENTICON_VECTORS.len(), 5);
        for v in IDENTICON_VECTORS {
            let got = Identicon::for_pubkey(&v.pubkey).to_canonical_bytes();
            assert_eq!(
                got.as_slice(),
                v.canonical.as_slice(),
                "идентикон не совпал с вектором для pubkey {:02x?}",
                v.pubkey
            );
        }
    }

    /// Свёрка safety number с зафиксированными векторами (оба порядка ключей).
    #[test]
    fn safety_vectors_match_implementation() {
        assert_eq!(SAFETY_VECTORS.len(), 5);
        for v in SAFETY_VECTORS {
            assert_eq!(safety_number(&v.key_a, &v.key_b), v.number);
            assert_eq!(safety_number(&v.key_b, &v.key_a), v.number, "симметрия");
        }
    }

    /// Печать фактических значений векторов (заполнение MUSH-B-SPEC
    /// приложения B): cargo test vectors -- --nocapture.
    #[test]
    fn print_actual_vector_values() {
        for i in 1..=5u32 {
            let pk = golden_pubkey(i);
            let canon = Identicon::for_pubkey(&pk).to_canonical_bytes();
            println!("GOLDEN_PUBKEY_{i} = {}", hex(&pk));
            println!("CANON_{i} = {}", hex(&canon));
        }
        let pairs = [(1, 2), (2, 3), (3, 4), (4, 5), (5, 1)];
        for (a, b) in pairs {
            let pa = golden_pubkey(a);
            let pb = golden_pubkey(b);
            println!("SAFETY_{a}{b} = {}", safety_number(&pa, &pb));
        }
    }

    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write;
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            write!(out, "{b:02x}").expect("write to string");
        }
        out
    }
}
