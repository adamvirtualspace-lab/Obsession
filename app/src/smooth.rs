//! Smooth lines and shapes for the canvases, without MSAA. Each line or
//! shape is a mesh with a soft edge about one screen pixel wide: its colour
//! fades to clear across the edge, so curves don't show steps. (iced's 4×
//! MSAA does the same for every mesh, but costs ~120 MB of GPU memory at
//! 2560 × 1600 and can only be chosen at startup.)
//!
//! `Shapes` collects them; `into_geometry` makes one canvas geometry, which
//! keeps the order it's drawn in among the canvas's other geometries.

use std::sync::atomic::{AtomicU32, Ordering};

use iced::advanced::graphics::color;
use iced::advanced::graphics::mesh::{Indexed, Mesh, SolidVertex2D};
use iced::{Color, Point, Rectangle, Size, Transformation, Vector};
use iced_wgpu::Geometry;

/// The window's scale factor (screen pixels per logical pixel).
static SCALE: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0

pub fn set_scale(scale: f32) {
    if scale > 0.0 {
        SCALE.store(scale.to_bits(), Ordering::Relaxed);
    }
}

pub fn scale() -> f32 {
    f32::from_bits(SCALE.load(Ordering::Relaxed))
}

/// Lines and filled shapes with soft edges, in the canvas's coordinates.
pub struct Shapes {
    vertices: Vec<SolidVertex2D>,
    indices: Vec<u32>,
    /// The soft edge's width: one screen pixel.
    aa: f32,
}

impl Default for Shapes {
    fn default() -> Self {
        Self::new()
    }
}

impl Shapes {
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            aa: 1.0 / scale(),
        }
    }

    fn vertex(&mut self, p: Point, color: color::Packed) -> u32 {
        self.vertices.push(SolidVertex2D {
            position: [p.x, p.y],
            color,
        });
        self.vertices.len() as u32 - 1
    }

    fn quad(&mut self, a: u32, b: u32, c: u32, d: u32) {
        self.indices.extend_from_slice(&[a, b, c, a, c, d]);
    }

    /// The edge colours for a line `width` wide: half its core width, and
    /// the colour (a line thinner than a pixel is a pixel wide, fainter).
    fn core(&self, width: f32, color: Color) -> (f32, Color) {
        if width < self.aa {
            (0.0, Color { a: color.a * width / self.aa, ..color })
        } else {
            ((width - self.aa) / 2.0, color)
        }
    }

    /// A line through `points`, `width` wide.
    pub fn line(&mut self, points: &[Point], width: f32, color: Color) {
        self.strip(points, width, color, false);
    }

    /// A closed line through `points` (back to the first).
    pub fn ring(&mut self, points: &[Point], width: f32, color: Color) {
        self.strip(points, width, color, true);
    }

    /// A dashed line: `dash` drawn, `gap` left out, starting `phase` into
    /// the pattern.
    pub fn dashed(&mut self, points: &[Point], width: f32, color: Color, dash: f32, gap: f32, phase: f32) {
        let period = dash + gap;
        if period <= 0.0 || gap <= 0.01 {
            self.line(points, width, color);
            return;
        }
        // Where along the pattern the line starts, and whether that's in a dash.
        let mut at = phase.rem_euclid(period);
        let mut piece: Vec<Point> = Vec::new();
        if at < dash {
            piece.push(points[0]);
        }
        for pair in points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let length = a.distance(b);
            let mut done = 0.0;
            while done < length {
                let (edge, drawing) = if at < dash { (dash, true) } else { (period, false) };
                let step = (edge - at).min(length - done);
                done += step;
                at += step;
                let p = lerp(a, b, done / length);
                if drawing {
                    piece.push(p);
                }
                if at >= edge - 1e-4 {
                    if drawing {
                        self.line(&piece, width, color);
                        piece.clear();
                    } else {
                        at = 0.0;
                        piece.push(p);
                    }
                }
            }
        }
        if piece.len() >= 2 {
            self.line(&piece, width, color);
        }
    }

    fn strip(&mut self, points: &[Point], width: f32, color: Color, closed: bool) {
        // Drop points on top of each other (they have no direction).
        let mut pts: Vec<Point> = Vec::with_capacity(points.len());
        for &p in points {
            if pts.last().is_none_or(|q: &Point| q.distance(p) > 0.01) {
                pts.push(p);
            }
        }
        if closed && pts.len() > 2 && pts[0].distance(pts[pts.len() - 1]) <= 0.01 {
            pts.pop();
        }
        let n = pts.len();
        if n < 2 {
            return;
        }
        let (half, color) = self.core(width, color);
        let aa = self.aa;
        let solid = color::pack(color);
        let clear = color::pack(Color { a: 0.0, ..color });
        let dir = |i: usize, j: usize| unit(pts[j] - pts[i]);
        let mut rows: Vec<[u32; 4]> = Vec::with_capacity(n + 2);
        let row = |s: &mut Self, p: Point, normal: Vector, ends: bool| {
            let c = if ends { clear } else { solid };
            [
                s.vertex(p - normal * (half + aa), clear),
                s.vertex(p - normal * half, c),
                s.vertex(p + normal * half, c),
                s.vertex(p + normal * (half + aa), clear),
            ]
        };
        if !closed {
            // A soft end before the first point.
            let t = dir(0, 1);
            rows.push(row(self, pts[0] - t * aa, perp(t), true));
        }
        for (i, &p) in pts.iter().enumerate() {
            let before = if i > 0 {
                Some(dir(i - 1, i))
            } else if closed {
                Some(dir(n - 1, 0))
            } else {
                None
            };
            let after = if i + 1 < n {
                Some(dir(i, i + 1))
            } else if closed {
                Some(dir(n - 1, 0))
            } else {
                None
            };
            let normal = match (before, after) {
                (Some(a), Some(b)) => {
                    // The miter: the average normal, longer at a bend
                    // (capped so a sharp turn doesn't spike).
                    let m = unit(perp(a) + perp(b));
                    let cos = (m.x * perp(b).x + m.y * perp(b).y).max(0.5);
                    m * (1.0 / cos)
                }
                (Some(a), None) => perp(a),
                (None, Some(b)) => perp(b),
                (None, None) => return,
            };
            rows.push(row(self, p, normal, false));
        }
        if !closed {
            let t = dir(n - 2, n - 1);
            rows.push(row(self, pts[n - 1] + t * aa, perp(t), true));
        }
        for k in 0..rows.len() - 1 {
            self.join(rows[k], rows[k + 1]);
        }
        if closed {
            self.join(rows[rows.len() - 1], rows[0]);
        }
    }

    fn join(&mut self, a: [u32; 4], b: [u32; 4]) {
        for k in 0..3 {
            self.quad(a[k], a[k + 1], b[k + 1], b[k]);
        }
    }

    /// A filled convex shape (its corners in order, either way round).
    pub fn fill(&mut self, points: &[Point], color: Color) {
        let n = points.len();
        if n < 3 {
            return;
        }
        let aa = self.aa;
        let solid = color::pack(color);
        let clear = color::pack(Color { a: 0.0, ..color });
        // Which way round the corners go, so "outward" is outward.
        let area: f32 = (0..n)
            .map(|i| {
                let (a, b) = (points[i], points[(i + 1) % n]);
                a.x * b.y - b.x * a.y
            })
            .sum();
        let out = if area > 0.0 { -1.0 } else { 1.0 };
        let mut inner = Vec::with_capacity(n);
        let mut outer = Vec::with_capacity(n);
        for i in 0..n {
            let prev = points[(i + n - 1) % n];
            let next = points[(i + 1) % n];
            let a = perp(unit(points[i] - prev));
            let b = perp(unit(next - points[i]));
            let m = unit(a + b);
            let cos = (m.x * b.x + m.y * b.y).max(0.3);
            let normal = m * (out / cos);
            // The edge straddles the true outline: half inside, half out.
            inner.push(self.vertex(points[i] - normal * (aa / 2.0), solid));
            outer.push(self.vertex(points[i] + normal * (aa / 2.0), clear));
        }
        for i in 1..n - 1 {
            self.indices.extend_from_slice(&[inner[0], inner[i], inner[i + 1]]);
        }
        for i in 0..n {
            let j = (i + 1) % n;
            self.quad(inner[i], inner[j], outer[j], outer[i]);
        }
    }

    /// A filled circle.
    pub fn circle(&mut self, centre: Point, radius: f32, color: Color) {
        let points = circle_points(centre, radius);
        self.fill(&points, color);
    }

    /// A circle's outline, `width` wide.
    pub fn circle_ring(&mut self, centre: Point, radius: f32, width: f32, color: Color) {
        let points = circle_points(centre, radius);
        self.ring(&points, width, color);
    }

    /// One canvas geometry for everything collected (drawn in canvas
    /// coordinates, clipped to the canvas).
    pub fn into_geometry(self, size: Size) -> Geometry {
        let meshes = if self.indices.is_empty() {
            Vec::new()
        } else {
            vec![Mesh::Solid {
                buffers: Indexed {
                    vertices: self.vertices,
                    indices: self.indices,
                },
                transformation: Transformation::IDENTITY,
                clip_bounds: Rectangle::with_size(size),
            }]
        };
        Geometry::Live {
            meshes,
            images: Vec::new(),
            text: Vec::new(),
        }
    }
}

/// Corners for a circle: enough that its outline looks round on screen.
fn circle_points(centre: Point, radius: f32) -> Vec<Point> {
    let n = ((radius * scale()).sqrt() * 4.0).clamp(12.0, 64.0) as usize;
    (0..n)
        .map(|i| {
            let t = i as f32 / n as f32 * std::f32::consts::TAU;
            Point::new(centre.x + radius * t.cos(), centre.y + radius * t.sin())
        })
        .collect()
}

/// Points along a cubic Bézier: enough that it looks smooth on screen.
pub fn bezier(a: Point, c1: Point, c2: Point, b: Point) -> Vec<Point> {
    let rough = a.distance(c1) + c1.distance(c2) + c2.distance(b);
    let n = ((rough * scale()).sqrt() * 1.2).clamp(8.0, 96.0) as usize;
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let u = 1.0 - t;
            let (k0, k1, k2, k3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            Point::new(
                k0 * a.x + k1 * c1.x + k2 * c2.x + k3 * b.x,
                k0 * a.y + k1 * c1.y + k2 * c2.y + k3 * b.y,
            )
        })
        .collect()
}

fn lerp(a: Point, b: Point, t: f32) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn unit(v: Vector) -> Vector {
    let length = (v.x * v.x + v.y * v.y).sqrt();
    if length == 0.0 { Vector::new(0.0, 0.0) } else { v * (1.0 / length) }
}

/// Turned a quarter to the left.
fn perp(v: Vector) -> Vector {
    Vector::new(-v.y, v.x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_has_soft_edges_and_ends() {
        let mut s = Shapes::new();
        s.line(&[Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(20.0, 5.0)], 2.0, Color::WHITE);
        // A row of 4 per point, plus one soft row past each end.
        assert_eq!(s.vertices.len(), 5 * 4);
        // 4 rows joined, 3 quads each, 2 triangles a quad.
        assert_eq!(s.indices.len(), 4 * 3 * 6);
        let clear = s.vertices.iter().filter(|v| v.color.components()[3] == 0.0).count();
        assert_eq!(clear, 3 * 2 + 2 * 4);
    }

    #[test]
    fn dashes_cover_the_pattern() {
        let mut s = Shapes::new();
        // 30 long, dashes of 6 with gaps of 4: 6+4+6+4+6+4 → three dashes.
        s.dashed(&[Point::new(0.0, 0.0), Point::new(30.0, 0.0)], 1.5, Color::WHITE, 6.0, 4.0, 0.0);
        // Each dash: 2 points + 2 soft ends = 4 rows.
        assert_eq!(s.vertices.len(), 3 * 4 * 4);
        let mut solid: Vec<f32> = s
            .vertices
            .iter()
            .filter(|v| v.color.components()[3] > 0.0)
            .map(|v| v.position[0])
            .collect();
        solid.sort_by(f32::total_cmp);
        solid.dedup();
        assert_eq!(solid, vec![0.0, 6.0, 10.0, 16.0, 20.0, 26.0]);
    }

    #[test]
    fn a_fill_fades_outward() {
        let mut s = Shapes::new();
        // Clockwise and anticlockwise give the same outline.
        for square in [
            [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0)],
            [Point::new(0.0, 0.0), Point::new(0.0, 10.0), Point::new(10.0, 10.0), Point::new(10.0, 0.0)],
        ] {
            s.vertices.clear();
            s.indices.clear();
            s.fill(&square, Color::WHITE);
            let clear: Vec<&SolidVertex2D> = s.vertices.iter().filter(|v| v.color.components()[3] == 0.0).collect();
            assert!(clear.iter().all(|v| v.position[0] < 0.0 || v.position[0] > 10.0));
        }
    }
}
