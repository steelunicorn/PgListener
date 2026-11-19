//! Значки приложения для светлой, тёмной и «матричной» тем.
//!
//! В exe встроены готовые PNG из набора PgListener-Windows: 256 px для окна и
//! 32 px для области уведомлений. При запуске они только декодируются в RGBA,
//! без пережатия — поэтому значки остаются такими, какими их собрали.

/// Набор значков одной темы.
pub struct Art {
    /// Готовый `.ico` набора для окна: Windows сам выберет из него нужный размер.
    pub ico: &'static [u8],
    /// PNG 256 px: значок окна до первого кадра (пока `App` не поставит `.ico`).
    pub window: &'static [u8],
    /// PNG 32 px для области уведомлений.
    pub tray: &'static [u8],
}

/// Размер значка окна на старте.
pub const WINDOW_SIZE: u32 = 256;
/// Размер значка в области уведомлений.
pub const TRAY_SIZE: u32 = 32;

pub const LIGHT: Art = Art {
    ico: include_bytes!("../icons/light.ico"),
    window: include_bytes!("../icons/light-256.png"),
    tray: include_bytes!("../icons/light-32.png"),
};

pub const DARK: Art = Art {
    ico: include_bytes!("../icons/dark.ico"),
    window: include_bytes!("../icons/dark-256.png"),
    tray: include_bytes!("../icons/dark-32.png"),
};

pub const MATRIX: Art = Art {
    ico: include_bytes!("../icons/matrix.ico"),
    window: include_bytes!("../icons/matrix-256.png"),
    tray: include_bytes!("../icons/matrix-32.png"),
};

/// Декодирует PNG в RGBA8 и при необходимости уменьшает до `size`×`size`.
pub fn rgba(png_bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(
        png::Transformations::EXPAND | png::Transformations::STRIP_16,
    );
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let pixels = to_rgba(&buf, info.color_type, info.width as usize, info.height as usize)?;
    let (w, h) = (info.width, info.height);
    if w == size && h == size {
        Some(pixels)
    } else {
        Some(resize(&pixels, w as usize, h as usize, size as usize, size as usize))
    }
}

/// Приводит пиксели любого поддерживаемого формата к RGBA8.
fn to_rgba(src: &[u8], color: png::ColorType, width: usize, height: usize) -> Option<Vec<u8>> {
    let pixels = width * height;
    let mut out = Vec::with_capacity(pixels * 4);
    match color {
        png::ColorType::Rgba => out.extend_from_slice(src),
        png::ColorType::Rgb => {
            for px in src.chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &g in src.iter().take(pixels) {
                out.extend_from_slice(&[g, g, g, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for px in src.chunks_exact(2) {
                out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
            }
        }
        png::ColorType::Indexed => return None,
    }
    Some(out)
}

/// Уменьшение усреднением по пикселям исходника (для значков это плавнее
/// ближайшего соседа и не требует внешних крейтов).
fn resize(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0u8; dw * dh * 4];
    for y in 0..dh {
        let y0 = y * sh / dh;
        let y1 = ((y + 1) * sh / dh).max(y0 + 1);
        for x in 0..dw {
            let x0 = x * sw / dw;
            let x1 = ((x + 1) * sw / dw).max(x0 + 1);
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            for sy in y0..y1.min(sh) {
                for sx in x0..x1.min(sw) {
                    let i = (sy * sw + sx) * 4;
                    for c in 0..4 {
                        acc[c] += src[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let i = (y * dw + x) * 4;
            for c in 0..4 {
                out[i + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_theme_has_window_and_tray_icons() {
        for (name, art) in [("light", LIGHT), ("dark", DARK), ("matrix", MATRIX)] {
            // Windows разбирает .ico сам, но подпись формата проверить полезно:
            // 00 00 01 00 — один ICO-файл с несколькими размерами.
            assert_eq!(&art.ico[..4], &[0, 0, 1, 0], "{name}: это не .ico");
            let count = u16::from_le_bytes([art.ico[4], art.ico[5]]);
            assert!(count >= 4, "{name}: в .ico всего {count} размеров");
            for (png, size) in [
                (art.window, WINDOW_SIZE),
                (art.tray, TRAY_SIZE),
            ] {
                let px = rgba(png, size).unwrap_or_else(|| panic!("{name}: decode failed"));
                assert_eq!(px.len(), (size * size * 4) as usize, "{name}: {size} px");
                let max_alpha = px.chunks_exact(4).map(|p| p[3]).max().unwrap_or(0);
                assert_eq!(max_alpha, 255, "{name}: {size} px без непрозрачных пикселей");
                let has_transparent = px.chunks_exact(4).any(|p| p[3] == 0);
                assert!(has_transparent, "{name}: {size} px без прозрачного фона");
            }
        }
    }

    #[test]
    fn tray_size_is_smaller_than_window_size() {
        assert!(TRAY_SIZE < WINDOW_SIZE);
    }
}
