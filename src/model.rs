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

/// How a box places its icons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arrange {
    /// One after another with no gaps. A new icon goes after the last one; dragging an
    /// icon inside the box moves it in the order.
    #[default]
    Auto,
    /// Each icon stays in the cell it was dropped in, so there can be empty cells.
    /// Dropping on a taken cell swaps the two icons.
    Free,
}

fn is_auto(a: &Arrange) -> bool {
    *a == Arrange::Auto
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DesktopBox {
    pub id: u64,
    pub name: String,
    pub rect: Rect,
    /// Shell parsing names of the icons in this box, in display order: index `i` is the
    /// box's `i`th cell. In a free box an empty string marks an empty cell.
    pub members: Vec<String>,
    /// Rolled up to its title bar. `rect` keeps the open size, and the icons wait at
    /// `Metrics::park` until the box opens again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
    #[serde(default, skip_serializing_if = "is_auto")]
    pub arrange: Arrange,
}

impl DesktopBox {
    /// The part on screen: the whole box, or only its title bar when rolled up.
    pub fn shown(&self, m: &Metrics) -> Rect {
        if self.collapsed { Rect { h: m.title, ..self.rect } } else { self.rect }
    }

    /// The icons with their cells, skipping empty cells.
    fn icons(&self) -> impl Iterator<Item = (usize, &String)> {
        self.members.iter().enumerate().filter(|(_, k)| !k.is_empty())
    }

    /// Where each member belongs: its slot, or the parking spot when the box is rolled up.
    pub fn targets(&self, m: &Metrics) -> Vec<Move> {
        self.icons()
            .map(|(i, key)| {
                let (x, y) = if self.collapsed { m.park } else { m.slot(&self.rect, i) };
                Move { key: key.clone(), x, y }
            })
            .collect()
    }

    /// Switching to automatic closes the gaps, keeping the icons' order.
    pub fn set_arrange(&mut self, arrange: Arrange) {
        self.arrange = arrange;
        if arrange == Arrange::Auto {
            self.members.retain(|k| !k.is_empty());
        }
    }

    fn trim_gaps(&mut self) {
        while self.members.last().is_some_and(String::is_empty) {
            self.members.pop();
        }
    }

    /// Makes cell `cell` exist, adding empty cells before it if needed.
    fn reach(&mut self, cell: usize) {
        if self.members.len() <= cell {
            self.members.resize(cell + 1, String::new());
        }
    }

    /// The empty cell closest to `cell` on the grid, `cell` itself if it is empty, or the
    /// cell after the last icon if the box is full.
    fn empty_cell_near(&self, cell: usize, m: &Metrics) -> usize {
        let cols = m.columns(self.rect.w) as usize;
        let taken = |i: usize| self.members.get(i).is_some_and(|k| !k.is_empty());
        let distance = |i: usize| (i / cols).abs_diff(cell / cols) + (i % cols).abs_diff(cell % cols);
        (0..m.capacity(&self.rect).max(cell + 1))
            .filter(|&i| !taken(i))
            .min_by_key(|&i| (distance(i), i))
            .unwrap_or(self.members.len())
    }
}

/// Where versions before 1.1 parked icons, far past any screen. Explorer moves icons it
/// finds outside the desktop back onto it after a drop, so icons are no longer parked here,
/// but ones left here by an older version are still recognized.
const LEGACY_PARKED: (i32, i32) = (30000, 30000);

/// Where a box's members go while it rolls open or shut and shows `h` pixels of itself.
/// Icons can't be clipped, so each one waits, parked, until its whole cell is covered.
pub fn rolling_targets(b: &DesktopBox, m: &Metrics, h: i32) -> Vec<Move> {
    b.icons()
        .map(|(i, key)| {
            let (x, y) = m.slot(&b.rect, i);
            let covered = y - m.icon_y + m.cell_h <= b.rect.y + h;
            let (x, y) = if covered { (x, y) } else { m.park };
            Move { key: key.clone(), x, y }
        })
        .collect()
}

/// Icons of rolled-up boxes that are not parked any more. Explorer pulls icons it finds
/// outside the desktop back in after something is dropped on it.
pub fn strays(boxes: &[DesktopBox], icons: &[Icon], m: &Metrics) -> Vec<Move> {
    let by_key: HashMap<&str, &Icon> = icons.iter().map(|i| (i.key.as_str(), i)).collect();
    boxes
        .iter()
        .filter(|b| b.collapsed)
        .flat_map(|b| b.icons())
        .filter(|(_, k)| by_key.get(k.as_str()).is_some_and(|i| (i.x, i.y) != m.park))
        .map(|(_, k)| Move { key: k.clone(), x: m.park.0, y: m.park.1 })
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
    /// Where rolled-up boxes keep their icons, as an icon position. See `Desktop::metrics`.
    pub park: (i32, i32),
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

    /// How many cells fit in the box at its current size.
    pub fn capacity(&self, r: &Rect) -> usize {
        let rows = ((r.h - self.title - 2 * self.pad - self.border) / self.cell_h).max(1);
        (rows * self.columns(r.w)) as usize
    }

    /// The cell nearest to an icon position, anywhere in the box, empty or not.
    pub fn cell_at(&self, r: &Rect, x: i32, y: i32) -> usize {
        let cols = self.columns(r.w);
        let rows = self.capacity(r) as i32 / cols;
        let left = x - self.icon_x - r.x - self.border - self.pad;
        let top = y - self.icon_y - r.y - self.title - self.pad;
        let col = ((left + self.cell_w / 2) / self.cell_w).clamp(0, cols - 1);
        let row = ((top + self.cell_h / 2) / self.cell_h).clamp(0, rows - 1);
        (row * cols + col) as usize
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
/// an icon dragged out leaves its box, one dropped in joins (after the last icon, or in a
/// free box in the cell it was dropped on), one dragged within its box is moved, and
/// deleted files drop out. A rolled-up box keeps
/// its parked icons wherever they are, and takes icons dropped on its title bar.
///
/// A rolled-up box also keeps members missing from `icons`: while Explorer reloads the
/// desktop (F5, or a setting that changes which icons it shows) the list is incomplete, and
/// a member dropped then would stay parked, out of reach, with no box to bring it back.
/// Members that really are gone drop out once the box opens. Any parked icon that no box
/// owns is brought back to a free spot.
pub fn reconcile(
    boxes: &mut [DesktopBox],
    icons: &[Icon],
    m: &Metrics,
    area_of: &dyn Fn(&Rect) -> Rect,
) -> Outcome {
    let mut out = Outcome::default();
    let by_key: HashMap<&str, &Icon> = icons.iter().map(|i| (i.key.as_str(), i)).collect();

    for b in boxes.iter_mut() {
        let (rect, collapsed) = (b.rect, b.collapsed);
        let stays = |k: &String| {
            k.is_empty()
                || collapsed
                || by_key.get(k.as_str()).is_some_and(|i| {
                    let (cx, cy) = m.center(i.x, i.y);
                    rect.contains(cx, cy)
                })
        };
        match b.arrange {
            Arrange::Auto => {
                let before = b.members.len();
                b.members.retain(stays);
                out.changed |= b.members.len() != before;
            }
            // An icon leaving a free box leaves its cell empty, so the others stay put.
            Arrange::Free => {
                for k in b.members.iter_mut().filter(|k| !stays(k)) {
                    k.clear();
                    out.changed = true;
                }
                b.trim_gaps();
            }
        }
    }

    // Icons that just joined are where the box put them, not where they were dropped.
    let mut joined: HashSet<&str> = HashSet::new();
    let owned: HashSet<String> = boxes.iter().flat_map(|b| b.members.iter().cloned()).collect();
    for icon in icons.iter().filter(|i| !owned.contains(&i.key)) {
        let (cx, cy) = m.center(icon.x, icon.y);
        if let Some(b) = boxes.iter_mut().find(|b| b.shown(m).contains(cx, cy)) {
            match b.arrange {
                Arrange::Auto => b.members.push(icon.key.clone()),
                Arrange::Free => {
                    // Dropped on a rolled-up title bar there is no cell to aim at.
                    let aim = if b.collapsed { 0 } else { m.cell_at(&b.rect, icon.x, icon.y) };
                    let cell = b.empty_cell_near(aim, m);
                    b.reach(cell);
                    b.members[cell] = icon.key.clone();
                }
            }
            joined.insert(icon.key.as_str());
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
                by_key.get(t.key.as_str()).is_some_and(|icon| (icon.x, icon.y) != (t.x, t.y))
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
            .icons()
            .filter(|(_, k)| !joined.contains(k.as_str()) && !on_grid(by_key[k.as_str()]))
            .map(|(_, k)| k.clone())
            .collect();
        for key in dropped {
            let icon = by_key[key.as_str()];
            let from = b.members.iter().position(|k| *k == key).unwrap();
            match b.arrange {
                Arrange::Auto => {
                    let to = m.nearest_slot(&b.rect, icon.x, icon.y, b.members.len());
                    if from != to {
                        let k = b.members.remove(from);
                        b.members.insert(to, k);
                        out.changed = true;
                    }
                }
                // Into the cell it was dropped on: an empty one, or a swap with its icon.
                Arrange::Free => {
                    let to = m.cell_at(&b.rect, icon.x, icon.y);
                    if from != to {
                        b.reach(to);
                        b.members.swap(from, to);
                        b.trim_gaps();
                        out.changed = true;
                    }
                }
            }
        }

        for (i, key) in b.icons() {
            let (x, y) = m.slot(&b.rect, i);
            let icon = by_key[key.as_str()];
            if icon.x != x || icon.y != y {
                out.moves.push(Move { key: key.clone(), x, y });
            }
        }
    }

    let owned: HashSet<&str> = boxes.iter().flat_map(|b| b.members.iter().map(String::as_str)).collect();
    let mut taken: Vec<(i32, i32)> = icons.iter().filter(|i| !is_parked(m, i.x, i.y)).map(|i| (i.x, i.y)).collect();
    for icon in icons.iter().filter(|i| is_parked(m, i.x, i.y) && !owned.contains(i.key.as_str())) {
        let area = area_of(&Rect { x: 0, y: 0, w: 1, h: 1 });
        if let Some((x, y)) = free_cell(&area, boxes, &taken, m) {
            out.moves.push(Move { key: icon.key.clone(), x, y });
            taken.push((x, y));
        }
    }
    out
}

/// True when most icons in open boxes are suddenly outside them at once. A person drags
/// one icon or a handful; Explorer resetting its layout (some desktop reloads do) moves them
/// all. Read as drags, a reset would empty the boxes.
pub fn looks_like_reset(boxes: &[DesktopBox], icons: &[Icon], m: &Metrics) -> bool {
    let by_key: HashMap<&str, &Icon> = icons.iter().map(|i| (i.key.as_str(), i)).collect();
    let (mut present, mut outside) = (0, 0);
    for b in boxes.iter().filter(|b| !b.collapsed) {
        for icon in b.members.iter().filter_map(|k| by_key.get(k.as_str())) {
            present += 1;
            let (cx, cy) = m.center(icon.x, icon.y);
            outside += usize::from(!b.rect.contains(cx, cy));
        }
    }
    outside >= 3 && outside * 2 > present
}

/// Puts the desktop back as it was before Explorer reloaded it: every box's icons in their
/// slots, and loose icons where `before` had them. Icons that are new since then stay where
/// Explorer put them. Boxes keep their members.
pub fn restore_layout(
    boxes: &mut [DesktopBox],
    before: &[Icon],
    now: &[Icon],
    m: &Metrics,
    area_of: &dyn Fn(&Rect) -> Rect,
) -> Vec<Move> {
    let mut moves = place_members(boxes, m, area_of);
    let owned: HashSet<&str> = boxes.iter().flat_map(|b| b.members.iter().map(String::as_str)).collect();
    let was: HashMap<&str, &Icon> = before.iter().map(|i| (i.key.as_str(), i)).collect();
    for icon in now.iter().filter(|i| !owned.contains(i.key.as_str())) {
        if let Some(old) = was.get(icon.key.as_str()).filter(|old| (old.x, old.y) != (icon.x, icon.y)) {
            moves.push(Move { key: icon.key.clone(), x: old.x, y: old.y });
        }
    }
    moves
}

/// At the parking spot, or the one older versions used, which no real screen reaches.
fn is_parked(m: &Metrics, x: i32, y: i32) -> bool {
    (x, y) == m.park || (x >= LEGACY_PARKED.0 - 1000 && y >= LEGACY_PARKED.1 - 1000)
}

/// The first grid cell, column by column from the top-left as Explorer arranges icons,
/// that no box (open size) and no icon covers. Returns the icon position for that cell.
fn free_cell(area: &Rect, boxes: &[DesktopBox], icons: &[(i32, i32)], m: &Metrics) -> Option<(i32, i32)> {
    let (mut x, right, bottom) = (area.x, area.x + area.w, area.y + area.h);
    while x + m.cell_w <= right {
        let mut y = area.y;
        while y + m.cell_h <= bottom {
            let cell = Rect { x, y, w: m.cell_w, h: m.cell_h };
            let free = boxes.iter().all(|b| !cell.intersects(&b.rect))
                && icons.iter().all(|&(ix, iy)| {
                    !cell.intersects(&Rect { x: ix - m.icon_x, y: iy - m.icon_y, w: m.cell_w, h: m.cell_h })
                });
            if free {
                return Some((x + m.icon_x, y + m.icon_y));
            }
            y += m.cell_h;
        }
        x += m.cell_w;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Parked with one pixel of the cell inside the right edge of AREA, as on a real desktop.
    const M: Metrics =
        Metrics { title: 30, border: 4, pad: 6, cell_w: 100, cell_h: 100, icon_x: 0, icon_y: 0, park: (1919, 0) };
    // What a real desktop reports: a 76x100 cell with a 48px icon centered in it.
    const REAL: Metrics =
        Metrics { title: 26, border: 3, pad: 4, cell_w: 76, cell_h: 100, icon_x: 14, icon_y: 2, park: (1933, 2) };
    const AREA: Rect = Rect { x: 0, y: 0, w: 1920, h: 1040 };
    const PARKED: (i32, i32) = M.park;

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
            arrange: Arrange::Auto,
        }
    }
    fn free_box(members: &[&str]) -> DesktopBox {
        DesktopBox { arrange: Arrange::Free, ..make_box(members) }
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
    fn icon_dropped_in_an_automatic_box_goes_after_the_last_one() {
        let mut bs = [make_box(&["a", "b"])];
        let (s0, s1) = (slot(&bs[0], 0), slot(&bs[0], 1));
        // Dropped on the first cell, it still goes last.
        let icons = [icon("a", s0), icon("b", s1), icon("new", (s0.0 + 5, s0.1 + 5))];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["a", "b", "new"]);
        assert!(o.changed);
        assert_eq!(o.moves, [Move { key: "new".into(), x: slot(&bs[0], 2).0, y: slot(&bs[0], 2).1 }]);
    }

    #[test]
    fn icon_dropped_in_a_free_box_stays_in_its_cell() {
        let mut bs = [free_box(&["a"])];
        let s4 = slot(&bs[0], 4); // second row, middle
        let icons = [icon("a", slot(&bs[0], 0)), icon("new", (s4.0 + 8, s4.1 - 6))];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["a", "", "", "", "new"]);
        assert_eq!(o.moves, [Move { key: "new".into(), x: s4.0, y: s4.1 }]);
    }

    #[test]
    fn icon_dropped_on_a_taken_cell_of_a_free_box_takes_the_nearest_empty_one() {
        let mut bs = [free_box(&["a"])];
        let s0 = slot(&bs[0], 0);
        let icons = [icon("a", s0), icon("new", (s0.0 + 5, s0.1 + 5))];
        reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["a", "new"]);
    }

    #[test]
    fn icon_leaving_a_free_box_leaves_a_gap() {
        let mut bs = [free_box(&["a", "b", "c"])];
        let icons = [icon("a", slot(&bs[0], 0)), icon("b", (1500, 800)), icon("c", slot(&bs[0], 2))];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["a", "", "c"]);
        assert!(o.moves.is_empty(), "the others stay where they are: {:?}", o.moves);
    }

    #[test]
    fn dragging_onto_a_taken_cell_of_a_free_box_swaps() {
        let mut bs = [free_box(&["a", "b", "c"])];
        let s2 = slot(&bs[0], 2);
        let icons = [icon("a", (s2.0 + 7, s2.1)), icon("b", slot(&bs[0], 1)), icon("c", s2)];
        reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["c", "b", "a"]);
    }

    #[test]
    fn dragging_into_an_empty_cell_of_a_free_box_moves_there() {
        let mut bs = [free_box(&["a", "b"])];
        let s5 = slot(&bs[0], 5);
        let icons = [icon("a", (s5.0 - 4, s5.1 + 3)), icon("b", slot(&bs[0], 1))];
        reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["", "b", "", "", "", "a"]);
    }

    #[test]
    fn switching_a_free_box_to_automatic_closes_the_gaps() {
        let mut b = free_box(&["a", "", "", "b"]);
        b.set_arrange(Arrange::Auto);
        assert_eq!(members(&b), ["a", "b"]);
        let moves = b.targets(&M);
        assert_eq!(moves.len(), 2);
    }

    #[test]
    fn empty_cells_are_never_moved_or_parked() {
        let mut b = free_box(&["a", "", "b"]);
        assert_eq!(b.targets(&M).len(), 2);
        b.collapsed = true;
        assert!(b.targets(&M).iter().all(|t| !t.key.is_empty()));
        assert_eq!(rolling_targets(&b, &M, 1000).len(), 2);
    }

    #[test]
    fn automatic_boxes_are_saved_as_before() {
        let json = serde_json::to_string(&make_box(&["a"])).unwrap();
        assert!(!json.contains("arrange"), "{json}");
        let json = serde_json::to_string(&free_box(&["a"])).unwrap();
        assert!(json.contains(r#""arrange":"free""#), "{json}");
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
    fn strays_are_rolled_up_icons_explorer_pulled_back_on_screen() {
        let bs = [rolled_up(&["a", "b"]), make_box(&["c"])];
        let icons = [icon("a", PARKED), icon("b", (1800, 930)), icon("c", (5, 5)), icon("loose", (9, 9))];
        assert_eq!(strays(&bs, &icons, &M), [Move { key: "b".into(), x: PARKED.0, y: PARKED.1 }]);
    }

    #[test]
    fn icons_parked_by_an_older_version_move_to_the_new_spot_or_come_back() {
        let bs = [rolled_up(&["a"])];
        let icons = [icon("a", LEGACY_PARKED), icon("orphan", LEGACY_PARKED)];
        assert_eq!(strays(&bs, &icons, &M), [Move { key: "a".into(), x: PARKED.0, y: PARKED.1 }]);
        let mut bs = bs;
        let o = reconcile(&mut bs, &icons, &M, &area);
        let back = o.moves.iter().find(|m| m.key == "orphan").expect("the orphan comes back");
        assert!(AREA.contains(back.x, back.y), "{back:?}");
    }

    #[test]
    fn a_parked_icon_is_never_inside_a_box() {
        // Even a box against the right edge of the screen: the parked cell's center is past it.
        let mut bs = [make_box(&[])];
        bs[0].rect = Rect { x: 1600, y: 0, w: 320, h: 400 };
        let o = reconcile(&mut bs, &[icon("p", PARKED)], &M, &area);
        assert!(bs[0].members.is_empty() && !o.changed);
    }

    #[test]
    fn rolled_up_box_keeps_members_missing_while_explorer_reloads() {
        let mut bs = [rolled_up(&["a", "b"])];
        // Mid-refresh, only some icons are listed so far.
        let o = reconcile(&mut bs, &[icon("a", PARKED)], &M, &area);
        assert_eq!(members(&bs[0]), ["a", "b"]);
        assert!(o.moves.is_empty());
        // Once "b" is listed again, wherever Explorer put it, it goes back to the box.
        let o = reconcile(&mut bs, &[icon("a", PARKED), icon("b", (0, 0))], &M, &area);
        assert_eq!(o.moves, [Move { key: "b".into(), x: PARKED.0, y: PARKED.1 }]);
    }

    #[test]
    fn missing_members_of_a_reopened_box_drop_out() {
        let mut bs = [rolled_up(&["a", "gone"])];
        bs[0].collapsed = false;
        let icons = [icon("a", slot(&bs[0], 0))];
        reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(members(&bs[0]), ["a"]);
    }

    #[test]
    fn parked_icon_with_no_box_comes_back_to_a_free_spot() {
        let mut bs = [make_box(&[])];
        bs[0].rect = Rect { x: 0, y: 0, w: 320, h: 400 }; // covers the first column
        let icons = [icon("lost", PARKED), icon("other", (400, 0))];
        let o = reconcile(&mut bs, &icons, &M, &area);
        assert_eq!(o.moves.len(), 1, "{:?}", o.moves);
        let back = &o.moves[0];
        assert_eq!(back.key, "lost");
        let cell = Rect { x: back.x, y: back.y, w: M.cell_w, h: M.cell_h };
        assert!(!cell.intersects(&bs[0].rect), "not under a box: {back:?}");
        assert!(!cell.intersects(&Rect { x: 400, y: 0, w: M.cell_w, h: M.cell_h }), "not on another icon: {back:?}");
        assert!(AREA.contains(back.x, back.y));
    }

    #[test]
    fn explorer_resetting_the_layout_is_not_read_as_drags() {
        let bs = [make_box(&["a", "b", "c", "d"])];
        // Explorer's default layout: a column down the left, far from the box.
        let reset: Vec<_> = ["a", "b", "c", "d"].iter().enumerate().map(|(i, k)| icon(k, (1500, i as i32 * 100))).collect();
        assert!(looks_like_reset(&bs, &reset, &M));
        // Dragging one icon out is a drag.
        let one_out = [icon("a", (1500, 800)), icon("b", slot(&bs[0], 1)), icon("c", slot(&bs[0], 2)), icon("d", slot(&bs[0], 3))];
        assert!(!looks_like_reset(&bs, &one_out, &M));
    }

    #[test]
    fn restoring_after_a_reload_puts_boxed_and_loose_icons_back() {
        let mut bs = [make_box(&["a", "b"])];
        let before = [icon("a", slot(&bs[0], 0)), icon("b", slot(&bs[0], 1)), icon("loose", (1500, 800))];
        let now = [icon("a", (0, 0)), icon("b", (0, 100)), icon("loose", (0, 200)), icon("new", (0, 300))];
        let moves = restore_layout(&mut bs, &before, &now, &M, &area);
        let to = |k: &str| moves.iter().find(|m| m.key == k).map(|m| (m.x, m.y));
        assert_eq!(to("a"), Some(slot(&bs[0], 0)));
        assert_eq!(to("b"), Some(slot(&bs[0], 1)));
        assert_eq!(to("loose"), Some((1500, 800)));
        assert_eq!(to("new"), None, "an icon Explorer just added stays where it is");
        assert_eq!(members(&bs[0]), ["a", "b"]);
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
