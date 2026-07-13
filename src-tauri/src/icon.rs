//! Dynamic system-tray battery icon, rendered in Rust. A crisp battery meter:
//! a warm-dark rounded tile (the "empty" track) with a terracotta fill that
//! rises from the bottom proportional to remaining %, a thin bright surface
//! line at the fill top, and a crisp white number (remaining %) in Arial Bold.
//! Flat and high-contrast so it stays sharp at 16-24px. Rendered at 512,
//! downsampled to 128.

use std::sync::OnceLock;

use ab_glyph::{FontVec, PxScale};
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_text_mut, text_size};

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
const TRACK: [u8; 3] = [0x20, 0x1b, 0x18]; // warm dark "empty" track
const ACCENT: [u8; 3] = [0xc1, 0x5f, 0x3c];
const AMBER: [u8; 3] = [0xcc, 0x77, 0x22];
const RED: [u8; 3] = [0xaa, 0x22, 0x00];

fn font() -> Option<&'static FontVec> {
    static FONT: OnceLock<Option<FontVec>> = OnceLock::new();
    FONT.get_or_init(|| {
        for path in [
            "C:/Windows/Fonts/arialbd.ttf",
            "C:/Windows/Fonts/segoeuib.ttf",
            "C:/Windows/Fonts/ariblk.ttf",
        ] {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(f) = FontVec::try_from_vec(bytes) {
                    return Some(f);
                }
            }
        }
        None
    })
    .as_ref()
}

fn lerp(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
    ]
}

fn fill_color(frac: f32) -> [u8; 3] {
    if frac > 0.4 {
        ACCENT
    } else if frac > 0.15 {
        lerp(ACCENT, AMBER, (0.4 - frac) / 0.25)
    } else {
        lerp(AMBER, RED, (0.15 - frac) / 0.15)
    }
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

fn fit_px(font: &FontVec, label: &str, start: f32, target_w: f32, target_h: f32) -> f32 {
    let mut px = start;
    for _ in 0..40 {
        let (tw, th) = text_size(PxScale::from(px), font, label);
        if tw == 0 || th == 0 {
            break;
        }
        let s = (tw as f32 / target_w).max(th as f32 / target_h);
        if s > 0.985 && s < 1.015 {
            break;
        }
        px /= s;
    }
    px
}

/// `used_pct` is utilization (0-100). Returns (rgba, width, height).
pub fn battery_icon(used_pct: f64) -> (Vec<u8>, u32, u32) {
    let s: u32 = 512;
    let sf = s as f32;
    let r = 96.0f32;

    let remaining = (100.0 - used_pct).clamp(0.0, 100.0);
    let frac = (remaining / 100.0) as f32;
    let fill = fill_color(frac);
    let surface = lerp(fill, WHITE, 0.30);

    let fill_h = sf * frac;
    let y_top = sf - fill_h;

    let mut img = RgbaImage::from_pixel(s, s, Rgba([0, 0, 0, 0]));
    for y in 0..s {
        for x in 0..s {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            if !inside_rr(fx, fy, 0.0, 0.0, sf, sf, r) {
                continue;
            }
            let mut c = TRACK;
            if fill_h > 4.0 && inside_rr(fx, fy, 0.0, y_top, sf, sf, r) {
                c = fill;
                let d = fy - y_top;
                if d >= 0.0 && d < 10.0 {
                    c = surface; // bright liquid-surface line
                }
            }
            img.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
        }
    }

    let label = (remaining.round() as i32).to_string();
    if let Some(f) = font() {
        let px = fit_px(f, &label, sf * 0.62, sf * 0.66, sf * 0.54);
        let scale = PxScale::from(px);
        let (tw, th) = text_size(scale, f, &label);
        let x = (s as i32 - tw as i32) / 2;
        let y = (s as i32 - th as i32) / 2;
        draw_text_mut(&mut img, Rgba([WHITE[0], WHITE[1], WHITE[2], 255]), x, y, scale, f, &label);
    }

    let out = image::imageops::resize(&img, 128, 128, image::imageops::FilterType::Lanczos3);
    (out.into_raw(), 128, 128)
}
