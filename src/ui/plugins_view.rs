//! The `/plugins` page's shim in `ui` (`docs/plugins-menu.md`): the
//! [`PageKit`] the page — which lives in the `alter_zero_plugins` crate,
//! beside the store it works on — draws itself
//! through, implemented as thin delegations to the picker family's own
//! helpers so the page can never look unlike its siblings; and the four
//! functions the enumeration sites call — the page's build (lines and
//! caret), its rows, its height and its paint. No layout here.

use std::ops::Range;

use alter_zero_plugins::page::{KitColor, PageBuild, PageKit};

use super::model_view::{
    Description, age_text, compact_page, compact_page_with_header, first_page_that_fits,
    linked_segments, list_counter_line, list_window, marked_row_head, model_description_rows,
    model_linked_rows, model_placeholder_row, model_rule, model_wrapped_rows, picker_page,
    placeholder_window, row_head_cols, search_line, view_prompt_row, whole_url_run,
};
use super::theme::*;
use super::wrap::{clamp_spans, cols, ellipsize};
use super::*;

/// The picker family, as the `/plugins` page asks for it.
struct FamilyKit;

impl PageKit for FamilyKit {
    fn cols(&self, text: &str) -> usize {
        cols(text)
    }

    fn ellipsize(&self, text: &str, max: usize) -> String {
        ellipsize(text, max)
    }

    fn clamp(&self, spans: Vec<Span<'static>>, width: usize) -> Line<'static> {
        clamp_spans(spans, width)
    }

    fn wrap_spans(&self, segments: &[(String, Style)], width: u16) -> Vec<Vec<Span<'static>>> {
        super::inline::wrap_inline(segments, width)
    }

    fn linked_segments(&self, text: &str, base: Style) -> Vec<(String, Style)> {
        linked_segments(text, base)
    }

    fn url_run(&self, text: &str, base: Style) -> Option<(String, Style)> {
        whole_url_run(text, base)
    }

    fn age(&self, seconds: i64) -> String {
        age_text(seconds)
    }

    fn elapsed(&self, secs: u64) -> String {
        crate::app::format_elapsed(secs)
    }

    fn field_width(&self, cells: u16) -> u16 {
        super::layout::text_field_width(cells)
    }

    fn indent(&self) -> &'static str {
        MODEL_INDENT
    }

    fn prompt(&self) -> &'static str {
        MODEL_PROMPT
    }

    fn action_marker(&self) -> &'static str {
        HOOKS_MARKER
    }

    fn chip_gap(&self) -> &'static str {
        ASK_CHIP_GAP
    }

    fn value_gap(&self) -> usize {
        SETTINGS_VALUE_GAP
    }

    fn tall_list_rows(&self) -> usize {
        usize::from(THEME_MENU_MAX_ROWS)
    }

    fn list_rows(&self) -> usize {
        usize::from(SETTINGS_MENU_MAX_ROWS)
    }

    fn color(&self, role: KitColor) -> Color {
        match role {
            KitColor::Meta => model_meta_color(),
            KitColor::Accent => model_selected_color(),
            KitColor::Muted => model_id_color(),
            KitColor::Ok => model_active_color(),
            KitColor::Fail => tool_fail_color(),
            KitColor::Error => toast_error_color(),
            KitColor::Warning => ask_warning_color(),
            KitColor::ValueOn => settings_value_color(),
            KitColor::ValueOff => settings_value_off_color(),
            KitColor::ChipFg => ask_chip_current_fg(),
            KitColor::ChipBg => ask_chip_current_bg(),
            KitColor::ChipIdle => ask_chip_color(),
            KitColor::Label => mcp_detail_label_color(),
            KitColor::Value => mcp_detail_value_color(),
            KitColor::State => mcp_detail_state_color(),
            KitColor::Description => mcp_description_color(),
        }
    }

    fn rule(&self, width: u16) -> Line<'static> {
        model_rule(width)
    }

    fn title(&self, text: &str, width: u16) -> Line<'static> {
        super::header::page_title_line(text, width)
    }

    fn search_line(&self, query: &str) -> Line<'static> {
        search_line(query)
    }

    fn row_head(&self, name: &str, selected: bool) -> Vec<Span<'static>> {
        marked_row_head(name, selected)
    }

    fn row_head_cols(&self) -> usize {
        row_head_cols()
    }

    fn counter(&self, selected: usize, total: usize) -> Line<'static> {
        list_counter_line(selected, total)
    }

    fn placeholder_row(&self, text: &str, color: Color, width: u16) -> Line<'static> {
        model_placeholder_row(text, color, width)
    }

    fn wrapped_rows(&self, text: &str, color: Color, width: u16) -> Vec<Line<'static>> {
        model_wrapped_rows(text, color, width)
    }

    fn linked_rows(&self, text: &str, color: Color, width: u16) -> Vec<Line<'static>> {
        model_linked_rows(text, color, width)
    }

    fn description_row(&self, text: &str, width: u16) -> Line<'static> {
        model_description_rows(text, Description::OneRow, width)
            .pop()
            .unwrap_or_default()
    }

    fn prompt_row(&self, lines: &[Line<'static>]) -> Option<u16> {
        view_prompt_row(lines)
    }

    fn list_window(&self, len: usize, selected: usize, max: usize) -> (usize, Range<usize>, usize) {
        list_window(len, selected, max)
    }

    fn picker_page(
        &self,
        header: &[Line<'static>],
        search: &Line<'static>,
        body: Vec<Line<'static>>,
        width: u16,
    ) -> Vec<Line<'static>> {
        picker_page(header, search, body, width)
    }

    fn compact_page_with_header(
        &self,
        header: &[Line<'static>],
        search: &Line<'static>,
        window: &dyn Fn(usize) -> (usize, Vec<Line<'static>>, usize),
        tail: &[Line<'static>],
        width: u16,
    ) -> Vec<Line<'static>> {
        compact_page_with_header(header, search, window, tail, width)
    }

    fn compact_page(
        &self,
        height: u16,
        search: &Line<'static>,
        window: &dyn Fn(usize) -> (usize, Vec<Line<'static>>, usize),
        tail: &[Line<'static>],
        width: u16,
    ) -> Vec<Line<'static>> {
        compact_page(height, search, window, tail, width)
    }

    fn placeholder_window(
        &self,
        block: &[Line<'static>],
        max: usize,
    ) -> (usize, Vec<Line<'static>>, usize) {
        placeholder_window(block, max)
    }

    fn first_page_that_fits(
        &self,
        height: u16,
        pages: &[&dyn Fn() -> Vec<Line<'static>>],
    ) -> Vec<Line<'static>> {
        first_page_that_fits(height, pages)
    }
}

/// The page as lines and its caret — the crate's one builder through the
/// family's kit, so the reserved height, the paint, the cursor seat and the
/// flow all read the same page. Empty when the page is closed.
pub(super) fn plugins_build(app: &App, width: u16, height: u16) -> PageBuild {
    match app.plugins_menu.as_deref() {
        Some(page) => page.build(&FamilyKit, width, height),
        None => PageBuild {
            lines: Vec::new(),
            cursor: None,
        },
    }
}

/// The page as lines — what [`render_plugins_menu`] paints, what
/// [`plugins_menu_height`] counts and what flows (`docs/view-flow.md`).
#[must_use]
pub fn plugins_view_lines(app: &App, width: u16, height: u16) -> Vec<Line<'static>> {
    plugins_build(app, width, height).lines
}

/// The rows the page's own frame occupies on a terminal `height` rows tall.
pub(super) fn plugins_menu_rows(app: &App, width: u16, height: u16) -> u16 {
    u16::try_from(plugins_view_lines(app, width, height).len()).unwrap_or(u16::MAX)
}

/// The inline live-region height while `/plugins` is open, or `None` when it
/// is not: the streaming strip keeps its rows above the page, clamped to the
/// terminal.
#[must_use]
pub fn plugins_menu_height(app: &App, width: u16, term_height: u16) -> Option<u16> {
    app.plugins_menu.as_ref()?;
    Some(super::layout::view_height(
        app,
        width,
        plugins_menu_rows(app, width, term_height),
        term_height,
    ))
}

/// Render the page into the live region in place of the composer —
/// bottom-anchored, so a plugin's page taller than the area keeps its
/// actions and hint on screen while its top flows. See
/// `docs/plugins-menu.md`.
pub fn render_plugins_menu(area: Rect, buf: &mut Buffer, app: &App) {
    super::view_flow::render_framed_tail(
        area,
        buf,
        plugins_view_lines(app, area.width, area.height),
    );
}
