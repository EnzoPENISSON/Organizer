//! Talks to Explorer's desktop view through the shell's documented COM interfaces.
//! Icons and box windows share one coordinate space: the view's client area.

use crate::model::{Icon, Metrics, Move};
use std::{cell::{Cell, RefCell}, rc::Rc};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, POINT, WPARAM},
        System::{
            Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance, CoTaskMemFree, IServiceProvider},
            Variant::VARIANT,
        },
        UI::{
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::{
                FindWindowExW, SMTO_ABORTIFHUNG, SW_HIDE, SW_SHOWNA, SendMessageTimeoutW, ShowWindowAsync,
            },
            Shell::{
                Common::{ITEMIDLIST, STRRET},
                FWF_AUTOARRANGE, FWF_NOICONS, FWF_SNAPTOGRID, IFolderView2, IShellBrowser, IShellFolder,
                IShellView, IShellWindows, SHGDN_FORPARSING, SVGIO_ALLVIEW, SVSI_POSITIONITEM,
                SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows, StrRetToStrW,
            },
        },
    },
    core::{GUID, Interface, PWSTR, Result, w},
};

const SID_STOP_LEVEL_BROWSER: GUID = GUID::from_u128(0x4C96BE40_915C_11CF_99D3_00AA004AE837);
const CSIDL_DESKTOP: i32 = 0;
const PLACEMENT_MASK: u32 = (FWF_AUTOARRANGE.0 | FWF_SNAPTOGRID.0) as u32;
/// Names are reread this often, to catch a rename that left an old handle still answering.
const READS_PER_NAME_REFRESH: u32 = 30;
const LVM_SETITEMPOSITION: u32 = 0x100F;

type Items = Rc<Vec<(String, Pidl)>>;

#[derive(Clone)]
pub struct Desktop {
    view: IFolderView2,
    folder: IShellFolder,
    hwnd: HWND,
    list: HWND,
    items: Rc<RefCell<Items>>,
    reads: Rc<Cell<u32>>,
    direct_ok: Rc<Cell<bool>>,
    /// The fast move has been checked against every icon since the handles were reloaded.
    verified: Rc<Cell<bool>>,
}

/// Item ID list owned by this process, freed on drop.
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
    }
}

impl Desktop {
    pub fn connect() -> Result<Self> {
        unsafe {
            let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;
            let mut desktop_hwnd = 0i32;
            let dispatch = windows.FindWindowSW(
                &VARIANT::from(CSIDL_DESKTOP),
                &VARIANT::default(),
                SWC_DESKTOP,
                &mut desktop_hwnd,
                SWFO_NEEDDISPATCH,
            )?;
            let provider: IServiceProvider = dispatch.cast()?;
            let browser: IShellBrowser = provider.QueryService(&SID_STOP_LEVEL_BROWSER)?;
            let shell_view: IShellView = browser.QueryActiveShellView()?;
            let view: IFolderView2 = shell_view.cast()?;
            let folder: IShellFolder = view.GetFolder()?;
            let hwnd = shell_view.GetWindow()?;
            let list = FindWindowExW(Some(hwnd), None, w!("SysListView32"), None).unwrap_or_default();
            Ok(Self {
                view,
                folder,
                hwnd,
                list,
                items: Rc::default(),
                reads: Rc::default(),
                direct_ok: Rc::new(Cell::new(true)),
                verified: Rc::default(),
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Explorer's list control, which draws the icons.
    pub fn list(&self) -> HWND {
        self.list
    }

    /// Shows or hides every desktop icon, the same setting as the desktop's View > Show
    /// desktop icons. The list control is also switched directly, in case a Windows version
    /// does not redraw on the flag alone; the two always agree.
    pub fn set_icons_visible(&self, visible: bool) -> Result<()> {
        let flag = FWF_NOICONS.0 as u32;
        let result = unsafe { self.view.SetCurrentFolderFlags(flag, if visible { 0 } else { flag }) };
        if !self.list.is_invalid() {
            let _ = unsafe { ShowWindowAsync(self.list, if visible { SW_SHOWNA } else { SW_HIDE }) };
        }
        result
    }

    /// `title`, `border` and `pad` are CSS pixels from the theme.
    pub fn metrics(&self, title: f32, border: f32, pad: f32) -> Result<Metrics> {
        let dpi = unsafe { GetDpiForWindow(self.hwnd) }.max(96) as i32;
        let scale = |v: f32| (v * dpi as f32 / 96.0).ceil() as i32;
        let mut spacing = POINT::default();
        unsafe { self.view.GetSpacing(&mut spacing)? };
        let (mut mode, mut icon_size) = (Default::default(), 0i32);
        unsafe { self.view.GetViewModeAndIconSize(&mut mode, &mut icon_size)? };
        let cell_w = spacing.x.max(1);
        Ok(Metrics {
            title: scale(title),
            border: scale(border),
            pad: scale(pad),
            cell_w,
            cell_h: spacing.y.max(1),
            // Explorer centers the image in its cell, 2px (at 100%) below the cell's top.
            icon_x: ((cell_w - icon_size) / 2).max(0),
            icon_y: scale(2.0),
        })
    }

    fn load_items(&self) -> Result<Items> {
        let items = unsafe {
            let count = self.view.ItemCount(SVGIO_ALLVIEW)?;
            let mut items = Vec::with_capacity(count as usize);
            for index in 0..count {
                let pidl = Pidl(self.view.Item(index)?);
                let mut name = STRRET::default();
                self.folder.GetDisplayNameOf(pidl.0, SHGDN_FORPARSING, &mut name)?;
                let mut text = PWSTR::null();
                StrRetToStrW(&mut name, Some(pidl.0), &mut text)?;
                let key = text.to_string().unwrap_or_default();
                CoTaskMemFree(Some(text.0 as *const _));
                items.push((key, pidl));
            }
            Rc::new(items)
        };
        *self.items.borrow_mut() = items.clone();
        self.verified.set(false);
        Ok(items)
    }

    /// The cached items, reloaded if the icon count changed. Keeping them makes routine
    /// reads and moves far cheaper, because names need several calls into Explorer per icon.
    /// The cache is cloned out, never borrowed, because COM calls can dispatch messages.
    fn cached_items(&self) -> Result<Items> {
        let items = self.items.borrow().clone();
        let count = unsafe { self.view.ItemCount(SVGIO_ALLVIEW)? };
        if items.is_empty() || items.len() != count as usize {
            return self.load_items();
        }
        Ok(items)
    }

    pub fn icons(&self) -> Result<Vec<Icon>> {
        let read = |items: &Items| -> Result<Vec<Icon>> {
            items
                .iter()
                .map(|(key, pidl)| {
                    let p = unsafe { self.view.GetItemPosition(pidl.0)? };
                    Ok(Icon { key: key.clone(), x: p.x, y: p.y })
                })
                .collect()
        };
        let reads = self.reads.get() + 1;
        self.reads.set(reads % READS_PER_NAME_REFRESH);
        let items = if reads >= READS_PER_NAME_REFRESH { self.load_items()? } else { self.cached_items()? };
        // A failed read means a handle went stale, such as a file replaced by another.
        read(&items).or_else(|_| read(&self.load_items()?))
    }

    fn position_batch(&self, items: &Items, moves: &[Move]) -> Result<()> {
        let (mut pidls, mut points) = (Vec::with_capacity(moves.len()), Vec::with_capacity(moves.len()));
        for m in moves {
            if let Some((_, pidl)) = items.iter().find(|(key, _)| *key == m.key) {
                pidls.push(pidl.0 as *const ITEMIDLIST);
                points.push(POINT { x: m.x, y: m.y });
            }
        }
        if !pidls.is_empty() {
            unsafe {
                self.view.SelectAndPositionItems(
                    pidls.len() as u32,
                    pidls.as_ptr(),
                    Some(points.as_ptr()),
                    SVSI_POSITIONITEM.0 as u32,
                )?
            };
        }
        Ok(())
    }

    /// Moves icons by messaging Explorer's list control, which repaints in about 6ms.
    /// The shell's own call takes about 250ms to show on screen, too slow to track a drag.
    ///
    /// The list control knows icons only by index, so this relies on its order matching
    /// the view's, which a desktop refresh can break. A wrong index moves the wrong icon.
    /// So every moved icon is read back through its own handle, and the first time after
    /// the handles are reloaded, every other icon is checked too. If anything is off, the
    /// fast path is switched off for good and the shell's call puts every icon right,
    /// including bystanders that were moved by mistake. Returns false in that case.
    fn move_direct(&self, items: &Items, moves: &[Move]) -> bool {
        if !self.direct_ok.get() || self.list.is_invalid() {
            return false;
        }
        let position = |i: usize| unsafe { self.view.GetItemPosition(items[i].1.0) }.ok();
        let before: Option<Vec<Option<POINT>>> =
            (!self.verified.get()).then(|| (0..items.len()).map(position).collect());
        let mut sent_to = Vec::with_capacity(moves.len());
        for m in moves {
            let Some(index) = items.iter().position(|(key, _)| *key == m.key) else { continue };
            let lparam = (m.y as u16 as u32) << 16 | m.x as u16 as u32;
            let sent = unsafe {
                SendMessageTimeoutW(
                    self.list,
                    LVM_SETITEMPOSITION,
                    WPARAM(index),
                    LPARAM(lparam as isize),
                    SMTO_ABORTIFHUNG,
                    200,
                    None,
                )
            };
            if sent.0 == 0 {
                break;
            }
            sent_to.push((index, m.x, m.y));
        }
        let moved_right = sent_to.len() == moves.len()
            && sent_to.iter().all(|&(i, x, y)| position(i).is_some_and(|p| p.x == x && p.y == y));
        let bystanders: Vec<Move> = before
            .iter()
            .flat_map(|before| {
                before.iter().enumerate().filter_map(|(i, was)| {
                    let was = (*was)?;
                    let intended = sent_to.iter().any(|&(s, _, _)| s == i);
                    (!intended && position(i) != Some(was))
                        .then(|| Move { key: items[i].0.clone(), x: was.x, y: was.y })
                })
            })
            .collect();
        if moved_right && bystanders.is_empty() {
            self.verified.set(true);
            return true;
        }
        self.direct_ok.set(false);
        crate::store::log("list order does not match view order, using the slower move");
        let repair: Vec<Move> = moves.iter().cloned().chain(bystanders).collect();
        if let Err(e) = self.position_batch(items, &repair) {
            crate::store::log(&format!("could not put icons back: {e}"));
        }
        false
    }

    /// Reloads names and handles, so positions are right for a whole drag.
    pub fn refresh(&self) -> Result<()> {
        self.load_items().map(|_| ())
    }

    /// For moves during a drag. Fast, using the handles from the last `refresh`.
    pub fn follow(&self, moves: &[Move]) -> Result<()> {
        let items = self.items.borrow().clone();
        if self.move_direct(&items, moves) {
            return Ok(());
        }
        self.apply(moves)
    }

    /// Moves are by name; a name no longer on the desktop is skipped. Ends with the shell's
    /// own call, so Explorer's view agrees with the list control and remembers the position.
    /// The handles are fresh, so the fast move is checked against every icon here.
    pub fn apply(&self, moves: &[Move]) -> Result<()> {
        if moves.is_empty() {
            return Ok(());
        }
        let items = self.load_items()?;
        self.move_direct(&items, moves);
        self.position_batch(&items, moves)?;
        Ok(())
    }

    /// Reloads the desktop the way F5 does, so Explorer applies a changed setting such as
    /// a hidden Recycle Bin. Icons keep their positions.
    pub fn refresh_view(&self) -> Result<()> {
        unsafe { self.view.cast::<IShellView>()?.Refresh() }
    }

    pub fn folder_flags(&self) -> Result<u32> {
        Ok(unsafe { self.view.GetCurrentFolderFlags()? })
    }

    /// Sets auto-arrange and align-to-grid to how `flags` has them.
    pub fn set_placement_flags(&self, flags: u32) -> Result<()> {
        unsafe { self.view.SetCurrentFolderFlags(PLACEMENT_MASK, flags & PLACEMENT_MASK) }
    }

    /// Auto-arrange and align-to-grid would both pull icons back out of their boxes.
    pub fn allow_free_placement(&self) -> Result<()> {
        if self.folder_flags()? & PLACEMENT_MASK != 0 {
            self.set_placement_flags(0)?;
        }
        Ok(())
    }
}
