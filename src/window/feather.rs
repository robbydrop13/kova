//! Feather icons as `NSBezierPath`s. Each icon is a few strokes on Feather's
//! 24 pt grid (2 pt stroke, round caps and joins); `path` maps the grid onto
//! any rect, so an icon centres exactly in its button and scales with the
//! row. The grid is y-down like SVG, which is what the sidebar views are
//! (`isFlipped`), so the coordinates are used as published: chevron-down
//! points down on screen.
//!
//! The geometry (`Icon::segments`, `map_point`, `stroke_width`) is pure and
//! tested; only `path` touches AppKit.

use objc2::rc::Retained;
use objc2_app_kit::{NSBezierPath, NSLineCapStyle, NSLineJoinStyle};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};

/// Feather's grid size.
pub const GRID: f64 = 24.0;
/// Feather's stroke width on the grid.
pub const STROKE: f64 = 2.0;

/// The icons the sidebar draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// `chevron-down`: an open group.
    ChevronDown,
    /// `chevron-right`: a folded group.
    ChevronRight,
    /// `plus`: add a pane.
    Plus,
    /// `x`: close.
    X,
    /// `play`: focus, start, resume; filled for the links and the Next pill.
    Play,
    /// `square`: stop; filled for the Stop link.
    Square,
    /// `maximize-2`: restore a minimized pane.
    Maximize,
    /// `minimize-2`: minimize a pane.
    Minimize,
}

/// One stroke of an icon, in grid coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    /// Open polyline through the points.
    Polyline(Vec<(f64, f64)>),
    /// Closed polygon through the points.
    Polygon(Vec<(f64, f64)>),
    /// Rounded rectangle: x, y, w, h, corner radius.
    RoundRect(f64, f64, f64, f64, f64),
}

impl Icon {
    /// The strokes, in Feather's published coordinates.
    pub fn segments(self) -> Vec<Segment> {
        use Segment::*;
        match self {
            Icon::ChevronDown => vec![Polyline(vec![(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)])],
            Icon::ChevronRight => vec![Polyline(vec![(9.0, 18.0), (15.0, 12.0), (9.0, 6.0)])],
            Icon::Plus => vec![
                Polyline(vec![(12.0, 5.0), (12.0, 19.0)]),
                Polyline(vec![(5.0, 12.0), (19.0, 12.0)]),
            ],
            Icon::X => vec![
                Polyline(vec![(18.0, 6.0), (6.0, 18.0)]),
                Polyline(vec![(6.0, 6.0), (18.0, 18.0)]),
            ],
            Icon::Play => vec![Polygon(vec![(5.0, 3.0), (19.0, 12.0), (5.0, 21.0)])],
            Icon::Square => vec![RoundRect(3.0, 3.0, 18.0, 18.0, 2.0)],
            Icon::Maximize => vec![
                Polyline(vec![(15.0, 3.0), (21.0, 3.0), (21.0, 9.0)]),
                Polyline(vec![(9.0, 21.0), (3.0, 21.0), (3.0, 15.0)]),
                Polyline(vec![(21.0, 3.0), (14.0, 10.0)]),
                Polyline(vec![(3.0, 21.0), (10.0, 14.0)]),
            ],
            Icon::Minimize => vec![
                Polyline(vec![(4.0, 14.0), (10.0, 14.0), (10.0, 20.0)]),
                Polyline(vec![(20.0, 10.0), (14.0, 10.0), (14.0, 4.0)]),
                Polyline(vec![(14.0, 10.0), (21.0, 3.0)]),
                Polyline(vec![(3.0, 21.0), (10.0, 14.0)]),
            ],
        }
    }
}

/// A grid point placed in `rect` (x, y, w, h): the grid stretches to the
/// rect, so a square rect keeps the icon's proportions.
pub fn map_point(rect: (f64, f64, f64, f64), p: (f64, f64)) -> (f64, f64) {
    let (x, y, w, h) = rect;
    (x + p.0 / GRID * w, y + p.1 / GRID * h)
}

/// The stroke width of an icon drawn `size` points wide.
pub fn stroke_width(size: f64) -> f64 {
    STROKE * size / GRID
}

/// A square box of `size` centred in the rect (x, y, w, h).
pub fn centred_box(rect: (f64, f64, f64, f64), size: f64) -> (f64, f64, f64, f64) {
    let (x, y, w, h) = rect;
    (x + (w - size) / 2.0, y + (h - size) / 2.0, size, size)
}

/// The icon as a path over `rect`, line width, caps and joins set: stroke
/// it for the outline icon, fill it for the solid play and square.
pub fn path(icon: Icon, rect: (f64, f64, f64, f64)) -> Retained<NSBezierPath> {
    let path = NSBezierPath::bezierPath();
    path.setLineWidth(stroke_width(rect.2));
    path.setLineCapStyle(NSLineCapStyle::Round);
    path.setLineJoinStyle(NSLineJoinStyle::Round);
    let pt = |p: (f64, f64)| {
        let (x, y) = map_point(rect, p);
        CGPoint { x, y }
    };
    for seg in icon.segments() {
        match &seg {
            Segment::Polyline(points) | Segment::Polygon(points) => {
                let closed = matches!(seg, Segment::Polygon(_));
                let mut it = points.iter();
                if let Some(&first) = it.next() {
                    path.moveToPoint(pt(first));
                }
                for &p in it {
                    path.lineToPoint(pt(p));
                }
                if closed {
                    path.closePath();
                }
            }
            &Segment::RoundRect(x, y, w, h, r) => {
                let (ox, oy) = map_point(rect, (x, y));
                let sw = w / GRID * rect.2;
                let sh = h / GRID * rect.3;
                let radius = r / GRID * rect.2;
                let cg = CGRect { origin: CGPoint { x: ox, y: oy }, size: CGSize { width: sw, height: sh } };
                path.appendBezierPathWithRoundedRect_xRadius_yRadius(cg, radius, radius);
            }
        }
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_points_scale_into_the_rect() {
        // A 16 pt box at (10, 20): the grid centre lands on the box centre.
        let r = (10.0, 20.0, 16.0, 16.0);
        assert_eq!(map_point(r, (12.0, 12.0)), (18.0, 28.0));
        assert_eq!(map_point(r, (0.0, 0.0)), (10.0, 20.0));
        assert_eq!(map_point(r, (24.0, 24.0)), (26.0, 36.0));
        // The 2 pt stroke follows the scale.
        assert!((stroke_width(16.0) - 4.0 / 3.0).abs() < 1e-9);
        assert_eq!(stroke_width(24.0), 2.0);
        assert_eq!(centred_box((0.0, 0.0, 24.0, 24.0), 16.0), (4.0, 4.0, 16.0, 16.0));
        assert_eq!(centred_box((100.0, 50.0, 20.0, 30.0), 12.0), (104.0, 59.0, 12.0, 12.0));
    }

    #[test]
    fn chevrons_point_the_way_their_names_say_in_a_flipped_view() {
        // y grows downward: the tip of chevron-down has the largest y, the
        // tip of chevron-right the largest x.
        let down = Icon::ChevronDown.segments();
        let Segment::Polyline(p) = &down[0] else { panic!("polyline") };
        assert!(p[1].1 > p[0].1 && p[1].1 > p[2].1);
        let right = Icon::ChevronRight.segments();
        let Segment::Polyline(p) = &right[0] else { panic!("polyline") };
        assert!(p[1].0 > p[0].0 && p[1].0 > p[2].0);
        // Play points right and closes; square is one rounded rect.
        assert!(matches!(&Icon::Play.segments()[0], Segment::Polygon(p) if p[1].0 == 19.0));
        assert_eq!(Icon::Square.segments(), vec![Segment::RoundRect(3.0, 3.0, 18.0, 18.0, 2.0)]);
        assert_eq!(Icon::Plus.segments().len(), 2);
        assert_eq!(Icon::X.segments().len(), 2);
        assert_eq!(Icon::Maximize.segments().len(), 4);
        assert_eq!(Icon::Minimize.segments().len(), 4);
        // Every point stays on the grid.
        for icon in [Icon::ChevronDown, Icon::ChevronRight, Icon::Plus, Icon::X, Icon::Play, Icon::Square, Icon::Maximize, Icon::Minimize] {
            for seg in icon.segments() {
                match seg {
                    Segment::Polyline(p) | Segment::Polygon(p) => {
                        assert!(p.iter().all(|&(x, y)| (0.0..=GRID).contains(&x) && (0.0..=GRID).contains(&y)));
                    }
                    Segment::RoundRect(x, y, w, h, _) => assert!(x + w <= GRID && y + h <= GRID),
                }
            }
        }
    }
}
