//! Small reusable pieces of UI.

use super::bidi;
use super::theme::{pal, regular, semibold, Icon};
use eframe::egui::{
    self, pos2, text::LayoutJob, text::TextWrapping, vec2, Align2, Color32, CornerRadius, FontId,
    Frame, Margin, Rect, Response, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui, Vec2,
};

/// A rounded card.
pub fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

fn filled_button(ui: &mut Ui, text: &str, fill: Color32, fg: Color32, enabled: bool) -> Response {
    let p = pal(ui);
    let (fill, fg) = if enabled {
        (fill, fg)
    } else {
        (p.hover, p.faint)
    };
    let button = egui::Button::new(RichText::new(text).font(semibold(14.0)).color(fg))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(0.0, 34.0));
    let prev = ui.spacing().button_padding;
    ui.spacing_mut().button_padding = vec2(16.0, 7.0);
    let r = ui.add_enabled(enabled, button);
    ui.spacing_mut().button_padding = prev;
    if r.hovered() && enabled {
        ui.painter()
            .rect_filled(r.rect, CornerRadius::same(8), Color32::from_white_alpha(22));
    }
    r
}

/// The main action on a screen.
pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let p = pal(ui);
    filled_button(ui, text, p.accent, p.on_accent, enabled)
}

/// A destructive action.
pub fn danger_button(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let p = pal(ui);
    filled_button(ui, text, p.danger, Color32::WHITE, enabled)
}

/// A secondary action.
pub fn ghost_button(ui: &mut Ui, text: &str) -> Response {
    ghost_button_enabled(ui, text, true)
}

pub fn ghost_button_enabled(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    let p = pal(ui);
    let button =
        egui::Button::new(RichText::new(text).color(if enabled { p.text } else { p.faint }))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(8))
            .min_size(vec2(0.0, 34.0));
    ui.add_enabled(enabled, button)
}

/// A square icon-only button with a tooltip.
pub fn icon_button(ui: &mut Ui, icon: Icon, tooltip: &str) -> Response {
    let p = pal(ui);
    let label = if icon.glyph().is_empty() {
        tooltip.chars().take(1).collect::<String>()
    } else {
        icon.glyph().to_string()
    };
    let button = egui::Button::new(RichText::new(label).size(15.0).color(p.muted))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(32.0, 32.0));
    ui.add(button).on_hover_text(tooltip)
}

/// A small pill label.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    if bidi::needs_native(text) {
        if let Some(size) = bidi::measure(ui, text, 11.5, true) {
            let (rect, resp) = ui.allocate_exact_size(vec2(size.x + 14.0, 20.0), Sense::hover());
            ui.painter()
                .rect_filled(rect, CornerRadius::same(10), color.gamma_multiply(0.16));
            let inner = rect.shrink2(vec2(7.0, 0.0));
            bidi::paint(ui, inner, text, 11.5, true, color, false);
            return resp;
        }
    }
    let font = semibold(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = vec2(galley.size().x + 14.0, 20.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    paint_badge_at(ui, rect, galley, color);
    resp
}

fn paint_badge_at(ui: &Ui, rect: Rect, galley: std::sync::Arc<egui::Galley>, color: Color32) {
    ui.painter()
        .rect_filled(rect, CornerRadius::same(10), color.gamma_multiply(0.16));
    let pos = pos2(
        rect.center().x - galley.size().x / 2.0,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(pos, galley, color);
}

/// Paints a badge right-aligned inside `rect` and returns the width it used.
pub fn paint_badge(ui: &Ui, rect: Rect, text: &str, color: Color32) -> f32 {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), semibold(11.5), color);
    let w = galley.size().x + 14.0;
    let r = Rect::from_min_size(pos2(rect.left(), rect.center().y - 10.0), vec2(w, 20.0));
    paint_badge_at(ui, r, galley, color);
    w
}

/// Paints a custom checkbox in `rect`. `state`: Some(true) checked,
/// Some(false) unchecked, None = mixed.
pub fn paint_checkbox(ui: &Ui, rect: Rect, state: Option<bool>, hovered: bool) {
    let p = pal(ui);
    let r = Rect::from_center_size(rect.center(), Vec2::splat(18.0));
    match state {
        Some(false) => {
            ui.painter().rect_stroke(
                r,
                CornerRadius::same(5),
                Stroke::new(1.5_f32, if hovered { p.accent } else { p.faint }),
                StrokeKind::Inside,
            );
        }
        Some(true) => {
            ui.painter().rect_filled(r, CornerRadius::same(5), p.accent);
            let c = r.center();
            let pts = [
                c + vec2(-4.5, 0.0),
                c + vec2(-1.5, 3.2),
                c + vec2(4.8, -3.6),
            ];
            ui.painter()
                .line_segment([pts[0], pts[1]], Stroke::new(2.0_f32, p.on_accent));
            ui.painter()
                .line_segment([pts[1], pts[2]], Stroke::new(2.0_f32, p.on_accent));
        }
        None => {
            ui.painter().rect_filled(r, CornerRadius::same(5), p.accent);
            let c = r.center();
            ui.painter().line_segment(
                [c + vec2(-4.5, 0.0), c + vec2(4.5, 0.0)],
                Stroke::new(2.0_f32, p.on_accent),
            );
        }
    }
}

/// An on/off switch. Returns true when clicked.
pub fn toggle(ui: &mut Ui, on: bool, enabled: bool) -> Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(
        vec2(38.0, 22.0),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let t = ui.ctx().animate_bool_with_time(resp.id, on, 0.12);
    let track = if !enabled {
        p.border
    } else if on {
        p.accent
    } else {
        p.faint.gamma_multiply(0.6)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(11), track);
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    ui.painter().circle_filled(
        pos2(x, rect.center().y),
        8.0,
        if enabled { Color32::WHITE } else { p.faint },
    );
    resp
}

/// A row of mutually exclusive choices. Returns the newly picked index.
pub fn segmented(ui: &mut Ui, options: &[&str], current: usize) -> Option<usize> {
    let p = pal(ui);
    let mut picked = None;
    Frame::new()
        .fill(p.hover)
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::same(3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                // In a right-to-left row the first widget lands on the right:
                // add the options backwards so they still read left to right.
                let mut order: Vec<usize> = (0..options.len()).collect();
                if ui.layout().prefer_right_to_left() {
                    order.reverse();
                }
                for i in order {
                    let label = &options[i];
                    let active = i == current;
                    let text = RichText::new(*label).color(if active { p.text } else { p.muted });
                    let text = if active {
                        text.font(semibold(13.5))
                    } else {
                        text.size(13.5)
                    };
                    let b = egui::Button::new(text)
                        .fill(if active { p.card } else { Color32::TRANSPARENT })
                        .stroke(if active {
                            Stroke::new(1.0_f32, p.border)
                        } else {
                            Stroke::NONE
                        })
                        .corner_radius(CornerRadius::same(7))
                        .min_size(vec2(0.0, 28.0));
                    if ui.add(b).clicked() && !active {
                        picked = Some(i);
                    }
                }
            });
        });
    picked
}

/// A filter chip.
pub fn chip(ui: &mut Ui, label: &str, count: Option<usize>, active: bool) -> Response {
    let p = pal(ui);
    let text = match count {
        Some(n) => format!("{label}  {n}"),
        None => label.to_string(),
    };
    let (fill, fg, stroke) = if active {
        (
            p.accent.gamma_multiply(0.16),
            p.accent,
            Stroke::new(1.0_f32, p.accent.gamma_multiply(0.6)),
        )
    } else {
        (
            Color32::TRANSPARENT,
            p.muted,
            Stroke::new(1.0_f32, p.border),
        )
    };
    let b = egui::Button::new(RichText::new(text).size(13.0).color(fg))
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(15))
        .min_size(vec2(0.0, 30.0));
    ui.add(b)
}

/// Lays out one line of text, cut off with an ellipsis at `max_width`.
pub fn elided(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = TextWrapping {
        max_width: max_width.max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    ui.fonts(|f| f.layout_job(job))
}

/// Paints one elided line, vertically centred in `rect`. Returns whether it was cut.
pub fn text_in(ui: &Ui, rect: Rect, text: &str, font: FontId, color: Color32) -> bool {
    // Kurdish, Arabic, Persian, Hebrew: drawn by Windows (see `bidi`).
    if bidi::needs_native(text) {
        let bold = bidi::is_bold(&font);
        if let Some(cut) = bidi::paint(ui, rect, text, font.size, bold, color, false) {
            return cut;
        }
    }
    let galley = elided(ui, text, font, color, rect.width());
    let cut = galley.elided;
    ui.painter().galley(
        pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    cut
}

/// Like [`text_in`], right-aligned.
pub fn text_in_right(ui: &Ui, rect: Rect, text: &str, font: FontId, color: Color32) {
    if bidi::needs_native(text) {
        let bold = bidi::is_bold(&font);
        if bidi::paint(ui, rect, text, font.size, bold, color, true).is_some() {
            return;
        }
    }
    let galley = elided(ui, text, font, color, rect.width());
    ui.painter().galley(
        pos2(
            rect.right() - galley.size().x,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
}

/// How wide one line of `text` is, at most `max`. Pairs with [`text_in`].
pub fn text_width(ui: &Ui, text: &str, font: FontId, max: f32) -> f32 {
    if bidi::needs_native(text) {
        if let Some(size) = bidi::measure(ui, text, font.size, bidi::is_bold(&font)) {
            return size.x.min(max);
        }
    }
    elided(ui, text, font, Color32::WHITE, max).size().x
}

/// One line of a menu. Like `ui.button`, but a name in a right-to-left
/// script (a project, a category) is drawn properly.
pub fn menu_item(ui: &mut Ui, text: &str) -> Response {
    if !bidi::needs_native(text) {
        return ui.button(text);
    }
    let p = pal(ui);
    let width = text_width(ui, text, regular(14.0), 320.0) + 16.0;
    let size = vec2(width.max(ui.available_width()), 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), p.hover);
    }
    text_in(
        ui,
        rect.shrink2(vec2(8.0, 0.0)),
        text,
        regular(14.0),
        p.text,
    );
    resp
}

/// One choice in a drop-down list. Like `ui.selectable_label`, with
/// right-to-left names drawn properly. Returns true when clicked.
pub fn option_item(ui: &mut Ui, selected: bool, text: &str) -> bool {
    if !bidi::needs_native(text) {
        return ui.selectable_label(selected, text).clicked();
    }
    let p = pal(ui);
    let width = text_width(ui, text, regular(14.0), 320.0) + 16.0;
    let size = vec2(width.max(ui.available_width()), 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if selected {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), p.selected);
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), p.hover);
    }
    text_in(
        ui,
        rect.shrink2(vec2(8.0, 0.0)),
        text,
        regular(14.0),
        p.text,
    );
    resp.clicked()
}

/// The text to hand to a drop-down as its selected text: nothing when the
/// name is right-to-left, because [`combo_overlay`] then draws it.
pub fn combo_text(text: &str) -> String {
    if bidi::needs_native(text) {
        String::new()
    } else {
        text.to_string()
    }
}

/// Draws a right-to-left selected name over a closed drop-down.
pub fn combo_overlay(ui: &Ui, combo: &Response, text: &str) {
    if bidi::needs_native(text) {
        let p = pal(ui);
        let rect = Rect::from_min_max(
            pos2(combo.rect.left() + 8.0, combo.rect.top()),
            pos2(combo.rect.right() - 24.0, combo.rect.bottom()),
        );
        text_in(ui, rect, text, regular(14.0), p.text);
    }
}

/// A one-line text box in the app's style. Right-to-left text is shown
/// joined and in reading order while it is typed (see `bidi::show_edit`).
pub fn text_edit(ui: &mut Ui, value: &mut String, hint: &str, width: f32) -> Response {
    let p = pal(ui);
    bidi::show_edit(ui, value, 14.0, p.text, |edit| {
        edit.hint_text(RichText::new(hint).color(p.faint))
            .desired_width(width)
            .margin(Margin::symmetric(8, 6))
    })
    .response
}

fn tile_color(name: &str) -> Color32 {
    const COLORS: [Color32; 8] = [
        Color32::from_rgb(0x10, 0xa3, 0x7f),
        Color32::from_rgb(0x08, 0x6f, 0xa8),
        Color32::from_rgb(0x7c, 0x5c, 0xd6),
        Color32::from_rgb(0xd9, 0x6a, 0x2b),
        Color32::from_rgb(0xc2, 0x48, 0x7a),
        Color32::from_rgb(0x2f, 0x86, 0xc9),
        Color32::from_rgb(0x4e, 0x9a, 0x3c),
        Color32::from_rgb(0xb8, 0x8a, 0x1f),
    ];
    let h = name
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    COLORS[(h % COLORS.len() as u32) as usize]
}

/// Paints a program icon, or a coloured tile with the program's initial.
pub fn program_icon(ui: &Ui, rect: Rect, name: &str, texture: Option<&TextureHandle>) {
    match texture {
        Some(tex) => {
            ui.painter().image(
                tex.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            let radius = (rect.width() * 0.26) as u8;
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius), tile_color(name));
            let initial: String = name
                .chars()
                .find(|c| c.is_alphanumeric())
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default();
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                initial,
                semibold(rect.height() * 0.5),
                Color32::WHITE,
            );
        }
    }
}

/// A designed empty / error state: icon, title, explanation.
pub fn empty_state(ui: &mut Ui, icon: Icon, color: Color32, title: &str, text: &str) {
    let p = pal(ui);
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.22).clamp(24.0, 140.0));
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(72.0), Sense::hover());
        ui.painter()
            .circle_filled(rect.center(), 36.0, color.gamma_multiply(0.14));
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon.glyph(),
            regular(30.0),
            color,
        );
        ui.add_space(10.0);
        ui.label(RichText::new(title).font(semibold(18.0)).color(p.text));
        ui.add_space(2.0);
        ui.scope(|ui| {
            ui.set_max_width(460.0);
            ui.label(RichText::new(text).color(p.muted));
        });
        ui.add_space(8.0);
    });
}

/// Section title inside a page.
pub fn section(ui: &mut Ui, title: &str) {
    ui.label(RichText::new(title).font(semibold(15.0)));
}

/// A thin horizontal line.
pub fn divider(ui: &mut Ui) {
    let p = pal(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0, p.border);
}

/// The frame used by modal dialogs.
pub fn modal_frame(ctx: &egui::Context) -> Frame {
    let p = super::theme::pal_ctx(ctx);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(22))
}

/// A right-aligned row of buttons with a fixed height. A bare right-to-left
/// layout takes all the height its parent may use, which stretches modals and
/// cards to the full window; this does not.
pub fn button_row(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 36.0),
        egui::Layout::right_to_left(egui::Align::Center),
        add,
    );
}

/// A small round colour dot (category and project colours).
pub fn dot(ui: &mut Ui, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(12.0, 20.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 5.0, color);
    resp
}

/// A big number with a caption under it, for summary rows.
pub fn stat(ui: &mut Ui, value: &str, caption: &str, color: Option<Color32>) {
    let p = pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        ui.label(
            RichText::new(value)
                .font(semibold(24.0))
                .color(color.unwrap_or(p.text)),
        );
        ui.label(RichText::new(caption).size(12.5).color(p.muted));
    });
}

/// `[r, g, b]` as a colour.
pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

/// A horizontal progress bar with rounded ends. `fraction` may exceed 1.
pub fn progress(ui: &mut Ui, fraction: f32, color: Color32, height: f32) -> Response {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let r = CornerRadius::same((height / 2.0) as u8);
    ui.painter().rect_filled(rect, r, p.hover);
    let w = rect.width() * fraction.clamp(0.0, 1.0);
    if w > 0.5 {
        let fill = Rect::from_min_size(rect.min, vec2(w.max(height), height));
        ui.painter().rect_filled(fill, r, color);
    }
    resp
}

/// A rounded card with little padding, for lists that paint their own rows.
pub fn card_plain(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        });
}

/// One setting: title and explanation on the left, its control on the right.
/// The row has a fixed height, so the right-aligned control cannot stretch
/// the card it sits in.
pub fn setting_row(ui: &mut Ui, title: &str, hint: &str, control: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    let width = ui.available_width();
    let text_width = (width - 210.0).max(120.0);
    let hint_galley = (!hint.is_empty()).then(|| {
        ui.painter()
            .layout(hint.to_string(), regular(12.0), p.muted, text_width)
    });
    let height = match &hint_galley {
        Some(g) => (26.0 + g.size().y + 6.0).max(44.0),
        None => 40.0,
    };
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let title_pos = match &hint_galley {
        Some(_) => pos2(rect.left(), rect.top() + 4.0),
        None => pos2(rect.left(), rect.center().y - 10.0),
    };
    ui.painter()
        .text(title_pos, Align2::LEFT_TOP, title, regular(14.0), p.text);
    if let Some(g) = hint_galley {
        ui.painter()
            .galley(pos2(rect.left(), rect.top() + 25.0), g, p.muted);
    }
    let control_rect =
        Rect::from_min_max(pos2(rect.left() + text_width + 8.0, rect.top()), rect.max);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(control_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    control(&mut child);
}

/// An on/off switch painted at a fixed place, for rows that lay themselves
/// out by hand (it does not move the layout cursor).
pub fn toggle_at(ui: &Ui, rect: Rect, id: egui::Id, on: bool) -> Response {
    let p = pal(ui);
    let resp = ui.interact(rect, id, Sense::click());
    let t = ui.ctx().animate_bool_with_time(id, on, 0.12);
    let track = if on {
        p.accent
    } else {
        p.faint.gamma_multiply(0.6)
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(11), track);
    let x = egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t);
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), 8.0, Color32::WHITE);
    resp
}
