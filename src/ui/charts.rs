//! Chart painting shared by the Today and Reports pages.
//!
//! Everything is drawn with the egui painter in the family style: rounded
//! bars, thin grid lines in the border colour, muted labels, semibold
//! numbers. Each chart shows exact values on hover and returns what was
//! clicked, so the pages can drill down.

use super::app::Icons;
use super::theme::{pal, regular, semibold, Palette};
use super::widgets;
use crate::aggregate::{DayRow, Delta, HourMatrix, Row, SegKind, Segment};
use crate::model::AwayLabel;
use crate::rules::Classifier;
use crate::timeutil::{self, Day, Tz};
use eframe::egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, Response,
    RichText, Sense, Stroke, StrokeKind, Ui, Vec2,
};

/// The gap between cards.
pub const GAP: f32 = 14.0;

/// Linear blend of two colours.
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// A card with a fixed outer size, so cards in a row line up.
pub fn card_sized(ui: &mut Ui, width: f32, height: f32, add: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    ui.allocate_ui_with_layout(vec2(width, height), Layout::top_down(Align::Min), |ui| {
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(16))
            .show(ui, |ui| {
                ui.set_width(width - 34.0);
                ui.set_height(height - 34.0);
                add(ui)
            });
    });
}

/// A card as wide as the page whose height follows its content.
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

/// A card title with an optional muted remark on the same line.
pub fn card_title(ui: &mut Ui, title: &str, remark: &str) {
    let p = pal(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(semibold(15.0)));
        if !remark.is_empty() {
            ui.label(RichText::new(remark).size(12.5).color(p.faint));
        }
    });
}

/// A muted line for a card that has nothing to show.
pub fn nothing(ui: &mut Ui, text: &str) {
    let p = pal(ui);
    ui.add_space(6.0);
    ui.label(RichText::new(text).color(p.faint));
}

/// One summary figure in a fixed-width column; `extra` goes under the caption.
pub fn stat_cell(
    ui: &mut Ui,
    width: f32,
    value: &str,
    caption: &str,
    color: Option<Color32>,
    extra: impl FnOnce(&mut Ui),
) {
    ui.allocate_ui_with_layout(vec2(width, 74.0), Layout::top_down(Align::Min), |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 2.0;
        widgets::stat(ui, value, caption, color);
        extra(ui);
    });
}

// --------------------------------------------------------------------------------------
// Delta badges
// --------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DeltaUnit {
    /// Show the change in percent of the previous value.
    Percent,
    /// Show the change as a duration.
    Duration,
    /// Show the change in points (scores).
    Points,
}

/// The text of a change: `↑ 12%`, `↓ 1h 05m`, `↑ 4 pts`, `new`, `no change`.
pub fn delta_text(delta: &Delta, unit: DeltaUnit) -> String {
    let diff = delta.diff();
    if diff == 0 {
        return "no change".into();
    }
    let arrow = if diff > 0 { "↑" } else { "↓" };
    match unit {
        DeltaUnit::Percent => match delta.pct() {
            Some(pct) if pct.abs() >= 999.5 => format!("{arrow} 999%+"),
            Some(pct) => format!("{arrow} {:.0}%", pct.abs().max(1.0)),
            None => "new".into(),
        },
        DeltaUnit::Duration => format!("{arrow} {}", timeutil::fmt_dur(diff.abs())),
        DeltaUnit::Points => format!("{arrow} {} pts", diff.abs()),
    }
}

/// A pill showing the change against the previous period. `up_is_good`:
/// `Some(true)` paints a rise green and a fall red, `Some(false)` the other
/// way round, `None` paints both neutral.
pub fn delta_badge(
    ui: &mut Ui,
    delta: &Delta,
    unit: DeltaUnit,
    up_is_good: Option<bool>,
) -> Response {
    let p = pal(ui);
    let diff = delta.diff();
    let color = match up_is_good {
        _ if diff == 0 => p.muted,
        Some(good_up) => {
            if (diff > 0) == good_up {
                p.good
            } else {
                p.danger
            }
        }
        None => p.muted,
    };
    widgets::badge(ui, &delta_text(delta, unit), color)
}

// --------------------------------------------------------------------------------------
// Legend and share bar
// --------------------------------------------------------------------------------------

/// Colour dots with names, wrapping onto more lines when needed.
pub fn legend(ui: &mut Ui, items: &[(Color32, String)]) {
    let p = pal(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 2.0);
        for (color, name) in items {
            widgets::dot(ui, *color);
            if super::bidi::needs_native(name) {
                super::bidi::label(ui, name, 12.5, false, p.muted);
            } else {
                ui.label(RichText::new(name).size(12.5).color(p.muted));
            }
            ui.add_space(8.0);
        }
    });
}

/// A single horizontal bar split by share. Returns the hovered/clicked part.
pub fn share_bar(ui: &mut Ui, parts: &[(Color32, i64, String)], height: f32) -> Option<usize> {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    let total: i64 = parts.iter().map(|x| x.1).sum();
    let radius = CornerRadius::same((height / 2.0) as u8);
    ui.painter().rect_filled(rect, radius, p.hover);
    if total <= 0 {
        return None;
    }
    let pointer = resp.hover_pos();
    let mut x = rect.left();
    let mut hit = None;
    let visible: Vec<usize> = (0..parts.len()).filter(|i| parts[*i].1 > 0).collect();
    for (n, &i) in visible.iter().enumerate() {
        let (color, secs, name) = &parts[i];
        let w = rect.width() * *secs as f32 / total as f32;
        let mut piece = Rect::from_min_size(pos2(x, rect.top()), vec2(w, height));
        x += w;
        if n + 1 < visible.len() {
            // A hairline between parts keeps neighbours apart without relying on colour.
            piece.max.x -= 1.5;
        }
        if piece.width() < 0.5 {
            continue;
        }
        let r = (height / 2.0).min(piece.width() / 2.0) as u8;
        let corners = CornerRadius {
            nw: if n == 0 { r } else { 0 },
            sw: if n == 0 { r } else { 0 },
            ne: if n + 1 == visible.len() { r } else { 0 },
            se: if n + 1 == visible.len() { r } else { 0 },
        };
        ui.painter().rect_filled(piece, corners, *color);
        if pointer.is_some_and(|pt| pt.x >= piece.left() && pt.x < x) {
            hit = Some(i);
            ui.painter()
                .rect_filled(piece, corners, Color32::from_white_alpha(28));
            let text = format!(
                "{name}\n{}  ·  {:.0}%",
                timeutil::fmt_dur(*secs),
                *secs as f32 * 100.0 / total as f32
            );
            resp.clone().on_hover_text_at_pointer(text);
        }
    }
    if resp.clicked() {
        hit
    } else {
        None
    }
}

// --------------------------------------------------------------------------------------
// Day timeline
// --------------------------------------------------------------------------------------

/// The hours a day bar should show: from the hour of the first activity to
/// the hour after the last (or `now`), at least nine hours, inside the day.
pub fn timeline_window(
    segments: &[Segment],
    day: (i64, i64),
    now: Option<i64>,
    tz: &Tz,
) -> (i64, i64) {
    let first = segments.first().map(|s| s.start);
    let last = segments.iter().map(|s| s.end).max();
    let (Some(first), Some(last)) = (first, last) else {
        // Nothing recorded: office hours.
        let from = (day.0 + 8 * 3600).min(day.1);
        return (from, (from + 10 * 3600).min(day.1));
    };
    let last = now.map_or(last, |n| last.max(n.min(day.1)));
    let mut from = first - (tz.second_of_day(first) % 3600) as i64;
    let rest = (tz.second_of_day(last) % 3600) as i64;
    let mut to = if rest == 0 { last } else { last + 3600 - rest };
    while to - from < 9 * 3600 && (from > day.0 || to < day.1) {
        if from > day.0 {
            from -= 3600;
        }
        if to - from < 9 * 3600 && to < day.1 {
            to += 3600;
        }
    }
    (from.max(day.0), to.min(day.1))
}

fn segment_color(p: &Palette, classifier: &Classifier, kind: SegKind) -> Color32 {
    match kind {
        SegKind::Work(c) => classifier
            .categories
            .get(c)
            .map(|c| widgets::rgb(c.color))
            .unwrap_or(p.faint),
        SegKind::Away(_) => p.faint,
    }
}

fn segment_name(classifier: &Classifier, kind: SegKind) -> String {
    match kind {
        SegKind::Work(c) => classifier
            .categories
            .get(c)
            .map(|c| c.name.clone())
            .unwrap_or_default(),
        SegKind::Away(AwayLabel::None) => "Away".into(),
        SegKind::Away(label) => label.name().into(),
    }
}

/// The bar of a day: worked time as full-height pieces in the category
/// colour, away time as a thin grey line, unrecorded time empty. Hour ticks
/// below, an optional "now" marker. Returns the index of a clicked segment.
pub fn day_timeline(
    ui: &mut Ui,
    segments: &[Segment],
    window: (i64, i64),
    now: Option<i64>,
    classifier: &Classifier,
    tz: &Tz,
) -> Option<usize> {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 66.0), Sense::click());
    let bar = Rect::from_min_max(
        pos2(rect.left(), rect.top() + 12.0),
        pos2(rect.right(), rect.top() + 46.0),
    );
    let span = (window.1 - window.0).max(1) as f32;
    let x_of = |t: i64| bar.left() + bar.width() * ((t - window.0) as f32 / span).clamp(0.0, 1.0);
    let painter = ui.painter();
    painter.rect_filled(bar, CornerRadius::same(7), p.hover);

    // Hour ticks and labels.
    let hours = ((window.1 - window.0) / 3600).max(1);
    let step = if hours <= 13 {
        1
    } else if hours <= 18 {
        2
    } else {
        3
    };
    let mut t = window.0;
    let mut n = 0;
    while t <= window.1 {
        let x = x_of(t);
        painter.line_segment(
            [pos2(x, bar.bottom() + 2.0), pos2(x, bar.bottom() + 6.0)],
            Stroke::new(1.0_f32, p.border),
        );
        if n % step == 0 {
            let align = if t == window.0 {
                Align2::LEFT_TOP
            } else if t >= window.1 {
                Align2::RIGHT_TOP
            } else {
                Align2::CENTER_TOP
            };
            painter.text(
                pos2(x, bar.bottom() + 7.0),
                align,
                timeutil::fmt_hm(t, tz),
                regular(11.5),
                p.faint,
            );
        }
        t += 3600;
        n += 1;
    }

    let pointer = resp.hover_pos();
    let mut hit = None;
    for (i, seg) in segments.iter().enumerate() {
        if seg.end <= window.0 || seg.start >= window.1 {
            continue;
        }
        let (x0, x1) = (x_of(seg.start), x_of(seg.end));
        let x1 = x1.max(x0 + 1.0);
        let color = segment_color(&p, classifier, seg.kind);
        let piece = match seg.kind {
            SegKind::Work(_) => Rect::from_min_max(pos2(x0, bar.top()), pos2(x1, bar.bottom())),
            // Away time is thinner, so it reads as "not work" without colour.
            SegKind::Away(_) => Rect::from_min_max(
                pos2(x0, bar.center().y - 3.0),
                pos2(x1, bar.center().y + 3.0),
            ),
        };
        let radius = CornerRadius::same(piece.width().min(6.0) as u8 / 2);
        let fill = match seg.kind {
            SegKind::Work(_) => color,
            SegKind::Away(_) => color.gamma_multiply(0.55),
        };
        painter.rect_filled(piece, radius, fill);
        let hovered = pointer
            .is_some_and(|pt| pt.x >= x0 && pt.x < x1 && pt.y >= bar.top() && pt.y <= bar.bottom());
        if hovered {
            hit = Some(i);
            painter.rect_stroke(
                piece.expand(1.0),
                radius,
                Stroke::new(1.5_f32, p.text),
                StrokeKind::Outside,
            );
        }
    }

    if let Some(now) = now.filter(|n| *n >= window.0 && *n <= window.1) {
        let x = x_of(now);
        painter.line_segment(
            [pos2(x, bar.top() - 5.0), pos2(x, bar.bottom() + 3.0)],
            Stroke::new(2.0_f32, p.accent),
        );
        let align = if x > bar.right() - 30.0 {
            Align2::RIGHT_BOTTOM
        } else {
            Align2::CENTER_BOTTOM
        };
        painter.text(
            pos2(x, bar.top() - 4.0),
            align,
            "now",
            semibold(10.5),
            p.accent,
        );
    }

    if let Some(i) = hit {
        let seg = &segments[i];
        let mut text = format!(
            "{} – {}  ·  {}\n{}",
            timeutil::fmt_hm(seg.start, tz),
            timeutil::fmt_hm(seg.end, tz),
            timeutil::fmt_dur(seg.end - seg.start),
            segment_name(classifier, seg.kind)
        );
        if !seg.app.is_empty() {
            text.push_str("  ·  ");
            text.push_str(&seg.app);
        }
        resp.clone().on_hover_text_at_pointer(text);
        if resp.clicked() {
            return Some(i);
        }
    }
    None
}

// --------------------------------------------------------------------------------------
// Ranked list
// --------------------------------------------------------------------------------------

pub struct RankOptions<'a> {
    /// Paint program icons (rows without a colour of their own).
    pub icons: Option<&'a Icons>,
    /// Bar colour for rows without a colour of their own.
    pub accent: Color32,
    /// Rows react to hover and click.
    pub clickable: bool,
    /// At most this many rows.
    pub max_rows: usize,
}

/// A ranked list: mark, name, duration and share, with a thin bar under each
/// row. Returns the index of the clicked row.
pub fn ranked_list(ui: &mut Ui, rows: &[Row], options: &RankOptions) -> Option<usize> {
    let p = pal(ui);
    let max = rows.iter().map(|r| r.secs).max().unwrap_or(0).max(1);
    let mut clicked = None;
    let prev_spacing = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 2.0;
    for (i, row) in rows.iter().take(options.max_rows).enumerate() {
        let can_click = options.clickable && !row.other;
        let (rect, resp) = ui.allocate_exact_size(
            vec2(ui.available_width(), 36.0),
            if can_click {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        if can_click && resp.hovered() {
            ui.painter()
                .rect_filled(rect.expand2(vec2(6.0, 0.0)), CornerRadius::same(7), p.hover);
        }
        let color = row.color.map(widgets::rgb);
        let mut left = rect.left();
        if let Some(color) = color {
            ui.painter()
                .circle_filled(pos2(left + 9.0, rect.top() + 14.0), 5.0, color);
            left += 26.0;
        } else if let Some(icons) = options.icons.filter(|_| !row.other) {
            let icon = Rect::from_min_size(pos2(left, rect.top() + 4.0), Vec2::splat(20.0));
            widgets::program_icon(
                ui,
                icon,
                if row.exe.is_empty() {
                    &row.name
                } else {
                    &row.exe
                },
                icons.get(&row.exe),
            );
            left += 28.0;
        }
        let right = rect.right();
        let top = Rect::from_min_max(pos2(left, rect.top()), pos2(right, rect.top() + 28.0));
        let share = Rect::from_min_max(pos2(right - 36.0, top.top()), top.max);
        let dur = Rect::from_min_max(
            pos2(right - 102.0, top.top()),
            pos2(right - 40.0, top.bottom()),
        );
        let name = Rect::from_min_max(top.min, pos2(right - 106.0, top.bottom()));
        let cut = widgets::text_in(
            ui,
            name,
            &row.name,
            regular(14.0),
            if row.other { p.muted } else { p.text },
        );
        widgets::text_in_right(
            ui,
            dur,
            &timeutil::fmt_dur(row.secs),
            semibold(13.5),
            p.text,
        );
        widgets::text_in_right(
            ui,
            share,
            &format!("{:.0}%", row.share * 100.0),
            regular(12.5),
            p.faint,
        );
        let track = Rect::from_min_max(
            pos2(left, rect.bottom() - 6.0),
            pos2(right, rect.bottom() - 3.0),
        );
        ui.painter()
            .rect_filled(track, CornerRadius::same(2), p.hover);
        let w = (track.width() * row.secs as f32 / max as f32).max(3.0);
        ui.painter().rect_filled(
            Rect::from_min_size(track.min, vec2(w, track.height())),
            CornerRadius::same(2),
            if row.other {
                p.faint
            } else {
                color.unwrap_or(options.accent)
            },
        );
        let resp = if cut {
            resp.on_hover_text(&row.name)
        } else {
            resp
        };
        if can_click && resp.clicked() {
            clicked = Some(i);
        }
    }
    ui.spacing_mut().item_spacing.y = prev_spacing;
    clicked
}

// --------------------------------------------------------------------------------------
// Bars per day
// --------------------------------------------------------------------------------------

/// A round axis maximum (seconds) and the grid step under it.
fn nice_axis(max_secs: i64) -> (i64, i64) {
    let steps = [900, 1800, 3600, 2 * 3600, 3 * 3600, 4 * 3600, 6 * 3600];
    for step in steps {
        if max_secs <= step * 4 {
            let top = ((max_secs + step - 1) / step).max(1) * step;
            return (top, step);
        }
    }
    let step = 12 * 3600;
    (((max_secs + step - 1) / step).max(1) * step, step)
}

fn axis_label(secs: i64) -> String {
    if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}m", secs / 60)
    }
}

/// Stacked bars, one per day, split by category (`colors` and `names` follow
/// `Classifier::categories`). Returns the clicked day.
pub fn stacked_days(
    ui: &mut Ui,
    rows: &[DayRow],
    colors: &[Color32],
    names: &[String],
    height: f32,
    today: Day,
) -> Option<Day> {
    let p = pal(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
    if rows.is_empty() {
        return None;
    }
    let painter = ui.painter();
    let plot = Rect::from_min_max(
        pos2(rect.left() + 34.0, rect.top() + 8.0),
        pos2(rect.right(), rect.bottom() - 22.0),
    );
    let max = rows.iter().map(|r| r.worked).max().unwrap_or(0);
    let (top, step) = nice_axis(max.max(1800));
    let y_of = |secs: i64| plot.bottom() - plot.height() * secs as f32 / top as f32;
    let mut v = 0;
    while v <= top {
        let y = y_of(v);
        painter.line_segment(
            [pos2(plot.left(), y), pos2(plot.right(), y)],
            Stroke::new(1.0_f32, if v == 0 { p.faint } else { p.border }),
        );
        painter.text(
            pos2(plot.left() - 6.0, y),
            Align2::RIGHT_CENTER,
            axis_label(v),
            regular(11.5),
            p.faint,
        );
        v += step;
    }

    let n = rows.len();
    let slot = plot.width() / n as f32;
    let bar_w = (slot * 0.64).clamp(1.5, 46.0);
    // How often a day gets a label under it.
    let every = if slot >= 54.0 {
        1
    } else if slot >= 20.0 {
        if n > 16 {
            2
        } else {
            1
        }
    } else {
        0
    };
    let pointer = resp
        .hover_pos()
        .filter(|pt| pt.x >= plot.left() && pt.x < plot.right());
    let hovered = pointer.map(|pt| (((pt.x - plot.left()) / slot) as usize).min(n - 1));
    for (i, row) in rows.iter().enumerate() {
        let cx = plot.left() + slot * (i as f32 + 0.5);
        if hovered == Some(i) {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(cx - slot / 2.0, plot.top()),
                    pos2(cx + slot / 2.0, plot.bottom()),
                ),
                CornerRadius::same(4),
                p.hover,
            );
        }
        let last = row.by_category.iter().rposition(|v| *v > 0);
        let mut acc = 0;
        for (c, secs) in row.by_category.iter().enumerate() {
            if *secs <= 0 {
                continue;
            }
            let y1 = y_of(acc);
            acc += secs;
            let y0 = y_of(acc);
            let piece = Rect::from_min_max(pos2(cx - bar_w / 2.0, y0), pos2(cx + bar_w / 2.0, y1));
            let r = (bar_w / 2.0).min(5.0).min(piece.height()) as u8;
            let corners = if last == Some(c) {
                CornerRadius {
                    nw: r,
                    ne: r,
                    sw: 0,
                    se: 0,
                }
            } else {
                CornerRadius::ZERO
            };
            painter.rect_filled(piece, corners, colors.get(c).copied().unwrap_or(p.faint));
        }
        // Labels under the axis.
        let (_, month, dom) = timeutil::civil_from_days(row.day);
        let weekend = timeutil::weekday(row.day) >= 5;
        let color = if row.day == today {
            p.accent
        } else if weekend {
            p.faint
        } else {
            p.muted
        };
        let label = if every == 1 && slot >= 54.0 {
            Some(format!(
                "{} {dom}",
                timeutil::WEEKDAYS[timeutil::weekday(row.day) as usize]
            ))
        } else if every > 0 {
            (i % every == 0).then(|| dom.to_string())
        } else {
            // Many days: name the months, and the first day shown.
            (dom == 1 || i == 0).then(|| {
                if dom == 1 {
                    timeutil::MONTHS[month as usize - 1].to_string()
                } else {
                    timeutil::fmt_day_short(row.day)
                }
            })
        };
        if let Some(label) = label {
            let align = if every == 0 {
                Align2::LEFT_TOP
            } else {
                Align2::CENTER_TOP
            };
            let x = if every == 0 { cx - slot / 2.0 } else { cx };
            if every == 0 {
                painter.line_segment(
                    [pos2(x, plot.bottom()), pos2(x, plot.bottom() + 4.0)],
                    Stroke::new(1.0_f32, p.faint),
                );
            }
            painter.text(
                pos2(x + if every == 0 { 3.0 } else { 0.0 }, plot.bottom() + 5.0),
                align,
                label,
                regular(11.5),
                color,
            );
        }
    }

    if let Some(i) = hovered {
        let row = &rows[i];
        let mut text = format!(
            "{}\n{} worked",
            timeutil::fmt_day(row.day),
            timeutil::fmt_dur(row.worked)
        );
        let mut parts: Vec<(i64, &str)> = row
            .by_category
            .iter()
            .enumerate()
            .filter(|(_, v)| **v > 0)
            .map(|(c, v)| (*v, names.get(c).map(String::as_str).unwrap_or("")))
            .collect();
        parts.sort_by_key(|(v, _)| std::cmp::Reverse(*v));
        for (secs, name) in parts.iter().take(5) {
            text.push_str(&format!("\n{name}: {}", timeutil::fmt_dur(*secs)));
        }
        resp.clone().on_hover_text_at_pointer(text);
        if resp.clicked() {
            return Some(row.day);
        }
    }
    None
}

// --------------------------------------------------------------------------------------
// Heat maps
// --------------------------------------------------------------------------------------

/// Five steps from "nothing" to the accent colour.
fn heat_color(p: &Palette, level: usize) -> Color32 {
    match level {
        0 => p.hover,
        1 => mix(p.hover, p.accent, 0.22),
        2 => mix(p.hover, p.accent, 0.42),
        3 => mix(p.hover, p.accent, 0.66),
        _ => p.accent,
    }
}

fn heat_level(value: i64, max: i64) -> usize {
    if value <= 0 || max <= 0 {
        0
    } else {
        (1 + (value * 4 - 1) / max).clamp(1, 4) as usize
    }
}

fn heat_legend(ui: &mut Ui, p: &Palette, max_text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.label(RichText::new("Less").size(11.5).color(p.faint));
        for level in 0..5 {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(12.0), Sense::hover());
            ui.painter()
                .rect_filled(r, CornerRadius::same(3), heat_color(p, level));
        }
        ui.label(RichText::new("More").size(11.5).color(p.faint));
        ui.add_space(8.0);
        ui.label(RichText::new(max_text).size(11.5).color(p.faint));
    });
}

/// A calendar of worked time: weeks as columns, weekdays as rows. `days`
/// must be consecutive. Returns the clicked day.
pub fn calendar_heatmap(
    ui: &mut Ui,
    days: &[(Day, i64)],
    monday_first: bool,
    today: Day,
) -> Option<Day> {
    let p = pal(ui);
    let &(first, _) = days.first()?;
    const CELL: f32 = 14.0;
    const PITCH: f32 = 17.0;
    const LEFT: f32 = 32.0;
    const TOP: f32 = 18.0;
    let origin = timeutil::week_start(first, monday_first);
    let last = days[days.len() - 1].0;
    let weeks = ((last - origin) / 7 + 1) as usize;
    let size = vec2(LEFT + weeks as f32 * PITCH, TOP + 7.0 * PITCH);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    let max = days.iter().map(|d| d.1).max().unwrap_or(0);

    // Weekday names on every other row.
    for row in [0usize, 2, 4, 6] {
        let wd = if monday_first { row } else { (row + 6) % 7 };
        painter.text(
            pos2(
                rect.left(),
                rect.top() + TOP + row as f32 * PITCH + CELL / 2.0,
            ),
            Align2::LEFT_CENTER,
            timeutil::WEEKDAYS[wd],
            regular(11.0),
            p.faint,
        );
    }

    let pointer = resp.hover_pos();
    let mut hit: Option<(Day, i64)> = None;
    let mut labelled_until = f32::MIN;
    for &(day, secs) in days {
        let col = ((day - origin) / 7) as f32;
        let row = ((day - origin) % 7) as f32;
        let cell = Rect::from_min_size(
            pos2(
                rect.left() + LEFT + col * PITCH,
                rect.top() + TOP + row * PITCH,
            ),
            Vec2::splat(CELL),
        );
        painter.rect_filled(
            cell,
            CornerRadius::same(3),
            heat_color(&p, heat_level(secs, max)),
        );
        if day == today {
            painter.rect_stroke(
                cell,
                CornerRadius::same(3),
                Stroke::new(1.5_f32, p.text),
                StrokeKind::Outside,
            );
        }
        let (_, month, dom) = timeutil::civil_from_days(day);
        if dom == 1 || day == first {
            // The month name over the column where the month starts.
            let x = cell.left();
            // The first, partial month is only named when it has room
            // before the next month's name.
            if x > labelled_until && dom <= 14 {
                painter.text(
                    pos2(x, rect.top()),
                    Align2::LEFT_TOP,
                    timeutil::MONTHS[month as usize - 1],
                    regular(11.5),
                    p.muted,
                );
                labelled_until = x + 28.0;
            }
        }
        if pointer.is_some_and(|pt| cell.expand(1.5).contains(pt)) {
            hit = Some((day, secs));
            painter.rect_stroke(
                cell,
                CornerRadius::same(3),
                Stroke::new(1.5_f32, p.accent),
                StrokeKind::Outside,
            );
        }
    }
    heat_legend(
        ui,
        &p,
        &if max > 0 {
            format!("up to {} a day", timeutil::fmt_dur(max))
        } else {
            "nothing recorded yet".to_string()
        },
    );
    if let Some((day, secs)) = hit {
        let text = if secs > 0 {
            format!(
                "{}\n{} worked",
                timeutil::fmt_day(day),
                timeutil::fmt_dur(secs)
            )
        } else {
            format!("{}\nNothing recorded", timeutil::fmt_day(day))
        };
        resp.clone().on_hover_text_at_pointer(text);
        if resp.clicked() {
            return Some(day);
        }
    }
    None
}

/// Worked time by weekday (rows) and hour of day (columns).
pub fn hour_heatmap(ui: &mut Ui, matrix: &HourMatrix, monday_first: bool) {
    let p = pal(ui);
    const LEFT: f32 = 32.0;
    const TOP: f32 = 18.0;
    const ROW: f32 = 17.0;
    let width = ui.available_width();
    let pitch = ((width - LEFT) / 24.0).clamp(10.0, 22.0);
    let size = vec2(LEFT + pitch * 24.0, TOP + 7.0 * ROW);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    let max = matrix.max();
    for hour in (0..24).step_by(3) {
        painter.text(
            pos2(rect.left() + LEFT + hour as f32 * pitch, rect.top()),
            Align2::LEFT_TOP,
            format!("{hour:02}"),
            regular(11.0),
            p.faint,
        );
    }
    let pointer = resp.hover_pos();
    let mut hit = None;
    for row in 0..7usize {
        let wd = if monday_first { row } else { (row + 6) % 7 };
        let y = rect.top() + TOP + row as f32 * ROW;
        painter.text(
            pos2(rect.left(), y + 7.0),
            Align2::LEFT_CENTER,
            timeutil::WEEKDAYS[wd],
            regular(11.0),
            p.faint,
        );
        for hour in 0..24usize {
            let secs = matrix.worked[wd][hour];
            let cell = Rect::from_min_size(
                pos2(rect.left() + LEFT + hour as f32 * pitch, y),
                vec2(pitch - 3.0, ROW - 3.0),
            );
            painter.rect_filled(
                cell,
                CornerRadius::same(3),
                heat_color(&p, heat_level(secs, max)),
            );
            if pointer.is_some_and(|pt| cell.expand(1.5).contains(pt)) {
                hit = Some((wd, hour, secs, matrix.productive[wd][hour]));
                painter.rect_stroke(
                    cell,
                    CornerRadius::same(3),
                    Stroke::new(1.5_f32, p.accent),
                    StrokeKind::Outside,
                );
            }
        }
    }
    heat_legend(
        ui,
        &p,
        &if max > 0 {
            format!("up to {} in one weekday hour", timeutil::fmt_dur(max))
        } else {
            "nothing recorded yet".to_string()
        },
    );
    if let Some((wd, hour, secs, productive)) = hit {
        resp.on_hover_text_at_pointer(format!(
            "{} {hour:02}:00 – {:02}:00\n{} worked in this range\n{} of it productive",
            timeutil::WEEKDAYS[wd],
            (hour + 1) % 24,
            timeutil::fmt_dur(secs),
            timeutil::fmt_dur(productive)
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_is_round() {
        assert_eq!(nice_axis(1800), (1800, 900));
        assert_eq!(nice_axis(3 * 3600 + 5), (4 * 3600, 3600));
        assert_eq!(nice_axis(7 * 3600 + 1), (8 * 3600, 2 * 3600));
        assert_eq!(nice_axis(9 * 3600), (9 * 3600, 3 * 3600));
        assert_eq!(nice_axis(30 * 3600), (36 * 3600, 12 * 3600));
        assert_eq!(axis_label(2 * 3600), "2h");
        assert_eq!(axis_label(900), "15m");
    }

    #[test]
    fn heat_levels() {
        assert_eq!(heat_level(0, 100), 0);
        assert_eq!(heat_level(1, 100), 1);
        assert_eq!(heat_level(25, 100), 1);
        assert_eq!(heat_level(26, 100), 2);
        assert_eq!(heat_level(75, 100), 3);
        assert_eq!(heat_level(100, 100), 4);
        assert_eq!(heat_level(5, 0), 0);
    }

    #[test]
    fn delta_texts() {
        let d = |cur, prev| Delta { cur, prev };
        assert_eq!(delta_text(&d(150, 100), DeltaUnit::Percent), "↑ 50%");
        assert_eq!(delta_text(&d(50, 100), DeltaUnit::Percent), "↓ 50%");
        assert_eq!(delta_text(&d(50, 0), DeltaUnit::Percent), "new");
        assert_eq!(delta_text(&d(7, 7), DeltaUnit::Percent), "no change");
        assert_eq!(delta_text(&d(7200, 3300), DeltaUnit::Duration), "↑ 1h 05m");
        assert_eq!(delta_text(&d(70, 74), DeltaUnit::Points), "↓ 4 pts");
        assert_eq!(delta_text(&d(100_000, 1), DeltaUnit::Percent), "↑ 999%+");
    }

    #[test]
    fn window_covers_the_activity() {
        let tz = Tz::Fixed(0);
        let day = (0, 86_400);
        // Nothing recorded: office hours.
        assert_eq!(timeline_window(&[], day, None, &tz), (8 * 3600, 18 * 3600));
        let seg = |start, end| Segment {
            start,
            end,
            kind: SegKind::Work(0),
            app: String::new(),
            block: 1,
        };
        let w = timeline_window(&[seg(9 * 3600 + 600, 12 * 3600 + 5)], day, None, &tz);
        assert!(w.0 <= 9 * 3600 && w.1 >= 13 * 3600);
        assert!(w.1 - w.0 >= 9 * 3600);
        assert_eq!(w.0 % 3600, 0);
        assert_eq!(w.1 % 3600, 0);
        // Late at night: clamped to the day.
        let w = timeline_window(&[seg(22 * 3600, 86_400 - 5)], day, Some(86_400 - 1), &tz);
        assert_eq!(w.1, 86_400);
        assert_eq!(w.1 - w.0, 9 * 3600);
        assert_eq!(
            mix(Color32::BLACK, Color32::WHITE, 0.5),
            Color32::from_rgb(128, 128, 128)
        );
    }
}
