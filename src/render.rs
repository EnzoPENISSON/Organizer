//! Draws a box's two layers as premultiplied BGRA pixels, with antialiased rounded
//! corners, gradients and a glow. Only runs when a box's look or size changes.

use crate::theme::{Fill, Rgba, Theme};

/// Theme lengths in device pixels.
#[derive(Clone, Copy, Debug)]
pub struct Sizes {
    pub radius: f32,
    pub border: f32,
    pub glow: f32,
    pub title: f32,
    /// How far in from each edge the frame must catch the mouse.
    pub grip: i32,
}

pub struct Layer {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

/// Premultiplied color, every channel 0..=1.
#[derive(Clone, Copy)]
struct Pre {
    r: f32,
    g: f32,
    b: f32,
    a: f32,
}

impl Pre {
    const NONE: Pre = Pre { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

    fn of(c: Rgba) -> Pre {
        Pre { r: c.r * c.a, g: c.g * c.a, b: c.b * c.a, a: c.a }
    }
    fn scaled(self, k: f32) -> Pre {
        Pre { r: self.r * k, g: self.g * k, b: self.b * k, a: self.a * k }
    }
    fn lerp(self, o: Pre, t: f32) -> Pre {
        let u = 1.0 - t;
        Pre { r: self.r * u + o.r * t, g: self.g * u + o.g * t, b: self.b * u + o.b * t, a: self.a * u + o.a * t }
    }
    fn over(self, below: Pre) -> Pre {
        let k = 1.0 - self.a;
        Pre { r: self.r + below.r * k, g: self.g + below.g * k, b: self.b + below.b * k, a: self.a + below.a * k }
    }
    fn to_bgra(self) -> u32 {
        let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        (byte(self.a) << 24) | (byte(self.r) << 16) | (byte(self.g) << 8) | byte(self.b)
    }
}

/// A theme fill resolved for one rectangle, ready to sample per pixel.
struct Sampler {
    stops: Vec<Pre>,
    dx: f32,
    dy: f32,
    length: f32,
    cx: f32,
    cy: f32,
}

impl Sampler {
    fn new(fill: &Fill, accent: Rgba, w: f32, h: f32) -> Sampler {
        let (angle, stops): (f32, Vec<Pre>) = match fill {
            Fill::Solid(p) => (0.0, vec![Pre::of(p.resolve(accent))]),
            Fill::Linear(a, s) => (*a, s.iter().map(|p| Pre::of(p.resolve(accent))).collect()),
        };
        let (sin, cos) = angle.to_radians().sin_cos();
        // The CSS gradient line length, so corners land exactly on the end colors.
        let length = (w * sin.abs() + h * cos.abs()).max(1.0);
        Sampler { stops, dx: sin, dy: -cos, length, cx: w / 2.0, cy: h / 2.0 }
    }

    fn at(&self, x: f32, y: f32) -> Pre {
        if self.stops.len() == 1 {
            return self.stops[0];
        }
        let t = (((x - self.cx) * self.dx + (y - self.cy) * self.dy) / self.length + 0.5).clamp(0.0, 1.0);
        let pos = t * (self.stops.len() - 1) as f32;
        let i = (pos as usize).min(self.stops.len() - 2);
        self.stops[i].lerp(self.stops[i + 1], pos - i as f32)
    }
}

/// Signed distance from a point to a rounded rectangle at the origin. Negative is inside.
fn rounded_distance(x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let qx = (x - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (y - h / 2.0).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

/// How far the tint layer extends past the box on each side, to fit the glow.
pub fn glow_margin(s: &Sizes) -> i32 {
    s.glow.ceil() as i32
}

/// The body and glow. Click-through, and larger than the box by `glow_margin`.
pub fn tint_layer(t: &Theme, s: &Sizes, accent: Rgba, w: i32, h: i32) -> Layer {
    let margin = glow_margin(s);
    let (lw, lh) = (w + 2 * margin, h + 2 * margin);
    let fill = Sampler::new(&t.background, accent, w as f32, h as f32);
    let glow = Pre::of(t.glow_color.resolve(accent));
    let mut px = vec![0u32; (lw * lh) as usize];
    for y in 0..lh {
        let py = (y - margin) as f32 + 0.5;
        for x in 0..lw {
            let qx = (x - margin) as f32 + 0.5;
            let d = rounded_distance(qx, py, w as f32, h as f32, s.radius);
            let body = fill.at(qx, py).scaled(coverage(d));
            let halo = if s.glow > 0.0 && d > -1.0 {
                let falloff = (1.0 - d.max(0.0) / s.glow).clamp(0.0, 1.0);
                glow.scaled(falloff * falloff * (1.0 - coverage(d)))
            } else {
                Pre::NONE
            };
            px[(y * lw + x) as usize] = body.over(halo).to_bgra();
        }
    }
    Layer { w: lw, h: lh, px }
}

/// The title bar and border. Pixels with zero alpha let clicks through to the icons,
/// so the middle stays empty, while invisible alpha 1 pixels keep the edges grabbable.
pub fn frame_layer(t: &Theme, s: &Sizes, accent: Rgba, w: i32, h: i32) -> Layer {
    let title = Sampler::new(&t.title_background, accent, w as f32, s.title);
    let border = Pre::of(t.border_color.resolve(accent));
    let (fw, fh) = (w as f32, h as f32);
    let inner_radius = (s.radius - s.border).max(0.0);
    let mut px = vec![0u32; (w * h) as usize];
    for y in 0..h {
        let py = y as f32 + 0.5;
        let in_title = y < s.title.ceil() as i32;
        let y_edge = y >= h - s.grip;
        for x in 0..w {
            let on_edge = x < s.grip || x >= w - s.grip;
            if !(in_title || y_edge || on_edge) {
                continue;
            }
            let qx = x as f32 + 0.5;
            let d = rounded_distance(qx, py, fw, fh, s.radius);
            let outer = coverage(d);
            if outer <= 0.0 {
                continue;
            }
            let mut c = if in_title { title.at(qx, py).scaled(outer) } else { Pre::NONE };
            if s.border > 0.0 {
                let inner = rounded_distance(qx - s.border, py - s.border, fw - 2.0 * s.border, fh - 2.0 * s.border, inner_radius);
                c = border.scaled((outer - coverage(inner)).clamp(0.0, 1.0)).over(c);
            }
            let bgra = c.to_bgra();
            px[(y * w + x) as usize] = if bgra >> 24 == 0 { 1 << 24 } else { bgra };
        }
    }
    Layer { w, h, px }
}

/// Paints `color` through a coverage mask (one byte per pixel) at the layer's origin.
/// GDI text has no alpha of its own, so it is drawn to a mask and composited here.
pub fn blend_mask(layer: &mut Layer, mask: &[u8], mask_w: i32, mask_h: i32, color: Rgba) {
    let c = Pre::of(color);
    for y in 0..mask_h.min(layer.h) {
        for x in 0..mask_w.min(layer.w) {
            let m = mask[(y * mask_w + x) as usize];
            if m == 0 {
                continue;
            }
            let i = (y * layer.w + x) as usize;
            let p = layer.px[i];
            let below = Pre {
                a: (p >> 24) as f32 / 255.0,
                r: ((p >> 16) & 0xFF) as f32 / 255.0,
                g: ((p >> 8) & 0xFF) as f32 / 255.0,
                b: (p & 0xFF) as f32 / 255.0,
            };
            layer.px[i] = c.scaled(m as f32 / 255.0).over(below).to_bgra();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Paint, Theme};

    const ACCENT: Rgba = Rgba { r: 0.0, g: 0.9, b: 1.0, a: 1.0 };
    const S: Sizes = Sizes { radius: 12.0, border: 1.0, glow: 10.0, title: 28.0, grip: 5 };

    fn alpha(l: &Layer, x: i32, y: i32) -> u32 {
        l.px[(y * l.w + x) as usize] >> 24
    }

    #[test]
    fn frame_catches_the_mouse_only_at_title_and_edges() {
        let l = frame_layer(&Theme::default(), &S, ACCENT, 200, 150);
        assert!(alpha(&l, 100, 10) > 100, "title is visible");
        assert_eq!(alpha(&l, 100, 80), 0, "middle must let clicks reach the icons");
        assert!(alpha(&l, 2, 80) >= 1, "left grip");
        assert!(alpha(&l, 197, 80) >= 1, "right grip");
        assert!(alpha(&l, 100, 147) >= 1, "bottom grip");
    }

    #[test]
    fn grips_work_even_with_an_invisible_theme() {
        let mut t = Theme::default();
        t.title_background = Fill::Solid(Paint::Color(Rgba::TRANSPARENT));
        t.border_color = Paint::Color(Rgba::TRANSPARENT);
        let l = frame_layer(&t, &S, ACCENT, 200, 150);
        assert_eq!(alpha(&l, 100, 10), 1);
        assert_eq!(alpha(&l, 2, 80), 1);
    }

    #[test]
    fn corners_are_rounded_and_antialiased() {
        let l = frame_layer(&Theme::default(), &S, ACCENT, 200, 150);
        assert_eq!(alpha(&l, 0, 0), 0, "corner is cut away");
        // Along a row near the top, alpha climbs from nothing through partial coverage.
        let row: Vec<u32> = (0..12).map(|x| alpha(&l, x, 2)).collect();
        let solid = alpha(&l, 30, 2);
        assert_eq!(row[0], 0, "{row:?}");
        assert!(row.iter().any(|a| *a > 1 && *a < solid), "soft edge: {row:?} below {solid}");
    }

    #[test]
    fn tint_has_a_glow_that_fades_outward() {
        let l = tint_layer(&Theme::default(), &S, ACCENT, 200, 150);
        assert_eq!((l.w, l.h), (220, 170));
        let y = l.h / 2;
        assert!(alpha(&l, 9, y) > alpha(&l, 4, y), "glow fades with distance");
        assert_eq!(alpha(&l, 0, y), 0, "glow ends inside the layer");
    }

    #[test]
    fn output_is_valid_premultiplied_color() {
        for l in [
            frame_layer(&Theme::default(), &S, ACCENT, 90, 70),
            tint_layer(&Theme::default(), &S, ACCENT, 90, 70),
        ] {
            for p in l.px {
                let (a, r, g, b) = (p >> 24, (p >> 16) & 255, (p >> 8) & 255, p & 255);
                assert!(r <= a && g <= a && b <= a, "{p:#x}");
            }
        }
    }

    #[test]
    fn gradient_reaches_both_end_colors() {
        let fill = Fill::Linear(90.0, vec![Paint::Color(Rgba::from_rgb24(0)), Paint::Color(Rgba::from_rgb24(0xFFFFFF))]);
        let s = Sampler::new(&fill, ACCENT, 100.0, 10.0);
        assert!(s.at(0.0, 5.0).r < 0.01);
        assert!(s.at(100.0, 5.0).r > 0.99);
        assert!((s.at(50.0, 5.0).r - 0.5).abs() < 0.01);
    }

    #[test]
    fn text_mask_composites_over_the_layer() {
        let mut l = frame_layer(&Theme::default(), &S, ACCENT, 50, 50);
        let mask = vec![255u8; 50 * 28];
        blend_mask(&mut l, &mask, 50, 28, Rgba::from_rgb24(0xFFFFFF));
        assert_eq!(l.px[(10 * 50 + 25) as usize], 0xFFFFFFFF);
    }

    #[test]
    fn tiny_sizes_do_not_panic() {
        let t = Theme::default();
        for (w, h) in [(1, 1), (2, 40), (40, 2)] {
            let _ = frame_layer(&t, &S, ACCENT, w, h);
            let _ = tint_layer(&t, &S, ACCENT, w, h);
        }
    }
}
