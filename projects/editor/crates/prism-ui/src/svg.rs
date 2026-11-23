//! Lightweight analytic-SDF SVG rasterizer tailored for 24×24 Lucide icons.
//!
//! Parses standard SVG shape elements (`<path>`, `<circle>`, `<rect>`, `<line>`,
//! `<polyline>`, `<polygon>`) and rasterizes `stroke-width="2"` round-cap/join
//! strokes into an RGBA8 white-alpha mask (`[255, 255, 255, alpha]`) so the
//! GPU UI overlay can tint any icon cleanly at runtime.

use glam::Vec2;

const VIEWBOX_SIZE: f32 = 24.0;
const DEFAULT_STROKE_WIDTH: f32 = 2.0;

#[derive(Clone, Debug)]
enum SvgPrimitive {
    Segment(Vec2, Vec2),
    Circle { center: Vec2, radius: f32 },
    RoundedRect { min: Vec2, max: Vec2, rx: f32 },
}

/// Rasterize a 24×24 Lucide SVG string into a `size × size` RGBA8 white-alpha
/// buffer (`[255, 255, 255, coverage]`).
pub fn rasterize_lucide_svg(svg: &str, size: u32) -> Vec<u8> {
    let primitives = parse_svg_primitives(svg);
    let size = size.max(1);
    let mut out = vec![0u8; (size * size * 4) as usize];
    let scale = VIEWBOX_SIZE / size as f32;
    let half_stroke = DEFAULT_STROKE_WIDTH * 0.5;
    let feather = scale.max(0.25);

    for py in 0..size {
        for px in 0..size {
            let p = Vec2::new((px as f32 + 0.5) * scale, (py as f32 + 0.5) * scale);
            let mut min_dist = f32::MAX;
            for prim in &primitives {
                let d = match *prim {
                    SvgPrimitive::Segment(a, b) => dist_to_segment(p, a, b) - half_stroke,
                    SvgPrimitive::Circle { center, radius } => {
                        (p.distance(center) - radius).abs() - half_stroke
                    }
                    SvgPrimitive::RoundedRect { min, max, rx } => {
                        let center = (min + max) * 0.5;
                        let half = (max - min) * 0.5;
                        sd_round_box(p - center, half, rx).abs() - half_stroke
                    }
                };
                min_dist = min_dist.min(d);
            }
            let cov = (1.0 - smoothstep(-feather * 0.6, feather * 0.6, min_dist)).clamp(0.0, 1.0);
            let alpha = (cov * 255.0).round() as u8;
            let idx = ((py * size + px) * 4) as usize;
            out[idx] = 255;
            out[idx + 1] = 255;
            out[idx + 2] = 255;
            out[idx + 3] = alpha;
        }
    }
    out
}

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0).max(1e-5)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn dist_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq <= 1e-8 {
        return p.distance(a);
    }
    let t = (p - a).dot(ab) / len_sq;
    let proj = a + ab * t.clamp(0.0, 1.0);
    p.distance(proj)
}

fn sd_round_box(p: Vec2, b: Vec2, r: f32) -> f32 {
    let r = r.min(b.x).min(b.y).max(0.0);
    let q = p.abs() - b + Vec2::splat(r);
    q.x.max(q.y).min(0.0) + q.max(Vec2::ZERO).length() - r
}

fn parse_svg_primitives(svg: &str) -> Vec<SvgPrimitive> {
    let mut out = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('>') else {
            break;
        };
        let tag = &rest[..end];
        rest = &rest[end + 1..];

        let tag_trimmed = tag.trim();
        if tag_trimmed.starts_with('/')
            || tag_trimmed.starts_with('?')
            || tag_trimmed.starts_with('!')
        {
            continue;
        }
        let name = tag_trimmed
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        match name {
            "path" => {
                if let Some(d) = attr(tag_trimmed, "d") {
                    parse_path_data(d, &mut out);
                }
            }
            "line" => {
                let x1 = attr_f32(tag_trimmed, "x1").unwrap_or(0.0);
                let y1 = attr_f32(tag_trimmed, "y1").unwrap_or(0.0);
                let x2 = attr_f32(tag_trimmed, "x2").unwrap_or(0.0);
                let y2 = attr_f32(tag_trimmed, "y2").unwrap_or(0.0);
                out.push(SvgPrimitive::Segment(Vec2::new(x1, y1), Vec2::new(x2, y2)));
            }
            "circle" => {
                let cx = attr_f32(tag_trimmed, "cx").unwrap_or(0.0);
                let cy = attr_f32(tag_trimmed, "cy").unwrap_or(0.0);
                let r = attr_f32(tag_trimmed, "r").unwrap_or(0.0);
                out.push(SvgPrimitive::Circle {
                    center: Vec2::new(cx, cy),
                    radius: r,
                });
            }
            "rect" => {
                let x = attr_f32(tag_trimmed, "x").unwrap_or(0.0);
                let y = attr_f32(tag_trimmed, "y").unwrap_or(0.0);
                let w = attr_f32(tag_trimmed, "width").unwrap_or(0.0);
                let h = attr_f32(tag_trimmed, "height").unwrap_or(0.0);
                let rx = attr_f32(tag_trimmed, "rx").unwrap_or(0.0);
                out.push(SvgPrimitive::RoundedRect {
                    min: Vec2::new(x, y),
                    max: Vec2::new(x + w, y + h),
                    rx,
                });
            }
            "polyline" | "polygon" => {
                if let Some(pts) = attr(tag_trimmed, "points") {
                    let nums = parse_numbers(pts);
                    let mut points = Vec::with_capacity(nums.len() / 2);
                    for &[x, y] in nums.as_chunks::<2>().0 {
                        points.push(Vec2::new(x, y));
                    }
                    for w in points.windows(2) {
                        out.push(SvgPrimitive::Segment(w[0], w[1]));
                    }
                    if name == "polygon" && points.len() >= 2 {
                        out.push(SvgPrimitive::Segment(*points.last().unwrap(), points[0]));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn attr<'a>(tag: &'a str, key: &str) -> Option<&'a str> {
    let mut i = 0;
    let bytes = tag.as_bytes();
    while i < bytes.len() {
        if let Some(pos) = tag[i..].find(key) {
            let idx = i + pos;
            let before_ok = idx == 0 || bytes[idx - 1].is_ascii_whitespace();
            let after_idx = idx + key.len();
            if before_ok && tag[after_idx..].trim_start().starts_with('=') {
                let eq_pos = tag[after_idx..].find('=').unwrap() + after_idx;
                let after_eq = tag[eq_pos + 1..].trim_start();
                let quote = after_eq.as_bytes().first().copied()?;
                if quote == b'"' || quote == b'\'' {
                    let rest = &after_eq[1..];
                    let end = rest.find(quote as char)?;
                    return Some(&rest[..end]);
                }
            }
            i = after_idx;
        } else {
            break;
        }
    }
    None
}

fn attr_f32(tag: &str, key: &str) -> Option<f32> {
    attr(tag, key)?.trim().parse::<f32>().ok()
}

fn parse_numbers(s: &str) -> Vec<f32> {
    let mut nums = Vec::new();
    let mut i = 0;
    let b = s.as_bytes();
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let start = i;
        if b[i] == b'-' || b[i] == b'+' {
            i += 1;
        }
        let mut seen_dot = false;
        while i < b.len() {
            if b[i].is_ascii_digit() {
                i += 1;
            } else if b[i] == b'.' && !seen_dot {
                seen_dot = true;
                i += 1;
            } else {
                break;
            }
        }
        if i > start {
            if let Ok(v) = s[start..i].parse::<f32>() {
                nums.push(v);
            }
        } else {
            i += 1;
        }
    }
    nums
}

fn parse_path_data(d: &str, out: &mut Vec<SvgPrimitive>) {
    let mut tokens = Vec::<(char, Vec<f32>)>::new();
    let mut cur_cmd: Option<char> = None;
    let mut arg_start = 0usize;
    for (idx, ch) in d.char_indices() {
        if ch.is_ascii_alphabetic() {
            if let Some(cmd) = cur_cmd {
                tokens.push((cmd, parse_numbers(&d[arg_start..idx])));
            }
            cur_cmd = Some(ch);
            arg_start = idx + ch.len_utf8();
        }
    }
    if let Some(cmd) = cur_cmd {
        tokens.push((cmd, parse_numbers(&d[arg_start..])));
    }

    let mut cur = Vec2::ZERO;
    let mut subpath_start = Vec2::ZERO;
    let mut last_cubic_ctrl: Option<Vec2> = None;
    let mut last_quad_ctrl: Option<Vec2> = None;

    for (cmd, args) in tokens {
        let rel = cmd.is_ascii_lowercase();
        let upper = cmd.to_ascii_uppercase();
        let mut i = 0;
        match upper {
            'M' => {
                let mut first = true;
                while i + 1 < args.len() {
                    let pt = Vec2::new(args[i], args[i + 1]) + if rel { cur } else { Vec2::ZERO };
                    if first {
                        cur = pt;
                        subpath_start = pt;
                        first = false;
                    } else {
                        out.push(SvgPrimitive::Segment(cur, pt));
                        cur = pt;
                    }
                    i += 2;
                }
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'L' => {
                while i + 1 < args.len() {
                    let pt = Vec2::new(args[i], args[i + 1]) + if rel { cur } else { Vec2::ZERO };
                    out.push(SvgPrimitive::Segment(cur, pt));
                    cur = pt;
                    i += 2;
                }
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'H' => {
                while i < args.len() {
                    let x = args[i] + if rel { cur.x } else { 0.0 };
                    let pt = Vec2::new(x, cur.y);
                    out.push(SvgPrimitive::Segment(cur, pt));
                    cur = pt;
                    i += 1;
                }
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'V' => {
                while i < args.len() {
                    let y = args[i] + if rel { cur.y } else { 0.0 };
                    let pt = Vec2::new(cur.x, y);
                    out.push(SvgPrimitive::Segment(cur, pt));
                    cur = pt;
                    i += 1;
                }
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'C' => {
                while i + 5 < args.len() {
                    let base = if rel { cur } else { Vec2::ZERO };
                    let c1 = Vec2::new(args[i], args[i + 1]) + base;
                    let c2 = Vec2::new(args[i + 2], args[i + 3]) + base;
                    let end = Vec2::new(args[i + 4], args[i + 5]) + base;
                    subdivide_cubic(cur, c1, c2, end, out);
                    last_cubic_ctrl = Some(c2);
                    cur = end;
                    i += 6;
                }
                last_quad_ctrl = None;
            }
            'S' => {
                while i + 3 < args.len() {
                    let base = if rel { cur } else { Vec2::ZERO };
                    let c1 = last_cubic_ctrl.map_or(cur, |c| cur * 2.0 - c);
                    let c2 = Vec2::new(args[i], args[i + 1]) + base;
                    let end = Vec2::new(args[i + 2], args[i + 3]) + base;
                    subdivide_cubic(cur, c1, c2, end, out);
                    last_cubic_ctrl = Some(c2);
                    cur = end;
                    i += 4;
                }
                last_quad_ctrl = None;
            }
            'Q' => {
                while i + 3 < args.len() {
                    let base = if rel { cur } else { Vec2::ZERO };
                    let c = Vec2::new(args[i], args[i + 1]) + base;
                    let end = Vec2::new(args[i + 2], args[i + 3]) + base;
                    subdivide_quad(cur, c, end, out);
                    last_quad_ctrl = Some(c);
                    cur = end;
                    i += 4;
                }
                last_cubic_ctrl = None;
            }
            'T' => {
                while i + 1 < args.len() {
                    let base = if rel { cur } else { Vec2::ZERO };
                    let c = last_quad_ctrl.map_or(cur, |q| cur * 2.0 - q);
                    let end = Vec2::new(args[i], args[i + 1]) + base;
                    subdivide_quad(cur, c, end, out);
                    last_quad_ctrl = Some(c);
                    cur = end;
                    i += 2;
                }
                last_cubic_ctrl = None;
            }
            'A' => {
                while i + 6 < args.len() {
                    let rx = args[i].abs();
                    let ry = args[i + 1].abs();
                    let x_rot = args[i + 2].to_radians();
                    let large_arc = args[i + 3] != 0.0;
                    let sweep = args[i + 4] != 0.0;
                    let base = if rel { cur } else { Vec2::ZERO };
                    let end = Vec2::new(args[i + 5], args[i + 6]) + base;
                    subdivide_arc(cur, end, rx, ry, x_rot, large_arc, sweep, out);
                    cur = end;
                    i += 7;
                }
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            'Z' => {
                if cur.distance(subpath_start) > 1e-4 {
                    out.push(SvgPrimitive::Segment(cur, subpath_start));
                }
                cur = subpath_start;
                last_cubic_ctrl = None;
                last_quad_ctrl = None;
            }
            _ => {}
        }
    }
}

fn subdivide_cubic(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, out: &mut Vec<SvgPrimitive>) {
    const STEPS: usize = 16;
    let mut prev = p0;
    for s in 1..=STEPS {
        let t = s as f32 / STEPS as f32;
        let u = 1.0 - t;
        let pt =
            p0 * (u * u * u) + p1 * (3.0 * u * u * t) + p2 * (3.0 * u * t * t) + p3 * (t * t * t);
        out.push(SvgPrimitive::Segment(prev, pt));
        prev = pt;
    }
}

fn subdivide_quad(p0: Vec2, p1: Vec2, p2: Vec2, out: &mut Vec<SvgPrimitive>) {
    const STEPS: usize = 12;
    let mut prev = p0;
    for s in 1..=STEPS {
        let t = s as f32 / STEPS as f32;
        let u = 1.0 - t;
        let pt = p0 * (u * u) + p1 * (2.0 * u * t) + p2 * (t * t);
        out.push(SvgPrimitive::Segment(prev, pt));
        prev = pt;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "SVG elliptical arc parametrization requires all 7 arc parameters"
)]
fn subdivide_arc(
    p0: Vec2,
    p1: Vec2,
    mut rx: f32,
    mut ry: f32,
    phi: f32,
    large_arc: bool,
    sweep: bool,
    out: &mut Vec<SvgPrimitive>,
) {
    if p0.distance(p1) < 1e-4 {
        return;
    }
    if rx < 1e-4 || ry < 1e-4 {
        out.push(SvgPrimitive::Segment(p0, p1));
        return;
    }
    let cos_p = phi.cos();
    let sin_p = phi.sin();
    let mid = (p0 - p1) * 0.5;
    let x1p = cos_p * mid.x + sin_p * mid.y;
    let y1p = -sin_p * mid.x + cos_p * mid.y;

    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }

    let rx2 = rx * rx;
    let ry2 = ry * ry;
    let num = (rx2 * ry2 - rx2 * y1p * y1p - ry2 * x1p * x1p).max(0.0);
    let den = (rx2 * y1p * y1p + ry2 * x1p * x1p).max(1e-8);
    let factor = (num / den).sqrt() * if large_arc == sweep { -1.0 } else { 1.0 };
    let cxp = factor * (rx * y1p / ry);
    let cyp = factor * (-ry * x1p / rx);

    let center = Vec2::new(
        cos_p * cxp - sin_p * cyp + (p0.x + p1.x) * 0.5,
        sin_p * cxp + cos_p * cyp + (p0.y + p1.y) * 0.5,
    );

    let vec_angle = |u: Vec2, v: Vec2| -> f32 {
        let dot = u.dot(v);
        let det = u.x * v.y - u.y * v.x;
        det.atan2(dot)
    };
    let u = Vec2::new((x1p - cxp) / rx, (y1p - cyp) / ry);
    let v = Vec2::new((-x1p - cxp) / rx, (-y1p - cyp) / ry);
    let theta1 = vec_angle(Vec2::X, u);
    let mut dtheta = vec_angle(u, v);
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f32::consts::TAU;
    } else if sweep && dtheta < 0.0 {
        dtheta += std::f32::consts::TAU;
    }

    const STEPS: usize = 20;
    let mut prev = p0;
    for s in 1..=STEPS {
        let ang = theta1 + dtheta * (s as f32 / STEPS as f32);
        let lx = rx * ang.cos();
        let ly = ry * ang.sin();
        let pt = Vec2::new(
            cos_p * lx - sin_p * ly + center.x,
            sin_p * lx + cos_p * ly + center.y,
        );
        out.push(SvgPrimitive::Segment(prev, pt));
        prev = pt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rasterizes_lucide_x_and_check_with_antialiased_alpha() {
        let x_svg =
            r#"<svg viewBox="0 0 24 24"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>"#;
        let px = rasterize_lucide_svg(x_svg, 32);
        assert_eq!(px.len(), 32 * 32 * 4);
        let lit = px.as_chunks::<4>().0.iter().filter(|p| p[3] > 200).count();
        assert!(lit > 40, "expected lit pixels for X icon, got {lit}");
        assert_eq!(px[3], 0, "corner pixel should remain transparent");

        let undo_svg = r#"<svg viewBox="0 0 24 24"><path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11"/></svg>"#;
        let undo_px = rasterize_lucide_svg(undo_svg, 32);
        let undo_lit = undo_px
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 200)
            .count();
        assert!(
            undo_lit > 40,
            "expected lit pixels for Undo icon, got {undo_lit}"
        );
    }
}
