//! Handle-free чистые функции для UI (MUSH-B-SPEC §3): идентикон и safety
//! number не требуют ноды — Android рисует их до старта ноды (onboarding,
//! сверка ключей). Rust — единственный источник правды рендеринга: Kotlin
//! не рендерит идентикон из bytes для UI, bytes нужны только для
//! golden-vector сверки Rust↔Kotlin (те же векторы, что в `crypto::vectors`).

use mush_core::base64;
use mush_core::crypto::{identicon, safety};

/// Декодировать base64(pubkey) в [u8; 32]. Err — невалидный base64
/// или длина != 32 (JNI-слой бросает IllegalArgumentException).
pub fn decode_pubkey(b64: &str) -> Result<[u8; 32], String> {
    let bytes = base64::decode(b64.trim()).ok_or_else(|| "bad base64".to_string())?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| "pubkey must be 32 bytes".to_string())
}

/// SVG-идентикон pubkey (size=64) для UI-превью.
pub fn identicon_svg(pubkey_b64: &str) -> Result<String, String> {
    let pk = decode_pubkey(pubkey_b64)?;
    Ok(identicon::Identicon::for_pubkey(&pk).render_svg(64))
}

/// Канонические 10 байт идентикона (fg 3 || bg 3 || cells 4) — golden-формат.
pub fn identicon_bytes(pubkey_b64: &str) -> Result<Vec<u8>, String> {
    let pk = decode_pubkey(pubkey_b64)?;
    Ok(identicon::Identicon::for_pubkey(&pk).to_canonical_bytes())
}

/// Safety number пары identity-ключей (12 групп по 5 цифр).
pub fn safety_number(pk_a_b64: &str, pk_b_b64: &str) -> Result<String, String> {
    let a = decode_pubkey(pk_a_b64)?;
    let b = decode_pubkey(pk_b_b64)?;
    Ok(safety::safety_number(&a, &b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mush_core::base64 as b64;
    use mush_core::crypto::vectors;

    #[test]
    fn identicon_bytes_match_golden_vectors() {
        for v in vectors::IDENTICON_VECTORS {
            let got = identicon_bytes(&b64::encode(&v.pubkey)).expect("identicon_bytes");
            assert_eq!(got, v.canonical.to_vec(), "canonical != вектору");
        }
    }

    #[test]
    fn identicon_svg_is_svg_for_golden_pubkey() {
        let v = &vectors::IDENTICON_VECTORS[0];
        let svg = identicon_svg(&b64::encode(&v.pubkey)).expect("svg");
        assert!(svg.starts_with("<svg"), "должен начинаться с <svg");
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn safety_number_matches_golden_vectors() {
        for v in vectors::SAFETY_VECTORS {
            let a = b64::encode(&v.key_a);
            let b = b64::encode(&v.key_b);
            // Оба порядка аргументов — одна строка.
            assert_eq!(safety_number(&a, &b).unwrap(), v.number);
            assert_eq!(safety_number(&b, &a).unwrap(), v.number);
        }
    }

    #[test]
    fn decode_pubkey_rejects_garbage() {
        assert!(decode_pubkey("!!!").is_err());
        assert!(decode_pubkey("").is_err());
        // Валидный base64, но не 32 байта.
        assert!(decode_pubkey(&b64::encode(&[0u8; 31])).is_err());
        assert!(decode_pubkey(&b64::encode(&[0u8; 33])).is_err());
    }
}
