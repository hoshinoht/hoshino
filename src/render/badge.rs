//! Tier 3 "expressive" shape badges for Full one-shot output.
//!
//! Colour stays a fixed semantic role; shape is the emphasis tier. These
//! Material 3 Expressive named shapes are reserved for rare states (the OS
//! identity and a disk at 95% or more), so a card carries at most two. They are
//! drawn only through the reviewed query-free Kitty direct transfer; every
//! other surface keeps its existing text.
//!
//! The geometry is a direct port of `androidx.compose.material3.MaterialShapes`
//! (`cookie9()` and `softBurst()`), built the way `androidx.graphics.shapes`
//! builds a `RoundedPolygon`: vertices with per-vertex `CornerRounding`
//! (smoothing 0), cuts clipped proportionally when two roundings share an edge,
//! then `normalized()` into the unit square. Corners are exact circular arcs
//! where the library uses cubic approximations of the same arcs.

use std::f64::consts::PI;

use image::{Rgba, RgbaImage};

use crate::render::theme::{Palette, Rgb, Role};

/// Placement size in terminal cells (≈42×40 pt at an 8.4×20 pt cell).
pub const BADGE_COLUMNS: u16 = 5;
pub const BADGE_ROWS: u16 = 2;
/// Upper bound per raster side, so a badge transfer stays small and bounded
/// whatever the (non-query) cell geometry claims.
pub const MAX_BADGE_PX: u32 = 256;
/// Fixed near-square fallback raster (2x of a 42×40 pt placement).
pub const FALLBACK_BADGE_PX: (u32, u32) = (84, 80);

/// Which rare state a row carries. The role decides colour; the shape decides
/// emphasis.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Badge {
    /// The OS identity in Full's compact header.
    Identity,
    /// A disk at or above the `▲ full` threshold.
    DiskFull,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BadgeShape {
    Cookie9Sided,
    SoftBurst,
}

impl Badge {
    pub const ALL: [Badge; 2] = [Badge::Identity, Badge::DiskFull];

    pub const fn shape(self) -> BadgeShape {
        match self {
            Badge::Identity => BadgeShape::Cookie9Sided,
            Badge::DiskFull => BadgeShape::SoftBurst,
        }
    }

    pub const fn role(self) -> Role {
        match self {
            Badge::Identity => Role::Primary,
            Badge::DiskFull => Role::Warning,
        }
    }

    pub const fn color(self, palette: Palette) -> Rgb {
        palette.color(self.role())
    }
}

/// Raster size for a 5×2-cell placement. Derived from non-query cell geometry
/// when known, otherwise the fixed near-square fallback; always bounded.
pub fn raster_size(cell: Option<(u32, u32)>) -> (u32, u32) {
    let (width, height) = cell
        .and_then(|(width, height)| {
            Some((
                width.checked_mul(u32::from(BADGE_COLUMNS))?,
                height.checked_mul(u32::from(BADGE_ROWS))?,
            ))
        })
        .filter(|(width, height)| *width > 0 && *height > 0)
        .unwrap_or(FALLBACK_BADGE_PX);
    let largest = width.max(height);
    if largest <= MAX_BADGE_PX {
        return (width, height);
    }
    let scale = |side: u32| {
        ((u64::from(side) * u64::from(MAX_BADGE_PX)) / u64::from(largest)).max(1) as u32
    };
    (scale(width), scale(height))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

impl Point {
    const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
    fn sub(self, other: Point) -> Point {
        Point::new(self.x - other.x, self.y - other.y)
    }
    fn add(self, other: Point) -> Point {
        Point::new(self.x + other.x, self.y + other.y)
    }
    fn scale(self, factor: f64) -> Point {
        Point::new(self.x * factor, self.y * factor)
    }
    fn length(self) -> f64 {
        self.x.hypot(self.y)
    }
    fn unit(self) -> Point {
        let length = self.length();
        if length == 0.0 {
            self
        } else {
            self.scale(1.0 / length)
        }
    }
    fn dot(self, other: Point) -> f64 {
        self.x * other.x + self.y * other.y
    }
    fn rotate(self, radians: f64, center: Point) -> Point {
        let offset = self.sub(center);
        let (sin, cos) = radians.sin_cos();
        Point::new(
            offset.x * cos - offset.y * sin,
            offset.x * sin + offset.y * cos,
        )
        .add(center)
    }
}

/// `RoundedPolygon.star(9, innerRadius = .8, rounding = CornerRounding(.5))`
/// rotated by -90°.
fn cookie9_vertices() -> Vec<(Point, f64)> {
    let count = 9;
    let mut vertices = Vec::with_capacity(count * 2);
    for index in 0..count {
        for (radius, step) in [(1.0, 2 * index), (0.8, 2 * index + 1)] {
            let angle = PI / count as f64 * step as f64;
            let point = Point::new(radius * angle.cos(), radius * angle.sin());
            vertices.push((point.rotate(-PI / 2.0, Point::new(0.0, 0.0)), 0.5));
        }
    }
    vertices
}

/// `customPolygon([(0.193, 0.277) r.053, (0.176, 0.055) r.053], reps = 10)`
/// about (0.5, 0.5).
fn soft_burst_vertices() -> Vec<(Point, f64)> {
    let center = Point::new(0.5, 0.5);
    let points = [
        (Point::new(0.193, 0.277), 0.053),
        (Point::new(0.176, 0.055), 0.053),
    ];
    let reps = 10;
    (0..points.len() * reps)
        .map(|index| {
            let (point, rounding) = points[index % points.len()];
            let angle = ((index / points.len()) as f64 * 360.0 / reps as f64).to_radians();
            (point.rotate(angle, center), rounding)
        })
        .collect()
}

/// Flattened outline of a rounded polygon, normalized into the unit square
/// (largest side spans 0..1, the other is centred), like `normalized()`.
fn outline(shape: BadgeShape) -> Vec<Point> {
    let vertices = match shape {
        BadgeShape::Cookie9Sided => cookie9_vertices(),
        BadgeShape::SoftBurst => soft_burst_vertices(),
    };
    let count = vertices.len();
    // Expected round cut per vertex: r·cot(α/2), α the angle between edges.
    let corners = (0..count)
        .map(|index| {
            let (vertex, radius) = vertices[index];
            let previous = vertices[(index + count - 1) % count].0;
            let next = vertices[(index + 1) % count].0;
            let to_previous = previous.sub(vertex).unit();
            let to_next = next.sub(vertex).unit();
            let cos = to_previous.dot(to_next).clamp(-1.0, 1.0);
            let sin = (1.0 - cos * cos).sqrt();
            let cut = if sin > 1e-9 {
                radius * (cos + 1.0) / sin
            } else {
                0.0
            };
            (to_previous, to_next, cos, sin, cut)
        })
        .collect::<Vec<_>>();
    // Edge i joins vertex i and i+1; shrink both cuts when they overlap.
    let ratios = (0..count)
        .map(|index| {
            let next = (index + 1) % count;
            let side = vertices[next].0.sub(vertices[index].0).length();
            let expected = corners[index].4 + corners[next].4;
            if expected > side {
                side / expected
            } else {
                1.0
            }
        })
        .collect::<Vec<_>>();
    let mut path = Vec::new();
    for index in 0..count {
        let (vertex, radius) = vertices[index];
        let (to_previous, to_next, cos, _sin, expected) = corners[index];
        let allowed = [ratios[(index + count - 1) % count], ratios[index]]
            .into_iter()
            .map(|ratio| expected * ratio)
            .fold(f64::INFINITY, f64::min);
        if allowed < 1e-9 || expected < 1e-9 {
            path.push(vertex);
            continue;
        }
        let cut = allowed.min(expected);
        let actual_radius = radius * cut / expected;
        let start = vertex.add(to_previous.scale(cut));
        let end = vertex.add(to_next.scale(cut));
        // The arc centre sits on the bisector at r / sin(α/2) from the vertex.
        let half_sin = ((1.0 - cos) / 2.0).sqrt();
        let bisector = to_previous.add(to_next).unit();
        let center = vertex.add(bisector.scale(actual_radius / half_sin.max(1e-9)));
        let from = start.sub(center);
        let to = end.sub(center);
        let mut sweep = to.y.atan2(to.x) - from.y.atan2(from.x);
        while sweep > PI {
            sweep -= 2.0 * PI;
        }
        while sweep < -PI {
            sweep += 2.0 * PI;
        }
        let steps = ((sweep.abs() / (PI / 32.0)).ceil() as usize).max(2);
        let base = from.y.atan2(from.x);
        let arc_radius = from.length();
        for step in 0..=steps {
            let angle = base + sweep * step as f64 / steps as f64;
            path.push(Point::new(
                center.x + arc_radius * angle.cos(),
                center.y + arc_radius * angle.sin(),
            ));
        }
    }
    normalize(path)
}

fn normalize(path: Vec<Point>) -> Vec<Point> {
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for point in &path {
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }
    let (width, height) = (max_x - min_x, max_y - min_y);
    let side = width.max(height);
    let offset_x = min_x - (side - width) / 2.0;
    let offset_y = min_y - (side - height) / 2.0;
    path.into_iter()
        .map(|point| Point::new((point.x - offset_x) / side, (point.y - offset_y) / side))
        .collect()
}

/// Rasterise `shape` filled with `color` into a `width`×`height` RGBA image.
/// The unit-square outline is fitted to the shorter side and centred, so a
/// near-square cell box keeps the shape's true aspect. Edges are anti-aliased
/// by exact signed distance (one pixel ramp). Returns `None` for an empty or
/// out-of-bounds request.
pub fn rasterize(shape: BadgeShape, width: u32, height: u32, color: Rgb) -> Option<RgbaImage> {
    if width == 0 || height == 0 || width > MAX_BADGE_PX || height > MAX_BADGE_PX {
        return None;
    }
    let side = f64::from(width.min(height));
    // Keep a half pixel of clearance so the anti-aliased rim is never clipped.
    let inset = 1.0;
    let scale = side - 2.0 * inset;
    let origin = Point::new(
        (f64::from(width) - scale) / 2.0,
        (f64::from(height) - scale) / 2.0,
    );
    let polygon = outline(shape)
        .into_iter()
        .map(|point| origin.add(point.scale(scale)))
        .collect::<Vec<_>>();
    let mut image = RgbaImage::new(width, height);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let sample = Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5);
        let distance = edge_distance(&polygon, sample);
        let signed = if contains(&polygon, sample) {
            distance
        } else {
            -distance
        };
        let coverage = (signed + 0.5).clamp(0.0, 1.0);
        let alpha = (coverage * 255.0).round() as u8;
        *pixel = Rgba([color.red, color.green, color.blue, alpha]);
    }
    Some(image)
}

fn edge_distance(polygon: &[Point], sample: Point) -> f64 {
    let mut best = f64::INFINITY;
    for index in 0..polygon.len() {
        let start = polygon[index];
        let end = polygon[(index + 1) % polygon.len()];
        let edge = end.sub(start);
        let length = edge.dot(edge);
        let t = if length == 0.0 {
            0.0
        } else {
            (sample.sub(start).dot(edge) / length).clamp(0.0, 1.0)
        };
        best = best.min(sample.sub(start.add(edge.scale(t))).length());
    }
    best
}

/// Even-odd point-in-polygon; the outlines are simple (non-self-intersecting).
fn contains(polygon: &[Point], sample: Point) -> bool {
    let mut inside = false;
    let mut previous = polygon[polygon.len() - 1];
    for &point in polygon {
        if (point.y > sample.y) != (previous.y > sample.y) {
            let x =
                (previous.x - point.x) * (sample.y - point.y) / (previous.y - point.y) + point.x;
            if sample.x < x {
                inside = !inside;
            }
        }
        previous = point;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINK: Rgb = Rgb::new(0xFF, 0xB8, 0xD1);

    fn alpha(image: &RgbaImage, x: u32, y: u32) -> u8 {
        image.get_pixel(x, y).0[3]
    }

    #[test]
    fn outlines_are_normalized_into_the_unit_square() {
        for shape in [BadgeShape::Cookie9Sided, BadgeShape::SoftBurst] {
            let path = outline(shape);
            assert!(path.len() > 40);
            let max_x = path.iter().map(|p| p.x).fold(f64::MIN, f64::max);
            let min_x = path.iter().map(|p| p.x).fold(f64::MAX, f64::min);
            let max_y = path.iter().map(|p| p.y).fold(f64::MIN, f64::max);
            let min_y = path.iter().map(|p| p.y).fold(f64::MAX, f64::min);
            assert!(min_x >= -1e-9 && min_y >= -1e-9);
            assert!(max_x <= 1.0 + 1e-9 && max_y <= 1.0 + 1e-9);
            assert!((max_x - min_x).max(max_y - min_y) > 1.0 - 1e-9);
        }
    }

    #[test]
    fn cookie9_is_a_scalloped_disc_and_soft_burst_is_spikier() {
        // Cookie: every outline point lies between the inner and outer radii
        // (inner 0.8 of outer after rounding), so its minimum radius is high.
        let spread = |shape| {
            let path = outline(shape);
            let radii = path
                .iter()
                .map(|p| Point::new(p.x - 0.5, p.y - 0.5).length())
                .collect::<Vec<_>>();
            let max = radii.iter().cloned().fold(f64::MIN, f64::max);
            let min = radii.iter().cloned().fold(f64::MAX, f64::min);
            min / max
        };
        let cookie = spread(BadgeShape::Cookie9Sided);
        let burst = spread(BadgeShape::SoftBurst);
        assert!(cookie > 0.8 && cookie < 0.97, "cookie ratio {cookie}");
        assert!(burst < cookie, "burst {burst} vs cookie {cookie}");
    }

    #[test]
    fn centre_is_filled_and_corners_are_empty() {
        for shape in [BadgeShape::Cookie9Sided, BadgeShape::SoftBurst] {
            let image = rasterize(shape, 84, 80, PINK).unwrap();
            assert_eq!(image.dimensions(), (84, 80));
            assert_eq!(alpha(&image, 42, 40), 255);
            assert_eq!(*image.get_pixel(42, 40), Rgba([0xFF, 0xB8, 0xD1, 255]));
            for (x, y) in [(0, 0), (83, 0), (0, 79), (83, 79)] {
                assert_eq!(alpha(&image, x, y), 0, "{shape:?} corner {x},{y}");
            }
            // The anti-aliased rim has intermediate coverage.
            assert!(
                image
                    .pixels()
                    .any(|pixel| pixel.0[3] > 0 && pixel.0[3] < 255)
            );
        }
    }

    #[test]
    fn coverage_is_symmetric() {
        // Cookie9 has an outer vertex straight up, so it mirrors about the
        // vertical axis; softBurst's ten repetitions include a 180° turn.
        let size = 96;
        let cookie = rasterize(BadgeShape::Cookie9Sided, size, size, PINK).unwrap();
        let burst = rasterize(BadgeShape::SoftBurst, size, size, PINK).unwrap();
        let (mut mirror, mut turned) = (0u64, 0u64);
        for y in 0..size {
            for x in 0..size {
                let diff = alpha(&cookie, x, y).abs_diff(alpha(&cookie, size - 1 - x, y));
                assert!(diff <= 2, "cookie {x},{y}");
                mirror += u64::from(diff);
                let diff = alpha(&burst, x, y).abs_diff(alpha(&burst, size - 1 - x, size - 1 - y));
                assert!(diff <= 2, "burst {x},{y}");
                turned += u64::from(diff);
            }
        }
        assert!(mirror / u64::from(size * size) <= 1);
        assert!(turned / u64::from(size * size) <= 1);
    }

    #[test]
    fn raster_output_is_bounded() {
        assert!(rasterize(BadgeShape::SoftBurst, 0, 10, PINK).is_none());
        assert!(rasterize(BadgeShape::SoftBurst, MAX_BADGE_PX + 1, 10, PINK).is_none());
        assert_eq!(raster_size(None), FALLBACK_BADGE_PX);
        assert_eq!(raster_size(Some((8, 20))), (40, 40));
        assert_eq!(raster_size(Some((17, 40))), (85, 80));
        let (width, height) = raster_size(Some((400, 900)));
        assert!(width <= MAX_BADGE_PX && height <= MAX_BADGE_PX);
        assert_eq!(width.max(height), MAX_BADGE_PX);
        assert_eq!(raster_size(Some((u32::MAX, 20))), FALLBACK_BADGE_PX);
        let image = rasterize(BadgeShape::Cookie9Sided, MAX_BADGE_PX, MAX_BADGE_PX, PINK).unwrap();
        assert_eq!(
            image.as_raw().len(),
            (MAX_BADGE_PX * MAX_BADGE_PX * 4) as usize
        );
    }

    #[test]
    fn badges_keep_their_roles_and_shapes() {
        let palette = Palette::for_theme(crate::config::Theme::DuskDarker);
        assert_eq!(Badge::Identity.shape(), BadgeShape::Cookie9Sided);
        assert_eq!(Badge::DiskFull.shape(), BadgeShape::SoftBurst);
        assert_eq!(Badge::Identity.color(palette).hex(), 0xFFB8D1);
        assert_eq!(Badge::DiskFull.color(palette).hex(), 0xF4DA86);
    }
}
