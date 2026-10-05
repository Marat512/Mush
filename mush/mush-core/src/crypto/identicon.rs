//! Идентикон публичного ключа (MUSH-B-SPEC §3): порт stewartlord/identicon.js
//! 2.3.3 (Blockies-семейство), адаптированный под 5×5-сетку и SHA-256 хэш.
//!
//! Назначение — визуальная аутентификация identity-ключа (QR/превью):
//! человек сверяет два идентикона вместо 64 hex-символов. Алгоритм
//! детерминирован и фиксирован побайтово: каноническая форма
//! (`to_canonical_bytes`) — контракт для cross-language сверки (Kotlin через
//! JNI обязан воспроизвести её побайтово), golden vectors — в `vectors`.

use sha2::Digest;

/// Идентикон identity-ключа: цвета fg/bg и зеркальная 5×5-сетка
/// (true = ячейка закрашена foreground-цветом).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identicon {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    /// Сетка 5×5: cells[x][y], вертикальное зеркало (cells[x] == cells[4-x]).
    pub cells: [[bool; 5]; 5],
}

impl Identicon {
    /// Идентикон для identity-ключа: SHA-256(pubkey) -> цвета + сетка.
    pub fn for_pubkey(pubkey: &[u8; 32]) -> Identicon {
        let digest = sha2::Sha256::digest(pubkey);
        let hash_hex = hex_lower(&digest);

        // Foreground: hue из последних 7 hex-символов (28 бит), как в
        // identicon.js (color = hsl2rgb(hue, 0.7, 0.5)).
        let hue = u32::from_str_radix(&hash_hex[57..], 16).expect("7 hex digits") as f64
            / 0xfffffff_u32 as f64;
        let foreground = hsl2rgb(hue, 0.7, 0.5);
        let background = [240u8; 3];

        // Сетка: 15 бит из первых 15 hex-символов. Чётный разряд = fg.
        let mut cells = [[false; 5]; 5];
        for i in 0..15 {
            let digit = u8::from_str_radix(&hash_hex[i..i + 1], 16).expect("hex digit");
            if digit.is_multiple_of(2) {
                let (x, y) = match i {
                    0..=4 => (2, i),
                    5..=9 => (1, i - 5),
                    _ => (0, i - 10),
                };
                cells[x][y] = true;
                cells[4 - x][y] = true; // зеркало по горизонтали
            }
        }
        Identicon {
            foreground,
            background,
            cells,
        }
    }

    /// Каноническая байтовая форма (fg 3 || bg 3 || cells 25 бит -> 4 байта,
    /// row-major, bit i = cells[i/5][i%5]). Контракт для golden vectors и
    /// cross-language сверки — побайтовое совпадение обязательно.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(10);
        out.extend_from_slice(&self.foreground);
        out.extend_from_slice(&self.background);
        let mut packed = [0u8; 4];
        for y in 0..5 {
            for x in 0..5 {
                if self.cells[x][y] {
                    let i = y * 5 + x;
                    packed[i / 8] |= 1 << (i % 8);
                }
            }
        }
        out.extend_from_slice(&packed);
        out
    }

    /// SVG-превью (viewBox-стиль, как Svg в identicon.js): bg-прямоугольник
    /// на весь размер + fg-прямоугольники для закрашенных ячеек.
    pub fn render_svg(&self, size: u32) -> String {
        let mut svg = String::with_capacity(1024);
        svg.push_str(&format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 5 5\" width=\"{size}\" height=\"{size}\">"
        ));
        svg.push_str(&format!(
            "<rect width=\"5\" height=\"5\" fill=\"rgb({},{},{})\"/>",
            self.background[0], self.background[1], self.background[2]
        ));
        for y in 0..5 {
            for x in 0..5 {
                if self.cells[x][y] {
                    svg.push_str(&format!(
                        "<rect x=\"{x}\" y=\"{y}\" width=\"1\" height=\"1\" fill=\"rgb({},{},{})\"/>",
                        self.foreground[0], self.foreground[1], self.foreground[2]
                    ));
                }
            }
        }
        svg.push_str("</svg>");
        svg
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(out, "{b:02x}").expect("write to string");
    }
    out
}

/// HSL -> RGB, порт gist aemkei/1325937 (исходник identicon.js 2.3.3).
/// Все побитовые JS-операции перенесены через trunc-to-int32.
fn hsl2rgb(hue: f64, saturation: f64, brightness: f64) -> [u8; 3] {
    let h = hue * 6.0;
    let mut b = brightness;
    // JS: s = [b += s *= b < .5 ? b : 1 - b, b - h % 1 * s * 2, b -= s *= 2, b,
    //          b + h % 1 * s, b + s]
    let sel = {
        let mut sat = saturation;
        sat *= if b < 0.5 { b } else { 1.0 - b };
        b += sat;
        let s0 = b;
        let s1 = b - (h % 1.0) * sat * 2.0;
        sat *= 2.0;
        b -= sat;
        [s0, s1, b, b, b + (h % 1.0) * sat, b + sat]
    };
    let hi = h as i32; // trunc-to-int32 (JS `h|0`-семантика для floor(h))
    let f = (hi % 6) as usize;
    let g = ((hi | 16) % 6) as usize;
    let t = ((hi | 8) % 6) as usize;
    [
        (sel[f] * 255.0).round() as u8,
        (sel[g] * 255.0).round() as u8,
        (sel[t] * 255.0).round() as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_same_pubkey() {
        let pk = [0x42u8; 32];
        let a = Identicon::for_pubkey(&pk);
        let b = Identicon::for_pubkey(&pk);
        assert_eq!(a, b);
        assert_eq!(a.to_canonical_bytes(), b.to_canonical_bytes());
    }

    #[test]
    fn distinct_pubkeys_produce_distinct_identicons() {
        let a = Identicon::for_pubkey(&[0x01u8; 32]);
        let b = Identicon::for_pubkey(&[0x02u8; 32]);
        assert_ne!(a.to_canonical_bytes(), b.to_canonical_bytes());
    }

    #[test]
    fn grid_is_horizontally_mirrored() {
        let icon = Identicon::for_pubkey(&[0xABu8; 32]);
        for y in 0..5 {
            for x in 0..5 {
                assert_eq!(
                    icon.cells[x][y],
                    icon.cells[4 - x][y],
                    "зеркало нарушено в x={x} y={y}"
                );
            }
        }
    }

    #[test]
    fn canonical_bytes_layout_is_3_plus_3_plus_4() {
        let icon = Identicon::for_pubkey(&[0x11u8; 32]);
        let canon = icon.to_canonical_bytes();
        assert_eq!(canon.len(), 10);
        assert_eq!(&canon[..3], &icon.foreground);
        assert_eq!(&canon[3..6], &icon.background);
        // Побитовая распаковка совпадает с cells.
        for y in 0..5 {
            for x in 0..5 {
                let i = y * 5 + x;
                let bit = (canon[6 + i / 8] >> (i % 8)) & 1 == 1;
                assert_eq!(bit, icon.cells[x][y]);
            }
        }
    }

    #[test]
    fn svg_is_nonempty_with_rects() {
        let icon = Identicon::for_pubkey(&[0x77u8; 32]);
        let svg = icon.render_svg(128);
        assert!(svg.starts_with("<svg"), "должен начинаться с <svg");
        assert!(svg.ends_with("</svg>"));
        let fg_cells = icon.cells.iter().flatten().filter(|&&c| c).count();
        let rects = svg.matches("<rect").count();
        // 1 фон + fg_cells.
        assert_eq!(rects, 1 + fg_cells);
    }
}
