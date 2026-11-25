//! Mio's display identity. Frontends share the same palette and vector mark;
//! persisted filenames, SDK crate names and native protocols are independent.
#![allow(dead_code)] // Each frontend consumes a different subset of the identity.

pub const CLIENT_NAME: &str = "Mio Client";
pub const LAUNCHER_NAME: &str = "Mio Launcher";
pub const WORDMARK: &str = "MIO";
pub const TAGLINE: &str = "自分らしいプレイを、もっと自由に。";
pub const TAGLINE_EN: &str = "Your game. Your way.";
pub const BG: [u8; 3] = [12, 14, 22];
pub const SURFACE: [u8; 3] = [21, 25, 38];
pub const FIELD: [u8; 3] = [15, 18, 29];
pub const BORDER: [u8; 3] = [49, 55, 78];
pub const TEXT: [u8; 3] = [242, 244, 255];
pub const MUTED: [u8; 3] = [182, 190, 211];
pub const ACCENT: [u8; 3] = [164, 175, 255];
pub const SECONDARY: [u8; 3] = [102, 224, 201];
pub const LOGO_BG: [u8; 3] = [33, 37, 62];
pub const MARK: [[f32; 2]; 5] = [
    [0.23, 0.73],
    [0.23, 0.32],
    [0.5, 0.55],
    [0.77, 0.32],
    [0.77, 0.73],
];
pub const MARK_WIDTH: f32 = 0.075;
pub const DOT: [f32; 2] = [0.79, 0.18];
pub const DOT_RADIUS: f32 = 0.045;

pub const fn color(rgb: [u8; 3], alpha: f32) -> [f32; 4] {
    [
        rgb[0] as f32 / 255.,
        rgb[1] as f32 / 255.,
        rgb[2] as f32 / 255.,
        alpha,
    ]
}
fn pixel(x: f32, y: f32) -> [u8; 4] {
    let dx = x - x.clamp(0.24, 0.76);
    let dy = y - y.clamp(0.24, 0.76);
    if dx * dx + dy * dy > 0.24 * 0.24 {
        return [0; 4];
    }
    let mut rgb = LOGO_BG;
    for segment in MARK.windows(2) {
        let [a, b] = [segment[0], segment[1]];
        let vx = b[0] - a[0];
        let vy = b[1] - a[1];
        let t = (((x - a[0]) * vx + (y - a[1]) * vy) / (vx * vx + vy * vy)).clamp(0., 1.);
        let dx = x - a[0] - t * vx;
        let dy = y - a[1] - t * vy;
        if dx * dx + dy * dy <= (MARK_WIDTH * 0.5).powi(2) {
            rgb = TEXT;
        }
    }
    if (x - DOT[0]).powi(2) + (y - DOT[1]).powi(2) <= DOT_RADIUS.powi(2) {
        rgb = SECONDARY;
    }
    [rgb[0], rgb[1], rgb[2], 255]
}
/// Unmultiplied RGBA, with four coverage samples per pixel for the native icon.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let size = size.clamp(1, 256);
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut total = [0u32; 4];
            for [sx, sy] in [[0.25, 0.25], [0.75, 0.25], [0.25, 0.75], [0.75, 0.75]] {
                let c = pixel((x as f32 + sx) / size as f32, (y as f32 + sy) / size as f32);
                for k in 0..3 {
                    total[k] += u32::from(c[k]) * u32::from(c[3]);
                }
                total[3] += u32::from(c[3]);
            }
            let a = total[3];
            rgba.extend([
                total[0].checked_div(a).unwrap_or(0) as u8,
                total[1].checked_div(a).unwrap_or(0) as u8,
                total[2].checked_div(a).unwrap_or(0) as u8,
                (a / 4) as u8,
            ]);
        }
    }
    rgba
}
