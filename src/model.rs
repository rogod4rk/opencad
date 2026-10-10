//! Core CAD document model: entities, layers, undo/redo, DXF import/export, coordinate parsing.
//! Pure logic, no GUI — unit-tested headlessly. Ready to FFI to OCCT for DWG/3D later.

use std::collections::HashMap;

pub type Pt = (f64, f64);

#[derive(Clone, Debug, PartialEq)]
pub enum Entity {
    Line { x1: f64, y1: f64, x2: f64, y2: f64 },
    Circle { cx: f64, cy: f64, r: f64 },
    Arc { cx: f64, cy: f64, r: f64, a0: f64, a1: f64 },
    Rect { x: f64, y: f64, w: f64, h: f64 },
    Poly { pts: Vec<Pt>, closed: bool },
    Point { x: f64, y: f64 },
    Text { x: f64, y: f64, h: f64, s: String },
    Dim { x1: f64, y1: f64, x2: f64, y2: f64, vertical: bool, off: f64 },
}

/// The drawn geometry of a linear dimension: (dimension line, extension line 1, extension line 2).
/// `off` is the signed perpendicular distance from the measured points to the dimension line.
pub fn dim_geom(x1: f64, y1: f64, x2: f64, y2: f64, vertical: bool, off: f64) -> [(Pt, Pt); 3] {
    if vertical {
        let xl = x1.max(x2) + off;
        [((xl, y1), (xl, y2)), ((x1, y1), (xl, y1)), ((x2, y2), (xl, y2))]
    } else {
        let yl = y1.max(y2) + off;
        [((x1, yl), (x2, yl)), ((x1, y1), (x1, yl)), ((x2, y2), (x2, yl))]
    }
}

fn seg_dist(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let (dx, dy) = (x2 - x1, y2 - y1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((px - x1) * dx + (py - y1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (x, y) = (x1 + t * dx, y1 + t * dy);
    ((px - x).powi(2) + (py - y).powi(2)).sqrt()
}

impl Entity {
    pub fn translate(&mut self, dx: f64, dy: f64) {
        match self {
            Entity::Line { x1, y1, x2, y2 } => { *x1 += dx; *y1 += dy; *x2 += dx; *y2 += dy; }
            Entity::Circle { cx, cy, .. } | Entity::Arc { cx, cy, .. } => { *cx += dx; *cy += dy; }
            Entity::Rect { x, y, .. } => { *x += dx; *y += dy; }
            Entity::Poly { pts, .. } => { for p in pts.iter_mut() { p.0 += dx; p.1 += dy; } }
            Entity::Point { x, y } | Entity::Text { x, y, .. } => { *x += dx; *y += dy; }
            Entity::Dim { x1, y1, x2, y2, .. } => { *x1 += dx; *y1 += dy; *x2 += dx; *y2 += dy; }
        }
    }

    pub fn dist_to(&self, px: f64, py: f64) -> f64 {
        match self {
            Entity::Line { x1, y1, x2, y2 } => seg_dist(px, py, *x1, *y1, *x2, *y2),
            Entity::Dim { .. } => {
                // hit-test the drawn dimension line + extension lines, not the invisible baseline
                entity_segs(self).iter().map(|(a, b)| seg_dist(px, py, a.0, a.1, b.0, b.1))
                    .fold(f64::MAX, f64::min)
            }
            Entity::Circle { cx, cy, r } => ((px - cx).hypot(py - cy) - r).abs(),
            Entity::Arc { cx, cy, r, a0, a1 } => {
                let ang = (py - cy).atan2(px - cx);
                if angle_in(ang, *a0, *a1) {
                    ((px - cx).hypot(py - cy) - r).abs()
                } else {
                    let e0 = (cx + r * a0.cos(), cy + r * a0.sin());
                    let e1 = (cx + r * a1.cos(), cy + r * a1.sin());
                    (px - e0.0).hypot(py - e0.1).min((px - e1.0).hypot(py - e1.1))
                }
            }
            Entity::Rect { x, y, w, h } => {
                let e = [(*x, *y, *x + *w, *y), (*x + *w, *y, *x + *w, *y + *h),
                         (*x + *w, *y + *h, *x, *y + *h), (*x, *y + *h, *x, *y)];
                e.iter().map(|s| seg_dist(px, py, s.0, s.1, s.2, s.3)).fold(f64::MAX, f64::min)
            }
            Entity::Poly { pts, closed } => {
                let n = pts.len();
                if n < 2 { return f64::MAX; }
                let lim = if *closed { n } else { n - 1 };
                (0..lim).map(|i| { let a = pts[i]; let b = pts[(i + 1) % n]; seg_dist(px, py, a.0, a.1, b.0, b.1) })
                    .fold(f64::MAX, f64::min)
            }
            Entity::Point { x, y } | Entity::Text { x, y, .. } => (px - x).hypot(py - y),
        }
    }

    /// Object-snap candidate points (endpoints, midpoints, centers, quadrants).
    pub fn snaps(&self) -> Vec<Pt> {
        match self {
            Entity::Line { x1, y1, x2, y2 } =>
                vec![(*x1, *y1), (*x2, *y2), ((*x1 + *x2) / 2.0, (*y1 + *y2) / 2.0)],
            Entity::Rect { x, y, w, h } =>
                vec![(*x, *y), (*x + *w, *y), (*x + *w, *y + *h), (*x, *y + *h)],
            Entity::Circle { cx, cy, r } =>
                vec![(*cx, *cy), (*cx + *r, *cy), (*cx - *r, *cy), (*cx, *cy + *r), (*cx, *cy - *r)],
            Entity::Arc { cx, cy, r, a0, a1 } =>
                vec![(*cx, *cy), (*cx + *r * a0.cos(), *cy + *r * a0.sin()), (*cx + *r * a1.cos(), *cy + *r * a1.sin())],
            Entity::Poly { pts, .. } => pts.clone(),
            Entity::Point { x, y } | Entity::Text { x, y, .. } => vec![(*x, *y)],
            Entity::Dim { x1, y1, x2, y2, .. } => vec![(*x1, *y1), (*x2, *y2)],
        }
    }

    fn ensure_poly(&mut self) {
        let np = if let Entity::Rect { x, y, w, h } = self { Some((*x, *y, *w, *h)) } else { None };
        if let Some((x, y, w, h)) = np {
            *self = Entity::Poly { pts: vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)], closed: true };
        }
    }
    fn map_pts(&mut self, f: &dyn Fn(f64, f64) -> (f64, f64)) {
        match self {
            Entity::Line { x1, y1, x2, y2 } | Entity::Dim { x1, y1, x2, y2, .. } => {
                let a = f(*x1, *y1); *x1 = a.0; *y1 = a.1;
                let b = f(*x2, *y2); *x2 = b.0; *y2 = b.1;
            }
            Entity::Poly { pts, .. } => { for p in pts.iter_mut() { let a = f(p.0, p.1); p.0 = a.0; p.1 = a.1; } }
            Entity::Circle { cx, cy, .. } | Entity::Arc { cx, cy, .. } => { let a = f(*cx, *cy); *cx = a.0; *cy = a.1; }
            Entity::Rect { x, y, .. } => { let a = f(*x, *y); *x = a.0; *y = a.1; }
            Entity::Point { x, y } | Entity::Text { x, y, .. } => { let a = f(*x, *y); *x = a.0; *y = a.1; }
        }
    }
    pub fn rotate(&mut self, cx: f64, cy: f64, ang: f64) {
        self.ensure_poly();
        let (co, si) = (ang.cos(), ang.sin());
        self.map_pts(&move |x, y| { let (dx, dy) = (x - cx, y - cy); (cx + dx * co - dy * si, cy + dx * si + dy * co) });
        if let Entity::Arc { a0, a1, .. } = self { *a0 += ang; *a1 += ang; }
        if let Entity::Dim { vertical, off, .. } = self {
            // rotate the offset axis, flip `vertical` on a quarter-turn, then fix `off`'s sign
            // against the new axis. Dims are meant for axis-aligned rotations; this stays sane for those.
            let q = (ang / std::f64::consts::FRAC_PI_2).round() as i64;
            let u = if *vertical { (1.0, 0.0) } else { (0.0, 1.0) };
            let up = (u.0 * co - u.1 * si, u.0 * si + u.1 * co);
            if q % 2 != 0 { *vertical = !*vertical; }
            let un = if *vertical { (1.0, 0.0) } else { (0.0, 1.0) };
            if up.0 * un.0 + up.1 * un.1 < 0.0 { *off = -*off; }
        }
    }
    pub fn scale(&mut self, cx: f64, cy: f64, k: f64) {
        self.ensure_poly();
        self.map_pts(&move |x, y| (cx + (x - cx) * k, cy + (y - cy) * k));
        match self { Entity::Circle { r, .. } | Entity::Arc { r, .. } => *r *= k, Entity::Text { h, .. } => *h *= k, _ => {} }
    }
    pub fn mirror(&mut self, ax: f64, ay: f64, bx: f64, by: f64) {
        self.ensure_poly();
        let (dx, dy) = (bx - ax, by - ay);
        let den = (dx * dx + dy * dy).max(1e-9);
        self.map_pts(&move |x, y| { let t = ((x - ax) * dx + (y - ay) * dy) / den; let (px, py) = (ax + t * dx, ay + t * dy); (2.0 * px - x, 2.0 * py - y) });
        if let Entity::Arc { a0, a1, .. } = self { let aa = dy.atan2(dx); let n0 = 2.0 * aa - *a1; let n1 = 2.0 * aa - *a0; *a0 = n0; *a1 = n1; }
        if let Entity::Dim { vertical, off, .. } = self {
            // the offset runs along +y (horizontal dim) or +x (vertical dim); flip its sign only
            // when the mirror reflects that axis onto its negative.
            let l2 = (dx * dx + dy * dy).max(1e-9);
            let frac = if *vertical { dx * dx / l2 } else { dy * dy / l2 };
            if frac < 0.5 { *off = -*off; }
        }
    }
}

/// Circle through 3 points -> an Arc from the first point to the third (through the second).
pub fn arc3(a: Pt, b: Pt, c: Pt) -> Option<Entity> {
    let d = 2.0 * (a.0 * (b.1 - c.1) + b.0 * (c.1 - a.1) + c.0 * (a.1 - b.1));
    if d.abs() < 1e-6 { return None; }
    let (sa, sb, sc) = (a.0 * a.0 + a.1 * a.1, b.0 * b.0 + b.1 * b.1, c.0 * c.0 + c.1 * c.1);
    let ux = (sa * (b.1 - c.1) + sb * (c.1 - a.1) + sc * (a.1 - b.1)) / d;
    let uy = (sa * (c.0 - b.0) + sb * (a.0 - c.0) + sc * (b.0 - a.0)) / d;
    let r = (a.0 - ux).hypot(a.1 - uy);
    let a0 = (a.1 - uy).atan2(a.0 - ux);
    let a1 = (c.1 - uy).atan2(c.0 - ux);
    let ab = (b.1 - uy).atan2(b.0 - ux);
    // Keep a1 > a0 (CCW), choosing the sweep that actually passes through the middle point b.
    let tau = std::f64::consts::TAU;
    let s_a1 = (a1 - a0).rem_euclid(tau);
    let s_b = (ab - a0).rem_euclid(tau);
    let (a0, a1) = if s_b <= s_a1 { (a0, a0 + s_a1) } else { (a1, a1 + (tau - s_a1)) };
    Some(Entity::Arc { cx: ux, cy: uy, r, a0, a1 })
}

/// Offset a line/circle/arc/rect by `dist` toward the half-space containing `side`.
pub fn offset_entity(e: &Entity, dist: f64, side: Pt) -> Option<Entity> {
    match e {
        Entity::Line { x1, y1, x2, y2 } => {
            let (x1, y1, x2, y2) = (*x1, *y1, *x2, *y2);
            let (dx, dy) = (x2 - x1, y2 - y1);
            let l = dx.hypot(dy).max(1e-9);
            let (nx, ny) = (-dy / l, dx / l);
            let s = if (side.0 - x1) * nx + (side.1 - y1) * ny >= 0.0 { 1.0 } else { -1.0 };
            let (ox, oy) = (nx * dist * s, ny * dist * s);
            Some(Entity::Line { x1: x1 + ox, y1: y1 + oy, x2: x2 + ox, y2: y2 + oy })
        }
        Entity::Circle { cx, cy, r } => {
            let (cx, cy, r) = (*cx, *cy, *r);
            let out = (side.0 - cx).hypot(side.1 - cy) > r;
            Some(Entity::Circle { cx, cy, r: (r + if out { dist } else { -dist }).max(1.0) })
        }
        Entity::Arc { cx, cy, r, a0, a1 } => {
            let (cx, cy, r, a0, a1) = (*cx, *cy, *r, *a0, *a1);
            let out = (side.0 - cx).hypot(side.1 - cy) > r;
            Some(Entity::Arc { cx, cy, r: (r + if out { dist } else { -dist }).max(1.0), a0, a1 })
        }
        Entity::Rect { x, y, w, h } => {
            let (x, y, w, h) = (*x, *y, *w, *h);
            let (cx, cy) = (x + w / 2.0, y + h / 2.0);
            let out = (side.0 - cx).abs() > w / 2.0 || (side.1 - cy).abs() > h / 2.0;
            let dd = if out { dist } else { -dist };
            Some(Entity::Rect { x: x - dd, y: y - dd, w: (w + 2.0 * dd).max(1.0), h: (h + 2.0 * dd).max(1.0) })
        }
        _ => None,
    }
}

#[derive(Clone)]
pub struct Layer {
    pub name: String,
    pub color: Option<[u8; 3]>,
    pub on: bool,
    pub frozen: bool,
    pub locked: bool,
}
impl Layer {
    pub fn new(name: &str, color: Option<[u8; 3]>) -> Self {
        Self { name: name.into(), color, on: true, frozen: false, locked: false }
    }
}

#[derive(Clone)]
pub struct Obj {
    pub e: Entity,
    pub layer: String,
}

pub struct Document {
    pub objs: Vec<Obj>,
    pub layers: Vec<Layer>,
    pub current: String,
    hist: Vec<Vec<Obj>>,
    idx: usize,
}

fn fmt(n: f64) -> String { format!("{:.3}", n) }
fn pr(o: &mut String, c: i32, v: &str) { o.push_str(&c.to_string()); o.push('\n'); o.push_str(v); o.push('\n'); }
fn ln(o: &mut String, x1: f64, y1: f64, x2: f64, y2: f64, ly: &str) {
    pr(o, 0, "LINE"); pr(o, 8, ly);
    pr(o, 10, &fmt(x1)); pr(o, 20, &fmt(y1)); pr(o, 30, "0.0");
    pr(o, 11, &fmt(x2)); pr(o, 21, &fmt(y2)); pr(o, 31, "0.0");
}
fn lwpoly(o: &mut String, pts: &[Pt], closed: bool, ly: &str) {
    pr(o, 0, "LWPOLYLINE"); pr(o, 8, ly);
    pr(o, 90, &pts.len().to_string());
    pr(o, 70, if closed { "1" } else { "0" });
    for p in pts { pr(o, 10, &fmt(p.0)); pr(o, 20, &fmt(p.1)); }
}

impl Document {
    pub fn new() -> Self {
        let mut d = Self {
            objs: vec![],
            layers: vec![Layer::new("0", None), Layer::new("dimensions", Some([255, 180, 84]))],
            current: "0".into(),
            hist: vec![],
            idx: 0,
        };
        d.seed();
        d.hist = vec![d.objs.clone()];
        d.idx = 0;
        d
    }

    pub fn add(&mut self, e: Entity) { let layer = self.current.clone(); self.objs.push(Obj { e, layer }); }
    pub fn add_on(&mut self, e: Entity, layer: &str) { self.objs.push(Obj { e, layer: layer.into() }); }

    pub fn commit(&mut self) {
        self.hist.truncate(self.idx + 1);
        self.hist.push(self.objs.clone());
        if self.hist.len() > 100 { self.hist.remove(0); }
        self.idx = self.hist.len() - 1;
    }
    pub fn undo(&mut self) { if self.idx > 0 { self.idx -= 1; self.objs = self.hist[self.idx].clone(); } }
    pub fn redo(&mut self) { if self.idx + 1 < self.hist.len() { self.idx += 1; self.objs = self.hist[self.idx].clone(); } }

    pub fn layer(&self, name: &str) -> &Layer {
        self.layers.iter().find(|l| l.name == name).unwrap_or(&self.layers[0])
    }

    fn seed(&mut self) {
        let (w, h) = (4000.0, 3000.0);
        let (x, y) = (-w / 2.0, -h / 2.0);
        self.add(Entity::Rect { x, y, w, h });
        self.add(Entity::Line { x1: x + 800.0, y1: y, x2: x + 800.0, y2: y + 900.0 });
        self.add(Entity::Arc { cx: x + 800.0, cy: y, r: 900.0, a0: std::f64::consts::FRAC_PI_2, a1: std::f64::consts::PI });
        self.add(Entity::Line { x1: x + w, y1: y + 900.0, x2: x + w, y2: y + 2100.0 });
        self.add_on(Entity::Dim { x1: x, y1: y, x2: x + w, y2: y, vertical: false, off: 450.0 }, "dimensions");
    }

    pub fn to_dxf(&self) -> String {
        let mut o = String::new();
        pr(&mut o, 0, "SECTION"); pr(&mut o, 2, "TABLES");
        pr(&mut o, 0, "TABLE"); pr(&mut o, 2, "LAYER"); pr(&mut o, 70, &self.layers.len().to_string());
        for l in &self.layers {
            pr(&mut o, 0, "LAYER"); pr(&mut o, 2, &l.name);
            pr(&mut o, 70, if l.frozen { "1" } else { "0" });
            pr(&mut o, 62, &l.color.map_or(7, rgb_to_aci).to_string());
            pr(&mut o, 6, "CONTINUOUS");
        }
        pr(&mut o, 0, "ENDTAB"); pr(&mut o, 0, "ENDSEC");
        pr(&mut o, 0, "SECTION"); pr(&mut o, 2, "ENTITIES");
        for ob in &self.objs {
            let ly = ob.layer.as_str();
            match &ob.e {
                Entity::Line { x1, y1, x2, y2 } => ln(&mut o, *x1, *y1, *x2, *y2, ly),
                Entity::Dim { x1, y1, x2, y2, vertical, off } => {
                    for (a, b) in dim_geom(*x1, *y1, *x2, *y2, *vertical, *off) {
                        ln(&mut o, a.0, a.1, b.0, b.1, ly);
                    }
                }
                Entity::Rect { x, y, w, h } =>
                    lwpoly(&mut o, &[(*x, *y), (*x + *w, *y), (*x + *w, *y + *h), (*x, *y + *h)], true, ly),
                Entity::Poly { pts, closed } => lwpoly(&mut o, pts, *closed, ly),
                Entity::Circle { cx, cy, r } => {
                    pr(&mut o, 0, "CIRCLE"); pr(&mut o, 8, ly);
                    pr(&mut o, 10, &fmt(*cx)); pr(&mut o, 20, &fmt(*cy)); pr(&mut o, 30, "0.0"); pr(&mut o, 40, &fmt(*r));
                }
                Entity::Arc { cx, cy, r, a0, a1 } => {
                    pr(&mut o, 0, "ARC"); pr(&mut o, 8, ly);
                    pr(&mut o, 10, &fmt(*cx)); pr(&mut o, 20, &fmt(*cy)); pr(&mut o, 30, "0.0"); pr(&mut o, 40, &fmt(*r));
                    pr(&mut o, 50, &fmt(a0.to_degrees())); pr(&mut o, 51, &fmt(a1.to_degrees()));
                }
                Entity::Point { x, y } => {
                    pr(&mut o, 0, "POINT"); pr(&mut o, 8, ly);
                    pr(&mut o, 10, &fmt(*x)); pr(&mut o, 20, &fmt(*y)); pr(&mut o, 30, "0.0");
                }
                Entity::Text { x, y, h, s } => {
                    pr(&mut o, 0, "TEXT"); pr(&mut o, 8, ly);
                    pr(&mut o, 10, &fmt(*x)); pr(&mut o, 20, &fmt(*y)); pr(&mut o, 30, "0.0");
                    pr(&mut o, 40, &fmt(*h)); pr(&mut o, 1, s);
                }
            }
        }
        pr(&mut o, 0, "ENDSEC"); pr(&mut o, 0, "EOF");
        o
    }

    /// Parse a DXF (R12 ASCII and the common R2000 subset) into a Document.
    /// Reads LINE, CIRCLE, ARC, POINT, TEXT/MTEXT, LWPOLYLINE, legacy POLYLINE/VERTEX,
    /// ELLIPSE and SPLINE (polyline-approximated) in ENTITIES, plus the LAYER table.
    pub fn from_dxf(text: &str) -> Document {
        let toks = dxf_pairs(text);
        let mut objs: Vec<Obj> = vec![];
        let mut layers: Vec<Layer> = vec![];
        let mut section = String::new();
        let mut open_poly: Option<(Vec<Pt>, String, bool)> = None; // legacy POLYLINE/VERTEX accumulator
        let mut i = 0;
        while i < toks.len() {
            let (code, val) = (toks[i].0, toks[i].1.as_str());
            if code == 0 && val == "SECTION" && i + 1 < toks.len() && toks[i + 1].0 == 2 {
                section = toks[i + 1].1.clone();
                i += 2;
                continue;
            }
            if code == 0 && val == "ENDSEC" { section.clear(); i += 1; continue; }
            if code == 0 {
                let kind = val.to_string();
                let mut f: HashMap<i32, String> = HashMap::new();
                // Accumulate every (10,20) pair into p10 and every (11,21) pair into p11; scalars still land in f.
                // Handles multi-vertex entities (LWPOLYLINE, SPLINE control/fit points) and ELLIPSE's major-axis vector.
                let mut p10: Vec<Pt> = vec![];
                let mut p11: Vec<Pt> = vec![];
                let (mut px, mut qx): (Option<f64>, Option<f64>) = (None, None);
                let mut text = String::new();
                let mut j = i + 1;
                while j < toks.len() && toks[j].0 != 0 {
                    let (c, v) = (toks[j].0, toks[j].1.as_str());
                    match c {
                        10 => px = v.parse().ok(),
                        20 => if let (Some(x), Ok(y)) = (px.take(), v.parse::<f64>()) { if let Some(p) = finite_pt((x, y)) { p10.push(p); } },
                        11 => qx = v.parse().ok(),
                        21 => if let (Some(x), Ok(y)) = (qx.take(), v.parse::<f64>()) { if let Some(p) = finite_pt((x, y)) { p11.push(p); } },
                        1 | 3 => text.push_str(v), // TEXT/MTEXT body (3 = MTEXT continuation)
                        _ => {}
                    }
                    f.entry(c).or_insert_with(|| v.to_string());
                    j += 1;
                }
                let g = |k: i32| -> f64 {
                    f.get(&k).and_then(|s| s.parse::<f64>().ok())
                        .filter(|v| v.is_finite() && v.abs() < 1e12)
                        .unwrap_or(0.0)
                };
                if section == "ENTITIES" {
                    let layer = f.get(&8).cloned().unwrap_or_else(|| "0".into());
                    match kind.as_str() {
                        "LINE" => objs.push(Obj { e: Entity::Line { x1: g(10), y1: g(20), x2: g(11), y2: g(21) }, layer }),
                        "CIRCLE" => objs.push(Obj { e: Entity::Circle { cx: g(10), cy: g(20), r: g(40) }, layer }),
                        "ARC" => {
                            let a0 = g(50).to_radians();
                            let a1 = a0 + (g(51).to_radians() - a0).rem_euclid(std::f64::consts::TAU);
                            objs.push(Obj { e: Entity::Arc { cx: g(10), cy: g(20), r: g(40), a0, a1 }, layer });
                        }
                        "POINT" => objs.push(Obj { e: Entity::Point { x: g(10), y: g(20) }, layer }),
                        "TEXT" => objs.push(Obj { e: Entity::Text { x: g(10), y: g(20), h: if g(40) > 0.0 { g(40) } else { 250.0 }, s: text }, layer }),
                        "MTEXT" => objs.push(Obj { e: Entity::Text { x: g(10), y: g(20), h: if g(40) > 0.0 { g(40) } else { 250.0 }, s: mtext_plain(&text) }, layer }),
                        "LWPOLYLINE" => {
                            let closed = f.get(&70).and_then(|s| s.parse::<i32>().ok()).map_or(false, |fl| fl & 1 == 1);
                            if p10.len() >= 2 { objs.push(Obj { e: Entity::Poly { pts: p10, closed }, layer }); }
                        }
                        "ELLIPSE" => {
                            // 10/20 = center, 11/21 = major-axis endpoint RELATIVE to center, 40 = minor/major ratio.
                            if let (Some(ctr), Some(maj)) = (p10.first().copied(), p11.first().copied()) {
                                let major_end = (ctr.0 + maj.0, ctr.1 + maj.1);
                                let minor = (maj.0.hypot(maj.1) * g(40)).max(1.0);
                                objs.push(Obj { e: ellipse(ctr, major_end, minor), layer });
                            }
                        }
                        "SPLINE" => {
                            // approximate by a polyline through its fit points (11/21), else its control points (10/20).
                            let pts = if p11.len() >= 2 { p11 } else { p10 };
                            let closed = f.get(&70).and_then(|s| s.parse::<i32>().ok()).map_or(false, |fl| fl & 1 == 1);
                            if pts.len() >= 2 { objs.push(Obj { e: Entity::Poly { pts, closed }, layer }); }
                        }
                        "POLYLINE" => {
                            let closed = f.get(&70).and_then(|s| s.parse::<i32>().ok()).map_or(false, |fl| fl & 1 == 1);
                            open_poly = Some((vec![], layer, closed));
                        }
                        "VERTEX" => {
                            if let Some((pts, _, _)) = open_poly.as_mut() { pts.push((g(10), g(20))); }
                        }
                        "SEQEND" => {
                            if let Some((pts, ly, closed)) = open_poly.take() {
                                if pts.len() >= 2 { objs.push(Obj { e: Entity::Poly { pts, closed }, layer: ly }); }
                            }
                        }
                        _ => {}
                    }
                } else if section == "TABLES" && kind == "LAYER" {
                    if let Some(name) = f.get(&2) {
                        let frozen = f.get(&70).and_then(|s| s.parse::<i32>().ok()).map_or(false, |fl| fl & 1 == 1);
                        let color = f.get(&62).and_then(|s| s.parse::<i32>().ok())
                            .and_then(|c| if c == 7 { None } else { Some(aci_to_rgb(c)) });
                        if !layers.iter().any(|l| &l.name == name) {
                            layers.push(Layer { name: name.clone(), color, on: true, frozen, locked: false });
                        }
                    }
                }
                i = j;
                continue;
            }
            i += 1;
        }
        if !layers.iter().any(|l| l.name == "0") { layers.insert(0, Layer::new("0", None)); }
        let mut d = Document { objs, layers, current: "0".into(), hist: vec![], idx: 0 };
        d.hist = vec![d.objs.clone()];
        d.idx = 0;
        d
    }
}

fn dxf_pairs(text: &str) -> Vec<(i32, String)> {
    let mut out = vec![];
    let mut lines = text.lines();
    while let (Some(c), Some(v)) = (lines.next(), lines.next()) {
        if let Ok(code) = c.trim().parse::<i32>() { out.push((code, v.trim().to_string())); }
    }
    out
}
fn aci_to_rgb(c: i32) -> [u8; 3] {
    match c {
        1 => [255, 80, 80], 2 => [255, 180, 84], 3 => [84, 217, 140],
        4 => [55, 212, 232], 5 => [120, 150, 255], 6 => [200, 120, 255],
        _ => [205, 217, 229],
    }
}
/// Nearest standard ACI index (1-6) for an RGB layer color; 7 (white/default) otherwise.
fn rgb_to_aci(rgb: [u8; 3]) -> i32 {
    let pal: [(i32, [u8; 3]); 6] = [(1, [255, 80, 80]), (2, [255, 180, 84]), (3, [84, 217, 140]),
        (4, [55, 212, 232]), (5, [120, 150, 255]), (6, [200, 120, 255])];
    let d2 = |a: [u8; 3], b: [u8; 3]| (0..3).map(|i| (a[i] as i32 - b[i] as i32).pow(2)).sum::<i32>();
    pal.iter().min_by_key(|(_, c)| d2(rgb, *c)).map(|(i, _)| *i).unwrap_or(7)
}

/// Strip the common MTEXT inline formatting so the stored string is readable plain text.
fn mtext_plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('P') | Some('p') => out.push(' '),          // paragraph break
                Some('{') => out.push('{'),
                Some('}') => out.push('}'),
                Some('\\') => out.push('\\'),
                // formatting code like \f.., \A1;, \H2x; — skip to the terminating ';'
                Some(_) => { while let Some(&n) = chars.peek() { chars.next(); if n == ';' { break; } } }
                None => {}
            },
            '{' | '}' => {} // grouping braces
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

// ---- intersection + modify geometry (TRIM / EXTEND / FILLET / CHAMFER) ----
fn dist(a: Pt, b: Pt) -> f64 { (a.0 - b.0).hypot(a.1 - b.1) }
fn unit(from: Pt, to: Pt) -> Pt { let (dx, dy) = (to.0 - from.0, to.1 - from.1); let l = dx.hypot(dy).max(1e-9); (dx / l, dy / l) }
fn line_param(x1: f64, y1: f64, x2: f64, y2: f64, p: Pt) -> f64 {
    let (dx, dy) = (x2 - x1, y2 - y1); let l2 = (dx * dx + dy * dy).max(1e-9);
    ((p.0 - x1) * dx + (p.1 - y1) * dy) / l2
}
pub fn seg_inter(a: Pt, b: Pt, c: Pt, d: Pt) -> Option<Pt> {
    let r = (b.0 - a.0, b.1 - a.1); let s = (d.0 - c.0, d.1 - c.1);
    let den = r.0 * s.1 - r.1 * s.0; if den.abs() < 1e-9 { return None; }
    let t = ((c.0 - a.0) * s.1 - (c.1 - a.1) * s.0) / den;
    let u = ((c.0 - a.0) * r.1 - (c.1 - a.1) * r.0) / den;
    if t < 0.0 || t > 1.0 || u < 0.0 || u > 1.0 { return None; }
    Some((a.0 + t * r.0, a.1 + t * r.1))
}
pub fn inter_lines_inf(a: Pt, b: Pt, c: Pt, d: Pt) -> Option<Pt> {
    let den = (a.0 - b.0) * (c.1 - d.1) - (a.1 - b.1) * (c.0 - d.0);
    if den.abs() < 1e-9 { return None; }
    let (pre, post) = (a.0 * b.1 - a.1 * b.0, c.0 * d.1 - c.1 * d.0);
    Some(((pre * (c.0 - d.0) - (a.0 - b.0) * post) / den, (pre * (c.1 - d.1) - (a.1 - b.1) * post) / den))
}
pub fn entity_segs(e: &Entity) -> Vec<(Pt, Pt)> {
    match e {
        Entity::Line { x1, y1, x2, y2 } => vec![((*x1, *y1), (*x2, *y2))],
        Entity::Dim { x1, y1, x2, y2, vertical, off } =>
            dim_geom(*x1, *y1, *x2, *y2, *vertical, *off).to_vec(),
        Entity::Rect { x, y, w, h } => {
            let (x, y, w, h) = (*x, *y, *w, *h);
            let p = [(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
            (0..4).map(|i| (p[i], p[(i + 1) % 4])).collect()
        }
        Entity::Poly { pts, closed } => {
            let n = pts.len(); if n < 2 { return vec![]; }
            let lim = if *closed { n } else { n - 1 };
            (0..lim).map(|i| (pts[i], pts[(i + 1) % n])).collect()
        }
        _ => vec![],
    }
}
fn line_circle_t(a: Pt, b: Pt, c: Pt, r: f64) -> Vec<f64> {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1); let (fx, fy) = (a.0 - c.0, a.1 - c.1);
    let aa = dx * dx + dy * dy; let bb = 2.0 * (fx * dx + fy * dy); let cc = fx * fx + fy * fy - r * r;
    let disc = bb * bb - 4.0 * aa * cc; if disc < 0.0 { return vec![]; }
    let sq = disc.sqrt();
    [(-bb - sq) / (2.0 * aa), (-bb + sq) / (2.0 * aa)].into_iter().filter(|t| *t >= 0.0 && *t <= 1.0).collect()
}
fn angle_in(a: f64, a0: f64, a1: f64) -> bool {
    let tau = std::f64::consts::TAU;
    (a - a0).rem_euclid(tau) <= (a1 - a0).rem_euclid(tau) + 1e-6
}
/// Params in (0,1) where segment a->b crosses the other entities.
pub fn line_intersections(a: Pt, b: Pt, others: &[&Entity]) -> Vec<f64> {
    let mut ts = vec![];
    for o in others {
        for (c, d) in entity_segs(o) {
            if let Some(p) = seg_inter(a, b, c, d) {
                let t = line_param(a.0, a.1, b.0, b.1, p);
                if t > 1e-4 && t < 1.0 - 1e-4 { ts.push(t); }
            }
        }
        if let Entity::Circle { cx, cy, r } = o {
            for t in line_circle_t(a, b, (*cx, *cy), *r) { if t > 1e-4 && t < 1.0 - 1e-4 { ts.push(t); } }
        }
        if let Entity::Arc { cx, cy, r, a0, a1 } = o {
            for t in line_circle_t(a, b, (*cx, *cy), *r) {
                let (x, y) = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
                if angle_in((y - cy).atan2(x - cx), *a0, *a1) && t > 1e-4 && t < 1.0 - 1e-4 { ts.push(t); }
            }
        }
    }
    ts.sort_by(|x, y| x.partial_cmp(y).unwrap());
    ts
}
/// Trim a line: remove the clicked segment up to the nearest crossings. Returns the remaining 0-2 lines.
pub fn trim_line(e: &Entity, others: &[&Entity], click: Pt) -> Option<Vec<Entity>> {
    if let Entity::Line { x1, y1, x2, y2 } = e {
        let (x1, y1, x2, y2) = (*x1, *y1, *x2, *y2);
        let mut all = vec![0.0];
        all.extend(line_intersections((x1, y1), (x2, y2), others));
        all.push(1.0);
        if all.len() == 2 { return None; } // no crossing edges -> nothing to trim
        let tc = line_param(x1, y1, x2, y2, click);
        let (mut lo, mut hi) = (0.0, 1.0);
        for i in 0..all.len() - 1 { if tc >= all[i] && tc <= all[i + 1] { lo = all[i]; hi = all[i + 1]; break; } }
        let at = |t: f64| -> Pt { (x1 + t * (x2 - x1), y1 + t * (y2 - y1)) };
        let mut out = vec![];
        if lo > 1e-4 { let s = at(0.0); let q = at(lo); out.push(Entity::Line { x1: s.0, y1: s.1, x2: q.0, y2: q.1 }); }
        if hi < 1.0 - 1e-4 { let s = at(hi); let q = at(1.0); out.push(Entity::Line { x1: s.0, y1: s.1, x2: q.0, y2: q.1 }); }
        Some(out)
    } else { None }
}
/// Extend a line's nearest end (to the click) to the first boundary it meets.
pub fn extend_line(e: &Entity, others: &[&Entity], click: Pt) -> Option<Entity> {
    if let Entity::Line { x1, y1, x2, y2 } = e {
        let (x1, y1, x2, y2) = (*x1, *y1, *x2, *y2);
        let end1 = dist(click, (x1, y1)) < dist(click, (x2, y2));
        let fixed = if end1 { (x2, y2) } else { (x1, y1) };
        let moving = if end1 { (x1, y1) } else { (x2, y2) };
        let (dx, dy) = (moving.0 - fixed.0, moving.1 - fixed.1);
        let l = dx.hypot(dy).max(1e-9); let (ux, uy) = (dx / l, dy / l);
        let far = (fixed.0 + ux * 1e7, fixed.1 + uy * 1e7);
        let (mut best, mut bp) = (f64::MAX, None);
        for o in others {
            for (c, d) in entity_segs(o) {
                if let Some(p) = seg_inter(fixed, far, c, d) {
                    let t = (p.0 - fixed.0) * ux + (p.1 - fixed.1) * uy;
                    if t > l + 1.0 && t < best { best = t; bp = Some(p); }
                }
            }
        }
        bp.map(|p| if end1 { Entity::Line { x1: p.0, y1: p.1, x2, y2 } } else { Entity::Line { x1, y1, x2: p.0, y2: p.1 } })
    } else { None }
}
/// Fillet two lines with radius r. Returns (trimmed a, trimmed b, optional arc).
pub fn fillet_lines(a: &Entity, b: &Entity, r: f64) -> Option<(Entity, Entity, Option<Entity>)> {
    if let (Entity::Line { x1: ax1, y1: ay1, x2: ax2, y2: ay2 }, Entity::Line { x1: bx1, y1: by1, x2: bx2, y2: by2 }) = (a, b) {
        let (a1, a2) = ((*ax1, *ay1), (*ax2, *ay2));
        let (b1, b2) = ((*bx1, *by1), (*bx2, *by2));
        let ip = inter_lines_inf(a1, a2, b1, b2)?;
        let ka = if dist(a1, ip) > dist(a2, ip) { a1 } else { a2 };
        let kb = if dist(b1, ip) > dist(b2, ip) { b1 } else { b2 };
        let u = unit(ip, ka); let v = unit(ip, kb);
        let phi = (u.0 * v.0 + u.1 * v.1).clamp(-1.0, 1.0).acos();
        if r <= 0.0 || phi < 1e-3 || (phi - std::f64::consts::PI).abs() < 1e-3 {
            return Some((Entity::Line { x1: ka.0, y1: ka.1, x2: ip.0, y2: ip.1 },
                         Entity::Line { x1: kb.0, y1: kb.1, x2: ip.0, y2: ip.1 }, None));
        }
        let td = r / (phi / 2.0).tan();
        if td > dist(ip, ka) + 1e-6 || td > dist(ip, kb) + 1e-6 { return None; } // radius too big for these segments
        let ta = (ip.0 + u.0 * td, ip.1 + u.1 * td);
        let tb = (ip.0 + v.0 * td, ip.1 + v.1 * td);
        let mut bis = (u.0 + v.0, u.1 + v.1);
        let lb = bis.0.hypot(bis.1).max(1e-9); bis = (bis.0 / lb, bis.1 / lb);
        let cc = (ip.0 + bis.0 * (r / (phi / 2.0).sin()), ip.1 + bis.1 * (r / (phi / 2.0).sin()));
        let mut a0 = (ta.1 - cc.1).atan2(ta.0 - cc.0);
        let mut a1 = (tb.1 - cc.1).atan2(tb.0 - cc.0);
        let mut dd = a1 - a0;
        while dd <= -std::f64::consts::PI { dd += std::f64::consts::TAU; }
        while dd > std::f64::consts::PI { dd -= std::f64::consts::TAU; }
        if dd < 0.0 { std::mem::swap(&mut a0, &mut a1); dd = -dd; }
        Some((Entity::Line { x1: ka.0, y1: ka.1, x2: ta.0, y2: ta.1 },
              Entity::Line { x1: kb.0, y1: kb.1, x2: tb.0, y2: tb.1 },
              Some(Entity::Arc { cx: cc.0, cy: cc.1, r, a0, a1: a0 + dd })))
    } else { None }
}
/// Chamfer two lines with distance d. Returns (trimmed a, trimmed b, connector line).
pub fn chamfer_lines(a: &Entity, b: &Entity, d: f64) -> Option<(Entity, Entity, Entity)> {
    if let (Entity::Line { x1: ax1, y1: ay1, x2: ax2, y2: ay2 }, Entity::Line { x1: bx1, y1: by1, x2: bx2, y2: by2 }) = (a, b) {
        let (a1, a2) = ((*ax1, *ay1), (*ax2, *ay2));
        let (b1, b2) = ((*bx1, *by1), (*bx2, *by2));
        let ip = inter_lines_inf(a1, a2, b1, b2)?;
        let ka = if dist(a1, ip) > dist(a2, ip) { a1 } else { a2 };
        let kb = if dist(b1, ip) > dist(b2, ip) { b1 } else { b2 };
        let u = unit(ip, ka); let v = unit(ip, kb);
        if d <= 0.0 || d > dist(ip, ka) + 1e-6 || d > dist(ip, kb) + 1e-6 { return None; } // distance too big / nonpositive
        let ta = (ip.0 + u.0 * d, ip.1 + u.1 * d);
        let tb = (ip.0 + v.0 * d, ip.1 + v.1 * d);
        Some((Entity::Line { x1: ka.0, y1: ka.1, x2: ta.0, y2: ta.1 },
              Entity::Line { x1: kb.0, y1: kb.1, x2: tb.0, y2: tb.1 },
              Entity::Line { x1: ta.0, y1: ta.1, x2: tb.0, y2: tb.1 }))
    } else { None }
}
/// Build an ellipse as a sampled closed polyline.
pub fn ellipse(center: Pt, major_end: Pt, minor: f64) -> Entity {
    let a = dist(center, major_end);
    let ang = (major_end.1 - center.1).atan2(major_end.0 - center.0);
    let (ca, sa) = (ang.cos(), ang.sin());
    let mut pts = Vec::with_capacity(64);
    for i in 0..64 {
        let t = std::f64::consts::TAU * i as f64 / 64.0;
        let (x, y) = (a * t.cos(), minor * t.sin());
        pts.push((center.0 + x * ca - y * sa, center.1 + x * sa + y * ca));
    }
    Entity::Poly { pts, closed: true }
}

/// EXPLODE: break a compound entity into its primitive line segments.
/// Rect/Poly/Dim explode to lines; primitives (Line/Circle/Arc/Point/Text) can't explode (None).
pub fn explode_entity(e: &Entity) -> Option<Vec<Entity>> {
    let segs = match e {
        Entity::Rect { .. } | Entity::Poly { .. } | Entity::Dim { .. } => entity_segs(e),
        _ => return None,
    };
    if segs.is_empty() { return None; }
    Some(segs.into_iter().map(|(a, b)| Entity::Line { x1: a.0, y1: a.1, x2: b.0, y2: b.1 }).collect())
}

/// BREAK: remove the portion of a line between two points, returning the 0-2 remaining pieces.
pub fn break_line(e: &Entity, p1: Pt, p2: Pt) -> Option<Vec<Entity>> {
    if let Entity::Line { x1, y1, x2, y2 } = e {
        let (x1, y1, x2, y2) = (*x1, *y1, *x2, *y2);
        let mut t1 = line_param(x1, y1, x2, y2, p1).clamp(0.0, 1.0);
        let mut t2 = line_param(x1, y1, x2, y2, p2).clamp(0.0, 1.0);
        if t1 > t2 { std::mem::swap(&mut t1, &mut t2); }
        let at = |t: f64| -> Pt { (x1 + t * (x2 - x1), y1 + t * (y2 - y1)) };
        let mut out = vec![];
        if t1 > 1e-4 { let a = at(0.0); let b = at(t1); out.push(Entity::Line { x1: a.0, y1: a.1, x2: b.0, y2: b.1 }); }
        if t2 < 1.0 - 1e-4 { let a = at(t2); let b = at(1.0); out.push(Entity::Line { x1: a.0, y1: a.1, x2: b.0, y2: b.1 }); }
        Some(out)
    } else { None }
}

/// JOIN: merge two collinear line segments into one spanning their extreme endpoints.
pub fn join_lines(a: &Entity, b: &Entity) -> Option<Entity> {
    if let (Entity::Line { x1: ax1, y1: ay1, x2: ax2, y2: ay2 },
            Entity::Line { x1: bx1, y1: by1, x2: bx2, y2: by2 }) = (a, b) {
        let a1 = (*ax1, *ay1); let a2 = (*ax2, *ay2);
        let b1 = (*bx1, *by1); let b2 = (*bx2, *by2);
        if dist(a1, a2) < 1e-9 { return None; }
        let d = unit(a1, a2);
        let perp = |p: Pt| (p.0 - a1.0) * d.1 - (p.1 - a1.1) * d.0; // signed distance off a's infinite line
        // tolerance tracks float precision at this coordinate scale, NOT the line's length
        // (a length-scaled tol wrongly merges a long, clearly-parallel neighbour).
        let maxc = [a1, a2, b1, b2].iter().map(|p| p.0.abs().max(p.1.abs())).fold(0.0, f64::max);
        let tol = 1e-6 * (1.0 + maxc);
        if perp(b1).abs() > tol || perp(b2).abs() > tol { return None; } // not collinear
        let proj = |p: Pt| (p.0 - a1.0) * d.0 + (p.1 - a1.1) * d.1;
        let pts = [a1, a2, b1, b2];
        let (mut lo, mut hi) = (a1, a1);
        let (mut plo, mut phi) = (proj(a1), proj(a1));
        for &p in &pts { let t = proj(p); if t < plo { plo = t; lo = p; } if t > phi { phi = t; hi = p; } }
        Some(Entity::Line { x1: lo.0, y1: lo.1, x2: hi.0, y2: hi.1 })
    } else { None }
}

/// Rectangular ARRAY: copy the selection into a rows x cols grid (the original stays at r0,c0).
pub fn array_rect(items: &[Entity], rows: usize, cols: usize, dx: f64, dy: f64) -> Vec<Entity> {
    let mut out = vec![];
    for r in 0..rows.max(1) {
        for c in 0..cols.max(1) {
            if r == 0 && c == 0 { continue; }
            for e in items {
                let mut e2 = e.clone();
                e2.translate(c as f64 * dx, r as f64 * dy);
                out.push(e2);
            }
        }
    }
    out
}

/// Polar ARRAY: `count` total items around `center` spanning `fill` degrees (the original is item 1).
/// A full 360 spreads evenly by fill/count; a partial fill spreads by fill/(count-1), matching AutoCAD.
pub fn array_polar(items: &[Entity], center: Pt, count: usize, fill: f64) -> Vec<Entity> {
    let mut out = vec![];
    if count < 2 { return out; }
    let step_deg = if (fill.abs() - 360.0).abs() < 1e-6 { fill / count as f64 } else { fill / (count - 1) as f64 };
    for k in 1..count {
        let ang = (step_deg * k as f64).to_radians();
        for e in items {
            let mut e2 = e.clone();
            e2.rotate(center.0, center.1, ang);
            out.push(e2);
        }
    }
    out
}

// ---- grips (direct manipulation handles) ----
#[derive(Clone, Copy, Debug)]
pub enum Grip { P1, P2, Mid, Center, Corner(usize), Vertex(usize), ArcA0, ArcA1, Quad, Ins, D1, D2 }

pub fn grips_of(e: &Entity) -> Vec<(Pt, Grip)> {
    match e {
        Entity::Line { x1, y1, x2, y2 } =>
            vec![((*x1, *y1), Grip::P1), ((*x2, *y2), Grip::P2), (((*x1 + *x2) / 2.0, (*y1 + *y2) / 2.0), Grip::Mid)],
        Entity::Rect { x, y, w, h } =>
            vec![((*x, *y), Grip::Corner(0)), ((*x + *w, *y), Grip::Corner(1)), ((*x + *w, *y + *h), Grip::Corner(2)), ((*x, *y + *h), Grip::Corner(3))],
        Entity::Circle { cx, cy, r } =>
            vec![((*cx, *cy), Grip::Center), ((*cx + *r, *cy), Grip::Quad), ((*cx - *r, *cy), Grip::Quad), ((*cx, *cy + *r), Grip::Quad), ((*cx, *cy - *r), Grip::Quad)],
        Entity::Arc { cx, cy, r, a0, a1 } =>
            vec![((*cx, *cy), Grip::Center), ((*cx + *r * a0.cos(), *cy + *r * a0.sin()), Grip::ArcA0), ((*cx + *r * a1.cos(), *cy + *r * a1.sin()), Grip::ArcA1)],
        Entity::Poly { pts, .. } => pts.iter().enumerate().map(|(i, p)| ((p.0, p.1), Grip::Vertex(i))).collect(),
        Entity::Point { x, y } | Entity::Text { x, y, .. } => vec![((*x, *y), Grip::Ins)],
        Entity::Dim { x1, y1, x2, y2, .. } => vec![((*x1, *y1), Grip::D1), ((*x2, *y2), Grip::D2)],
    }
}

pub fn apply_grip(e: &mut Entity, g: Grip, np: Pt) {
    match (e, g) {
        (Entity::Line { x1, y1, .. }, Grip::P1) => { *x1 = np.0; *y1 = np.1; }
        (Entity::Line { x2, y2, .. }, Grip::P2) => { *x2 = np.0; *y2 = np.1; }
        (Entity::Line { x1, y1, x2, y2 }, Grip::Mid) => {
            let (mx, my) = ((*x1 + *x2) / 2.0, (*y1 + *y2) / 2.0);
            let (dx, dy) = (np.0 - mx, np.1 - my);
            *x1 += dx; *y1 += dy; *x2 += dx; *y2 += dy;
        }
        (Entity::Rect { x, y, w, h }, Grip::Corner(i)) => {
            let cs = [(*x, *y), (*x + *w, *y), (*x + *w, *y + *h), (*x, *y + *h)];
            let opp = cs[(i + 2) % 4];
            *x = opp.0.min(np.0); *y = opp.1.min(np.1);
            *w = (opp.0 - np.0).abs(); *h = (opp.1 - np.1).abs();
        }
        (Entity::Circle { cx, cy, .. }, Grip::Center) => { *cx = np.0; *cy = np.1; }
        (Entity::Circle { cx, cy, r }, Grip::Quad) => { *r = (np.0 - *cx).hypot(np.1 - *cy).max(1.0); }
        (Entity::Arc { cx, cy, .. }, Grip::Center) => { *cx = np.0; *cy = np.1; }
        (Entity::Arc { cx, cy, a0, a1, .. }, Grip::ArcA0) => {
            *a0 = (np.1 - *cy).atan2(np.0 - *cx);
            *a1 = *a0 + (*a1 - *a0).rem_euclid(std::f64::consts::TAU);
        }
        (Entity::Arc { cx, cy, a0, a1, .. }, Grip::ArcA1) => {
            *a1 = (np.1 - *cy).atan2(np.0 - *cx);
            *a1 = *a0 + (*a1 - *a0).rem_euclid(std::f64::consts::TAU);
        }
        (Entity::Poly { pts, .. }, Grip::Vertex(i)) => { if i < pts.len() { pts[i] = np; } }
        (Entity::Point { x, y }, Grip::Ins) => { *x = np.0; *y = np.1; }
        (Entity::Text { x, y, .. }, Grip::Ins) => { *x = np.0; *y = np.1; }
        (Entity::Dim { x1, y1, .. }, Grip::D1) => { *x1 = np.0; *y1 = np.1; }
        (Entity::Dim { x2, y2, .. }, Grip::D2) => { *x2 = np.0; *y2 = np.1; }
        _ => {}
    }
}

fn closest_on_seg(p: Pt, a: Pt, b: Pt) -> Pt {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = (dx * dx + dy * dy).max(1e-9);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0);
    (a.0 + t * dx, a.1 + t * dy)
}
/// Closest point on an entity to `p` (for the Nearest object snap).
pub fn nearest_point(e: &Entity, p: Pt) -> Option<Pt> {
    match e {
        Entity::Line { .. } | Entity::Dim { .. } | Entity::Rect { .. } | Entity::Poly { .. } => {
            let mut best: Option<(f64, Pt)> = None;
            for (a, b) in entity_segs(e) {
                let np = closest_on_seg(p, a, b);
                let d = dist(p, np);
                if best.map_or(true, |x| d < x.0) { best = Some((d, np)); }
            }
            best.map(|x| x.1)
        }
        Entity::Circle { cx, cy, r } | Entity::Arc { cx, cy, r, .. } => {
            let (cx, cy, r) = (*cx, *cy, *r);
            let d = (p.0 - cx).hypot(p.1 - cy).max(1e-9);
            Some((cx + (p.0 - cx) / d * r, cy + (p.1 - cy) / d * r))
        }
        _ => None,
    }
}

/// Is an entity inside (window) or touching (crossing) the box [lo,hi]?
pub fn entity_in_box(e: &Entity, lo: Pt, hi: Pt, crossing: bool) -> bool {
    let inside = |p: Pt| p.0 >= lo.0 && p.0 <= hi.0 && p.1 >= lo.1 && p.1 <= hi.1;
    let pts = e.snaps();
    if crossing {
        if pts.iter().any(|p| inside(*p)) { return true; }
        let edges = [(lo, (hi.0, lo.1)), ((hi.0, lo.1), hi), (hi, (lo.0, hi.1)), ((lo.0, hi.1), lo)];
        for (a, b) in entity_segs(e) {
            for (c, d) in edges { if seg_inter(a, b, c, d).is_some() { return true; } }
        }
        match e {
            Entity::Circle { cx, cy, r } =>
                edges.iter().any(|(c, d)| !line_circle_t(*c, *d, (*cx, *cy), *r).is_empty()),
            Entity::Arc { cx, cy, r, a0, a1 } => edges.iter().any(|(c, d)|
                line_circle_t(*c, *d, (*cx, *cy), *r).into_iter().any(|t| {
                    let (x, y) = (c.0 + t * (d.0 - c.0), c.1 + t * (d.1 - c.1));
                    angle_in((y - cy).atan2(x - cx), *a0, *a1)
                })),
            _ => false,
        }
    } else {
        !pts.is_empty() && pts.iter().all(|p| inside(*p))
    }
}

/// Parse an AutoCAD-style coordinate: `x,y` absolute, `@dx,dy` relative, `@d<ang` polar.
pub fn parse_coord(t: &str, base: Option<Pt>) -> Option<Pt> {
    let mut t = t.trim();
    let rel = t.starts_with('@');
    if rel { t = &t[1..]; }
    if let Some(i) = t.find('<') {
        let d: f64 = t[..i].trim().parse().ok()?;
        let a: f64 = t[i + 1..].trim().parse().ok()?;
        if !d.is_finite() || !a.is_finite() { return None; }
        let (bx, by) = if rel { base? } else { (0.0, 0.0) };
        let ang = a.to_radians();
        return finite_pt((bx + d * ang.cos(), by + d * ang.sin()));
    }
    let parts: Option<Vec<f64>> = t.split(|c| c == ',' || c == ' ')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<f64>().ok())
        .collect();
    let parts = parts?;
    if parts.len() < 2 { return None; }
    let out = if rel { let b = base?; (b.0 + parts[0], b.1 + parts[1]) } else { (parts[0], parts[1]) };
    finite_pt(out)
}

/// Reject non-finite / absurdly large coordinates that would panic or hang downstream.
fn finite_pt(p: Pt) -> Option<Pt> {
    if p.0.is_finite() && p.1.is_finite() && p.0.abs() < 1e12 && p.1.abs() < 1e12 { Some(p) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coords_abs_rel_polar() {
        assert_eq!(parse_coord("10,20", None), Some((10.0, 20.0)));
        assert_eq!(parse_coord("@5,0", Some((10.0, 20.0))), Some((15.0, 20.0)));
        let p = parse_coord("@100<90", Some((0.0, 0.0))).unwrap();
        assert!(p.0.abs() < 1e-6 && (p.1 - 100.0).abs() < 1e-6);
        assert_eq!(parse_coord("garbage", None), None);
    }

    #[test]
    fn arc3_passes_through_middle_and_trim_guard() {
        use std::f64::consts::TAU;
        let arc = arc3((0.0, 100.0), (100.0, 0.0), (0.0, -100.0)).unwrap();
        if let Entity::Arc { cx, cy, r, a0, a1 } = arc {
            assert!(a1 > a0, "arc must sweep CCW with a1 > a0");
            let ab = (0.0f64 - cy).atan2(100.0 - cx); // angle of the middle pick (100,0)
            assert!((ab - a0).rem_euclid(TAU) <= (a1 - a0).rem_euclid(TAU) + 1e-6, "middle point must lie on the swept arc");
            assert!((r - 100.0).abs() < 1e-6);
        } else { panic!() }
        // no cutting edge -> nothing to trim
        let line = Entity::Line { x1: 0.0, y1: 0.0, x2: 2000.0, y2: 0.0 };
        assert!(trim_line(&line, &[], (1000.0, 0.0)).is_none());
        // a crossing 1mm from the tip must still allow a trim (old 0.999 guard wrongly refused)
        let cut = Entity::Line { x1: 1999.0, y1: -10.0, x2: 1999.0, y2: 10.0 };
        assert!(!trim_line(&line, &[&cut], (1000.0, 0.0)).unwrap().is_empty());
    }

    #[test]
    fn rejects_nonfinite_coords() {
        assert_eq!(parse_coord("inf,0", None), None);
        assert_eq!(parse_coord("1e999,0", None), None);
        assert!(parse_coord("@nan<90", Some((0.0, 0.0))).is_none());
    }

    #[test]
    fn grips_and_box() {
        let mut l = Entity::Line { x1: 0.0, y1: 0.0, x2: 100.0, y2: 0.0 };
        apply_grip(&mut l, Grip::P2, (100.0, 50.0));
        if let Entity::Line { y2, .. } = l { assert!((y2 - 50.0).abs() < 1e-6); } else { panic!() }
        let mut c = Entity::Circle { cx: 0.0, cy: 0.0, r: 10.0 };
        apply_grip(&mut c, Grip::Quad, (0.0, 25.0));
        if let Entity::Circle { r, .. } = c { assert!((r - 25.0).abs() < 1e-6); } else { panic!() }
        assert_eq!(grips_of(&Entity::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 }).len(), 4);

        let line = Entity::Line { x1: 1.0, y1: 1.0, x2: 9.0, y2: 9.0 };
        assert!(entity_in_box(&line, (0.0, 0.0), (10.0, 10.0), false)); // fully contained -> window
        assert!(!entity_in_box(&line, (0.0, 0.0), (5.0, 5.0), false));  // not contained
        assert!(entity_in_box(&line, (0.0, 0.0), (5.0, 5.0), true));    // crosses -> crossing
    }

    #[test]
    fn dxf_import_roundtrip() {
        let mut d = Document::new();
        d.objs.clear();
        d.add(Entity::Circle { cx: 10.0, cy: 20.0, r: 30.0 });
        d.add(Entity::Line { x1: 0.0, y1: 0.0, x2: 100.0, y2: 50.0 });
        d.add(Entity::Arc { cx: 0.0, cy: 0.0, r: 5.0, a0: 0.0, a1: std::f64::consts::FRAC_PI_2 });
        let s = d.to_dxf();
        let d2 = Document::from_dxf(&s);
        let has_circle = d2.objs.iter().any(|o| if let Entity::Circle { r, .. } = &o.e { (*r - 30.0).abs() < 1e-6 } else { false });
        let has_line = d2.objs.iter().any(|o| if let Entity::Line { x2, .. } = &o.e { (*x2 - 100.0).abs() < 1e-6 } else { false });
        let has_arc = d2.objs.iter().any(|o| matches!(&o.e, Entity::Arc { .. }));
        assert!(has_circle && has_line && has_arc);
    }

    #[test]
    fn dxf_is_wellformed() {
        let d = Document::new();
        let s = d.to_dxf();
        assert!(s.contains("ENTITIES"));
        assert!(s.contains("TABLE"));
        assert!(s.trim_end().ends_with("EOF"));
        assert_eq!(s.lines().count() % 2, 0, "DXF must be group-code/value pairs");
    }

    #[test]
    fn translate_and_hittest() {
        let mut e = Entity::Line { x1: 0.0, y1: 0.0, x2: 100.0, y2: 0.0 };
        e.translate(0.0, 50.0);
        assert!(e.dist_to(50.0, 50.0) < 1e-6);
        assert!((e.dist_to(50.0, 0.0) - 50.0).abs() < 1e-6);
    }

    #[test]
    fn transforms_and_offset() {
        let mut e = Entity::Point { x: 10.0, y: 0.0 };
        e.rotate(0.0, 0.0, std::f64::consts::FRAC_PI_2);
        if let Entity::Point { x, y } = e { assert!(x.abs() < 1e-6 && (y - 10.0).abs() < 1e-6); } else { panic!() }
        let l = Entity::Line { x1: 0.0, y1: 0.0, x2: 100.0, y2: 0.0 };
        let o = offset_entity(&l, 50.0, (0.0, 100.0)).unwrap();
        if let Entity::Line { y1, y2, .. } = o { assert!((y1 - 50.0).abs() < 1e-6 && (y2 - 50.0).abs() < 1e-6); } else { panic!() }
        let arc = arc3((0.0, 100.0), (100.0, 0.0), (0.0, -100.0)).unwrap();
        if let Entity::Arc { r, .. } = arc { assert!((r - 100.0).abs() < 1e-6); } else { panic!() }
    }

    #[test]
    fn fillet_trim_extend() {
        let la = Entity::Line { x1: 0.0, y1: 0.0, x2: 1000.0, y2: 0.0 };
        let lb = Entity::Line { x1: 1000.0, y1: 0.0, x2: 1000.0, y2: 1000.0 };
        let (na, nb, arc) = fillet_lines(&la, &lb, 200.0).unwrap();
        // la is trimmed at its far end -> tangent point x2 = 800; lb trimmed -> tangent point y2 = 200
        if let Entity::Line { x2, .. } = na { assert!((x2 - 800.0).abs() < 1e-6); } else { panic!() }
        if let Entity::Line { y2, .. } = nb { assert!((y2 - 200.0).abs() < 1e-6); } else { panic!() }
        assert!(arc.is_some());

        let h = Entity::Line { x1: 0.0, y1: 0.0, x2: 1000.0, y2: 0.0 };
        let v1 = Entity::Line { x1: 300.0, y1: -100.0, x2: 300.0, y2: 100.0 };
        let v2 = Entity::Line { x1: 700.0, y1: -100.0, x2: 700.0, y2: 100.0 };
        let out = trim_line(&h, &[&v1, &v2], (500.0, 0.0)).unwrap();
        assert_eq!(out.len(), 2);

        let short = Entity::Line { x1: 0.0, y1: 0.0, x2: 400.0, y2: 0.0 };
        let wall = Entity::Line { x1: 900.0, y1: -100.0, x2: 900.0, y2: 100.0 };
        let ext = extend_line(&short, &[&wall], (400.0, 0.0)).unwrap();
        if let Entity::Line { x2, .. } = ext { assert!((x2 - 900.0).abs() < 1e-6); } else { panic!() }
    }

    #[test]
    fn arc_hittest_respects_sweep() {
        // quarter arc on +x/+y; a point near the ring but in the unswept quadrant is far, not ~0
        let arc = Entity::Arc { cx: 0.0, cy: 0.0, r: 100.0, a0: 0.0, a1: std::f64::consts::FRAC_PI_2 };
        assert!(arc.dist_to(100.0, 1.0) < 2.0, "on the swept arc -> near zero");
        assert!(arc.dist_to(-100.0, 0.0) > 50.0, "opposite side (unswept) -> far, measured to endpoint");
    }

    #[test]
    fn fillet_rejects_oversize_radius() {
        let la = Entity::Line { x1: 0.0, y1: 0.0, x2: 100.0, y2: 0.0 };
        let lb = Entity::Line { x1: 100.0, y1: 0.0, x2: 100.0, y2: 100.0 };
        assert!(fillet_lines(&la, &lb, 500.0).is_none(), "radius bigger than the legs must be rejected");
        assert!(fillet_lines(&la, &lb, 40.0).is_some());
        assert!(chamfer_lines(&la, &lb, 500.0).is_none());
        assert!(chamfer_lines(&la, &lb, 40.0).is_some());
    }

    #[test]
    fn legacy_polyline_import() {
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nPOLYLINE\n8\n0\n70\n1\n0\nVERTEX\n10\n0\n20\n0\n0\nVERTEX\n10\n50\n20\n0\n0\nVERTEX\n10\n50\n20\n50\n0\nSEQEND\n0\nENDSEC\n0\nEOF\n";
        let d = Document::from_dxf(dxf);
        let poly = d.objs.iter().find_map(|o| if let Entity::Poly { pts, closed } = &o.e { Some((pts.len(), *closed)) } else { None });
        assert_eq!(poly, Some((3, true)));
    }

    #[test]
    fn import_drops_nonfinite_and_keeps_text_literal() {
        // an LWPOLYLINE vertex with a non-finite coord must be dropped, not accepted as inf.
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nLWPOLYLINE\n8\n0\n70\n0\n10\n0\n20\n0\n10\n1e999\n20\n5\n10\n10\n20\n10\n0\nENDSEC\n0\nEOF\n";
        let d = Document::from_dxf(dxf);
        let poly = d.objs.iter().find_map(|o| if let Entity::Poly { pts, .. } = &o.e { Some(pts.clone()) } else { None })
            .expect("polyline imported");
        assert_eq!(poly.len(), 2, "the 1e999 vertex must be dropped");
        assert!(poly.iter().all(|p| p.0.is_finite() && p.1.is_finite()));

        // plain TEXT (group 1) must be stored literally, not run through the MTEXT stripper.
        let t = "0\nSECTION\n2\nENTITIES\n0\nTEXT\n8\n0\n10\n0\n20\n0\n40\n10\n1\nC:\\part{a}\n0\nENDSEC\n0\nEOF\n";
        let dt = Document::from_dxf(t);
        let s = dt.objs.iter().find_map(|o| if let Entity::Text { s, .. } = &o.e { Some(s.clone()) } else { None });
        assert_eq!(s.as_deref(), Some("C:\\part{a}"));
    }

    #[test]
    fn explode_entities() {
        // rect -> 4 lines
        let r = explode_entity(&Entity::Rect { x: 0.0, y: 0.0, w: 10.0, h: 20.0 }).unwrap();
        assert_eq!(r.len(), 4);
        assert!(r.iter().all(|e| matches!(e, Entity::Line { .. })));
        // closed triangle poly -> 3 lines
        let p = explode_entity(&Entity::Poly { pts: vec![(0.0, 0.0), (10.0, 0.0), (5.0, 8.0)], closed: true }).unwrap();
        assert_eq!(p.len(), 3);
        // dim -> its 3 drawn lines (dim line + 2 extension lines)
        let d = explode_entity(&Entity::Dim { x1: 0.0, y1: 0.0, x2: 100.0, y2: 0.0, vertical: false, off: 50.0 }).unwrap();
        assert_eq!(d.len(), 3);
        // primitives can't explode
        assert!(explode_entity(&Entity::Line { x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 }).is_none());
        assert!(explode_entity(&Entity::Circle { cx: 0.0, cy: 0.0, r: 5.0 }).is_none());
    }

    #[test]
    fn break_and_join_lines() {
        let l = Entity::Line { x1: 0.0, y1: 0.0, x2: 100.0, y2: 0.0 };
        // break out the middle third -> two pieces [0,30] and [70,100]
        let parts = break_line(&l, (30.0, 0.0), (70.0, 0.0)).unwrap();
        assert_eq!(parts.len(), 2);
        if let Entity::Line { x2, .. } = parts[0] { assert!((x2 - 30.0).abs() < 1e-6); } else { panic!() }
        if let Entity::Line { x1, .. } = parts[1] { assert!((x1 - 70.0).abs() < 1e-6); } else { panic!() }
        // break from the start -> one piece
        assert_eq!(break_line(&l, (0.0, 0.0), (40.0, 0.0)).unwrap().len(), 1);

        // join two collinear segments sharing the 50 point -> one 0..100 line
        let a = Entity::Line { x1: 0.0, y1: 0.0, x2: 50.0, y2: 0.0 };
        let b = Entity::Line { x1: 50.0, y1: 0.0, x2: 100.0, y2: 0.0 };
        let j = join_lines(&a, &b).unwrap();
        if let Entity::Line { x1, y1, x2, y2 } = j {
            assert!(x1.abs() < 1e-6 && y1.abs() < 1e-6 && (x2 - 100.0).abs() < 1e-6 && y2.abs() < 1e-6);
        } else { panic!() }
        // non-collinear must refuse
        let c = Entity::Line { x1: 50.0, y1: 0.0, x2: 50.0, y2: 80.0 };
        assert!(join_lines(&a, &c).is_none());
        // a long line and a clearly-parallel neighbour 1 unit away must NOT merge (tol is float-scale, not length-scale)
        let long = Entity::Line { x1: 0.0, y1: 0.0, x2: 1000.0, y2: 0.0 };
        let par = Entity::Line { x1: 0.0, y1: 1.0, x2: 1000.0, y2: 1.0 };
        assert!(join_lines(&long, &par).is_none(), "a 1-unit-off parallel line must not be swallowed");
        // genuinely collinear at large coordinates must still join
        let g1 = Entity::Line { x1: 1e8, y1: 1e8, x2: 1e8 + 100.0, y2: 1e8 };
        let g2 = Entity::Line { x1: 1e8 + 100.0, y1: 1e8, x2: 1e8 + 200.0, y2: 1e8 };
        assert!(join_lines(&g1, &g2).is_some(), "collinear lines at huge coords must still join");
        // break whose two points span the whole line -> zero pieces (caller must not delete)
        assert_eq!(break_line(&l, (-5.0, 0.0), (105.0, 0.0)).unwrap().len(), 0);
    }

    #[test]
    fn array_rect_and_polar() {
        let item = [Entity::Point { x: 0.0, y: 0.0 }];
        // 2 rows x 3 cols, 10 apart -> 6 total positions, 5 new copies (original excluded)
        let rect = array_rect(&item, 2, 3, 10.0, 20.0);
        assert_eq!(rect.len(), 5);
        // a copy at col 2, row 1 must sit at (20,20)
        assert!(rect.iter().any(|e| matches!(e, Entity::Point { x, y } if (*x - 20.0).abs() < 1e-6 && (*y - 20.0).abs() < 1e-6)));

        // polar: 4 items full circle around origin -> 3 new copies at 90/180/270
        let p = [Entity::Point { x: 10.0, y: 0.0 }];
        let pol = array_polar(&p, (0.0, 0.0), 4, 360.0);
        assert_eq!(pol.len(), 3);
        // first copy at 90deg -> (0,10)
        assert!(matches!(&pol[0], Entity::Point { x, y } if x.abs() < 1e-6 && (*y - 10.0).abs() < 1e-6));
        assert!(array_polar(&p, (0.0, 0.0), 1, 360.0).is_empty());
    }

    #[test]
    fn import_ellipse_spline_mtext() {
        // ELLIPSE: center (10,10), major vector (20,0) -> major end (30,10), ratio 0.5 -> minor 10.
        let ell = "0\nSECTION\n2\nENTITIES\n0\nELLIPSE\n8\n0\n10\n10\n20\n10\n11\n20\n21\n0\n40\n0.5\n0\nENDSEC\n0\nEOF\n";
        let d = Document::from_dxf(ell);
        let poly = d.objs.iter().find_map(|o| if let Entity::Poly { pts, closed } = &o.e { Some((pts.clone(), *closed)) } else { None });
        let (pts, closed) = poly.expect("ellipse -> closed poly");
        assert!(closed && pts.len() >= 16);
        // the +x extreme should be near the major end (30,10)
        let maxx = pts.iter().cloned().fold((f64::MIN, 0.0), |a, p| if p.0 > a.0 { p } else { a });
        assert!((maxx.0 - 30.0).abs() < 1.0 && (maxx.1 - 10.0).abs() < 1.0);

        // SPLINE via fit points (11/21).
        let sp = "0\nSECTION\n2\nENTITIES\n0\nSPLINE\n8\n0\n11\n0\n21\n0\n11\n50\n21\n80\n11\n100\n21\n0\n0\nENDSEC\n0\nEOF\n";
        let ds = Document::from_dxf(sp);
        assert!(ds.objs.iter().any(|o| matches!(&o.e, Entity::Poly { pts, .. } if pts.len() == 3)));

        // MTEXT with inline formatting codes -> plain text.
        let mt = "0\nSECTION\n2\nENTITIES\n0\nMTEXT\n8\n0\n10\n5\n20\n5\n40\n100\n1\n{\\fArial|b1;Hello}\\PWorld\n0\nENDSEC\n0\nEOF\n";
        let dm = Document::from_dxf(mt);
        let s = dm.objs.iter().find_map(|o| if let Entity::Text { s, .. } = &o.e { Some(s.clone()) } else { None });
        assert_eq!(s.as_deref(), Some("Hello World"));
    }

    #[test]
    fn dim_mirror_keeps_side_on_axis_flip() {
        // horizontal dim, line above the points (off > 0). Mirror across a VERTICAL line (left-right):
        // y is unchanged, so the dim line must stay above -> off stays positive.
        let mut d = Entity::Dim { x1: 0.0, y1: 0.0, x2: 100.0, y2: 0.0, vertical: false, off: 450.0 };
        d.mirror(50.0, 0.0, 50.0, 100.0); // vertical mirror line x=50
        if let Entity::Dim { off, .. } = d { assert!(off > 0.0, "left-right mirror must not flip a horizontal dim's side"); } else { panic!() }
        // Mirror across a HORIZONTAL line: y flips, so the side flips -> off negates.
        let mut d2 = Entity::Dim { x1: 0.0, y1: 0.0, x2: 100.0, y2: 0.0, vertical: false, off: 450.0 };
        d2.mirror(0.0, 10.0, 100.0, 10.0); // horizontal mirror line y=10
        if let Entity::Dim { off, .. } = d2 { assert!(off < 0.0, "mirror across a horizontal line must flip the side"); } else { panic!() }
    }

    #[test]
    fn undo_redo() {
        let mut d = Document::new();
        let n = d.objs.len();
        d.add(Entity::Circle { cx: 0.0, cy: 0.0, r: 10.0 });
        d.commit();
        assert_eq!(d.objs.len(), n + 1);
        d.undo();
        assert_eq!(d.objs.len(), n);
        d.redo();
        assert_eq!(d.objs.len(), n + 1);
    }
}
