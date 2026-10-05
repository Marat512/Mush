//! Safety number (MUSH-B-SPEC §3): человекочитаемый отпечаток пары
//! identity-ключей. Свёртка `SHA-256(min || max)` с сортировкой ключей
//! делает число симметричным: обе стороны сверяют одну и ту же строку.
//!
//! Формат: первые 30 байт свёртки, каждый байт -> две десятичные цифры
//! (младшие две цифры десятичной записи байта), группы по 5 цифр через
//! пробел (12 групп, 60 цифр).

use sha2::Digest;

/// Domain-separation safety number (MUSH-B-SPEC §3).
pub const SAFETY_DOMAIN: &[u8] = b"Mush-SafetyNumber-v1";

/// Отпечаток одного identity-ключа: SHA-256(ik || domain).
pub fn fingerprint(ik: &[u8; 32]) -> [u8; 32] {
    let mut h = sha2::Sha256::new();
    h.update(ik);
    h.update(SAFETY_DOMAIN);
    h.finalize().into()
}

/// Симметричная свёртка пары identity-ключей:
/// SHA-256(min(IK_A, IK_B) || max(IK_A, IK_B)) — лексикографический порядок
/// по сырым байтам, результат не зависит от порядка аргументов.
pub fn combined(a: &[u8; 32], b: &[u8; 32]) -> [u8; 32] {
    let (min, max) = if a <= b { (a, b) } else { (b, a) };
    let mut h = sha2::Sha256::new();
    h.update(min);
    h.update(max);
    h.finalize().into()
}

/// Safety number пары identity-ключей: 60 десятичных цифр, 12 групп по 5
/// через пробел. Одна и та же строка у обеих сторон (сортировка в `combined`).
///
/// Каждый байт свёртки даёт две цифры: (b % 100) / 10 и b % 10 — младшие
/// две цифры десятичной записи байта (для b < 100 это ровно b / 10, b % 10).
pub fn safety_number(ik_a: &[u8; 32], ik_b: &[u8; 32]) -> String {
    let digest = combined(ik_a, ik_b);
    let mut digits = String::with_capacity(60);
    for b in &digest[..30] {
        digits.push(char::from(b'0' + (b % 100) / 10));
        digits.push(char::from(b'0' + b % 10));
    }
    let groups: Vec<&str> = digits
        .as_bytes()
        .chunks(5)
        .map(|c| std::str::from_utf8(c).expect("digits are ascii"))
        .collect();
    groups.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::vectors;

    /// Формат: 60 цифр, 12 групп, каждая ровно по 5 цифр, разделитель пробел.
    fn assert_format(s: &str) {
        let groups: Vec<&str> = s.split(' ').collect();
        assert_eq!(groups.len(), 12, "групп должно быть 12: {s}");
        for g in &groups {
            assert_eq!(g.len(), 5, "группа должна быть из 5 цифр: {g}");
            assert!(g.bytes().all(|b| b.is_ascii_digit()), "не цифры: {g}");
        }
        assert_eq!(s.chars().filter(|c| c.is_ascii_digit()).count(), 60);
    }

    #[test]
    fn symmetric_and_deterministic() {
        let a = [0x11u8; 32];
        let b = [0x22u8; 32];
        let ab = safety_number(&a, &b);
        let ba = safety_number(&b, &a);
        assert_eq!(ab, ba, "safety number обязан быть симметричным");
        assert_eq!(ab, safety_number(&a, &b), "детерминизм");
    }

    #[test]
    fn format_is_12_groups_of_5_digits() {
        let a = [0x01u8; 32];
        let b = [0x02u8; 32];
        assert_format(&safety_number(&a, &b));
    }

    #[test]
    fn combined_sorts_keys_and_matches_direct_hash() {
        let a = [0xAAu8; 32];
        let b = [0xBBu8; 32];
        // min||max — прямой SHA-256 от конкатенации в сортированном порядке.
        let mut direct = sha2::Sha256::new();
        direct.update(a);
        direct.update(b);
        let direct: [u8; 32] = direct.finalize().into();
        assert_eq!(combined(&a, &b), direct);
        // Обратный порядок аргументов — та же свёртка.
        assert_eq!(combined(&b, &a), combined(&a, &b));
    }

    #[test]
    fn distinct_pairs_produce_distinct_numbers() {
        let base = [0x10u8; 32];
        let n1 = safety_number(&base, &[0x20u8; 32]);
        let n2 = safety_number(&base, &[0x21u8; 32]);
        assert_ne!(n1, n2);
        assert_format(&n1);
        assert_format(&n2);
    }

    #[test]
    fn golden_vectors_match() {
        for v in vectors::SAFETY_VECTORS {
            let got = safety_number(&v.key_a, &v.key_b);
            assert_eq!(got, v.number, "safety number не совпал с вектором");
            assert_format(&got);
        }
    }

    #[test]
    fn fingerprint_has_domain_separation() {
        let ik = [0x42u8; 32];
        assert_ne!(fingerprint(&ik), combined(&ik, &ik));
        // fingerprint != SHA-256(ik) — domain внутри.
        let plain = sha2::Sha256::digest(ik);
        assert_ne!(fingerprint(&ik).as_slice(), plain.as_slice());
    }
}
