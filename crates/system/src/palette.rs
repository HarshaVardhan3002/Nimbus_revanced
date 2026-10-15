//! A colour palette taken from the wallpaper, so the island matches the desktop.
//!
//! The work happens in OKLCH (perceptual lightness, chroma and hue). The dominant
//! hue comes from a chroma-weighted hue histogram; every role is then placed at a
//! fixed lightness and chroma, so any wallpaper gives a palette with the same
//! contrast. A picture with almost no colour gets a quiet silver palette instead of
//! a hue invented from noise.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Palette {
    /// Main colour: the active tab, sliders, switches that are on.
    pub accent: String,
    /// Text and icons drawn on the accent.
    pub ink: String,
    /// Second data colour (the GPU dial).
    pub chest: String,
    /// Healthy and finished (the memory dial, done to-dos).
    pub done: String,
    /// Warm highlight (the timer).
    pub ember: String,
    /// Panel background.
    pub panel: String,
    /// Dominant hue in degrees.
    pub hue: f32,
    /// The picture has almost no colour (black and white, grey).
    pub neutral: bool,
}

/// Palette from a picture's pixels. A few thousand pixels are plenty; pass a small
/// thumbnail rather than the full image.
pub fn from_pixels(pixels: impl Iterator<Item = [u8; 3]>) -> Palette {
    const BINS: usize = 72; // 5 degrees each
    let lut = linear_lut();
    let mut weight = [0f64; BINS];
    let mut vx = [0f64; BINS];
    let mut vy = [0f64; BINS];
    let mut chroma = [0f64; BINS];
    let (mut count, mut coloured) = (0usize, 0usize);

    for [r, g, b] in pixels {
        count += 1;
        let (l, a, bb) = oklab(lut[r as usize], lut[g as usize], lut[b as usize]);
        let c = (a * a + bb * bb).sqrt();
        if c < 0.03 {
            continue;
        }
        coloured += 1;
        // Mid tones say more about a picture's colour than near-black shadows or highlights.
        let lw = (1.0 - (l - 0.6).abs() * 1.4).clamp(0.12, 1.0);
        let w = (c * lw) as f64;
        let h = bb.atan2(a);
        let bin = ((h.to_degrees().rem_euclid(360.0) / 5.0) as usize).min(BINS - 1);
        weight[bin] += w;
        vx[bin] += w * h.cos() as f64;
        vy[bin] += w * h.sin() as f64;
        chroma[bin] += w * c as f64;
    }

    // Smooth the histogram around the circle, then take the strongest region.
    let near = |i: usize, d: isize| (i as isize + d).rem_euclid(BINS as isize) as usize;
    let peak = (0..BINS)
        .max_by(|&i, &j| {
            let s = |k: usize| (-3..=3).map(|d| weight[near(k, d)]).sum::<f64>();
            s(i).total_cmp(&s(j))
        })
        .unwrap_or(0);
    let (mut sx, mut sy, mut sw, mut sc) = (0.0, 0.0, 0.0, 0.0);
    for d in -4..=4 {
        let k = near(peak, d);
        sx += vx[k];
        sy += vy[k];
        sw += weight[k];
        sc += chroma[k];
    }
    let hue = (sy.atan2(sx).to_degrees() as f32).rem_euclid(360.0);
    let dom_chroma = if sw > 0.0 { (sc / sw) as f32 } else { 0.0 };
    let neutral = count == 0 || (coloured as f32 / count as f32) < 0.06 || dom_chroma < 0.045;
    build(if neutral { 255.0 } else { hue }, dom_chroma, neutral)
}

/// Palette around a chosen accent colour ("#RRGGBB"), for a fixed accent setting.
pub fn from_accent(hex: &str) -> Option<Palette> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    let lut = linear_lut();
    let (_, a, b) = oklab(lut[(v >> 16) as usize & 255], lut[(v >> 8) as usize & 255], lut[v as usize & 255]);
    let c = (a * a + b * b).sqrt();
    let hue = b.atan2(a).to_degrees().rem_euclid(360.0);
    Some(build(hue, c, c < 0.03))
}

fn build(hue: f32, dom_chroma: f32, neutral: bool) -> Palette {
    let away = |base: f32, min: f32, alt: f32| if hue_gap(hue, base) < min { alt } else { base };
    let (accent, ink, panel) = if neutral {
        (oklch(0.87, 0.018, hue), oklch(0.2, 0.01, hue), oklch(0.15, 0.005, hue))
    } else {
        let c = (dom_chroma * 1.25).clamp(0.09, 0.16);
        (oklch(0.78, c, hue), oklch(0.25, (c * 0.5).min(0.07), hue), oklch(0.155, (c * 0.14).min(0.022), hue))
    };
    // The data colours stay recognisable everywhere, and step aside when the accent sits on them.
    let chest = oklch(0.78, 0.13, away(305.0, 25.0, hue + 50.0));
    let done = oklch(0.8, 0.14, away(140.0, 35.0, 175.0));
    let ember = if !neutral && hue_gap(hue, 85.0) < 35.0 { oklch(0.84, 0.09, 30.0) } else { oklch(0.85, 0.15, 85.0) };
    Palette { accent, ink, chest, done, ember, panel, hue, neutral }
}

fn hue_gap(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn linear_lut() -> [f32; 256] {
    let mut t = [0f32; 256];
    for (i, v) in t.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    }
    t
}

/// Linear sRGB to OKLab (Björn Ottosson's matrices).
fn oklab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    (
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    )
}

fn linear_rgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
        -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
    ]
}

/// "#RRGGBB" for an OKLCH colour, lowering chroma until it fits in sRGB.
fn oklch(l: f32, c: f32, hue: f32) -> String {
    let (s, co) = hue.to_radians().sin_cos();
    let mut c = c;
    let rgb = loop {
        let rgb = linear_rgb(l, c * co, c * s);
        if c <= 0.0 || rgb.iter().all(|v| (-0.0005..=1.0005).contains(v)) {
            break rgb;
        }
        c -= 0.004;
    };
    let enc = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let e = if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (e * 255.0).round() as u8
    };
    format!("#{:02X}{:02X}{:02X}", enc(rgb[0]), enc(rgb[1]), enc(rgb[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_hue(hex: &str) -> f32 {
        from_accent(hex).unwrap().hue
    }

    #[test]
    fn orange_picture_gives_orange_accent() {
        let p = from_pixels(std::iter::repeat_n([230, 120, 60], 500).chain(std::iter::repeat_n([10, 10, 10], 500)));
        assert!(!p.neutral);
        assert!((25.0..70.0).contains(&p.hue), "hue {}", p.hue);
        assert!((25.0..70.0).contains(&hex_hue(&p.accent)));
    }

    #[test]
    fn black_and_white_picture_is_neutral() {
        let p = from_pixels((0..1000u32).map(|i| { let v = (i % 256) as u8; [v, v, v] }));
        assert!(p.neutral);
    }

    #[test]
    fn colours_round_trip_through_hex() {
        // The accent is placed at OKLCH lightness 0.78; reading it back lands near the same hue.
        let p = from_pixels(std::iter::repeat_n([60, 130, 220], 800));
        assert!((p.hue - hex_hue(&p.accent)).abs() < 4.0, "{} vs {}", p.hue, hex_hue(&p.accent));
    }
}
