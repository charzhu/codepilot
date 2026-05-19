//! Builds the `/skin` picker dialog for the TUI.
//!
//! Skins are broader than syntax themes: they affect the chat background,
//! shared popup surfaces, selected-row accents, and other TUI chrome. The
//! synthetic `default` entry means no skin override is applied or persisted.

use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::SideContentWidth;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::render::renderable::Renderable;
use crate::skin;
use crate::skin::DEFAULT_SKIN_ID;
use crate::skin::Skin;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Widget;

const WIDE_PREVIEW_MIN_WIDTH: u16 = 36;
const WIDE_PREVIEW_LEFT_INSET: u16 = 2;

struct SkinPreviewWideRenderable;
struct SkinPreviewNarrowRenderable;

/// Builds [`SelectionViewParams`] for the `/skin` picker dialog.
pub(crate) fn build_skin_picker_params(current_id: Option<&str>) -> SelectionViewParams {
    let original_skin_id = skin::current_skin_id()
        .unwrap_or(DEFAULT_SKIN_ID)
        .to_string();
    let entries = skin::list_skins();
    let effective_id = current_id
        .map(skin::normalize_skin_id)
        .filter(|id| skin::is_valid_skin_id(id))
        .unwrap_or_else(|| original_skin_id.clone());

    let mut initial_idx = None;
    let items: Vec<SelectionItem> = entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let is_current = entry.id == effective_id;
            if is_current {
                initial_idx = Some(idx);
            }
            let id_for_action = entry.id.to_string();
            SelectionItem {
                name: entry.display_name.to_string(),
                description: Some(entry.description.to_string()),
                is_current,
                is_default: entry.is_default,
                dismiss_on_select: true,
                search_value: Some(entry.id.to_string()),
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::SkinSelected {
                        id: id_for_action.clone(),
                    });
                })],
                ..Default::default()
            }
        })
        .collect();

    let preview_skin_ids: Vec<Option<String>> =
        items.iter().map(|item| item.search_value.clone()).collect();
    let on_selection_changed = Some(Box::new(
        move |idx: usize, tx: &crate::app_event_sender::AppEventSender| {
            if let Some(Some(id)) = preview_skin_ids.get(idx)
                && skin::set_runtime_skin_by_id(id)
            {
                tx.send(AppEvent::SkinPreviewed);
            }
        },
    )
        as Box<dyn Fn(usize, &crate::app_event_sender::AppEventSender) + Send + Sync>);

    let on_cancel = Some(
        Box::new(move |tx: &crate::app_event_sender::AppEventSender| {
            skin::set_runtime_skin_by_id(&original_skin_id);
            tx.send(AppEvent::SkinPreviewed);
        }) as Box<dyn Fn(&crate::app_event_sender::AppEventSender) + Send + Sync>,
    );

    SelectionViewParams {
        title: Some("Select TUI Skin".to_string()),
        subtitle: Some("Recolor backgrounds, surfaces, and accents.".to_string()),
        footer_hint: Some(standard_popup_hint_line()),
        items,
        is_searchable: true,
        search_placeholder: Some("Type to filter skins...".to_string()),
        initial_selected_idx: initial_idx,
        side_content: Box::new(SkinPreviewWideRenderable),
        side_content_width: SideContentWidth::Half,
        side_content_min_width: WIDE_PREVIEW_MIN_WIDTH,
        stacked_side_content: Some(Box::new(SkinPreviewNarrowRenderable)),
        preserve_side_content_bg: true,
        on_selection_changed,
        on_cancel,
        ..Default::default()
    }
}

impl Renderable for SkinPreviewWideRenderable {
    fn desired_height(&self, _width: u16) -> u16 {
        u16::MAX
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        render_preview(area, buf, PreviewLayout::Wide);
    }
}

impl Renderable for SkinPreviewNarrowRenderable {
    fn desired_height(&self, _width: u16) -> u16 {
        preview_lines(skin::current_skin(), PreviewLayout::Narrow).len() as u16
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        render_preview(area, buf, PreviewLayout::Narrow);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewLayout {
    Wide,
    Narrow,
}

fn render_preview(area: Rect, buf: &mut Buffer, layout: PreviewLayout) {
    if area.is_empty() {
        return;
    }

    skin::paint_background(area, buf);
    let lines = preview_lines(skin::current_skin(), layout);
    let top_padding = if layout == PreviewLayout::Wide {
        area.height.saturating_sub(lines.len() as u16) / 2
    } else {
        0
    };
    let left_padding = if layout == PreviewLayout::Wide {
        WIDE_PREVIEW_LEFT_INSET.min(area.width)
    } else {
        0
    };
    let render_area = Rect::new(
        area.x.saturating_add(left_padding),
        area.y.saturating_add(top_padding),
        area.width.saturating_sub(left_padding),
        area.height.saturating_sub(top_padding),
    );

    for (idx, line) in lines.into_iter().enumerate() {
        let y = render_area.y.saturating_add(idx as u16);
        if y >= area.y.saturating_add(area.height) {
            break;
        }
        line.render(
            Rect::new(render_area.x, y, render_area.width, /*height*/ 1),
            buf,
        );
    }
}

fn preview_lines(active_skin: Option<&Skin>, layout: PreviewLayout) -> Vec<Line<'static>> {
    let Some(skin) = active_skin else {
        return default_preview_lines();
    };

    let palette = skin.palette;
    let base = Style::default().fg(palette.text).bg(palette.background);
    let surface = Style::default().fg(palette.text).bg(palette.surface);
    let accent = Style::default()
        .fg(palette.primary)
        .bg(palette.background)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(palette.muted).bg(palette.background);
    let selected = Style::default()
        .fg(palette.text)
        .bg(palette.selection)
        .add_modifier(Modifier::BOLD);
    let border = Style::default().fg(palette.border).bg(palette.background);

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Skin: ", base),
            Span::styled(skin.display_name.to_string(), accent),
        ]),
        Line::from(Span::styled(skin.description.to_string(), muted)),
        Line::from(""),
        Line::from(vec![
            Span::styled("╭", border),
            Span::styled(" surface ", surface),
            Span::styled("╮ ", border),
            Span::styled("› selected command", selected),
        ]),
        Line::from(vec![
            swatch("P", palette.primary, palette.background),
            Span::styled(" ", base),
            swatch("S", palette.secondary, palette.background),
            Span::styled(" ", base),
            swatch("OK", palette.success, palette.background),
            Span::styled(" ", base),
            swatch("!", palette.warning, palette.background),
            Span::styled(" ", base),
            swatch("ERR", palette.danger, palette.background),
        ]),
        Line::from(vec![
            Span::styled("muted helper text", muted),
            Span::styled("  border sample", border),
        ]),
    ];

    if layout == PreviewLayout::Wide {
        lines.insert(3, Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("background ", base),
            Span::styled("surfaces ", surface),
            Span::styled("and accents", accent),
        ]));
    }

    lines
}

fn default_preview_lines() -> Vec<Line<'static>> {
    vec![
        Line::from(vec!["Skin: ".into(), "Default".bold()]),
        Line::from("No additional skin applied.".dim()),
        Line::from(""),
        Line::from("Uses terminal colors and Codex defaults."),
        Line::from("Choose a skin for full TUI recoloring.".dim()),
    ]
}

fn swatch(label: &'static str, color: Color, background: Color) -> Span<'static> {
    Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(background)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_event_sender::AppEventSender;
    use pretty_assertions::assert_eq;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn skin_picker_lists_default_first_and_selects_configured_skin() {
        skin::set_runtime_skin_by_id(DEFAULT_SKIN_ID);

        let params = build_skin_picker_params(Some("deep-ocean"));

        assert_eq!(params.items[0].name, "Default");
        assert!(params.items[0].is_default);
        let selected_idx = params
            .initial_selected_idx
            .expect("expected configured skin to be selected");
        assert_eq!(
            params.items[selected_idx].search_value.as_deref(),
            Some("deep-ocean"),
        );
    }

    #[test]
    fn default_skin_action_emits_skin_selected() {
        let params = build_skin_picker_params(Some(DEFAULT_SKIN_ID));
        let (tx_raw, mut rx) = unbounded_channel::<AppEvent>();
        let tx = AppEventSender::new(tx_raw);

        (params.items[0].actions[0].as_ref())(&tx);

        let event = rx.try_recv().expect("expected skin selected event");
        match event {
            AppEvent::SkinSelected { id } => assert_eq!(id, DEFAULT_SKIN_ID),
            other => panic!("expected skin selected event, got {other:?}"),
        }
    }

    #[test]
    fn skin_preview_uses_visually_distinct_palette_roles() {
        let skin = skin::skin_by_id("neon-circuit").expect("skin exists");
        let lines = preview_lines(Some(skin), PreviewLayout::Wide);
        let rendered = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Neon Circuit"));
        assert!(rendered.contains("selected command"));
        assert!(rendered.contains("ERR"));
    }
}
