//! Box geometry and icon membership. Pure logic with no Windows calls, so it is unit-tested.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopBox {
    pub id: u64,
    pub name: String,
    pub rect: Rect,
    /// Shell parsing names of the icons in this box, in display order.
    pub members: Vec<String>,
    /// Rolled up to its title bar. `rect` keeps the open size, and the icons wait at
    /// `PARKED` until the box opens again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
}

impl DesktopBox {
    /// The part on screen: the whole box, or only its title bar when rolled up.
    pub fn shown(&self, m: &Metrics) -> Rect {
        if self.collapsed { Rect { h: m.title, ..self.rect } } else { self.rect }
    }

    /// Where each member belongs: its slot, or `PARKED` when the box is rolled up.
    pub fn targets(&self, m: &Metrics) -> Vec<Move> {
        self.members
            .iter()
            .enumerate()
            .map(|(i, key)| {
                let (x, y) = if self.collapsed { PARKED } else { m.slot(&self.rect, i) };
                Move { key: key.clone(), x, y }
            })
            .collect()
    }
}

/// Where a rolled-up box keeps its icons: past the edge of any real screen, and inside
/// the 16-bit range Explorer's icon list accepts.
pub const PARKED: (i32, i32) = (30000, 30000);

/// Where a box's members go while it rolls open or shut and shows `h` pixels of itself.
/// Icons can't be clipped, so each one waits, parked, until its whole cell is covered.
pub fn rolling_targets(b: &DesktopBox, m: &Metrics, h: i32) -> Vec<Move> {
    b.members
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let (x, y) = m.slot(&b.rect, i);
            let covered = y - m.icon_y + m.cell_h <= b.rect.y + h;
            let (x, y) = if covered { (x, y) } else { PARKED };
            Move { key: key.clone(), x, y }
        })
        .collect()
}

/// Puts the icons of rolled-up boxes back in their slots without opening the boxes, so
/// no icon is left out of reach when Organizer quits or a box goes away.
pub fn unpark_moves(boxes: &[DesktopBox], m: &Metrics) -> Vec<Move> {
    boxes
        .iter()
        .filter(|b| b.collapsed)
        .flat_map(|b| DesktopBox { collapsed: false, ..b.clone() }.targets(m))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Icon {
    pub key: String,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    pub key: String,
    pub x: i32,
    pub y: i32,
}

/// Sizes in view pixels. `cell_*` is the desktop's own icon spacing. An icon's reported
/// position is the corner of its image, which sits `icon_x`/`icon_y` inside its cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub title: i32,
    pub border: i32,
    pub pad: i32,
    pub cell_w: i32,
    pub cell_h: i32,
    pub icon_x: i32,
    pub icon_y: i32,
}

impl Metrics {
    pub fn columns(&self, width: i32) -> i32 {
        ((width - 2 * (self.border + self.pad)) / self.cell_w).max(1)
    }

    /// Icon position for a slot. The cell, not just the image, fits inside the box.
    pub fn slot(&self, r: &Rect, index: usize) -> (i32, i32) {
        let cols = self.columns(r.w) as usize;
        (
            r.x + self.border + self.pad + self.icon_x + (index % cols) as i32 * self.cell_w,
            r.y + self.title + self.pad + self.icon_y + (index / cols) as i32 * self.cell_h,
        )
    }

    pub fn nearest_slot(&self, r: &Rect, x: i32, y: i32, len: usize) -> usize {
        let cols = self.columns(r.w);
        let left = x - self.icon_x - r.x - self.border - self.pad;
        let top = y - self.icon_y - r.y - self.title - self.pad;
        let col = ((left + self.cell_w / 2) / self.cell_w).clamp(0, cols - 1);
        let row = ((top + self.cell_h / 2) / self.cell_h).max(0);
        ((row * cols + col) as usize).min(len.saturating_sub(1))
    }

    /// How far in from an edge the mouse grabs it for resizing.
    pub fn grip(&self) -> i32 {
        self.border.max(1) + 3
    }

    pub fn min_width(&self) -> i32 {
        2 * (self.border + self.pad) + self.cell_w
    }

    pub fn height_for(&self, width: i32, count: usize) -> i32 {
        let cols = self.columns(width) as usize;
        let rows = count.div_ceil(cols).max(1) as i32;
        self.title + 2 * self.pad + self.border + rows * self.cell_h
    }

    fn center(&self, x: i32, y: i32) -> (i32, i32) {
        (x - self.icon_x + self.cell_w / 2, y - self.icon_y + self.cell_h / 2)
    }
}

/// Pushes `r` back inside `area` where it fits. The top-left wins if it is too big,
/// so the title bar always stays reachable.
pub fn fit_rect(r: Rect, area: &Rect) -> Rect {
    Rect {
        x: r.x.min(area.x + area.w - r.w).max(area.x),
        y: r.y.min(area.y + area.h - r.h).max(area.y),
        ..r
    }
}

/// Which edges a resize drag moves. No edges set means the whole box moves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    pub fn is_move(&self) -> bool {
        *self == Edges::default()
    }
}

pub fn drag_rect(start: Rect, e: Edges, dx: i32, dy: i32, min_w: i32, min_h: i32) -> Rect {
    if e.is_move() {
        return Rect { x: start.x + dx, y: start.y + dy, ..start };
    }
    let (mut left, mut top) = (start.x, start.y);
    let (mut right, mut bottom) = (start.x + start.w, start.y + start.h);
    if e.left {
        left = (left + dx).min(right - min_w);
    }
    if e.right {
        right = (right + dx).max(left + min_w);
    }
    if e.top {
        top = (top + dy).min(bottom - min_h);
    }
    if e.bottom {
        bottom = (bottom + dy).max(top + min_h);
    }
    Rect { x: left, y: top, w: right - left, h: bottom - top }
}

/// Finds a spot for a new box that covers no icons and no other box, so creating
/// a box never swallows icons. Scans from the top-right, where icons are least likely.
pub fn free_spot(area: &Rect, w: i32, h: i32, icons: &[Icon], boxes: &[DesktopBox], m: &Metrics) -> Rect {
    let mut y = area.y + 16;
    while y + h <= area.y + area.h {
        let mut x = area.x + area.w - w - 16;
        while x >= area.x {
            let r = Rect { x, y, w, h };
            let clear_of_icons = icons
                .iter()
                .all(|i| !r.intersects(&Rect { x: i.x - m.icon_x, y: i.y - m.icon_y, w: m.cell_w, h: m.cell_h }));
            if clear_of_icons && boxes.iter().all(|b| !r.intersects(&b.rect)) {
                return r;
            }
            x -= 32;
        }
        y += 32;
    }
    fit_rect(Rect { x: area.x + (area.w - w) / 2, y: area.y + (area.h - h) / 2, w, h }, area)
}

/// Grows a box to hold its members, then keeps the part on screen in its work area.
/// A rolled-up box near the bottom stays put; it moves up only once it opens.
fn settle_rect(b: &mut DesktopBox, m: &Metrics, area_of: &dyn Fn(&Rect) -> Rect) -> bool {
    let before = b.rect;
    b.rect.w = b.rect.w.max(m.min_width());
    b.rect.h = b.rect.h.max(m.height_for(b.rect.w, b.members.len()));
    let shown = b.shown(m);
    let fitted = fit_rect(shown, &area_of(&shown));
    (b.rect.x, b.rect.y) = (fitted.x, fitted.y);
    b.rect != before
}

/// Moves every member to its slot, regardless of where it is now. Used when a box was
/// moved or resized, and at startup, when a position outside the box means the box or
/// the screen changed rather than that the user dragged the icon out.
pub fn place_members(boxes: &mut [DesktopBox], m: &Metrics, area_of: &dyn Fn(&Rect) -> Rect) -> Vec<Move> {
    let mut moves = Vec::new();
    for b in boxes {
        settle_rect(b, m, area_of);
        moves.extend(b.targets(m));
    }
    moves
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// Membership, order or geometry changed, so state should be saved.
    pub changed: bool,
    /// Boxes whose rectangle changed, so their windows need updating.
    pub resized: Vec<u64>,
    pub moves: Vec<Move>,
}

/// Compares the icons' actual positions with the boxes and works out what the user did:
/// an icon dragged out leaves its box, one dropped in joins (at the drop position), one
/// dragged within its box is reordered, and deleted files drop out. A rolled-up box keeps
/// its parked icons wherever they are, and takes icons dropped on its title bar.
pub fn reconcile(
    boxes: &mut [DesktopBox],
    icons: &[Icon],
    m: &Metrics,
    area_of: &dyn Fn(&Rect) -> Rect,
) -> Outcome {
    let mut out = Outcome::default();
    let by_key: HashMap<&str, &Icon> = icons.iter().map(|i| (i.key.as_str(), i)).collect();

    for b in boxes.iter_mut() {
        let (rect, collapsed, before) = (b.rect, b.collapsed, b.members.len());
        b.members.retain(|k| {
            by_key.get(k.as_str()).is_some_and(|i| {
                let (cx, cy) = m.center(i.x, i.y);
                collapsed || rect.contains(cx, cy)
            })
        });
        out.changed |= b.members.len() != before;
    }

    let owned: HashSet<String> = boxes.iter().flat_map(|b| b.members.iter().cloned()).collect();
    for icon in icons.iter().filter(|i| !owned.contains(&i.key)) {
        let (cx, cy) = m.center(icon.x, icon.y);
        if let Some(b) = boxes.iter_mut().find(|b| b.shown(m).contains(cx, cy)) {
            b.members.push(icon.key.clone());
            out.changed = true;
        }
    }

    for b in boxes.iter_mut() {
        if settle_rect(b, m, area_of) {
            out.resized.push(b.id);
            out.changed = true;
        }

        // Nothing to reorder while rolled up. Icons that drifted out of the parking spot,
        // such as one just dropped on the title bar, go back to it.
        if b.collapsed {
            out.moves.extend(b.targets(m).into_iter().filter(|t| {
                let icon = by_key[t.key.as_str()];
                (icon.x, icon.y) != (t.x, t.y)
            }));
            continue;
        }

        // Only icons sitting off the grid were placed by the user. Icons still on a
        // slot must keep their order, even when a removal has shifted their indices.
        let cols = m.columns(b.rect.w) as usize;
        let slot_count = b.members.len().div_ceil(cols) * cols;
        let rect = b.rect;
        let on_grid = |i: &Icon| {
            (0..slot_count).any(|s| {
                let (x, y) = m.slot(&rect, s);
                (i.x - x).abs() <= 2 && (i.y - y).abs() <= 2
            })
        };
        let dropped: Vec<String> = b
            .members
            .iter()
            .filter(|k| !on_grid(by_key[k.as_str()]))
            .cloned()
            .collect();
        for key in dropped {
            let icon = by_key[key.as_str()];
            let from = b.members.iter().position(|k| *k == key).unwrap();
            let to = m.nearest_slot(&b.rect, icon.x, icon.y, b.members.len());
            if from != to {
                let k = b.members.remove(from);
                b.members.insert(to, k);
                out.changed = true;
            }
        }

        for (i, key) in b.members.iter().enumerate() {
            let (x, y) = m.slot(&b.rect, i);
            let icon = by_key[key.as_str()];
            if icon.x != x || icon.y != y {
                out.moves.push(Move { key: key.clone(), x, y });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: Metrics = Metrics { title: 30, border: 4, pad: 6, cell_w: 100, cell_h: 100, icon_x: 0, icon_y: 0 };
    // What a real desktop reports: a 76x100 cell with a 48px icon centered in it.
    const REAL: Metrics = Metrics { title: 26, border: 3, pad: 4, cell_w: 76, cell_h: 100, icon_x: 14, icon_y: 2 };
    const AREA: Rect = Rect { x: 0, y: 0, w: 1920, h: 1040 };

    fn area(_: &Rect) -> Rect {
        AREA
    }
    fn make_box(members: &[&str]) -> DesktopBox {
        DesktopBox {
            id: 1,
            name: "t".into(),
            rect: Rect { x: 100, y: 100, w: 320, h: 400 },
            members: members.iter().map(|s| s.to_string()).collect(),
            collapsed: false,
        }
    }
    fn rolled_up(members: &[&str]) -> DesktopBox {
        DesktopBox { collapsed: true, ..make_box(members) }
    }
    fn icon(key: &str, (x, y): (i32, i32)) -> Icon {
        Icon { key: key.into(), x, y }
    }
    fn slot(b: &DesktopBox, i: usize) -> (i32, i32) {
        M.slot(&b.rect, i)
    }
    fn members(b: &DesktopBox) -> Vec<&str> {
        b.members.iter().map(String::as_str).collect()
    }

    #[test]
    fn slots_wrap_into_rows() {
        let b = make_box(&[]);
        assert_eq!(M.columns(b.rect.w), 3);
        assert_eq!(slot(&b, 0), (110, 136));
        assert_eq!(slot(&b, 2), (310, 136));
        assert_eq!(slot(&b, 3), (110, 236));
    }

    #[test]
    fn whole_icon_cell_stays_inside_the_box() {
        let r = Rect { x: 870, y: 27, w: 318, h: 337 };
        for i in 0..8 {
            let (x, y) = REAL.slot(&r, i);
            let (left, top) = (x - REAL.icon_x, y - REAL.icon_y);
            assert!(left >= r.x + REAL.border, "slot {i} starts left of the border");
            assert!(left + REAL.cell_w <= r.x + r.w - REAL.border, "slot {i} ends right of the border");
            assert!(top >= r.y + REAL.title, "slot {i} starts above the content area");
        }
    }

    #[test]
    fn slot_positions_round_trip_with_icon_offsets() {
        let r = Rect { x: 870, y: 27, w: 318, h: 337 };
        for i in 0..8 {
            let (x, y) = REAL.slot(&r, i);
            assert_eq!(REAL.nearest_slot(&r, x, y, 8), i);
            let (cx, cy) = REAL.center(x, y);
            assert!(r.contains(cx, cy), "slot {i} center is outside the box");
        }
    }

    #[test]
    fn settled_box_produces_no_work() {
        let mut bs = [make_box(&["a", "b"])];
        let icons = [icon("a", slot(&bs[0], 0)), icon("b", slot(&bs[0], 1))];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert!(!o.changed && o.moves.is_empty() && o.resized.is_empty());
    }

    #[test]
    fn icon_dropped_in_box_joins_at_drop_position() {
        let mut bs = [make_box(&["a", "b"])];
        let (s0, s1) = (slot(&bs[0], 0), slot(&bs[0], 1));
        let icons = [icon("a", s0), icon("b", s1), icon("new", (s0.0 + 5, s0.1 + 5))];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["new", "a", "b"]);
        assert!(o.changed);
        assert_eq!(o.moves.len(), 3);
    }

    #[test]
    fn icon_dragged_out_leaves_and_others_keep_their_order() {
        let mut bs = [make_box(&["a", "b", "c", "d"])];
        let icons = [
            icon("a", slot(&bs[0], 0)),
            icon("b", (1500, 800)),
            icon("c", slot(&bs[0], 2)),
            icon("d", slot(&bs[0], 3)),
        ];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["a", "c", "d"]);
        assert!(o.moves.iter().all(|m| m.key != "b"), "a removed icon stays where it was dropped");
        assert_eq!(o.moves.iter().find(|m| m.key == "c").map(|m| (m.x, m.y)), Some(slot(&bs[0], 1)));
    }

    #[test]
    fn deleted_file_drops_out() {
        let mut bs = [make_box(&["a", "gone"])];
        let icons = [icon("a", slot(&bs[0], 0))];
        assert!(reconcile(&mut bs, &icons, &M, &area).changed);
        assert_eq!(members(&bs[0]), ["a"]);
    }

    #[test]
    fn drag_within_box_reorders() {
        let mut bs = [make_box(&["a", "b", "c"])];
        let s2 = slot(&bs[0], 2);
        let icons = [icon("a", (s2.0 + 7, s2.1)), icon("b", slot(&bs[0], 1)), icon("c", s2)];
        reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["b", "c", "a"]);
    }

    #[test]
    fn loose_icons_outside_every_box_are_untouched() {
        let mut bs = [make_box(&[])];
        let o = reconcile(&mut bs, &[icon("x", (1500, 800))], &M, &area);
        assert!(!o.changed && o.moves.is_empty());
    }

    #[test]
    fn box_grows_when_too_full() {
        let mut bs = [make_box(&[])];
        bs[0].rect.h = 100;
        let s = slot(&bs[0], 0);
        let icons: Vec<_> = (0..7).map(|i| icon(&format!("i{i}"), (s.0 + i, s.1))).collect();
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(o.resized, [1]);
        assert_eq!(bs[0].rect.h, M.height_for(320, 7));
        assert_eq!(bs[0].members.len(), 7);
    }

    #[test]
    fn growth_past_the_screen_edge_shifts_box_up() {
        let mut bs = [make_box(&["a"])];
        bs[0].rect = Rect { x: 100, y: 950, w: 320, h: 100 };
        reconcile(&mut bs, &[icon("a", (110, 986))], &M, &area);
        assert_eq!(bs[0].rect.y + bs[0].rect.h, AREA.h);
    }

    #[test]
    fn placing_moves_all_members_even_when_far_away() {
        let mut bs = [make_box(&["a", "b"])];
        let moves = place_members(&mut bs, &M, &area);
        assert_eq!(moves[1], Move { key: "b".into(), x: 210, y: 136 });
    }

    #[test]
    fn free_spot_avoids_icons_and_boxes() {
        let icons = [icon("x", (1500, 16))];
        let bs = [make_box(&[])];
        let r = free_spot(&AREA, 340, 240, &icons, &bs, &M);
        assert!(!r.intersects(&bs[0].rect));
        assert!(!r.intersects(&Rect { x: 1500, y: 16, w: 100, h: 100 }));
        assert!(r.x >= 0 && r.x + r.w <= AREA.w);
    }

    #[test]
    fn free_spot_falls_back_to_center_when_full() {
        let icons: Vec<_> = (0..20)
            .flat_map(|x| (0..11).map(move |y| icon("i", (x * 100, y * 100))))
            .collect();
        assert_eq!(free_spot(&AREA, 340, 240, &icons, &[], &M), Rect { x: 790, y: 400, w: 340, h: 240 });
    }

    #[test]
    fn rolled_up_box_shows_only_its_title_and_parks_its_icons() {
        let mut bs = [rolled_up(&["a", "b"])];
        assert_eq!(bs[0].shown(&M), Rect { h: M.title, ..bs[0].rect });
        let moves = place_members(&mut bs, &M, &area);
        assert!(moves.iter().all(|m| (m.x, m.y) == PARKED), "{moves:?}");
    }

    #[test]
    fn rolled_up_box_keeps_its_parked_icons() {
        let mut bs = [rolled_up(&["a", "b"])];
        let icons = [icon("a", PARKED), icon("b", PARKED)];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["a", "b"]);
        assert!(!o.changed && o.moves.is_empty());
    }

    #[test]
    fn rolled_up_box_parks_an_icon_that_came_back() {
        let mut bs = [rolled_up(&["a", "b"])];
        let icons = [icon("a", PARKED), icon("b", (1500, 800))];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["a", "b"]);
        assert_eq!(o.moves, [Move { key: "b".into(), x: PARKED.0, y: PARKED.1 }]);
    }

    #[test]
    fn icon_dropped_on_a_rolled_up_title_joins_and_is_parked() {
        let mut bs = [rolled_up(&["a"])];
        // Its cell's center, which decides where an icon is, lands on the title bar.
        let icons = [icon("a", PARKED), icon("new", (150, 60))];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["a", "new"]);
        assert!(o.changed);
        assert_eq!(o.moves, [Move { key: "new".into(), x: PARKED.0, y: PARKED.1 }]);
    }

    #[test]
    fn icons_where_a_rolled_up_body_would_be_stay_loose() {
        let mut bs = [rolled_up(&[])];
        let o = reconcile(&mut bs, &[icon("x", (150, 300))], &M, &area);
        assert!(bs[0].members.is_empty() && !o.changed && o.moves.is_empty());
    }

    #[test]
    fn rolled_up_box_at_the_bottom_is_not_pushed_up() {
        let mut bs = [rolled_up(&["a"])];
        bs[0].rect.y = AREA.h - M.title;
        place_members(&mut bs, &M, &area);
        assert_eq!(bs[0].rect.y, AREA.h - M.title);
    }

    #[test]
    fn unparking_puts_icons_in_their_slots_and_leaves_boxes_rolled_up() {
        let bs = [rolled_up(&["a", "b"]), make_box(&["c"])];
        let moves = unpark_moves(&bs, &M);
        assert_eq!(moves, [
            Move { key: "a".into(), x: slot(&bs[0], 0).0, y: slot(&bs[0], 0).1 },
            Move { key: "b".into(), x: slot(&bs[0], 1).0, y: slot(&bs[0], 1).1 },
        ]);
        assert!(bs[0].collapsed);
    }

    #[test]
    fn rolling_shows_icons_row_by_row_as_the_box_covers_them() {
        let b = make_box(&["a", "b", "c", "d"]); // 3 columns: a b c, then d
        let parked = |h| rolling_targets(&b, &M, h).iter().filter(|t| (t.x, t.y) == PARKED).count();
        assert_eq!(parked(M.title), 4, "only the title bar: all tucked away");
        assert_eq!(parked(M.title + M.pad + M.cell_h), 1, "first row covered");
        assert_eq!(parked(M.height_for(b.rect.w, 4)), 0, "fully open");
        let open = rolling_targets(&b, &M, M.height_for(b.rect.w, 4));
        assert_eq!(open, DesktopBox { collapsed: false, ..b.clone() }.targets(&M));
    }

    #[test]
    fn boxes_saved_before_roll_up_existed_load_open() {
        let json = r#"{"id":1,"name":"t","rect":{"x":0,"y":0,"w":10,"h":10},"members":[]}"#;
        let b: DesktopBox = serde_json::from_str(json).unwrap();
        assert!(!b.collapsed);
    }

    #[test]
    fn dragging_respects_edges_and_minimum_size() {
        let s = Rect { x: 100, y: 100, w: 300, h: 300 };
        assert_eq!(drag_rect(s, Edges::default(), 5, -5, 50, 50), Rect { x: 105, y: 95, w: 300, h: 300 });
        let right = Edges { right: true, ..Default::default() };
        assert_eq!(drag_rect(s, right, 20, 99, 50, 50), Rect { x: 100, y: 100, w: 320, h: 300 });
        let tl = Edges { left: true, top: true, ..Default::default() };
        assert_eq!(drag_rect(s, tl, 1000, 10, 50, 50), Rect { x: 350, y: 110, w: 50, h: 290 });
    }
}
