# Organizer

Group your Windows desktop icons into glowing, translucent boxes that you can style with CSS.
Built to stay out of the way: about 2 MB of RAM, almost no CPU, and it stops checking
entirely while a fullscreen game runs.

Your files never move. Boxes only control where the real desktop icons sit, so opening,
renaming, right-clicking and dragging icons all work as usual.

## Run

```
cargo build --release
target\release\organizer.exe
```

Organizer runs from the system tray (it may be under the `^` overflow arrow).

## Use

| To | Do this |
| --- | --- |
| Create a box | Tray icon > **New box**, right-click a box's title bar > **New box**, or right-click the desktop > **New box** (on Windows 11, under **Show more options**) |
| Add an icon | Drag it into a box |
| Remove an icon | Drag it out of the box |
| Reorder icons | Drag an icon within its box |
| Move a box (its icons follow) | Drag the title bar |
| Resize a box | Drag the left, right or bottom edge |
| Rename a box | Double-click the title bar |
| Delete a box | Right-click the title bar |
| Change the color or design | Tray icon > **Customize design (theme.css)** |
| Prevent accidental moves | Tray icon > **Lock boxes** |
| Start at login | Tray icon > **Start with Windows** |
| Check for updates | Automatic twice a day, or tray icon > **Check for updates**. A notification appears when a newer release is out |
| Undo everything | Tray icon > **Remove all boxes and restore original layout...** |

Deleting a box leaves its icons where they are. Moving a box over loose icons adds them to it.
A box grows when it is too small for its icons.

## Customize with CSS

Open `theme.css` from the tray menu. Save it and your boxes update within a second. A mistake
never breaks anything: the bad line is ignored and noted in `organizer.log`. Delete the file to
get the default back.

The box color is `--accent` in `:root`, used anywhere with `var(--accent)`. It defaults to
graphite (`#3B4252`) and applies to every box.

```css
:root {
  --accent: #3B4252;
  --panel: rgba(10, 12, 20, 0.85);
}

.box {
  background: linear-gradient(160deg,
    color-mix(in srgb, var(--accent) 16%, transparent),
    rgba(10, 12, 20, 0.12));
  border: 2px solid var(--accent);
  border-radius: 16px;
  box-shadow: 0 0 24px var(--accent);
  padding: 6px;
}

.title {
  height: 32px;
  padding: 0 14px;
  background: linear-gradient(90deg, var(--accent), var(--panel));
  color: #ffffff;
  font-family: "Bahnschrift";
  font-size: 13px;
  font-weight: 700;
  text-align: center;
  text-transform: uppercase;
  letter-spacing: 2px;
}
```

This is a focused subset of CSS drawn natively, not a browser, which is why it costs almost no
memory. These are all the supported features:

| Selector | Properties |
| --- | --- |
| `:root` | `--accent` (the box color), and `--your-variables`, used with `var(--name)` |
| `.box` | `background`, `border`, `border-width`, `border-color`, `border-radius`, `box-shadow`, `padding` |
| `.title` | `height`, `background`, `color`, `font-family`, `font-size`, `font-weight`, `text-align`, `text-transform`, `letter-spacing`, `padding` |

- **Colors:** `#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`, `white`, `black`, `transparent`,
  `var(--name)`, `color-mix(in srgb, COLOR 40%, COLOR)`
- **Fills:** a color, or `linear-gradient(135deg, COLOR, COLOR, ...)` with evenly spaced colors
- **Glow:** `box-shadow: 0 0 BLUR COLOR`, or `none`. Offsets are ignored, so the glow is centered.
- **Lengths:** `px`, or `0`
- **Fonts:** use a font installed on your PC

Not supported: other selectors, hover effects, animations, images, and backgrounds behind the
icons. The box background is drawn over your icons, so keep it translucent.

## Good to know

- Organizer turns off the desktop's **Auto arrange icons** and **Align icons to grid** while
  boxes exist, because either would pull icons out of their boxes. It records the original
  settings and icon positions before its first change; the restore command puts them back.
- If an icon was dragged out of a box while Organizer was closed, it returns to the box at the
  next start.
- A new file only joins a box if Windows happens to place it inside one.
- Icons are checked after you use the desktop, and otherwise every 30 seconds, so a change made
  from another window (such as deleting a file in a box) can take up to 30 seconds to show.
- To close Organizer, use **Exit** in its tray menu. Force-killing it can occasionally leave a
  stuck, invisible process that blocks a new copy from starting until you sign out or restart
  Explorer, because its windows live inside Explorer.
- Mixed-DPI multi-monitor setups have not been tested.

## Files

State is in `%APPDATA%\Organizer`: `boxes.json`, `theme.css`, `original-layout.json`, and
`organizer.log` for errors and ignored CSS. Set `ORGANIZER_DIR` to use another folder.

`organizer.exe list` prints the icons Organizer can see. It does not move anything, but it does
create `theme.css` if it is missing. Run it with output redirected, for example
`organizer.exe list > icons.txt`.

## Code

- `src/model.rs`: box geometry and membership logic
- `src/theme.rs`: the CSS parser, and the default theme
- `src/render.rs`: draws boxes to pixels
- `src/desktop.rs`: reads and moves icons in Explorer
- `src/ui.rs`: tray icon, box windows, dragging and the check loop
- `src/store.rs`: saved state

Run the unit tests with `cargo test`.
