//! The Aegis sheet (overlays.html `.sheet`) on gpui-kit's sheet layer: raised, its top corners rounded, the glass
//! shadow and a light top edge. Like the dialog it raises the browser shield until it closes, however it closes.

use std::rc::Rc;

use gpui::{App, Styled as _, Window};
use gpui_kit::component::sheet::Sheet;
use gpui_kit::component::{Placement, WindowExt as _};

use super::shield::BrowserShield;
use crate::ui::style::{hsla, palette, r, shadows, theme};

/// Opens a sheet from the bottom edge. As with `open_dialog`, `build` may run again while it is open.
pub fn open_sheet(
    window: &mut Window,
    cx: &mut App,
    build: impl Fn(Sheet, &mut Window, &mut App) -> Sheet + 'static,
) {
    let shield = Rc::new(BrowserShield::raise(cx));
    window.open_sheet_at(Placement::Bottom, cx, move |sheet, window, cx| {
        let _held_while_open = &shield;
        let theme = theme(cx);
        let colors = palette(cx);
        let panel = r(theme.radius.panel);
        let surface = sheet
            .bg(hsla(colors.surface.raised))
            .rounded_tl(panel)
            .rounded_tr(panel)
            .border_t_1()
            .border_color(hsla(colors.overlay.highlight))
            .shadow(shadows(&theme.shadow.light.glass))
            .pt(r(theme.space.s10))
            .px(r(theme.space.s16))
            .pb(r(theme.space.s16));
        build(surface, window, cx)
    });
}
