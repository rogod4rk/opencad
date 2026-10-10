#![allow(dead_code)]
//! OpenCAD — native AutoCAD-style 2D CAD (Rust + egui).
//! Command line with AutoCAD aliases + coordinate entry, object snaps, layers, DXF export.

mod model;
use eframe::egui;
use egui::{Align2, Color32, FontId, Pos2, Stroke};
use model::{apply_grip, arc3, array_polar, array_rect, break_line, chamfer_lines, dim_geom, ellipse, entity_in_box,
            entity_segs, explode_entity, extend_line, fillet_lines, grips_of, join_lines, nearest_point, offset_entity,
            parse_coord, seg_inter, trim_line, Document, Entity, Grip, Pt};
use std::f64::consts::TAU;
use std::collections::HashSet;

fn app_icon() -> egui::IconData {
    // Raw RGBA 256x256 generated into assets/ (no image crate needed).
    egui::IconData {
        rgba: include_bytes!("../assets/icon_256.rgba").to_vec(),
        width: 256,
        height: 256,
    }
}

fn main() -> eframe::Result<()> {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_title("OpenCAD")
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native("OpenCAD", opts, Box::new(|cc| {
        setup_style(&cc.egui_ctx);
        Ok(Box::new(App::new()))
    }))
}

/// AutoCAD-like dark theme: deep slate panels, cyan accent, tight spacing.
fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    let panel = Color32::from_rgb(22, 28, 36);
    let accent = Color32::from_rgb(55, 212, 232);
    v.panel_fill = panel;
    v.window_fill = panel;
    v.extreme_bg_color = Color32::from_rgb(13, 18, 24);
    v.faint_bg_color = Color32::from_rgb(30, 38, 48);
    v.override_text_color = Some(Color32::from_rgb(205, 217, 229));
    v.hyperlink_color = accent;
    v.selection.bg_fill = accent.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, accent);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, accent.linear_multiply(0.6));
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, accent);
    ctx.set_visuals(v);

    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.button_padding = egui::vec2(6.0, 3.0);
    ctx.set_style(style);
}

/// World <-> screen transform. World is Y-up (CAD), millimetres. `s` = pixels per mm.
struct View { x: f64, y: f64, s: f64 }
impl View {
    fn to_screen(&self, p: Pt, c: Pos2) -> Pos2 {
        Pos2::new(c.x + ((p.0 - self.x) * self.s) as f32, c.y - ((p.1 - self.y) * self.s) as f32)
    }
    fn to_world(&self, pos: Pos2, c: Pos2) -> Pt {
        (self.x + (pos.x - c.x) as f64 / self.s, self.y - (pos.y - c.y) as f64 / self.s)
    }
}

enum Cmd {
    None,
    Line { prev: Option<Pt> },
    Pline { pts: Vec<Pt> },
    Rect { a: Option<Pt> },
    Circle { c: Option<Pt> },
    Arc { pts: Vec<Pt> },
    Polygon { sides: Option<usize>, center: Option<Pt> },
    Erase,
    Move { base: Option<Pt> },
    Copy { base: Option<Pt> },
    Rotate { base: Option<Pt> },
    Scale { base: Option<Pt> },
    Mirror { p1: Option<Pt> },
    Offset { dist: Option<f64>, ent: Option<usize> },
    DimLinear { pts: Vec<Pt> },
    Trim,
    Extend,
    Fillet { first: Option<usize> },
    Chamfer { first: Option<usize> },
    Ellipse { center: Option<Pt>, major: Option<Pt> },
    Text { pos: Option<Pt>, height: Option<f64> },
    ArrayRect { rows: Option<usize>, cols: Option<usize>, dy: Option<f64> },
    ArrayPolar { center: Option<Pt>, count: Option<usize> },
    Break { ent: Option<usize>, p1: Option<Pt> },
    Join { first: Option<usize> },
    Point,
}

#[derive(Clone, Copy, PartialEq)]
enum Confirm { New, Import }

fn alias(n: &str) -> &str {
    match n {
        "L" | "LINE" => "LINE",
        "PL" | "PLINE" => "PLINE",
        "REC" | "RECTANG" | "RECTANGLE" => "RECTANG",
        "C" | "CIRCLE" => "CIRCLE",
        "A" | "ARC" => "ARC",
        "POL" | "POLYGON" => "POLYGON",
        "E" | "ERASE" | "DELETE" => "ERASE",
        "M" | "MOVE" => "MOVE",
        "CO" | "CP" | "COPY" => "COPY",
        "RO" | "ROTATE" => "ROTATE",
        "SC" | "SCALE" => "SCALE",
        "MI" | "MIRROR" => "MIRROR",
        "O" | "OFFSET" => "OFFSET",
        "TR" | "TRIM" => "TRIM",
        "EX" | "EXTEND" => "EXTEND",
        "F" | "FILLET" => "FILLET",
        "CHA" | "CHAMFER" => "CHAMFER",
        "EL" | "ELLIPSE" => "ELLIPSE",
        "T" | "DT" | "TEXT" => "TEXT",
        "DLI" | "DIM" | "DIMLINEAR" => "DIMLINEAR",
        "AR" | "ARRAY" | "ARRAYRECT" => "ARRAYRECT",
        "ARRAYPOLAR" | "ARPOLAR" => "ARRAYPOLAR",
        "BR" | "BREAK" => "BREAK",
        "J" | "JOIN" => "JOIN",
        "X" | "EXPLODE" => "EXPLODE",
        "PO" | "POINT" => "POINT",
        "U" | "UNDO" => "UNDO",
        "REDO" | "MREDO" => "REDO",
        "Z" | "ZOOM" => "ZOOM",
        other => other,
    }
}

struct App {
    doc: Document,
    view: View,
    cmd: Cmd,
    prompt: String,
    sel: HashSet<usize>,
    cmdline: String,
    cur: Pt,
    vp: (f32, f32),
    grid: bool,
    snap_grid: bool,
    ortho: bool,
    osnap: bool,
    last: String,
    fillet_r: f64,
    chamfer_d: f64,
    sel_box: Option<(Pt, Pt)>,
    grip_drag: Option<(usize, Grip)>,
    snap_marker: Option<(Pt, SnapKind)>,
    new_layer: String,
    ribbon_tab: usize,
    polar: bool,
    lwt: bool,
    confirm: Option<Confirm>,
}

#[derive(Clone, Copy)]
enum SnapKind { End, Mid, Center, Quad, Inter, Near }

impl App {
    fn new() -> Self {
        Self {
            doc: Document::new(),
            view: View { x: 0.0, y: 0.0, s: 0.11 },
            cmd: Cmd::None,
            prompt: "Type a command (LINE / L, CIRCLE / C, REC, MOVE / M …)".into(),
            sel: HashSet::new(),
            cmdline: String::new(),
            cur: (0.0, 0.0),
            vp: (900.0, 600.0),
            grid: true,
            snap_grid: false,
            ortho: false,
            osnap: true,
            last: String::new(),
            fillet_r: 0.0,
            chamfer_d: 0.0,
            sel_box: None,
            grip_drag: None,
            snap_marker: None,
            new_layer: String::new(),
            ribbon_tab: 0,
            polar: false,
            lwt: false,
            confirm: None,
        }
    }

    fn do_new(&mut self) { self.doc = Document::new(); self.clear_refs(); self.end_cmd(); self.zoom_extents(); }
    fn do_import_file(&mut self) {
        match std::fs::read("opencad.dxf") {
            Ok(b) => { self.import_bytes(&b); self.prompt = format!("Imported {} entities", self.doc.objs.len()); }
            Err(e) => { self.prompt = format!("Import failed: {}", e); }
        }
    }

    fn grip_at(&self, p: Pt) -> Option<(usize, Grip)> {
        let tol = 9.0 / self.view.s;
        for &i in &self.sel {
            if i >= self.doc.objs.len() { continue; }
            for (gp, g) in grips_of(&self.doc.objs[i].e) {
                if (gp.0 - p.0).hypot(gp.1 - p.1) < tol { return Some((i, g)); }
            }
        }
        None
    }

    /// Selection and grip handles index into doc.objs; any wholesale change to that vec
    /// (undo/redo, import, trim) invalidates them, so drop them together.
    fn clear_refs(&mut self) {
        self.sel.clear();
        self.grip_drag = None;
        self.sel_box = None;
    }
    fn do_undo(&mut self) { self.doc.undo(); self.clear_refs(); self.end_cmd(); }
    fn do_redo(&mut self) { self.doc.redo(); self.clear_refs(); self.end_cmd(); }
    fn import_bytes(&mut self, bytes: &[u8]) {
        let txt = String::from_utf8_lossy(bytes);
        self.doc = Document::from_dxf(&txt);
        self.clear_refs();
        self.end_cmd();
        self.zoom_extents();
    }

    fn finalize_box(&mut self, s: Pt, e: Pt) {
        if (s.0 - e.0).abs() < 1e-6 && (s.1 - e.1).abs() < 1e-6 { return; }
        let crossing = e.0 < s.0;
        let (lo, hi) = ((s.0.min(e.0), s.1.min(e.1)), (s.0.max(e.0), s.1.max(e.1)));
        self.sel.clear();
        for (i, ob) in self.doc.objs.iter().enumerate() {
            let l = self.doc.layer(&ob.layer);
            if !l.on || l.frozen || l.locked { continue; }
            if entity_in_box(&ob.e, lo, hi, crossing) { self.sel.insert(i); }
        }
    }

    fn base(&self) -> Option<Pt> {
        match &self.cmd {
            Cmd::Line { prev } => *prev,
            Cmd::Pline { pts } => pts.last().copied(),
            Cmd::Rect { a } => *a,
            Cmd::Circle { c } => *c,
            Cmd::Arc { pts } => pts.last().copied(),
            Cmd::Polygon { center, .. } => *center,
            Cmd::Move { base } | Cmd::Copy { base } | Cmd::Rotate { base } | Cmd::Scale { base } => *base,
            Cmd::Mirror { p1 } => *p1,
            Cmd::DimLinear { pts } => pts.last().copied(),
            Cmd::Ellipse { center, .. } => *center,
            Cmd::ArrayPolar { center, .. } => *center,
            _ => None,
        }
    }

    /// Apply a per-entity generator (ARRAY) to each selected object, preserving its layer.
    /// `per_source` is the copies each selected object will produce; refuses an array that would
    /// allocate more than ARRAY_MAX total (fat-finger / OOM guard) and skips an empty result.
    fn array_apply(&mut self, per_source: usize, gen: impl Fn(&Entity) -> Vec<Entity>) {
        const ARRAY_MAX: usize = 100_000;
        let ids: Vec<usize> = self.sel.iter().copied().filter(|&i| i < self.doc.objs.len()).collect();
        if per_source.saturating_mul(ids.len()) > ARRAY_MAX {
            self.prompt = format!("Array too large (~{} copies, max {}). Reduce the counts.", per_source.saturating_mul(ids.len()), ARRAY_MAX);
            self.cmd = Cmd::None;
            return;
        }
        let mut added = 0usize;
        for i in ids {
            let (e, ly) = (self.doc.objs[i].e.clone(), self.doc.objs[i].layer.clone());
            for ne in gen(&e) { self.doc.add_on(ne, &ly); added += 1; }
        }
        self.cmd = Cmd::None;
        if added > 0 { self.doc.commit(); self.prompt = format!("Array: {} copies added", added); }
        else { self.prompt = "Nothing to array (check counts).".into(); }
    }

    fn snap(&self, w: Pt, exclude: Option<usize>) -> (Pt, Option<SnapKind>) {
        if self.osnap {
            let tol = 14.0 / self.view.s;
            let mut cands: Vec<(u8, f64, Pt, SnapKind)> = vec![];
            let mut push = |pt: Pt, k: SnapKind, pri: u8| {
                let d = (pt.0 - w.0).hypot(pt.1 - w.1);
                if d < tol { cands.push((pri, d, pt, k)); }
            };
            for (idx, ob) in self.doc.objs.iter().enumerate() {
                if Some(idx) == exclude { continue; }
                let l = self.doc.layer(&ob.layer);
                if !l.on || l.frozen { continue; }
                for (gp, g) in grips_of(&ob.e) {
                    let (k, pri) = match g {
                        Grip::Mid => (SnapKind::Mid, 2),
                        Grip::Center => (SnapKind::Center, 3),
                        Grip::Quad => (SnapKind::Quad, 5),
                        _ => (SnapKind::End, 0),
                    };
                    push(gp, k, pri);
                }
            }
            let near: Vec<usize> = (0..self.doc.objs.len()).filter(|&i| {
                let ob = &self.doc.objs[i];
                let l = self.doc.layer(&ob.layer);
                l.on && !l.frozen && Some(i) != exclude && ob.e.dist_to(w.0, w.1) < 30.0 / self.view.s
            }).collect();
            for a in 0..near.len() {
                for b in (a + 1)..near.len() {
                    let (ea, eb) = (&self.doc.objs[near[a]].e, &self.doc.objs[near[b]].e);
                    for (p1, p2) in entity_segs(ea) {
                        for (p3, p4) in entity_segs(eb) {
                            if let Some(ip) = seg_inter(p1, p2, p3, p4) { push(ip, SnapKind::Inter, 1); }
                        }
                    }
                }
            }
            for &i in &near {
                if let Some(np) = nearest_point(&self.doc.objs[i].e, w) { push(np, SnapKind::Near, 6); }
            }
            cands.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap()));
            if let Some((_, _, pt, k)) = cands.first() { return (*pt, Some(*k)); }
        }
        if self.snap_grid {
            let g = 100.0;
            return (((w.0 / g).round() * g, (w.1 / g).round() * g), None);
        }
        (w, None)
    }

    fn ortho_pt(&self, p: Pt) -> Pt {
        if self.ortho {
            if let Some(b) = self.base() {
                return if (p.0 - b.0).abs() > (p.1 - b.1).abs() { (p.0, b.1) } else { (b.0, p.1) };
            }
        }
        p
    }

    fn hit(&self, p: Pt) -> Option<usize> {
        let tol = 8.0 / self.view.s;
        let mut best: Option<(f64, usize)> = None;
        for (i, ob) in self.doc.objs.iter().enumerate() {
            let l = self.doc.layer(&ob.layer);
            if !l.on || l.frozen || l.locked { continue; }
            let d = ob.e.dist_to(p.0, p.1);
            if d < tol && best.map_or(true, |b| d < b.0) { best = Some((d, i)); }
        }
        best.map(|b| b.1)
    }

    fn feed_point(&mut self, p: Pt) {
        let cmd = std::mem::replace(&mut self.cmd, Cmd::None);
        match cmd {
            Cmd::None => {
                let h = self.hit(p);
                self.sel.clear();
                if let Some(i) = h { self.sel.insert(i); }
            }
            Cmd::Line { prev } => {
                if let Some(pr) = prev {
                    self.doc.add(Entity::Line { x1: pr.0, y1: pr.1, x2: p.0, y2: p.1 });
                    self.doc.commit();
                }
                self.cmd = Cmd::Line { prev: Some(p) };
                self.prompt = "Specify next point (Enter to end)".into();
            }
            Cmd::Pline { mut pts } => {
                pts.push(p);
                self.cmd = Cmd::Pline { pts };
                self.prompt = "Specify next point (Enter to end)".into();
            }
            Cmd::Rect { a } => {
                if let Some(a0) = a {
                    self.doc.add(Entity::Rect {
                        x: a0.0.min(p.0), y: a0.1.min(p.1),
                        w: (p.0 - a0.0).abs(), h: (p.1 - a0.1).abs(),
                    });
                    self.doc.commit();
                    self.end_cmd();
                } else {
                    self.cmd = Cmd::Rect { a: Some(p) };
                    self.prompt = "Specify opposite corner".into();
                }
            }
            Cmd::Circle { c } => {
                if let Some(ct) = c {
                    let r = (p.0 - ct.0).hypot(p.1 - ct.1).max(1.0);
                    self.doc.add(Entity::Circle { cx: ct.0, cy: ct.1, r });
                    self.doc.commit();
                    self.end_cmd();
                } else {
                    self.cmd = Cmd::Circle { c: Some(p) };
                    self.prompt = "Specify radius (click or type a number)".into();
                }
            }
            Cmd::Arc { mut pts } => {
                pts.push(p);
                if pts.len() >= 3 {
                    if let Some(a) = arc3(pts[0], pts[1], pts[2]) { self.doc.add(a); }
                    else { self.doc.add(Entity::Poly { pts: pts.clone(), closed: false }); }
                    self.doc.commit();
                    self.end_cmd();
                } else {
                    self.cmd = Cmd::Arc { pts };
                    self.prompt = "Specify next point of arc".into();
                }
            }
            Cmd::Polygon { sides, center } => {
                if sides.is_none() {
                    self.cmd = Cmd::Polygon { sides, center };
                    self.prompt = "Type the number of sides first".into();
                } else if let Some(ct) = center {
                    let r = (p.0 - ct.0).hypot(p.1 - ct.1).max(1.0);
                    let n = sides.unwrap_or(6).max(3);
                    let mut pts = Vec::with_capacity(n);
                    for i in 0..n {
                        let t = TAU * i as f64 / n as f64 - std::f64::consts::FRAC_PI_2;
                        pts.push((ct.0 + r * t.cos(), ct.1 + r * t.sin()));
                    }
                    self.doc.add(Entity::Poly { pts, closed: true });
                    self.doc.commit();
                    self.end_cmd();
                } else {
                    self.cmd = Cmd::Polygon { sides, center: Some(p) };
                    self.prompt = "Specify radius".into();
                }
            }
            Cmd::Rotate { base } => {
                if let Some(b) = base {
                    let ang = (p.1 - b.1).atan2(p.0 - b.0);
                    for i in self.sel.iter().copied().collect::<Vec<_>>() { if i < self.doc.objs.len() { self.doc.objs[i].e.rotate(b.0, b.1, ang); } }
                    self.doc.commit();
                    self.end_cmd();
                } else {
                    self.cmd = Cmd::Rotate { base: Some(p) };
                    self.prompt = "Specify rotation angle (type degrees, or click)".into();
                }
            }
            Cmd::Scale { base } => {
                if base.is_none() {
                    self.cmd = Cmd::Scale { base: Some(p) };
                    self.prompt = "Specify scale factor (type a number)".into();
                } else {
                    self.cmd = Cmd::Scale { base };
                    self.prompt = "Type the scale factor".into();
                }
            }
            Cmd::Mirror { p1 } => {
                if let Some(a) = p1 {
                    for i in self.sel.iter().copied().collect::<Vec<_>>() {
                        if i >= self.doc.objs.len() { continue; }
                        let mut o = self.doc.objs[i].clone();
                        o.e.mirror(a.0, a.1, p.0, p.1);
                        self.doc.objs.push(o);
                    }
                    self.doc.commit();
                    self.end_cmd();
                } else {
                    self.cmd = Cmd::Mirror { p1: Some(p) };
                    self.prompt = "Specify second point of mirror line".into();
                }
            }
            Cmd::Offset { dist, ent } => {
                if let Some(d) = dist {
                    if let Some(ei) = ent {
                        let src = self.doc.objs[ei].e.clone();
                        let layer = self.doc.objs[ei].layer.clone();
                        if let Some(no) = offset_entity(&src, d, p) { self.doc.add_on(no, &layer); self.doc.commit(); }
                        self.cmd = Cmd::Offset { dist: Some(d), ent: None };
                        self.prompt = "Select object to offset (Esc to finish)".into();
                    } else if let Some(i) = self.hit(p) {
                        self.cmd = Cmd::Offset { dist: Some(d), ent: Some(i) };
                        self.prompt = "Specify side to offset".into();
                    } else {
                        self.cmd = Cmd::Offset { dist: Some(d), ent: None };
                    }
                } else {
                    self.cmd = Cmd::Offset { dist, ent };
                    self.prompt = "Type the offset distance first".into();
                }
            }
            Cmd::DimLinear { mut pts } => {
                pts.push(p);
                if pts.len() >= 3 {
                    let (a, b, loc) = (pts[0], pts[1], pts[2]);
                    let vertical = (b.0 - a.0).abs() < (b.1 - a.1).abs();
                    // signed offset from the measured points to where the user placed the dimension line
                    let off = if vertical { loc.0 - a.0.max(b.0) } else { loc.1 - a.1.max(b.1) };
                    self.doc.add_on(Entity::Dim { x1: a.0, y1: a.1, x2: b.0, y2: b.1, vertical, off }, "dimensions");
                    self.doc.commit();
                    self.end_cmd();
                } else {
                    let msg = if pts.len() == 1 { "Specify second extension line origin" } else { "Specify dimension line location" };
                    self.cmd = Cmd::DimLinear { pts };
                    self.prompt = msg.into();
                }
            }
            Cmd::Trim => {
                if let Some(i) = self.hit(p) {
                    let res = {
                        let others: Vec<&Entity> = self.doc.objs.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, o)| &o.e).collect();
                        trim_line(&self.doc.objs[i].e, &others, p)
                    };
                    if let Some(parts) = res {
                        let layer = self.doc.objs[i].layer.clone();
                        self.doc.objs.remove(i);
                        for e in parts { self.doc.add_on(e, &layer); }
                        self.doc.commit();
                        self.clear_refs(); // indices shifted
                    } else { self.prompt = "No crossing there to trim.".into(); }
                }
                self.cmd = Cmd::Trim;
            }
            Cmd::Extend => {
                if let Some(i) = self.hit(p) {
                    let res = {
                        let others: Vec<&Entity> = self.doc.objs.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, o)| &o.e).collect();
                        extend_line(&self.doc.objs[i].e, &others, p)
                    };
                    if let Some(ne) = res { self.doc.objs[i].e = ne; self.doc.commit(); }
                    else { self.prompt = "No boundary to extend to.".into(); }
                }
                self.cmd = Cmd::Extend;
            }
            Cmd::Fillet { first } => {
                if let Some(i) = self.hit(p) {
                    if let Some(fi) = first {
                        let (ca, cb) = (self.doc.objs[fi].e.clone(), self.doc.objs[i].e.clone());
                        if let Some((na, nb, arc)) = fillet_lines(&ca, &cb, self.fillet_r) {
                            let la = self.doc.objs[fi].layer.clone();
                            self.doc.objs[fi].e = na;
                            self.doc.objs[i].e = nb;
                            if let Some(ar) = arc { self.doc.add_on(ar, &la); }
                            self.doc.commit();
                            self.end_cmd();
                        } else { self.prompt = "Can't fillet: radius too large for these lines, or they don't meet. Pick two straight lines.".into(); self.cmd = Cmd::Fillet { first: None }; }
                    } else { self.cmd = Cmd::Fillet { first: Some(i) }; self.prompt = "Select second line".into(); }
                } else { self.cmd = Cmd::Fillet { first }; }
            }
            Cmd::Chamfer { first } => {
                if let Some(i) = self.hit(p) {
                    if let Some(fi) = first {
                        let (ca, cb) = (self.doc.objs[fi].e.clone(), self.doc.objs[i].e.clone());
                        if let Some((na, nb, con)) = chamfer_lines(&ca, &cb, self.chamfer_d) {
                            let la = self.doc.objs[fi].layer.clone();
                            self.doc.objs[fi].e = na;
                            self.doc.objs[i].e = nb;
                            self.doc.add_on(con, &la);
                            self.doc.commit();
                            self.end_cmd();
                        } else { self.prompt = "Can't chamfer: set a distance first (type a number), or it's too large for these lines.".into(); self.cmd = Cmd::Chamfer { first: None }; }
                    } else { self.cmd = Cmd::Chamfer { first: Some(i) }; self.prompt = "Select second line".into(); }
                } else { self.cmd = Cmd::Chamfer { first }; }
            }
            Cmd::Ellipse { center, major } => {
                if center.is_none() {
                    self.cmd = Cmd::Ellipse { center: Some(p), major: None };
                    self.prompt = "Endpoint of major axis".into();
                } else if major.is_none() {
                    self.cmd = Cmd::Ellipse { center, major: Some(p) };
                    self.prompt = "Distance to minor axis (click or type a number)".into();
                } else {
                    let ct = center.unwrap();
                    let mj = major.unwrap();
                    let minor = (ct.0 - p.0).hypot(ct.1 - p.1).max(1.0);
                    self.doc.add(ellipse(ct, mj, minor));
                    self.doc.commit();
                    self.end_cmd();
                }
            }
            Cmd::Text { pos, height } => {
                if pos.is_none() {
                    self.cmd = Cmd::Text { pos: Some(p), height: None };
                    self.prompt = "Specify text height (type a number)".into();
                } else {
                    self.cmd = Cmd::Text { pos, height };
                    self.prompt = "Type the text height".into();
                }
            }
            Cmd::ArrayPolar { center, count } => {
                if center.is_none() {
                    self.cmd = Cmd::ArrayPolar { center: Some(p), count };
                    self.prompt = "Enter number of items".into();
                } else {
                    self.cmd = Cmd::ArrayPolar { center, count };
                    self.prompt = "Type the number of items".into();
                }
            }
            Cmd::ArrayRect { rows, cols, dy } => { self.cmd = Cmd::ArrayRect { rows, cols, dy }; self.prompt = "Type the row/column counts and spacing (not a point)".into(); }
            Cmd::Break { ent, p1 } => {
                if ent.is_none() {
                    if let Some(i) = self.hit(p) { self.cmd = Cmd::Break { ent: Some(i), p1: None }; self.prompt = "Specify first break point".into(); }
                    else { self.cmd = Cmd::Break { ent: None, p1: None }; }
                } else if p1.is_none() {
                    self.cmd = Cmd::Break { ent, p1: Some(p) };
                    self.prompt = "Specify second break point".into();
                } else {
                    let i = ent.unwrap();
                    if i < self.doc.objs.len() {
                        match break_line(&self.doc.objs[i].e, p1.unwrap(), p) {
                            Some(parts) if !parts.is_empty() => {
                                let layer = self.doc.objs[i].layer.clone();
                                self.doc.objs.remove(i);
                                for e in parts { self.doc.add_on(e, &layer); }
                                self.doc.commit();
                            }
                            Some(_) => { self.prompt = "Break points span the whole line — nothing left; pick points on the line.".into(); }
                            None => { self.prompt = "BREAK works on straight lines.".into(); }
                        }
                    }
                    self.clear_refs();
                    self.end_cmd();
                }
            }
            Cmd::Join { first } => {
                if let Some(i) = self.hit(p) {
                    if let Some(fi) = first {
                        if i != fi && fi < self.doc.objs.len() && i < self.doc.objs.len() {
                            if let Some(j) = join_lines(&self.doc.objs[fi].e, &self.doc.objs[i].e) {
                                self.doc.objs[fi].e = j;   // write the merged line into the first object
                                self.doc.objs.remove(i);   // drop the second; fi's content is already updated
                                self.doc.commit();
                                self.clear_refs();
                                self.end_cmd();
                            } else { self.prompt = "Those lines aren't collinear.".into(); self.cmd = Cmd::Join { first: None }; }
                        } else { self.cmd = Cmd::Join { first: Some(fi) }; }
                    } else { self.cmd = Cmd::Join { first: Some(i) }; self.prompt = "Select second line to join".into(); }
                } else { self.cmd = Cmd::Join { first }; }
            }
            Cmd::Point => {
                self.doc.add(Entity::Point { x: p.0, y: p.1 });
                self.doc.commit();
                self.cmd = Cmd::Point; // AutoCAD-style: keep placing until Esc
                self.prompt = "Specify a point (Esc to finish)".into();
            }
            Cmd::Erase => {
                if let Some(i) = self.hit(p) { self.doc.objs.remove(i); self.doc.commit(); self.clear_refs(); }
                self.cmd = Cmd::Erase;
            }
            Cmd::Move { base } => {
                if let Some(b) = base {
                    let (dx, dy) = (p.0 - b.0, p.1 - b.1);
                    let ids: Vec<usize> = self.sel.iter().copied().collect();
                    for i in ids { if i < self.doc.objs.len() { self.doc.objs[i].e.translate(dx, dy); } }
                    self.doc.commit();
                    self.end_cmd();
                } else {
                    self.cmd = Cmd::Move { base: Some(p) };
                    self.prompt = "Specify destination".into();
                }
            }
            Cmd::Copy { base } => {
                if let Some(b) = base {
                    let (dx, dy) = (p.0 - b.0, p.1 - b.1);
                    let ids: Vec<usize> = self.sel.iter().copied().collect();
                    for i in ids { if i < self.doc.objs.len() { let mut o = self.doc.objs[i].clone(); o.e.translate(dx, dy); self.doc.objs.push(o); } }
                    self.doc.commit();
                    self.cmd = Cmd::Copy { base: Some(b) }; // keep copying from same base
                    self.prompt = "Specify next destination (Esc to finish)".into();
                } else {
                    self.cmd = Cmd::Copy { base: Some(p) };
                    self.prompt = "Specify destination".into();
                }
            }
        }
    }

    fn feed_value(&mut self, v: f64) {
        let cmd = std::mem::replace(&mut self.cmd, Cmd::None);
        match cmd {
            Cmd::Circle { c: Some(ct) } => {
                self.doc.add(Entity::Circle { cx: ct.0, cy: ct.1, r: v.max(1.0) });
                self.doc.commit();
                self.end_cmd();
            }
            Cmd::Polygon { sides: None, center } => {
                self.cmd = Cmd::Polygon { sides: Some(v.max(3.0).min(1000.0) as usize), center };
                self.prompt = "Specify center of polygon".into();
            }
            Cmd::Rotate { base: Some(b) } => {
                let ang = v.to_radians();
                for i in self.sel.iter().copied().collect::<Vec<_>>() { if i < self.doc.objs.len() { self.doc.objs[i].e.rotate(b.0, b.1, ang); } }
                self.doc.commit();
                self.end_cmd();
            }
            Cmd::Scale { base: Some(b) } => {
                if v.is_finite() && v > 0.0 {
                    for i in self.sel.iter().copied().collect::<Vec<_>>() { if i < self.doc.objs.len() { self.doc.objs[i].e.scale(b.0, b.1, v); } }
                    self.doc.commit();
                    self.end_cmd();
                } else {
                    self.cmd = Cmd::Scale { base: Some(b) };
                    self.prompt = "Enter a positive scale factor".into();
                }
            }
            Cmd::Offset { dist: None, ent } => {
                self.cmd = Cmd::Offset { dist: Some(v.max(0.0)), ent };
                self.prompt = "Select object to offset".into();
            }
            Cmd::Fillet { first } => {
                self.fillet_r = v.max(0.0);
                self.cmd = Cmd::Fillet { first };
                self.prompt = format!("Fillet radius {} — select first line", self.fillet_r);
            }
            Cmd::Chamfer { first } => {
                self.chamfer_d = v.max(0.0);
                self.cmd = Cmd::Chamfer { first };
                self.prompt = format!("Chamfer distance {} — select first line", self.chamfer_d);
            }
            Cmd::Ellipse { center: Some(ct), major: Some(mj) } => {
                self.doc.add(ellipse(ct, mj, v.max(1.0)));
                self.doc.commit();
                self.end_cmd();
            }
            Cmd::Text { pos: Some(pp), height: None } => {
                self.cmd = Cmd::Text { pos: Some(pp), height: Some(v.max(1.0)) };
                self.prompt = "Enter text, then press Enter".into();
            }
            Cmd::ArrayRect { rows, cols, dy } => {
                if rows.is_none() {
                    self.cmd = Cmd::ArrayRect { rows: Some((v.max(1.0)).min(10000.0) as usize), cols, dy };
                    self.prompt = "Enter number of columns".into();
                } else if cols.is_none() {
                    self.cmd = Cmd::ArrayRect { rows, cols: Some((v.max(1.0)).min(10000.0) as usize), dy };
                    self.prompt = "Enter row spacing (distance between rows)".into();
                } else if dy.is_none() {
                    self.cmd = Cmd::ArrayRect { rows, cols, dy: Some(v) };
                    self.prompt = "Enter column spacing (distance between columns)".into();
                } else {
                    let (r, c, ry, cx) = (rows.unwrap(), cols.unwrap(), dy.unwrap(), v);
                    let per = r.saturating_mul(c).saturating_sub(1); // r0c0 (the original) is not copied
                    self.array_apply(per, |e| array_rect(std::slice::from_ref(e), r, c, cx, ry));
                }
            }
            Cmd::ArrayPolar { center: Some(ct), count } => {
                if count.is_none() {
                    self.cmd = Cmd::ArrayPolar { center: Some(ct), count: Some((v.max(1.0)).min(10000.0) as usize) };
                    self.prompt = "Enter fill angle in degrees (360 for a full circle)".into();
                } else {
                    let cnt = count.unwrap();
                    // 0 means a full circle; otherwise clamp magnitude to one turn (sign = direction).
                    let fill = if v.abs() < 1e-6 { 360.0 } else { v.clamp(-360.0, 360.0) };
                    self.array_apply(cnt.saturating_sub(1), move |e| array_polar(std::slice::from_ref(e), ct, cnt, fill));
                }
            }
            other => { self.cmd = other; self.prompt = "(a point is expected here)".into(); }
        }
    }

    fn cmd_enter(&mut self) {
        let cmd = std::mem::replace(&mut self.cmd, Cmd::None);
        if let Cmd::Pline { pts } = cmd {
            if pts.len() > 1 {
                self.doc.add(Entity::Poly { pts, closed: false });
                self.doc.commit();
            }
        }
        self.end_cmd();
    }

    fn end_cmd(&mut self) {
        self.cmd = Cmd::None;
        self.prompt = "Type a command".into();
    }

    fn wants_string(&self) -> bool { matches!(self.cmd, Cmd::Text { pos: Some(_), height: Some(_) }) }
    fn feed_string(&mut self, s: &str) {
        let cmd = std::mem::replace(&mut self.cmd, Cmd::None);
        if let Cmd::Text { pos: Some(pp), height: Some(h) } = cmd {
            self.doc.add(Entity::Text { x: pp.0, y: pp.1, h, s: s.to_string() });
            self.doc.commit();
        }
        self.end_cmd();
    }

    fn submit(&mut self, t: &str) {
        let t = t.trim();
        if !matches!(self.cmd, Cmd::None) {
            if self.wants_string() { if t.is_empty() { self.end_cmd(); } else { self.feed_string(t); } return; }
            if t.is_empty() { self.cmd_enter(); return; }
            if let Some(p) = parse_coord(t, self.base()) { self.feed_point(p); return; } // typed coords are exact; no ortho
            if let Ok(v) = t.parse::<f64>() {
                if v.is_finite() && v.abs() < 1e12 { self.feed_value(v); } else { self.prompt = "Value out of range.".into(); }
                return;
            }
            self.prompt = "Invalid point or value — try again.".into();
            return;
        }
        if t.is_empty() {
            if !self.last.is_empty() { let l = self.last.clone(); self.start_cmd(&l); }
            return;
        }
        self.start_cmd(t);
    }

    fn start_cmd(&mut self, text: &str) {
        let name = text.split_whitespace().next().unwrap_or("").to_uppercase();
        let canon = alias(&name).to_string();
        match canon.as_str() {
            "LINE" => { self.cmd = Cmd::Line { prev: None }; self.prompt = "Specify first point".into(); }
            "PLINE" => { self.cmd = Cmd::Pline { pts: vec![] }; self.prompt = "Specify start point".into(); }
            "RECTANG" => { self.cmd = Cmd::Rect { a: None }; self.prompt = "Specify first corner".into(); }
            "CIRCLE" => { self.cmd = Cmd::Circle { c: None }; self.prompt = "Specify center point".into(); }
            "ARC" => { self.cmd = Cmd::Arc { pts: vec![] }; self.prompt = "Specify start point of arc".into(); }
            "POLYGON" => { self.cmd = Cmd::Polygon { sides: None, center: None }; self.prompt = "Enter number of sides".into(); }
            "ROTATE" => {
                if self.sel.is_empty() { self.prompt = "Select object(s) first, then ROTATE".into(); }
                else { self.cmd = Cmd::Rotate { base: None }; self.prompt = "Specify base point".into(); }
            }
            "SCALE" => {
                if self.sel.is_empty() { self.prompt = "Select object(s) first, then SCALE".into(); }
                else { self.cmd = Cmd::Scale { base: None }; self.prompt = "Specify base point".into(); }
            }
            "MIRROR" => {
                if self.sel.is_empty() { self.prompt = "Select object(s) first, then MIRROR".into(); }
                else { self.cmd = Cmd::Mirror { p1: None }; self.prompt = "Specify first point of mirror line".into(); }
            }
            "OFFSET" => { self.cmd = Cmd::Offset { dist: None, ent: None }; self.prompt = "Specify offset distance".into(); }
            "TRIM" => { self.cmd = Cmd::Trim; self.prompt = "Click the part of a line to trim (Esc to finish)".into(); }
            "EXTEND" => { self.cmd = Cmd::Extend; self.prompt = "Click a line near the end to extend (Esc to finish)".into(); }
            "FILLET" => { self.cmd = Cmd::Fillet { first: None }; self.prompt = format!("Select first line (radius {}; type a number to change)", self.fillet_r); }
            "CHAMFER" => { self.cmd = Cmd::Chamfer { first: None }; self.prompt = format!("Select first line (distance {}; type a number to change)", self.chamfer_d); }
            "ELLIPSE" => { self.cmd = Cmd::Ellipse { center: None, major: None }; self.prompt = "Specify center of ellipse".into(); }
            "POINT" => { self.cmd = Cmd::Point; self.prompt = "Specify a point (Esc to finish)".into(); }
            "TEXT" => { self.cmd = Cmd::Text { pos: None, height: None }; self.prompt = "Specify start point of text".into(); }
            "DIMLINEAR" => { self.cmd = Cmd::DimLinear { pts: vec![] }; self.prompt = "Specify first extension line origin".into(); }
            "ARRAYRECT" => {
                if self.sel.is_empty() { self.prompt = "Select object(s) first, then ARRAY".into(); }
                else { self.cmd = Cmd::ArrayRect { rows: None, cols: None, dy: None }; self.prompt = "Rectangular array — enter number of rows".into(); }
            }
            "ARRAYPOLAR" => {
                if self.sel.is_empty() { self.prompt = "Select object(s) first, then ARRAYPOLAR".into(); }
                else { self.cmd = Cmd::ArrayPolar { center: None, count: None }; self.prompt = "Polar array — specify center point".into(); }
            }
            "BREAK" => { self.cmd = Cmd::Break { ent: None, p1: None }; self.prompt = "Select the line to break".into(); }
            "JOIN" => { self.cmd = Cmd::Join { first: None }; self.prompt = "Select first line to join".into(); }
            "EXPLODE" => {
                if self.sel.is_empty() { self.prompt = "Select object(s) first, then EXPLODE".into(); }
                else {
                    let mut ids: Vec<usize> = self.sel.iter().copied().filter(|&i| i < self.doc.objs.len()).collect();
                    ids.sort_unstable(); ids.reverse(); // remove high-to-low so indices stay valid
                    let mut n = 0;
                    for i in ids {
                        if let Some(parts) = explode_entity(&self.doc.objs[i].e) {
                            let layer = self.doc.objs[i].layer.clone();
                            self.doc.objs.remove(i);
                            for e in parts { self.doc.add_on(e, &layer); }
                            n += 1;
                        }
                    }
                    self.clear_refs();
                    if n > 0 { self.doc.commit(); self.prompt = format!("Exploded {} object(s)", n); }
                    else { self.prompt = "Nothing there explodes (rect/polyline/dim only)".into(); }
                }
            }
            "ERASE" => {
                if !self.sel.is_empty() {
                    let mut ids: Vec<usize> = self.sel.iter().copied().collect();
                    ids.sort_unstable(); ids.reverse();
                    for i in ids { self.doc.objs.remove(i); }
                    self.sel.clear(); self.doc.commit(); self.prompt = "Type a command".into();
                } else { self.cmd = Cmd::Erase; self.prompt = "Click objects to erase (Esc to finish)".into(); }
            }
            "MOVE" => {
                if self.sel.is_empty() { self.prompt = "Select object(s) first, then MOVE".into(); }
                else { self.cmd = Cmd::Move { base: None }; self.prompt = "Specify base point".into(); }
            }
            "COPY" => {
                if self.sel.is_empty() { self.prompt = "Select object(s) first, then COPY".into(); }
                else { self.cmd = Cmd::Copy { base: None }; self.prompt = "Specify base point".into(); }
            }
            "UNDO" => { self.do_undo(); self.prompt = "Undo".into(); }
            "REDO" => { self.do_redo(); self.prompt = "Redo".into(); }
            "ZOOM" => { self.zoom_extents(); self.prompt = "Zoom extents".into(); }
            "" => {}
            _ => { self.prompt = format!("Unknown command: {}", name); return; }
        }
        if !canon.is_empty() { self.last = canon; }
    }

    fn zoom_extents(&mut self) {
        if self.doc.objs.is_empty() { return; }
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for ob in &self.doc.objs {
            for s in ob.e.snaps() {
                x0 = x0.min(s.0); y0 = y0.min(s.1); x1 = x1.max(s.0); y1 = y1.max(s.1);
            }
        }
        self.view.x = (x0 + x1) / 2.0;
        self.view.y = (y0 + y1) / 2.0;
        let sx = self.vp.0 as f64 / ((x1 - x0).abs() + 500.0);
        let sy = self.vp.1 as f64 / ((y1 - y0).abs() + 500.0);
        self.view.s = (sx.min(sy) * 0.85).max(0.002);
    }

    fn canvas(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (resp, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = resp.rect;
        let c = rect.center();
        self.vp = (rect.width(), rect.height());

        if resp.dragged_by(egui::PointerButton::Middle) {
            let d = resp.drag_delta();
            self.view.x -= d.x as f64 / self.view.s;
            self.view.y += d.y as f64 / self.view.s;
        }
        let scroll = ctx.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            if let Some(mp) = resp.hover_pos() {
                let before = self.view.to_world(mp, c);
                let f = if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 };
                self.view.s = (self.view.s * f).clamp(0.002, 40.0);
                let after = self.view.to_world(mp, c);
                self.view.x += before.0 - after.0;
                self.view.y += before.1 - after.1;
            }
        }
        if let Some(mp) = resp.hover_pos() {
            let w = self.view.to_world(mp, c);
            let excl = self.grip_drag.map(|(i, _)| i);
            let (sp, k) = self.snap(w, excl);
            // an object snap wins over ortho, so the point matches the drawn marker
            self.cur = if k.is_some() { sp } else { self.ortho_pt(sp) };
            self.snap_marker = k.map(|kk| (sp, kk));
        } else {
            self.snap_marker = None;
        }
        if resp.clicked() {
            if let Some(mp) = resp.interact_pointer_pos() {
                let w = self.view.to_world(mp, c);
                let (sp, k) = self.snap(w, None);
                let p = if k.is_some() { sp } else { self.ortho_pt(sp) };
                self.feed_point(p);
            }
        }
        // primary-button drag with no active command: grips (on a selected entity) or a selection box
        let prim = resp.dragged_by(egui::PointerButton::Primary) && matches!(self.cmd, Cmd::None);
        if prim {
            if self.grip_drag.is_none() && self.sel_box.is_none() {
                if let Some(gd) = self.grip_at(self.cur) { self.grip_drag = Some(gd); }
                else { self.sel_box = Some((self.cur, self.cur)); }
            } else if let Some((idx, g)) = self.grip_drag {
                if idx < self.doc.objs.len() { apply_grip(&mut self.doc.objs[idx].e, g, self.cur); }
            } else if let Some((s, _)) = self.sel_box {
                self.sel_box = Some((s, self.cur));
            }
        } else {
            if self.grip_drag.take().is_some() { self.doc.commit(); }
            if let Some((s, e)) = self.sel_box.take() { self.finalize_box(s, e); }
        }

        if self.grid { draw_grid(&painter, &self.view, rect, c); }
        // axes
        let ax = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(130, 150, 170, 70));
        painter.line_segment([Pos2::new(rect.left(), self.view.to_screen((0.0, 0.0), c).y),
                              Pos2::new(rect.right(), self.view.to_screen((0.0, 0.0), c).y)], ax);
        painter.line_segment([Pos2::new(self.view.to_screen((0.0, 0.0), c).x, rect.top()),
                              Pos2::new(self.view.to_screen((0.0, 0.0), c).x, rect.bottom())], ax);

        for (i, ob) in self.doc.objs.iter().enumerate() {
            let l = self.doc.layer(&ob.layer);
            if !l.on || l.frozen { continue; }
            let col = if self.sel.contains(&i) {
                Color32::from_rgb(55, 212, 232)
            } else if let Some(cc) = l.color {
                Color32::from_rgb(cc[0], cc[1], cc[2])
            } else {
                Color32::from_rgb(205, 217, 229)
            };
            draw_entity(&painter, &self.view, c, &ob.e, col);
        }
        self.draw_preview(&painter, c);
        if matches!(self.cmd, Cmd::None) { self.draw_grips(&painter, c); }
        if let Some((s, e)) = self.sel_box { draw_box(&painter, &self.view, c, s, e); }
        if let Some((pt, k)) = self.snap_marker { draw_snap(&painter, self.view.to_screen(pt, c), k); }

        if resp.hovered() { ctx.request_repaint(); }
    }

    fn draw_grips(&self, p: &egui::Painter, c: Pos2) {
        let col = Color32::from_rgb(55, 212, 232);
        for &i in &self.sel {
            if i >= self.doc.objs.len() { continue; }
            for (gp, _) in grips_of(&self.doc.objs[i].e) {
                let s = self.view.to_screen(gp, c);
                p.rect_filled(egui::Rect::from_center_size(s, egui::vec2(7.0, 7.0)), egui::Rounding::ZERO, col);
            }
        }
    }

    fn draw_preview(&self, p: &egui::Painter, c: Pos2) {
        let st = Stroke::new(1.3_f32, Color32::from_rgb(55, 212, 232));
        let seg = |a: Pt, b: Pt| p.line_segment([self.view.to_screen(a, c), self.view.to_screen(b, c)], st);
        match &self.cmd {
            Cmd::Line { prev: Some(pr) } => { seg(*pr, self.cur); }
            Cmd::Pline { pts } => { if let Some(l) = pts.last() { seg(*l, self.cur); } }
            Cmd::Rect { a: Some(a0) } => {
                let (p0, cu) = (*a0, self.cur);
                seg(p0, (cu.0, p0.1)); seg((cu.0, p0.1), cu); seg(cu, (p0.0, cu.1)); seg((p0.0, cu.1), p0);
            }
            Cmd::Circle { c: Some(ct) } => {
                let r = (self.cur.0 - ct.0).hypot(self.cur.1 - ct.1).max(1.0);
                draw_arc(p, &self.view, c, ct.0, ct.1, r, 0.0, TAU, st);
            }
            Cmd::Arc { pts } => {
                for w in pts.windows(2) { seg(w[0], w[1]); }
                if let Some(l) = pts.last() { seg(*l, self.cur); }
            }
            Cmd::Polygon { center: Some(ct), .. } => {
                let r = (self.cur.0 - ct.0).hypot(self.cur.1 - ct.1).max(1.0);
                draw_arc(p, &self.view, c, ct.0, ct.1, r, 0.0, TAU, st);
            }
            Cmd::Mirror { p1: Some(a) } => { seg(*a, self.cur); }
            Cmd::DimLinear { pts } => { if let Some(l) = pts.last() { seg(*l, self.cur); } }
            Cmd::Ellipse { center: Some(ct), major: None } => { seg(*ct, self.cur); }
            _ => {}
        }
    }
}

/// A compact ribbon tool button (label + hover tooltip). Returns whether it was clicked.
fn tool_btn(ui: &mut egui::Ui, label: &str, tip: &str) -> bool {
    ui.add(egui::Button::new(egui::RichText::new(label).size(12.0)).min_size(egui::vec2(36.0, 30.0)))
        .on_hover_text(tip)
        .clicked()
}

/// Paint a small vector icon for a command inside `r` (AutoCAD-style line glyphs).
fn paint_icon(p: &egui::Painter, r: egui::Rect, name: &str, col: Color32) {
    use std::f32::consts::{FRAC_PI_2, TAU};
    let st = Stroke::new(1.5_f32, col);
    let m = |u: f32, v: f32| Pos2::new(r.min.x + u * r.width(), r.min.y + v * r.height());
    let line = |a: (f32, f32), b: (f32, f32)| { p.line_segment([m(a.0, a.1), m(b.0, b.1)], st); };
    let poly = |pts: &[(f32, f32)], closed: bool| {
        for w in pts.windows(2) { p.line_segment([m(w[0].0, w[0].1), m(w[1].0, w[1].1)], st); }
        if closed && pts.len() > 1 { let a = pts[pts.len() - 1]; let b = pts[0]; p.line_segment([m(a.0, a.1), m(b.0, b.1)], st); }
    };
    let circ = |c: (f32, f32), rad: f32| { p.circle_stroke(m(c.0, c.1), rad * r.width(), st); };
    let arc = |c: (f32, f32), rad: f32, a0: f32, a1: f32| {
        let n = 12; let pts: Vec<Pos2> = (0..=n).map(|i| { let t = a0 + (a1 - a0) * i as f32 / n as f32; m(c.0 + rad * t.cos(), c.1 - rad * t.sin()) }).collect();
        for w in pts.windows(2) { p.line_segment([w[0], w[1]], st); }
    };
    match name {
        "LINE" => line((0.12, 0.88), (0.88, 0.12)),
        "PLINE" => poly(&[(0.1, 0.8), (0.4, 0.3), (0.6, 0.7), (0.9, 0.2)], false),
        "CIRCLE" => circ((0.5, 0.5), 0.4),
        "ARC" => arc((0.5, 0.6), 0.4, 0.2, 2.9),
        "RECTANG" => poly(&[(0.14, 0.26), (0.86, 0.26), (0.86, 0.74), (0.14, 0.74)], true),
        "POLYGON" => { let pts: Vec<(f32, f32)> = (0..6).map(|i| { let a = TAU * i as f32 / 6.0 - FRAC_PI_2; (0.5 + 0.38 * a.cos(), 0.5 + 0.38 * a.sin()) }).collect(); poly(&pts, true); }
        "ELLIPSE" => { let pts: Vec<Pos2> = (0..=16).map(|i| { let t = TAU * i as f32 / 16.0; m(0.5 + 0.42 * t.cos(), 0.5 + 0.28 * t.sin()) }).collect(); for w in pts.windows(2) { p.line_segment([w[0], w[1]], st); } }
        "POINT" => { line((0.5, 0.22), (0.5, 0.78)); line((0.22, 0.5), (0.78, 0.5)); }
        "TEXT" => { line((0.26, 0.84), (0.5, 0.16)); line((0.5, 0.16), (0.74, 0.84)); line((0.36, 0.56), (0.64, 0.56)); }
        "MOVE" => { line((0.5, 0.14), (0.5, 0.86)); line((0.14, 0.5), (0.86, 0.5)); line((0.5, 0.14), (0.42, 0.26)); line((0.5, 0.14), (0.58, 0.26)); line((0.14, 0.5), (0.26, 0.42)); line((0.14, 0.5), (0.26, 0.58)); }
        "COPY" => { poly(&[(0.14, 0.36), (0.6, 0.36), (0.6, 0.82), (0.14, 0.82)], true); poly(&[(0.4, 0.16), (0.86, 0.16), (0.86, 0.62), (0.4, 0.62)], true); }
        "ROTATE" => { arc((0.5, 0.5), 0.36, 0.6, 5.0); line((0.86, 0.46), (0.82, 0.58)); line((0.86, 0.46), (0.74, 0.5)); }
        "MIRROR" => { poly(&[(0.12, 0.8), (0.4, 0.8), (0.12, 0.2)], true); poly(&[(0.88, 0.8), (0.6, 0.8), (0.88, 0.2)], true); p.line_segment([m(0.5, 0.08), m(0.5, 0.92)], Stroke::new(1.0_f32, col)); }
        "SCALE" => { poly(&[(0.2, 0.6), (0.5, 0.6), (0.5, 0.9), (0.2, 0.9)], true); poly(&[(0.2, 0.2), (0.82, 0.2), (0.82, 0.82)], false); }
        "OFFSET" => { poly(&[(0.2, 0.74), (0.5, 0.26), (0.8, 0.74)], false); poly(&[(0.32, 0.84), (0.5, 0.52), (0.68, 0.84)], false); }
        "TRIM" => { line((0.14, 0.52), (0.86, 0.52)); line((0.56, 0.18), (0.72, 0.82)); }
        "EXTEND" => { line((0.14, 0.5), (0.64, 0.5)); line((0.64, 0.5), (0.54, 0.42)); line((0.64, 0.5), (0.54, 0.58)); line((0.86, 0.18), (0.86, 0.82)); }
        "FILLET" => { line((0.22, 0.18), (0.22, 0.5)); arc((0.52, 0.5), 0.3, FRAC_PI_2, std::f32::consts::PI); line((0.52, 0.8), (0.82, 0.8)); }
        "CHAMFER" => { line((0.2, 0.8), (0.5, 0.8)); line((0.5, 0.8), (0.8, 0.5)); line((0.8, 0.5), (0.8, 0.2)); }
        "ARRAYRECT" => { for rr in 0..3 { for cc in 0..3 { circ((0.26 + cc as f32 * 0.24, 0.26 + rr as f32 * 0.24), 0.035); } } }
        "ARRAYPOLAR" => { for i in 0..6 { let a = TAU * i as f32 / 6.0; circ((0.5 + 0.32 * a.cos(), 0.5 + 0.32 * a.sin()), 0.05); } }
        "BREAK" => { line((0.14, 0.5), (0.42, 0.5)); line((0.58, 0.5), (0.86, 0.5)); }
        "JOIN" => { line((0.14, 0.62), (0.46, 0.62)); line((0.54, 0.38), (0.86, 0.38)); line((0.46, 0.62), (0.54, 0.38)); }
        "EXPLODE" => { circ((0.5, 0.5), 0.08); for i in 0..6 { let a = TAU * i as f32 / 6.0; line((0.5 + 0.2 * a.cos(), 0.5 + 0.2 * a.sin()), (0.5 + 0.42 * a.cos(), 0.5 + 0.42 * a.sin())); } }
        "ERASE" => { poly(&[(0.24, 0.6), (0.54, 0.3), (0.76, 0.52), (0.46, 0.82)], true); line((0.46, 0.82), (0.82, 0.82)); }
        "DIMLINEAR" => { line((0.14, 0.4), (0.14, 0.82)); line((0.86, 0.4), (0.86, 0.82)); line((0.14, 0.6), (0.86, 0.6)); line((0.14, 0.6), (0.26, 0.54)); line((0.14, 0.6), (0.26, 0.66)); line((0.86, 0.6), (0.74, 0.54)); line((0.86, 0.6), (0.74, 0.66)); }
        _ => circ((0.5, 0.5), 0.3),
    }
}

/// A ribbon icon button: vector glyph above a small label, hover highlight + tooltip.
fn icon_btn(ui: &mut egui::Ui, name: &str, label: &str, tip: &str) -> bool {
    let size = egui::vec2(48.0, 50.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let p = ui.painter();
    if resp.hovered() { p.rect_filled(rect, egui::Rounding::same(5.0), Color32::from_rgb(42, 54, 68)); }
    let ic = egui::Rect::from_min_size(Pos2::new(rect.center().x - 11.0, rect.min.y + 6.0), egui::vec2(22.0, 22.0));
    paint_icon(p, ic, name, Color32::from_rgb(214, 224, 236));
    p.text(Pos2::new(rect.center().x, rect.max.y - 8.0), Align2::CENTER_CENTER, label, FontId::proportional(8.5), Color32::from_rgb(168, 182, 198));
    resp.on_hover_text(tip).clicked()
}

fn draw_grid(p: &egui::Painter, v: &View, rect: egui::Rect, c: Pos2) {
    let tl = v.to_world(rect.left_top(), c);
    let br = v.to_world(rect.right_bottom(), c);
    let (minx, maxx) = (tl.0.min(br.0), tl.0.max(br.0));
    let (miny, maxy) = (tl.1.min(br.1), tl.1.max(br.1));
    // guard against non-finite / absurd view bounds (would make the grid loops hang)
    if ![minx, maxx, miny, maxy].iter().all(|v| v.is_finite()) || (maxx - minx).max(maxy - miny) > 1e8 { return; }
    let minor = Stroke::new(0.5_f32, Color32::from_rgba_unmultiplied(130, 150, 170, 18));
    let major = Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(130, 150, 170, 44));
    let draw = |step: f64, st: Stroke| {
        let mut x = (minx / step).ceil() * step;
        while x <= maxx { let a = v.to_screen((x, miny), c); let b = v.to_screen((x, maxy), c); p.line_segment([a, b], st); x += step; }
        let mut y = (miny / step).ceil() * step;
        while y <= maxy { let a = v.to_screen((minx, y), c); let b = v.to_screen((maxx, y), c); p.line_segment([a, b], st); y += step; }
    };
    if 100.0 * v.s >= 5.0 { draw(100.0, minor); }
    draw(1000.0, major);
}

fn draw_arc(p: &egui::Painter, v: &View, c: Pos2, cx: f64, cy: f64, r: f64, a0: f64, a1: f64, st: Stroke) {
    let n = ((a1 - a0).abs() / std::f64::consts::TAU * 72.0).max(14.0).min(4096.0) as usize;
    let mut prev: Option<Pos2> = None;
    for i in 0..=n {
        let t = a0 + (a1 - a0) * i as f64 / n as f64;
        let pt = v.to_screen((cx + r * t.cos(), cy + r * t.sin()), c);
        if let Some(pp) = prev { p.line_segment([pp, pt], st); }
        prev = Some(pt);
    }
}

fn draw_box(p: &egui::Painter, v: &View, c: Pos2, s: Pt, e: Pt) {
    let crossing = e.0 < s.0;
    let col = if crossing { Color32::from_rgb(84, 217, 140) } else { Color32::from_rgb(55, 212, 232) };
    let rect = egui::Rect::from_two_pos(v.to_screen(s, c), v.to_screen(e, c));
    p.rect_filled(rect, egui::Rounding::ZERO, Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), 28));
    let st = Stroke::new(1.0_f32, col);
    p.line_segment([rect.left_top(), rect.right_top()], st);
    p.line_segment([rect.right_top(), rect.right_bottom()], st);
    p.line_segment([rect.right_bottom(), rect.left_bottom()], st);
    p.line_segment([rect.left_bottom(), rect.left_top()], st);
}

fn draw_ring(p: &egui::Painter, c: Pos2, r: f32, st: Stroke) {
    let n = 12;
    let mut prev: Option<Pos2> = None;
    for i in 0..=n {
        let t = std::f32::consts::TAU * i as f32 / n as f32;
        let pt = Pos2::new(c.x + r * t.cos(), c.y + r * t.sin());
        if let Some(pp) = prev { p.line_segment([pp, pt], st); }
        prev = Some(pt);
    }
}

fn draw_snap(p: &egui::Painter, s: Pos2, k: SnapKind) {
    let col = Color32::from_rgb(255, 180, 84);
    let st = Stroke::new(1.6_f32, col);
    let r = 5.0_f32;
    match k {
        SnapKind::End => {
            let rect = egui::Rect::from_center_size(s, egui::vec2(2.0 * r, 2.0 * r));
            p.line_segment([rect.left_top(), rect.right_top()], st);
            p.line_segment([rect.right_top(), rect.right_bottom()], st);
            p.line_segment([rect.right_bottom(), rect.left_bottom()], st);
            p.line_segment([rect.left_bottom(), rect.left_top()], st);
        }
        SnapKind::Mid => {
            p.line_segment([Pos2::new(s.x - r, s.y + r), Pos2::new(s.x, s.y - r)], st);
            p.line_segment([Pos2::new(s.x, s.y - r), Pos2::new(s.x + r, s.y + r)], st);
            p.line_segment([Pos2::new(s.x + r, s.y + r), Pos2::new(s.x - r, s.y + r)], st);
        }
        SnapKind::Center | SnapKind::Quad => draw_ring(p, s, r, st),
        SnapKind::Inter => {
            p.line_segment([Pos2::new(s.x - r, s.y - r), Pos2::new(s.x + r, s.y + r)], st);
            p.line_segment([Pos2::new(s.x + r, s.y - r), Pos2::new(s.x - r, s.y + r)], st);
        }
        SnapKind::Near => {
            p.line_segment([Pos2::new(s.x - r, s.y - r), Pos2::new(s.x + r, s.y + r)], st);
            p.line_segment([Pos2::new(s.x - r, s.y + r), Pos2::new(s.x + r, s.y - r)], st);
        }
    }
}

fn draw_entity(p: &egui::Painter, v: &View, c: Pos2, e: &Entity, col: Color32) {
    let st = Stroke::new(1.4_f32, col);
    let seg = |a: Pt, b: Pt| p.line_segment([v.to_screen(a, c), v.to_screen(b, c)], st);
    match e {
        Entity::Line { x1, y1, x2, y2 } => { seg((*x1, *y1), (*x2, *y2)); }
        Entity::Dim { x1, y1, x2, y2, vertical, off } => {
            let [dimline, e1, e2] = dim_geom(*x1, *y1, *x2, *y2, *vertical, *off);
            seg(dimline.0, dimline.1); seg(e1.0, e1.1); seg(e2.0, e2.1);
            let len = if *vertical { (y2 - y1).abs() } else { (x2 - x1).abs() };
            let m = v.to_screen(((dimline.0.0 + dimline.1.0) / 2.0, (dimline.0.1 + dimline.1.1) / 2.0), c);
            let txt = if len >= 1000.0 { format!("{:.2} m", len / 1000.0) } else { format!("{:.0}", len) };
            p.text(Pos2::new(m.x, m.y - 4.0), Align2::CENTER_BOTTOM, txt, FontId::monospace(12.0), col);
        }
        Entity::Rect { x, y, w, h } => {
            seg((*x, *y), (*x + *w, *y)); seg((*x + *w, *y), (*x + *w, *y + *h));
            seg((*x + *w, *y + *h), (*x, *y + *h)); seg((*x, *y + *h), (*x, *y));
        }
        Entity::Poly { pts, closed } => {
            let n = pts.len();
            if n >= 2 { for i in 0..(if *closed { n } else { n - 1 }) { seg(pts[i], pts[(i + 1) % n]); } }
        }
        Entity::Circle { cx, cy, r } => draw_arc(p, v, c, *cx, *cy, *r, 0.0, std::f64::consts::TAU, st),
        Entity::Arc { cx, cy, r, a0, a1 } => draw_arc(p, v, c, *cx, *cy, *r, *a0, *a1, st),
        Entity::Point { x, y } => {
            let s = v.to_screen((*x, *y), c);
            p.line_segment([Pos2::new(s.x - 4.0, s.y), Pos2::new(s.x + 4.0, s.y)], st);
            p.line_segment([Pos2::new(s.x, s.y - 4.0), Pos2::new(s.x, s.y + 4.0)], st);
        }
        Entity::Text { x, y, h, s } => {
            let pos = v.to_screen((*x, *y), c);
            p.text(pos, Align2::LEFT_BOTTOM, s, FontId::proportional((*h * v.s).max(9.0) as f32), col);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.input(|i| {
            if i.key_pressed(egui::Key::Escape) { self.cmd = Cmd::None; self.sel.clear(); self.prompt = "Type a command".into(); }
            if i.modifiers.command && i.key_pressed(egui::Key::Z) {
                if i.modifiers.shift { self.do_redo(); } else { self.do_undo(); }
            }
            if i.modifiers.command && i.key_pressed(egui::Key::Y) { self.do_redo(); }
        });

        // drag-and-drop a .dxf file to open it
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for path in dropped {
            if path.extension().map_or(false, |e| e.eq_ignore_ascii_case("dxf")) {
                match std::fs::read(&path) {
                    Ok(bytes) => { self.import_bytes(&bytes); self.prompt = format!("Imported {} ({} entities)", path.display(), self.doc.objs.len()); }
                    Err(e) => { self.prompt = format!("Open failed: {}", e); }
                }
            }
        }

        let draw_tools: &[(&'static str, &'static str, &'static str)] = &[
            ("Line", "LINE", "Line  (L)"), ("Pline", "PLINE", "Polyline  (PL)"), ("Circle", "CIRCLE", "Circle  (C)"),
            ("Arc", "ARC", "Arc, 3-point  (A)"), ("Rect", "RECTANG", "Rectangle  (REC)"), ("Polygon", "POLYGON", "Polygon  (POL)"),
            ("Ellipse", "ELLIPSE", "Ellipse  (EL)"), ("Point", "POINT", "Point  (PO)"),
        ];
        let modify_tools: &[(&'static str, &'static str, &'static str)] = &[
            ("Move", "MOVE", "Move  (M) — select first"), ("Copy", "COPY", "Copy  (CO)"), ("Rotate", "ROTATE", "Rotate  (RO)"),
            ("Mirror", "MIRROR", "Mirror  (MI)"), ("Scale", "SCALE", "Scale  (SC)"), ("Offset", "OFFSET", "Offset  (O)"),
            ("Trim", "TRIM", "Trim  (TR)"), ("Extend", "EXTEND", "Extend  (EX)"), ("Fillet", "FILLET", "Fillet  (F)"),
            ("Chamfer", "CHAMFER", "Chamfer  (CHA)"), ("Array", "ARRAYRECT", "Rectangular array  (AR)"),
            ("Polar", "ARRAYPOLAR", "Polar array"), ("Break", "BREAK", "Break  (BR)"), ("Join", "JOIN", "Join  (J)"),
            ("Explode", "EXPLODE", "Explode  (X)"), ("Erase", "ERASE", "Erase  (E)"),
        ];
        let anno_tools: &[(&'static str, &'static str, &'static str)] = &[
            ("Text", "TEXT", "Single-line text  (T)"), ("Linear", "DIMLINEAR", "Linear dimension  (DLI)"),
        ];
        let cap = Color32::from_rgb(128, 146, 166);
        let mut fire: Option<&'static str> = None;

        // ---- menu bar + quick actions ----
        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("New").clicked() { self.confirm = Some(Confirm::New); ui.close_menu(); }
                    if ui.button("Import DXF (opencad.dxf)").clicked() { self.confirm = Some(Confirm::Import); ui.close_menu(); }
                    if ui.button("Export DXF → opencad.dxf").clicked() {
                        self.prompt = match std::fs::write("opencad.dxf", self.doc.to_dxf()) {
                            Ok(_) => "Wrote opencad.dxf".into(), Err(e) => format!("Export failed: {}", e),
                        };
                        ui.close_menu();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui.button("Undo  Ctrl+Z").clicked() { self.do_undo(); ui.close_menu(); }
                    if ui.button("Redo  Ctrl+Y").clicked() { self.do_redo(); ui.close_menu(); }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Zoom Extents").clicked() { self.zoom_extents(); ui.close_menu(); }
                    ui.separator();
                    ui.checkbox(&mut self.grid, "Grid (F7)");
                    ui.checkbox(&mut self.osnap, "Object snap (F3)");
                    ui.checkbox(&mut self.ortho, "Ortho (F8)");
                    ui.checkbox(&mut self.snap_grid, "Snap to grid (F9)");
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("native · Rust").small().color(cap));
                    ui.label(egui::RichText::new("◢ OpenCAD").strong().color(Color32::from_rgb(55, 212, 232)));
                });
            });
        });

        // ---- ribbon tabs ----
        egui::TopBottomPanel::top("ribbontabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in ["Home", "Insert", "Annotate", "View"].iter().enumerate() {
                    if ui.selectable_label(self.ribbon_tab == i, egui::RichText::new(*t).size(13.0)).clicked() { self.ribbon_tab = i; }
                }
            });
        });

        // ---- ribbon content (icon panels for the active tab) ----
        egui::TopBottomPanel::top("ribbon").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let grp = |ui: &mut egui::Ui, title: &str, tools: &[(&'static str, &'static str, &'static str)], fire: &mut Option<&'static str>| {
                    ui.group(|ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| { for (l, c, t) in tools { if icon_btn(ui, c, l, t) { *fire = Some(*c); } } });
                            ui.label(egui::RichText::new(title).size(9.5).color(cap));
                        });
                    });
                };
                match self.ribbon_tab {
                    0 => { grp(ui, "Draw", draw_tools, &mut fire); grp(ui, "Modify", modify_tools, &mut fire); grp(ui, "Annotation", anno_tools, &mut fire); }
                    1 => {
                        ui.group(|ui| { ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                if ui.button("⬇ Import DXF").clicked() { self.confirm = Some(Confirm::Import); }
                                if ui.button("⬆ Export DXF").clicked() {
                                    self.prompt = match std::fs::write("opencad.dxf", self.doc.to_dxf()) { Ok(_) => "Wrote opencad.dxf".into(), Err(e) => format!("Export failed: {}", e) };
                                }
                            });
                            ui.label(egui::RichText::new("Data (DXF)").size(9.5).color(cap));
                        }); });
                        ui.label(egui::RichText::new("…or drag a .dxf onto the window").small().color(cap));
                    }
                    2 => { grp(ui, "Dimensions & Text", anno_tools, &mut fire); }
                    _ => {
                        ui.group(|ui| { ui.vertical(|ui| {
                            ui.horizontal(|ui| { if ui.button("Zoom Extents").clicked() { self.zoom_extents(); } });
                            ui.label(egui::RichText::new("Navigate").size(9.5).color(cap));
                        }); });
                        ui.group(|ui| { ui.vertical(|ui| {
                            ui.horizontal(|ui| { ui.checkbox(&mut self.grid, "Grid"); ui.checkbox(&mut self.osnap, "Osnap"); ui.checkbox(&mut self.ortho, "Ortho"); });
                            ui.label(egui::RichText::new("Display").size(9.5).color(cap));
                        }); });
                    }
                }
            });
            ui.add_space(4.0);
        });
        if let Some(c) = fire { self.start_cmd(c); }

        // ---- status bar (very bottom): coords + the AutoCAD toggle set ----
        egui::TopBottomPanel::bottom("statusbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.monospace(egui::RichText::new(format!("{:.2}, {:.2}, 0.00", self.cur.0, self.cur.1)).color(Color32::from_rgb(200, 212, 226)));
                ui.separator();
                if ui.selectable_label(self.snap_grid, "SNAP").on_hover_text("Snap to grid (F9)").clicked() { self.snap_grid = !self.snap_grid; }
                if ui.selectable_label(self.grid, "GRID").on_hover_text("Grid display (F7)").clicked() { self.grid = !self.grid; }
                if ui.selectable_label(self.ortho, "ORTHO").on_hover_text("Ortho (F8)").clicked() { self.ortho = !self.ortho; }
                if ui.selectable_label(self.polar, "POLAR").on_hover_text("Polar tracking").clicked() { self.polar = !self.polar; }
                if ui.selectable_label(self.osnap, "OSNAP").on_hover_text("Object snap (F3)").clicked() { self.osnap = !self.osnap; }
                if ui.selectable_label(self.lwt, "LWT").on_hover_text("Show lineweight").clicked() { self.lwt = !self.lwt; }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format!("{} objects", self.doc.objs.len())).small().color(Color32::from_rgb(130, 148, 168)));
                });
            });
        });

        // ---- command line ----
        egui::TopBottomPanel::bottom("cmdline").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&self.prompt).monospace().color(Color32::from_rgb(255, 180, 84)));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.cmdline)
                        .desired_width(f32::INFINITY)
                        .hint_text("type a command or coordinate (x,y  @dx,dy  @d<ang)"),
                );
                if r.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let t = std::mem::take(&mut self.cmdline);
                    self.submit(&t);
                    r.request_focus();
                }
            });
        });

        // ---- model / layout tabs ----
        egui::TopBottomPanel::bottom("layouttabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                let _ = ui.selectable_label(true, egui::RichText::new("Model").size(12.0));
                if ui.selectable_label(false, egui::RichText::new("Layout1").size(12.0).color(cap)).clicked() { self.prompt = "Paper-space layouts are on the roadmap.".into(); }
                if ui.selectable_label(false, egui::RichText::new("Layout2").size(12.0).color(cap)).clicked() { self.prompt = "Paper-space layouts are on the roadmap.".into(); }
            });
        });

        egui::SidePanel::right("right").default_width(220.0).show(ctx, |ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("DRAWING").small().strong());
            ui.monospace(format!("Entities  {}", self.doc.objs.len()));
            ui.monospace(format!("Selected  {}", self.sel.len()));
            ui.monospace(format!("Zoom      {:.0}%", self.view.s * 1000.0));
            ui.separator();
            ui.label(egui::RichText::new("LAYERS").small().strong());
            let before: Vec<String> = self.doc.layers.iter().map(|l| l.name.clone()).collect();
            let mut set_current: Option<String> = None;
            let mut to_delete: Option<usize> = None;
            for i in 0..self.doc.layers.len() {
                let is_cur = self.doc.layers[i].name == self.doc.current;
                ui.horizontal(|ui| {
                    let mut rgb = self.doc.layers[i].color.unwrap_or([205, 217, 229]);
                    if ui.color_edit_button_srgb(&mut rgb).changed() { self.doc.layers[i].color = Some(rgb); }
                    // current-layer marker: click to make it current
                    if ui.selectable_label(is_cur, if is_cur { "●" } else { "○" }).clicked() {
                        set_current = Some(self.doc.layers[i].name.clone());
                    }
                    // "0" is the default layer and must always exist: show it read-only (can't rename or delete).
                    if self.doc.layers[i].name == "0" {
                        ui.add_sized([78.0, 18.0], egui::Label::new(egui::RichText::new("0").strong()));
                    } else {
                        ui.add(egui::TextEdit::singleline(&mut self.doc.layers[i].name).desired_width(78.0));
                    }
                    let on = self.doc.layers[i].on;
                    if ui.selectable_label(on, "👁").on_hover_text("on/off").clicked() { self.doc.layers[i].on = !on; }
                    let fz = self.doc.layers[i].frozen;
                    if ui.selectable_label(fz, "❄").on_hover_text("freeze").clicked() { self.doc.layers[i].frozen = !fz; }
                    let lk = self.doc.layers[i].locked;
                    if ui.selectable_label(lk, "🔒").on_hover_text("lock").clicked() { self.doc.layers[i].locked = !lk; }
                    if self.doc.layers[i].name != "0" && ui.button("🗑").on_hover_text("delete").clicked() {
                        to_delete = Some(i);
                    }
                });
            }
            // propagate renames to objects + current, before any structural change
            for i in 0..before.len().min(self.doc.layers.len()) {
                if self.doc.layers[i].name != before[i] {
                    // "0" is reserved (the default/fallback layer); refuse a rename onto it so it can't be duplicated.
                    if self.doc.layers[i].name.trim() == "0" { self.doc.layers[i].name = before[i].clone(); continue; }
                    let (old, new) = (before[i].clone(), self.doc.layers[i].name.clone());
                    for o in self.doc.objs.iter_mut() { if o.layer == old { o.layer = new.clone(); } }
                    if self.doc.current == old { self.doc.current = new; }
                }
            }
            if let Some(c) = set_current { self.doc.current = c; }
            if let Some(i) = to_delete {
                let name = self.doc.layers[i].name.clone();
                for o in self.doc.objs.iter_mut() { if o.layer == name { o.layer = "0".into(); } }
                self.doc.layers.remove(i);
                if self.doc.current == name { self.doc.current = "0".into(); }
            }
            // invariant: a "0" layer always exists (layer() indexes layers[0] as a fallback).
            if !self.doc.layers.iter().any(|l| l.name == "0") { self.doc.layers.insert(0, model::Layer::new("0", None)); }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_layer).desired_width(110.0).hint_text("new layer"));
                if ui.button("+ Add").clicked() {
                    let n = self.new_layer.trim();
                    if !n.is_empty() && !self.doc.layers.iter().any(|l| l.name == n) {
                        self.doc.layers.push(model::Layer::new(n, Some([120, 150, 255])));
                        self.new_layer.clear();
                    }
                }
            });
            ui.separator();
            if ui.button("Export DXF → opencad.dxf").clicked() {
                self.prompt = match std::fs::write("opencad.dxf", self.doc.to_dxf()) {
                    Ok(_) => "Wrote opencad.dxf".into(),
                    Err(e) => format!("Export failed: {}", e),
                };
            }
            if ui.button("Import DXF (opencad.dxf)").clicked() { self.confirm = Some(Confirm::Import); }
            ui.label(egui::RichText::new("…or drag a .dxf onto the window").small().color(Color32::GRAY));
        });

        if let Some(act) = self.confirm {
            egui::Window::new(match act { Confirm::New => "New drawing", Confirm::Import => "Import DXF" })
                .collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label(match act {
                        Confirm::New => "Start a new drawing? Unsaved work will be lost (there's no undo past this).",
                        Confirm::Import => "Import opencad.dxf? It replaces the current drawing (no undo past this).",
                    });
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button(match act { Confirm::New => "New drawing", Confirm::Import => "Import" }).clicked() {
                            match act { Confirm::New => self.do_new(), Confirm::Import => self.do_import_file() }
                            self.confirm = None;
                        }
                        if ui.button("Cancel").clicked() { self.confirm = None; }
                    });
                });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(Color32::from_rgb(9, 13, 18)))
            .show(ctx, |ui| { self.canvas(ui, ctx); });
    }
}
