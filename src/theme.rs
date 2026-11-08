//! Parses the CSS subset behind `theme.css`. It is real CSS syntax, handled natively,
//! because a browser engine would cost more memory than the rest of the app by far.
//! Bad input never fails: the rule keeps its current value and a warning is returned.

use std::collections::HashMap;

/// Graphite, used when `theme.css` does not set `--accent`.
pub const DEFAULT_ACCENT: u32 = 0x3B4252;

pub const DEFAULT_CSS: &str = r#"/* Organizer theme — "Frosted Glass". Save this file and your boxes update within a second.

   This is a focused subset of CSS, not a browser. Selectors: :root, .box, .title
   var(--accent) is the box color, set with --accent in :root below.

   .box:    background, border, border-width, border-color, border-radius,
            box-shadow, padding
   .title:  height, background, color, font-family, font-size, font-weight,
            text-align, text-transform, letter-spacing, padding

   Colors:  #rgb #rrggbb #rrggbbaa rgb() rgba() white black transparent var(--name)
            color-mix(in srgb, COLOR 40%, COLOR)
   Fills:   a color, or linear-gradient(135deg, COLOR, COLOR, ...)
   Glow:    box-shadow: 0 0 BLUR COLOR (or none)

   Previous theme saved as theme.backup.css.
   Delete this file to get the default theme back. */

:root {
  /* Shared palette — tweak these to restyle every box at once. */
  --accent:   #3B4252;                  /* box color (graphite)   */
  --ink:      rgba(12, 14, 22, 0.55);   /* dark glass base        */
  --ink-deep: rgba(12, 14, 22, 0.82);   /* title bar base         */
  --frost:    rgba(255, 255, 255, 0.06);/* light sheen            */
  --edge:     rgba(255, 255, 255, 0.28);/* light rim for borders  */
  --text:     #f2f4f8;
}

.box {
  /* Translucent frosted panel: a soft sheen at the top fading into a
     faint accent tint, so icons stay readable on any wallpaper. */
  background: linear-gradient(180deg,
    var(--frost),
    color-mix(in srgb, var(--accent) 14%, transparent),
    color-mix(in srgb, var(--accent) 8%, rgba(12, 14, 22, 0.18)));

  /* Accent lifted toward white so the rim reads even with dark accents. */
  border: 1px solid color-mix(in srgb, var(--accent) 55%, var(--edge));
  border-radius: 14px;

  /* Soft, wide halo instead of a hard neon glow. */
  box-shadow: 0 0 22px color-mix(in srgb, var(--accent) 28%, transparent);
  padding: 6px;
}

.title {
  height: 30px;
  padding: 0 14px;

  /* Accent fades out to the right, sitting on deep glass. */
  background: linear-gradient(90deg,
    color-mix(in srgb, var(--accent) 70%, var(--ink-deep)),
    color-mix(in srgb, var(--accent) 25%, var(--ink-deep)),
    var(--ink));

  color: var(--text);
  font-family: "Segoe UI Variable Display";  /* DIN-style, ships with Windows */
  font-size: 13px;
  font-weight: 600;
  text-align: center;
  text-transform: uppercase;
  letter-spacing: 1.6px;
}
"#;

/// Straight-alpha color, every channel 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

    pub fn from_rgb24(rgb: u32) -> Rgba {
        Rgba {
            r: ((rgb >> 16) & 0xFF) as f32 / 255.0,
            g: ((rgb >> 8) & 0xFF) as f32 / 255.0,
            b: (rgb & 0xFF) as f32 / 255.0,
            a: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Color(Rgba),
    Accent,
    Mix(Box<Paint>, f32, Box<Paint>),
}

impl Paint {
    pub fn resolve(&self, accent: Rgba) -> Rgba {
        match self {
            Paint::Color(c) => *c,
            Paint::Accent => accent,
            Paint::Mix(a, p, b) => mix(a.resolve(accent), *p, b.resolve(accent)),
        }
    }
}

/// Same math as CSS color-mix in srgb: interpolates premultiplied values.
fn mix(a: Rgba, p: f32, b: Rgba) -> Rgba {
    let q = 1.0 - p;
    let alpha = a.a * p + b.a * q;
    if alpha <= 0.0 {
        return Rgba::TRANSPARENT;
    }
    let ch = |x: f32, y: f32| (x * a.a * p + y * b.a * q) / alpha;
    Rgba { r: ch(a.r, b.r), g: ch(a.g, b.g), b: ch(a.b, b.b), a: alpha }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    Solid(Paint),
    /// CSS angle in degrees (0 points up, clockwise), with evenly spaced stops.
    Linear(f32, Vec<Paint>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// `--accent` from `:root`, shared by every box.
    pub accent: Rgba,
    pub background: Fill,
    pub border_width: f32,
    pub border_color: Paint,
    pub radius: f32,
    pub glow_blur: f32,
    pub glow_color: Paint,
    pub padding: f32,
    pub title_height: f32,
    pub title_background: Fill,
    pub title_color: Paint,
    pub title_padding: f32,
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: i32,
    pub text_align: Align,
    pub uppercase: bool,
    pub letter_spacing: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

impl Default for Theme {
    fn default() -> Self {
        let fallback = Theme {
            accent: Rgba::from_rgb24(DEFAULT_ACCENT),
            background: Fill::Solid(Paint::Color(Rgba::TRANSPARENT)),
            border_width: 1.0,
            border_color: Paint::Accent,
            radius: 0.0,
            glow_blur: 0.0,
            glow_color: Paint::Accent,
            padding: 4.0,
            title_height: 28.0,
            title_background: Fill::Solid(Paint::Accent),
            title_color: Paint::Color(Rgba { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }),
            title_padding: 8.0,
            font_family: "Segoe UI".into(),
            font_size: 12.0,
            font_weight: 600,
            text_align: Align::Left,
            uppercase: false,
            letter_spacing: 0.0,
        };
        parse(DEFAULT_CSS, fallback).0
    }
}

/// Applies `css` on top of `base`, returning the theme and a warning for anything ignored.
pub fn parse(css: &str, base: Theme) -> (Theme, Vec<String>) {
    let mut theme = base;
    let mut warnings = Vec::new();
    let text = strip_comments(css);
    let rules = split_rules(&text);

    let mut vars = HashMap::new();
    for (selector, body) in &rules {
        if selector == ":root" {
            for (prop, value) in declarations(body) {
                if prop.starts_with("--") {
                    vars.insert(prop, value);
                } else {
                    warnings.push(format!(":root: only --variables are supported, ignored {prop}"));
                }
            }
        }
    }

    // `--accent` becomes the theme's accent instead of being substituted, so
    // `var(--accent)` keeps meaning "the accent" everywhere.
    if let Some(value) = vars.remove("--accent") {
        let value = substitute_vars(&value, &vars);
        match parse_paint(&value) {
            Some(p) => theme.accent = p.resolve(theme.accent),
            None => warnings.push(format!(":root: ignored `--accent: {value}`")),
        }
    }

    for (selector, body) in &rules {
        let apply: fn(&mut Theme, &str, &str) -> Option<()> = match selector.as_str() {
            ":root" => continue,
            ".box" => apply_box,
            ".title" => apply_title,
            other => {
                warnings.push(format!("unknown selector {other}: only :root, .box and .title are supported"));
                continue;
            }
        };
        for (prop, value) in declarations(body) {
            let value = substitute_vars(&value, &vars);
            if apply(&mut theme, &prop, &value).is_none() {
                warnings.push(format!("{selector}: ignored `{prop}: {value}`"));
            }
        }
    }
    (theme, warnings)
}

fn apply_box(t: &mut Theme, prop: &str, v: &str) -> Option<()> {
    match prop {
        "background" | "background-color" | "background-image" => t.background = parse_fill(v)?,
        "border" => {
            let parts = split_top(v, ' ');
            if parts.len() == 1 && parts[0] == "none" {
                t.border_width = 0.0;
                return Some(());
            }
            if parts.len() != 3 {
                return None;
            }
            let (width, color) = (parse_len(&parts[0])?, parse_paint(&parts[2])?);
            t.border_width = width.clamp(0.0, 12.0);
            t.border_color = color;
        }
        "border-width" => t.border_width = parse_len(v)?.clamp(0.0, 12.0),
        "border-color" => t.border_color = parse_paint(v)?,
        "border-radius" => t.radius = parse_len(v)?.clamp(0.0, 60.0),
        "box-shadow" => {
            if v == "none" {
                t.glow_blur = 0.0;
                return Some(());
            }
            let parts = split_top(v, ' ');
            if parts.len() != 4 {
                return None;
            }
            // Offsets are not drawn: a box's window only follows it for a centered glow.
            let (blur, color) = (parse_len(&parts[2])?, parse_paint(&parts[3])?);
            t.glow_blur = blur.clamp(0.0, 64.0);
            t.glow_color = color;
        }
        "padding" => t.padding = parse_len(v)?.clamp(0.0, 40.0),
        _ => return None,
    }
    Some(())
}

fn apply_title(t: &mut Theme, prop: &str, v: &str) -> Option<()> {
    match prop {
        "height" => t.title_height = parse_len(v)?.clamp(16.0, 80.0),
        "background" | "background-color" | "background-image" => t.title_background = parse_fill(v)?,
        "color" => t.title_color = parse_paint(v)?,
        "font-family" => {
            let first = split_top(v, ',').into_iter().next()?;
            t.font_family = first.trim_matches(['"', '\'']).to_string();
        }
        "font-size" => t.font_size = parse_len(v)?.clamp(6.0, 48.0),
        "font-weight" => {
            t.font_weight = match v {
                "normal" => 400,
                "bold" => 700,
                n => n.parse::<i32>().ok()?.clamp(100, 900),
            }
        }
        "text-align" => {
            t.text_align = match v {
                "left" => Align::Left,
                "center" => Align::Center,
                "right" => Align::Right,
                _ => return None,
            }
        }
        "text-transform" => {
            t.uppercase = match v {
                "uppercase" => true,
                "none" => false,
                _ => return None,
            }
        }
        "letter-spacing" => t.letter_spacing = if v == "normal" { 0.0 } else { parse_len(v)?.clamp(-2.0, 12.0) },
        "padding" => {
            // `all` or `vertical horizontal`; only the horizontal part affects the title.
            let parts = split_top(v, ' ');
            let horizontal = match parts.len() {
                1 => &parts[0],
                2 => &parts[1],
                _ => return None,
            };
            t.title_padding = parse_len(horizontal)?.clamp(0.0, 60.0);
        }
        _ => return None,
    }
    Some(())
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

fn split_rules(text: &str) -> Vec<(String, String)> {
    let mut rules = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else { break };
        rules.push((rest[..open].trim().to_string(), rest[open + 1..open + close].to_string()));
        rest = &rest[open + close + 1..];
    }
    rules
}

fn declarations(body: &str) -> Vec<(String, String)> {
    split_top(body, ';')
        .into_iter()
        .filter_map(|d| {
            let (prop, value) = d.split_once(':')?;
            Some((prop.trim().to_ascii_lowercase(), value.split_whitespace().collect::<Vec<_>>().join(" ")))
        })
        .collect()
}

/// Splits on `sep` outside parentheses and quotes, dropping empty pieces.
fn split_top(s: &str, sep: char) -> Vec<String> {
    let (mut parts, mut current, mut depth, mut quote) = (Vec::new(), String::new(), 0, None);
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth -= 1,
            (None, c) if c == sep && depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    parts.push(current);
    parts.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

fn substitute_vars(value: &str, vars: &HashMap<String, String>) -> String {
    let mut out = value.to_string();
    // Bounded, so variables that refer to each other cannot loop forever.
    for _ in 0..8 {
        let mut replaced = false;
        for (name, v) in vars {
            let token = format!("var({name})");
            if out.contains(&token) {
                out = out.replace(&token, v);
                replaced = true;
            }
        }
        if !replaced {
            break;
        }
    }
    out
}

fn parse_len(v: &str) -> Option<f32> {
    if v == "0" {
        return Some(0.0);
    }
    v.strip_suffix("px")?.trim().parse().ok().filter(|n: &f32| n.is_finite())
}

fn inside<'a>(v: &'a str, function: &str) -> Option<&'a str> {
    v.strip_prefix(function)?.trim_start().strip_prefix('(')?.strip_suffix(')')
}

pub fn parse_paint(v: &str) -> Option<Paint> {
    let v = v.trim();
    let lower = v.to_ascii_lowercase();
    if lower == "var(--accent)" {
        return Some(Paint::Accent);
    }
    if let Some(args) = inside(&lower, "color-mix") {
        let parts = split_top(args, ',');
        if parts.len() != 3 || parts[0] != "in srgb" {
            return None;
        }
        let (a, pa) = paint_with_percent(&parts[1])?;
        let (b, pb) = paint_with_percent(&parts[2])?;
        let p = match (pa, pb) {
            (Some(x), _) => x,
            (None, Some(y)) => 1.0 - y,
            (None, None) => 0.5,
        };
        return Some(Paint::Mix(Box::new(a), p.clamp(0.0, 1.0), Box::new(b)));
    }
    parse_color(&lower).map(Paint::Color)
}

fn paint_with_percent(v: &str) -> Option<(Paint, Option<f32>)> {
    let parts = split_top(v, ' ');
    match parts.as_slice() {
        [color] => Some((parse_paint(color)?, None)),
        [color, percent] => {
            let p = percent.strip_suffix('%')?.parse::<f32>().ok()? / 100.0;
            Some((parse_paint(color)?, Some(p)))
        }
        _ => None,
    }
}

fn parse_color(v: &str) -> Option<Rgba> {
    match v {
        "transparent" => return Some(Rgba::TRANSPARENT),
        "white" => return Some(Rgba::from_rgb24(0xFFFFFF)),
        "black" => return Some(Rgba::from_rgb24(0)),
        _ => {}
    }
    if let Some(hex) = v.strip_prefix('#') {
        return parse_hex(hex);
    }
    let args = inside(v, "rgba").or_else(|| inside(v, "rgb"))?;
    let parts: Vec<String> = args
        .split([',', ' ', '/'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect();
    if parts.len() != 3 && parts.len() != 4 {
        return None;
    }
    let channel = |s: &str| s.parse::<f32>().ok().map(|n| (n / 255.0).clamp(0.0, 1.0));
    let alpha = match parts.get(3) {
        None => 1.0,
        Some(a) => match a.strip_suffix('%') {
            Some(p) => p.parse::<f32>().ok()? / 100.0,
            None => a.parse::<f32>().ok()?,
        },
    };
    Some(Rgba {
        r: channel(&parts[0])?,
        g: channel(&parts[1])?,
        b: channel(&parts[2])?,
        a: alpha.clamp(0.0, 1.0),
    })
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.is_ascii() {
        return None;
    }
    let byte = |s: &str| u8::from_str_radix(s, 16).ok().map(|b| b as f32 / 255.0);
    let nibble = |i: usize| byte(&hex[i..i + 1].repeat(2));
    match hex.len() {
        3 => Some(Rgba { r: nibble(0)?, g: nibble(1)?, b: nibble(2)?, a: 1.0 }),
        6 | 8 => Some(Rgba {
            r: byte(&hex[0..2])?,
            g: byte(&hex[2..4])?,
            b: byte(&hex[4..6])?,
            a: if hex.len() == 8 { byte(&hex[6..8])? } else { 1.0 },
        }),
        _ => None,
    }
}

pub fn parse_fill(v: &str) -> Option<Fill> {
    let Some(args) = inside(v.trim(), "linear-gradient") else {
        return parse_paint(v).map(Fill::Solid);
    };
    let parts = split_top(args, ',');
    let first = parts.first()?.to_ascii_lowercase();
    let angle = match first.as_str() {
        "to top" => Some(0.0),
        "to right" => Some(90.0),
        "to bottom" => Some(180.0),
        "to left" => Some(270.0),
        s => s.strip_suffix("deg").and_then(|d| d.trim().parse::<f32>().ok()),
    };
    let stops = if angle.is_some() { &parts[1..] } else { &parts[..] };
    let stops: Option<Vec<Paint>> = stops.iter().map(|s| parse_paint(s)).collect();
    let stops = stops?;
    if stops.len() < 2 {
        return None;
    }
    Some(Fill::Linear(angle.unwrap_or(180.0), stops))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn default_css_has_no_warnings() {
        let (_, warnings) = parse(DEFAULT_CSS, Theme::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn applies_declarations_and_ignores_comments() {
        let css = "/* .box { padding: 99px; } */ .box { border-radius: 20px; padding: 8px } .title { height: 40px; text-align: center; font-family: 'Orbitron', sans-serif; }";
        let (t, w) = parse(css, Theme::default());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!((t.radius, t.padding, t.title_height), (20.0, 8.0, 40.0));
        assert_eq!(t.text_align, Align::Center);
        assert_eq!(t.font_family, "Orbitron");
    }

    #[test]
    fn root_variables_are_substituted() {
        let css = ":root { --neon: #00ff00; --edge: var(--neon); } .box { border-color: var(--edge); }";
        let (t, w) = parse(css, Theme::default());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(t.border_color, Paint::Color(Rgba::from_rgb24(0x00FF00)));
    }

    #[test]
    fn accent_defaults_to_graphite_and_is_set_from_root() {
        assert_eq!(Theme::default().accent, Rgba::from_rgb24(DEFAULT_ACCENT));
        let (t, w) = parse(":root { --cyan: #00e5ff; --accent: var(--cyan); } .box { border-color: var(--accent); }", Theme::default());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(t.accent, Rgba::from_rgb24(0x00E5FF));
        assert_eq!(t.border_color, Paint::Accent);
    }

    #[test]
    fn self_referencing_variables_do_not_hang() {
        let (t, w) = parse(":root { --a: var(--a); } .box { border-color: var(--a); }", Theme::default());
        assert_eq!(w.len(), 1);
        assert_eq!(t.border_color, Theme::default().border_color);
    }

    #[test]
    fn bad_values_warn_and_keep_the_previous_value() {
        let (t, w) = parse(
            ".box { border-radius: banana; float: left; } .nope { color: red; } .title { color: #12; }",
            Theme::default(),
        );
        assert_eq!(w.len(), 4, "{w:?}");
        assert_eq!(t, Theme::default());
    }

    #[test]
    fn unclosed_input_does_not_panic() {
        for css in [".box {", "}", "{}", "/* open", ".box { color", ".title { color: rgb(; }", "é{é:é}", ".title{color:#ééé}"] {
            let _ = parse(css, Theme::default());
        }
    }

    #[test]
    fn extreme_lengths_are_clamped() {
        let (t, _) = parse(".box { border-radius: 9999px; border-width: 500px; } .title { height: 1px; }", Theme::default());
        assert_eq!((t.radius, t.border_width, t.title_height), (60.0, 12.0, 16.0));
    }

    #[test]
    fn parses_color_forms() {
        let c = |s| parse_paint(s).unwrap().resolve(Rgba::TRANSPARENT);
        assert_eq!(c("#fff"), Rgba::from_rgb24(0xFFFFFF));
        assert_eq!(c("#102030"), Rgba::from_rgb24(0x102030));
        assert!(close(c("#00000080").a, 0.5));
        assert!(close(c("rgba(255, 0, 0, 0.25)").a, 0.25));
        assert!(close(c("rgb(255 0 0 / 50%)").a, 0.5));
        assert!(close(c("rgb(255, 128, 0)").g, 0.5));
        assert!(parse_paint("#12").is_none());
        assert!(parse_paint("rgb(1,2)").is_none());
    }

    #[test]
    fn color_mix_with_transparent_keeps_hue_and_scales_alpha() {
        let accent = Rgba::from_rgb24(0x00E5FF);
        let m = parse_paint("color-mix(in srgb, var(--accent) 40%, transparent)").unwrap().resolve(accent);
        assert!(close(m.a, 0.4));
        assert!(close(m.g, accent.g) && close(m.b, accent.b), "{m:?}");
    }

    #[test]
    fn parses_shadow_border_and_gradient() {
        let css = ".box { box-shadow: 0 0 20px #ff0000; border: 2px solid #00ff00; background: linear-gradient(to right, #000, #fff, #000); }";
        let (t, w) = parse(css, Theme::default());
        assert!(w.is_empty(), "{w:?}");
        assert_eq!((t.glow_blur, t.border_width), (20.0, 2.0));
        assert!(matches!(&t.background, Fill::Linear(a, s) if *a == 90.0 && s.len() == 3));
    }
}
