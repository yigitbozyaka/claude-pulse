//! Dynamic system-tray battery icon, styled after the "Claude Pulse" design: a
//! rounded chip whose orange gradient rises from the bottom to show the % of the
//! session window left, with that number set in JetBrains Mono ExtraBold.
//!
//! Everything — tile, meniscus and glyphs — is drawn 8x supersampled and then
//! box-downsampled to the exact size Windows asks for (SM_CXSMICON, the size the
//! shell stores tray icons at). Rasterising the font on the big canvas means the
//! digits pick up the same smooth anti-aliasing as the rounded corners instead of
//! the blocky hand-rolled bitmap font this used to use.

use std::sync::OnceLock;

use fontdue::Font;

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
const SHADOW: [u8; 3] = [0x00, 0x00, 0x00];
const TRACK: [u8; 3] = [0x20, 0x1d, 0x19]; // dark chip base ("empty")
const FILL_TOP: [u8; 3] = [0xe0, 0x8a, 0x63];
const FILL_BOT: [u8; 3] = [0xd9, 0x77, 0x57];

fn font() -> &'static Font {
    static F: OnceLock<Font> = OnceLock::new();
    F.get_or_init(|| {
        Font::from_bytes(
            include_bytes!("../assets/JetBrainsMono-ExtraBold.ttf").as_slice(),
            fontdue::FontSettings::default(),
        )
        .expect("bundled font is valid")
    })
}

fn lerp(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
    ]
}

fn inside_rr(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    if x1 <= x0 || y1 <= y0 {
        return false;
    }
    let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let nx = x.clamp(x0 + r, x1 - r);
    let ny = y.clamp(y0 + r, y1 - r);
    let dx = x - nx;
    let dy = y - ny;
    dx * dx + dy * dy <= r * r
}

/// Actual small-icon size for this display (16 @ 100%, 20 @ 125%, 24 @ 150%).
#[cfg(windows)]
fn tray_size() -> usize {
    #[link(name = "user32")]
    extern "system" {
        fn GetSystemMetrics(n: i32) -> i32;
    }
    const SM_CXSMICON: i32 = 49;
    (unsafe { GetSystemMetrics(SM_CXSMICON) }).clamp(16, 64) as usize
}
#[cfg(not(windows))]
fn tray_size() -> usize {
    32
}

/// Alpha-composite a fontdue coverage bitmap onto the (opaque) tile buffer.
/// Only paints where the tile is already solid, so text never bleeds onto the
/// transparent corners.
fn blit(hi: &mut [u8], big: usize, bmp: &[u8], w: usize, h: usize, ox: f32, oy: f32, color: [u8; 3], alpha: f32) {
    let ox = ox.round() as i32;
    let oy = oy.round() as i32;
    for j in 0..h {
        for i in 0..w {
            let cov = bmp[j * w + i] as f32 / 255.0 * alpha;
            if cov <= 0.0 {
                continue;
            }
            let x = ox + i as i32;
            let y = oy + j as i32;
            if x < 0 || y < 0 || x >= big as i32 || y >= big as i32 {
                continue;
            }
            let idx = (y as usize * big + x as usize) * 4;
            if hi[idx + 3] == 0 {
                continue;
            }
            for k in 0..3 {
                hi[idx + k] = (color[k] as f32 * cov + hi[idx + k] as f32 * (1.0 - cov)) as u8;
            }
        }
    }
}

/// Stamp the remaining-% number, centred, JetBrains Mono ExtraBold, sized to the
/// tighter of a width/height budget so 1–3 digits all sit nicely.
fn stamp_number(hi: &mut [u8], big: usize, t0: f32, t1: f32, value: u32) {
    let font = font();
    let label = value.to_string();
    let tile_px = t1 - t0;
    let max_w = tile_px * 0.80;
    let max_h = tile_px * 0.56;

    // pick a font size from both budgets at a reference size, take the tighter
    let refpx = 100.0f32;
    let h0 = font.metrics('0', refpx).height as f32;
    let w0: f32 = label.chars().map(|c| font.metrics(c, refpx).advance_width).sum();
    let px = (max_h * refpx / h0).min(max_w * refpx / w0);

    let total_w: f32 = label.chars().map(|c| font.metrics(c, px).advance_width).sum();
    let m0 = font.metrics('0', px);
    let dig_h = m0.height as f32;
    let dig_ymin = m0.ymin as f32;

    let cx = (t0 + t1) / 2.0;
    let cy = (t0 + t1) / 2.0;
    let start_x = cx - total_w / 2.0;
    let baseline = cy + dig_ymin + dig_h / 2.0; // centres the digit ink vertically

    let glyphs: Vec<(fontdue::Metrics, Vec<u8>)> =
        label.chars().map(|c| font.rasterize(c, px)).collect();

    // subtle drop shadow (design: text-shadow 0 1px 2px rgba(0,0,0,.45)) so the
    // white numerals stay legible over the light part of the fill
    let sh_dx = px * 0.03;
    let sh_dy = px * 0.06;
    let mut pen = start_x;
    for (m, bmp) in &glyphs {
        let gx = pen + m.xmin as f32;
        let gy = baseline - m.ymin as f32 - m.height as f32;
        blit(hi, big, bmp, m.width, m.height, gx + sh_dx, gy + sh_dy, SHADOW, 0.5);
        pen += m.advance_width;
    }
    let mut pen = start_x;
    for (m, bmp) in &glyphs {
        let gx = pen + m.xmin as f32;
        let gy = baseline - m.ymin as f32 - m.height as f32;
        blit(hi, big, bmp, m.width, m.height, gx, gy, WHITE, 1.0);
        pen += m.advance_width;
    }
}

/// `used_pct` is utilization (0-100). Returns (rgba, width, height).
pub fn battery_icon(used_pct: f64) -> (Vec<u8>, u32, u32) {
    render(tray_size(), used_pct)
}

pub(crate) fn render(size: usize, used_pct: f64) -> (Vec<u8>, u32, u32) {
    const SS: usize = 8; // supersample factor
    let big = size * SS;

    // inset the tile so it breathes like neighbouring tray icons
    let inset = size / 12;
    let tile = size - 2 * inset;
    let (t0, t1) = ((inset * SS) as f32, ((size - inset) * SS) as f32);
    let radius = tile as f32 * SS as f32 * 0.22;

    let remaining = (100.0 - used_pct).clamp(0.0, 100.0);
    let frac = (remaining / 100.0) as f32;

    // snap the liquid level to whole device pixels so the line stays sharp
    let fill_rows = (tile as f32 * frac).round() as usize;
    let y_top = ((size - inset - fill_rows) * SS) as f32;
    let line = SS as f32; // 1 device pixel
    let span = (t1 - y_top).max(1.0); // fill height, for the top→bottom gradient

    let mut hi = vec![0u8; big * big * 4];
    for y in 0..big {
        let fy = y as f32 + 0.5;
        for x in 0..big {
            let fx = x as f32 + 0.5;
            if !inside_rr(fx, fy, t0, t0, t1, t1, radius) {
                continue;
            }
            let c = if fill_rows >= 2 && fy >= y_top {
                // top→bottom orange gradient, with a bright meniscus on the surface
                let g = lerp(FILL_TOP, FILL_BOT, (fy - y_top) / span);
                if fy - y_top < line { lerp(g, WHITE, 0.35) } else { g }
            } else {
                TRACK
            };
            let i = (y * big + x) * 4;
            hi[i] = c[0];
            hi[i + 1] = c[1];
            hi[i + 2] = c[2];
            hi[i + 3] = 255;
        }
    }

    // stamp the number onto the big canvas so it anti-aliases on downsample
    stamp_number(&mut hi, big, t0, t1, remaining.round() as u32);

    // integer box-downsample: AA lands on the corners *and* the glyph edges
    let mut img = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for dy in 0..SS {
                let row = (y * SS + dy) * big;
                for dx in 0..SS {
                    let i = (row + x * SS + dx) * 4;
                    r += hi[i] as u32;
                    g += hi[i + 1] as u32;
                    b += hi[i + 2] as u32;
                    a += hi[i + 3] as u32;
                }
            }
            let i = (y * size + x) * 4;
            if a > 0 {
                img[i] = (r * 255 / a) as u8;
                img[i + 1] = (g * 255 / a) as u8;
                img[i + 2] = (b * 255 / a) as u8;
                img[i + 3] = (a / (SS * SS) as u32) as u8;
            }
        }
    }

    (img, size as u32, size as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // composite onto a taskbar-dark background and dump a BMP for eyeballing
    fn write_bmp(path: &std::path::Path, rgba: &[u8], w: u32, h: u32) {
        let data = (w * h * 4) as u32;
        let mut buf = Vec::new();
        buf.extend_from_slice(b"BM");
        buf.extend_from_slice(&(54 + data).to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(&54u32.to_le_bytes());
        buf.extend_from_slice(&40u32.to_le_bytes());
        buf.extend_from_slice(&(w as i32).to_le_bytes());
        buf.extend_from_slice(&(-(h as i32)).to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&32u16.to_le_bytes());
        buf.extend_from_slice(&[0u8; 24]);
        for px in rgba.chunks(4) {
            let a = px[3] as u32;
            let mix = |c: u8, bg: u32| ((c as u32 * a + bg * (255 - a)) / 255) as u8;
            buf.extend_from_slice(&[mix(px[2], 30), mix(px[1], 28), mix(px[0], 26), 255]);
        }
        std::fs::File::create(path).unwrap().write_all(&buf).unwrap();
    }

    #[test]
    fn preview() {
        let dir = std::env::temp_dir().join("claude_tray_preview");
        std::fs::create_dir_all(&dir).unwrap();
        for size in [16usize, 20, 24, 32] {
            for pct in [9.0, 22.0, 55.0, 78.0, 93.0, 0.0] {
                let (rgba, w, h) = render(size, pct);
                assert_eq!(rgba.len(), size * size * 4);
                write_bmp(&dir.join(format!("tray_{size}_{}.bmp", pct as u32)), &rgba, w, h);
            }
        }
    }
}
