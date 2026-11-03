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
// The redesigned views draw these icons next; nothing constructs them yet.
#[allow(dead_code)]
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
    /// `mail`: mark a pane unread.
    Mail,
    /// `check`: mark a pane read.
    Check,
    /// `sidebar`: the panes view of the sidebar.
    Sidebar,
    /// `clock`: the routines view of the sidebar.
    Clock,
    /// `message-square`: the Asks view, a question waiting for an answer.
    Message,
    /// `users`: the Agents view, the sessions an agent runs.
    Users,
    /// `file-text`: a scheduled action and the prompt it runs.
    FileText,
    /// `edit-2`: edit a scheduled action or an ask's prompt.
    Edit,
    /// `pause`: pause a scheduled action.
    Pause,
    /// `moon`: an ask left to run in the background, a quiet row.
    Moon,
    /// `alert-triangle`: a failed action or a warning on a row.
    Alert,
    /// `x-circle`: deny an ask, the ask card's negative button.
    XCircle,
    /// `check-circle`: approve an ask, the ask card's positive button.
    CheckCircle,
    /// `circle` at radius 7 (the prototype's ring): the neutral state of a row.
    Ring,
    /// `arrow-up-right`: open the item in its folder or tab.
    Open,
    /// `info`: details of an action or an ask.
    Info,
    /// `trash-2`: delete a scheduled action.
    Trash,
    /// `slash`: an action that is off, a disabled row.
    Off,
    /// `chevrons-up-down`: the sort order of a list.
    Sort,
    /// `send`: send a typed answer from the ask card.
    Send,
    /// `mic`: dictate an answer from the ask card.
    Mic,
}

/// Every icon, walked by the grid test so a new variant is never forgotten.
#[cfg(test)]
pub const ALL: [Icon; 29] = [
    Icon::ChevronDown,
    Icon::ChevronRight,
    Icon::Plus,
    Icon::X,
    Icon::Play,
    Icon::Square,
    Icon::Maximize,
    Icon::Minimize,
    Icon::Mail,
    Icon::Check,
    Icon::Sidebar,
    Icon::Clock,
    Icon::Message,
    Icon::Users,
    Icon::FileText,
    Icon::Edit,
    Icon::Pause,
    Icon::Moon,
    Icon::Alert,
    Icon::XCircle,
    Icon::CheckCircle,
    Icon::Ring,
    Icon::Open,
    Icon::Info,
    Icon::Trash,
    Icon::Off,
    Icon::Sort,
    Icon::Send,
    Icon::Mic,
];

/// One stroke of an icon, in grid coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    /// Open polyline through the points.
    Polyline(Vec<(f64, f64)>),
    /// Closed polygon through the points.
    Polygon(Vec<(f64, f64)>),
    /// Rounded rectangle: x, y, w, h, corner radius.
    RoundRect(f64, f64, f64, f64, f64),
    /// Full circle: centre x, centre y, radius.
    Circle(f64, f64, f64),
    /// Circular arc from `start_deg` to `end_deg`, swept in the increasing
    /// angle direction (Feather's y-down grid, so clockwise on screen).
    Arc { cx: f64, cy: f64, r: f64, start_deg: f64, end_deg: f64 },
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
            Icon::Mail => vec![
                RoundRect(2.0, 4.0, 20.0, 16.0, 2.0),
                Polyline(vec![(22.0, 6.0), (12.0, 13.0), (2.0, 6.0)]),
            ],
            Icon::Check => vec![Polyline(vec![(20.0, 6.0), (9.0, 17.0), (4.0, 12.0)])],
            Icon::Sidebar => vec![
                RoundRect(3.0, 3.0, 18.0, 18.0, 2.0),
                Polyline(vec![(9.0, 3.0), (9.0, 21.0)]),
            ],
            // A circle is a rounded rect whose radius is half its side.
            Icon::Clock => vec![
                RoundRect(2.0, 2.0, 20.0, 20.0, 10.0),
                Polyline(vec![(12.0, 6.0), (12.0, 12.0), (16.0, 14.0)]),
            ],
            Icon::Message => vec![
                svg_arc((21.0, 15.0), (19.0, 17.0), 2.0, false, true),
                Polyline(vec![(19.0, 17.0), (7.0, 17.0), (3.0, 21.0), (3.0, 5.0)]),
                svg_arc((3.0, 5.0), (5.0, 3.0), 2.0, false, true),
                Polyline(vec![(5.0, 3.0), (19.0, 3.0)]),
                svg_arc((19.0, 3.0), (21.0, 5.0), 2.0, false, true),
                Polyline(vec![(21.0, 5.0), (21.0, 15.0)]),
            ],
            Icon::Users => vec![
                Polyline(vec![(17.0, 21.0), (17.0, 19.0)]),
                svg_arc((17.0, 19.0), (13.0, 15.0), 4.0, false, false),
                Polyline(vec![(13.0, 15.0), (5.0, 15.0)]),
                svg_arc((5.0, 15.0), (1.0, 19.0), 4.0, false, false),
                Polyline(vec![(1.0, 19.0), (1.0, 21.0)]),
                Circle(9.0, 7.0, 4.0),
                Polyline(vec![(23.0, 21.0), (23.0, 19.0)]),
                svg_arc((23.0, 19.0), (20.0, 15.13), 4.0, false, false),
                svg_arc((16.0, 3.13), (16.0, 10.88), 4.0, false, true),
            ],
            Icon::FileText => vec![
                Polyline(vec![(14.0, 2.0), (6.0, 2.0)]),
                svg_arc((6.0, 2.0), (4.0, 4.0), 2.0, false, false),
                Polyline(vec![(4.0, 4.0), (4.0, 20.0)]),
                svg_arc((4.0, 20.0), (6.0, 22.0), 2.0, false, false),
                Polyline(vec![(6.0, 22.0), (18.0, 22.0)]),
                svg_arc((18.0, 22.0), (20.0, 20.0), 2.0, false, false),
                Polyline(vec![(20.0, 20.0), (20.0, 8.0), (14.0, 2.0)]),
                Polyline(vec![(14.0, 2.0), (14.0, 8.0), (20.0, 8.0)]),
                Polyline(vec![(16.0, 13.0), (8.0, 13.0)]),
                Polyline(vec![(16.0, 17.0), (8.0, 17.0)]),
            ],
            Icon::Edit => vec![
                Polyline(vec![(12.0, 20.0), (21.0, 20.0)]),
                svg_arc((16.5, 3.5), (19.5, 6.5), 2.121, false, true),
                Polyline(vec![(19.5, 6.5), (7.0, 19.0), (3.0, 20.0), (4.0, 16.0), (16.5, 3.5)]),
            ],
            Icon::Pause => vec![
                RoundRect(6.0, 4.0, 4.0, 16.0, 1.0),
                RoundRect(14.0, 4.0, 4.0, 16.0, 1.0),
            ],
            Icon::Moon => vec![
                svg_arc((21.0, 12.79), (11.21, 3.0), 9.0, true, true),
                svg_arc((11.21, 3.0), (21.0, 12.79), 7.0, false, false),
            ],
            Icon::Alert => vec![
                Polyline(vec![(10.29, 3.86), (1.82, 18.0)]),
                svg_arc((1.82, 18.0), (3.53, 21.0), 2.0, false, false),
                Polyline(vec![(3.53, 21.0), (20.47, 21.0)]),
                svg_arc((20.47, 21.0), (22.18, 18.0), 2.0, false, false),
                Polyline(vec![(22.18, 18.0), (13.71, 3.86)]),
                svg_arc((13.71, 3.86), (10.29, 3.86), 2.0, false, false),
                Polyline(vec![(12.0, 9.0), (12.0, 13.0)]),
                Polyline(vec![(12.0, 17.0), (12.01, 17.0)]),
            ],
            Icon::XCircle => vec![
                Circle(12.0, 12.0, 10.0),
                Polyline(vec![(15.0, 9.0), (9.0, 15.0)]),
                Polyline(vec![(9.0, 9.0), (15.0, 15.0)]),
            ],
            Icon::CheckCircle => vec![
                Polyline(vec![(22.0, 11.08), (22.0, 12.0)]),
                svg_arc((22.0, 12.0), (16.07, 2.86), 10.0, true, true),
                Polyline(vec![(22.0, 4.0), (12.0, 14.01), (9.0, 11.01)]),
            ],
            Icon::Ring => vec![Circle(12.0, 12.0, 7.0)],
            Icon::Open => vec![
                Polyline(vec![(7.0, 17.0), (17.0, 7.0)]),
                Polyline(vec![(7.0, 7.0), (17.0, 7.0), (17.0, 17.0)]),
            ],
            Icon::Info => vec![
                Circle(12.0, 12.0, 10.0),
                Polyline(vec![(12.0, 16.0), (12.0, 12.0)]),
                Polyline(vec![(12.0, 8.0), (12.01, 8.0)]),
            ],
            Icon::Trash => vec![
                Polyline(vec![(3.0, 6.0), (5.0, 6.0), (21.0, 6.0)]),
                Polyline(vec![(19.0, 6.0), (19.0, 20.0)]),
                svg_arc((19.0, 20.0), (17.0, 22.0), 2.0, false, true),
                Polyline(vec![(17.0, 22.0), (7.0, 22.0)]),
                svg_arc((7.0, 22.0), (5.0, 20.0), 2.0, false, true),
                Polyline(vec![(5.0, 20.0), (5.0, 6.0)]),
                Polyline(vec![(8.0, 6.0), (8.0, 4.0)]),
                svg_arc((8.0, 4.0), (10.0, 2.0), 2.0, false, true),
                Polyline(vec![(10.0, 2.0), (14.0, 2.0)]),
                svg_arc((14.0, 2.0), (16.0, 4.0), 2.0, false, true),
                Polyline(vec![(16.0, 4.0), (16.0, 6.0)]),
                Polyline(vec![(10.0, 11.0), (10.0, 17.0)]),
                Polyline(vec![(14.0, 11.0), (14.0, 17.0)]),
            ],
            Icon::Off => vec![
                Circle(12.0, 12.0, 10.0),
                Polyline(vec![(4.93, 4.93), (19.07, 19.07)]),
            ],
            Icon::Sort => vec![
                Polyline(vec![(7.0, 15.0), (12.0, 20.0), (17.0, 15.0)]),
                Polyline(vec![(7.0, 9.0), (12.0, 4.0), (17.0, 9.0)]),
            ],
            Icon::Send => vec![
                Polyline(vec![(22.0, 2.0), (11.0, 13.0)]),
                Polygon(vec![(22.0, 2.0), (15.0, 22.0), (11.0, 13.0), (2.0, 9.0)]),
            ],
            Icon::Mic => vec![
                svg_arc((12.0, 1.0), (9.0, 4.0), 3.0, false, false),
                Polyline(vec![(9.0, 4.0), (9.0, 12.0)]),
                svg_arc((9.0, 12.0), (15.0, 12.0), 3.0, false, false),
                Polyline(vec![(15.0, 12.0), (15.0, 4.0)]),
                svg_arc((15.0, 4.0), (12.0, 1.0), 3.0, false, false),
                Polyline(vec![(19.0, 10.0), (19.0, 12.0)]),
                svg_arc((19.0, 12.0), (5.0, 12.0), 7.0, false, true),
                Polyline(vec![(5.0, 12.0), (5.0, 10.0)]),
                Polyline(vec![(12.0, 19.0), (12.0, 23.0)]),
            ],
        }
    }
}

/// The `Segment::Arc` an SVG `a`/`A` command draws, for the commands whose
/// x and y radii are equal and whose grid is y-down: the centre sits at
/// distance `r` from both ends, on the side the two flags pick, and the
/// angles run from +x towards +y, which is clockwise on screen. The returned
/// arc always walks the same points in the increasing direction.
fn svg_arc(from: (f64, f64), to: (f64, f64), r: f64, large_arc: bool, sweep: bool) -> Segment {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    debug_assert!(len > 0.0, "an SVG arc needs two distinct ends");
    // SVG scales a radius too small for its chord up to half the chord.
    let r = r.max(len / 2.0);
    let h = (r * r - len * len / 4.0).max(0.0).sqrt();
    // SVG F.6.5: push the midpoint to one side of the chord, the side the
    // large-arc and sweep flags pick.
    let sign = if large_arc == sweep { -1.0 } else { 1.0 };
    let cx = (from.0 + to.0) / 2.0 + sign * h * (-dy / len);
    let cy = (from.1 + to.1) / 2.0 + sign * h * (dx / len);
    let angle = |p: (f64, f64)| {
        let a = (p.1 - cy).atan2(p.0 - cx).to_degrees().rem_euclid(360.0);
        // Rounding leaves an angle that should be 0 just below 360; snapping
        // keeps "3 o'clock" at 0 instead of a full extra turn.
        if a > 360.0 - 1e-6 { 0.0 } else { a }
    };
    let (a0, a1) = (angle(from), angle(to));
    let (start_deg, end_deg) = if sweep { (a0, a1) } else { (a1, a0) };
    let mut end_deg = end_deg;
    if end_deg <= start_deg {
        end_deg += 360.0;
    }
    Segment::Arc { cx, cy, r, start_deg, end_deg }
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
            &Segment::Circle(cx, cy, r) => {
                let (ox, oy) = map_point(rect, (cx, cy));
                let radius = r / GRID * rect.2;
                let cg = CGRect {
                    origin: CGPoint { x: ox - radius, y: oy - radius },
                    size: CGSize { width: radius * 2.0, height: radius * 2.0 },
                };
                path.appendBezierPathWithOvalInRect(cg);
            }
            &Segment::Arc { cx, cy, r, start_deg, end_deg } => {
                let (ox, oy) = map_point(rect, (cx, cy));
                let radius = r / GRID * rect.2;
                // The view is flipped: angle t lands on
                // (cx + r cos t, cy + r sin t) with y growing downward, so
                // the increasing direction reads clockwise on screen and
                // `clockwise: false` sweeps start -> end the way we store it.
                path.appendBezierPathWithArcWithCenter_radius_startAngle_endAngle_clockwise(
                    CGPoint { x: ox, y: oy },
                    radius,
                    start_deg,
                    end_deg,
                    false,
                );
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
        assert_eq!(Icon::Mail.segments()[0], Segment::RoundRect(2.0, 4.0, 20.0, 16.0, 2.0));
        assert!(matches!(&Icon::Check.segments()[0], Segment::Polyline(p) if p.len() == 3 && p[1] == (9.0, 17.0)));
        // The view switcher: a panel with its rail, and a clock face.
        assert_eq!(Icon::Sidebar.segments()[0], Segment::RoundRect(3.0, 3.0, 18.0, 18.0, 2.0));
        assert!(matches!(&Icon::Sidebar.segments()[1], Segment::Polyline(p) if p[0].0 == p[1].0));
        assert_eq!(Icon::Clock.segments()[0], Segment::RoundRect(2.0, 2.0, 20.0, 20.0, 10.0));
    }

    #[test]
    fn every_icon_stays_on_the_grid() {
        assert_eq!(ALL.len(), 29);
        for icon in ALL {
            let segments = icon.segments();
            assert!(!segments.is_empty(), "{icon:?} draws nothing");
            for seg in segments {
                // The centreline stays inside 0..=24, so the 1 pt stroke
                // halo stays inside -1..=25.
                let (x0, y0, x1, y1) = match &seg {
                    Segment::Polyline(p) | Segment::Polygon(p) => p.iter().fold(
                        (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
                        |(x0, y0, x1, y1), &(x, y)| (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                    ),
                    Segment::RoundRect(x, y, w, h, _) => (*x, *y, x + w, y + h),
                    Segment::Circle(cx, cy, r) | Segment::Arc { cx, cy, r, .. } => {
                        (cx - r, cy - r, cx + r, cy + r)
                    }
                };
                assert!(
                    x0 >= 0.0 && y0 >= 0.0 && x1 <= GRID && y1 <= GRID,
                    "{icon:?}: {seg:?} leaves the grid"
                );
            }
        }
    }

    #[test]
    fn a_quarter_svg_arc_gives_its_centre_and_angles() {
        // SVG sweep 1 (increasing angle) from 3 o'clock to 6 o'clock on the
        // y-down grid: the arc bends through the lower right, centre (0, 0).
        let Segment::Arc { cx, cy, r, start_deg, end_deg } =
            svg_arc((1.0, 0.0), (0.0, 1.0), 1.0, false, true)
        else {
            panic!("arc")
        };
        assert!(cx.abs() < 1e-9 && cy.abs() < 1e-9 && (r - 1.0).abs() < 1e-9);
        assert!(start_deg.abs() < 1e-6 && (end_deg - 90.0).abs() < 1e-6);
        // The same quarter walked the other way (sweep 0) keeps its points:
        // stored increasing, so start 0 and end 90 again.
        let Segment::Arc { cx, cy, start_deg, end_deg, .. } =
            svg_arc((0.0, 1.0), (1.0, 0.0), 1.0, false, false)
        else {
            panic!("arc")
        };
        assert!(cx.abs() < 1e-9 && cy.abs() < 1e-9);
        assert!(start_deg.abs() < 1e-6 && (end_deg - 90.0).abs() < 1e-6);
        // The moon's outer stroke is the large arc, more than half a turn.
        let Segment::Arc { start_deg, end_deg, .. } =
            svg_arc((21.0, 12.79), (11.21, 3.0), 9.0, true, true)
        else {
            panic!("arc")
        };
        assert!(end_deg - start_deg > 180.0 && end_deg - start_deg < 360.0);
    }

    #[test]
    fn the_new_icons_keep_their_strokes() {
        assert_eq!(Icon::Ring.segments(), vec![Segment::Circle(12.0, 12.0, 7.0)]);
        assert_eq!(Icon::Pause.segments(), vec![
            Segment::RoundRect(6.0, 4.0, 4.0, 16.0, 1.0),
            Segment::RoundRect(14.0, 4.0, 4.0, 16.0, 1.0),
        ]);
        assert!(matches!(
            &Icon::Message.segments()[0],
            Segment::Arc { cx, cy, r, .. } if *cx == 19.0 && *cy == 15.0 && *r == 2.0
        ));
        assert!(matches!(&Icon::Send.segments()[1], Segment::Polygon(p) if p.len() == 4));
        assert!(matches!(&Icon::Mic.segments()[8], Segment::Polyline(p) if p == &[(12.0, 19.0), (12.0, 23.0)]));
        assert!(matches!(&Icon::Off.segments()[0], Segment::Circle(12.0, 12.0, 10.0)));
        assert_eq!(Icon::Sort.segments().len(), 2);
    }
}
