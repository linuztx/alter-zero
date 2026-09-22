//! The `/export` page: the conversation as plain text, copied to the
//! clipboard or saved to a file. See `docs/export.md`.
//!
//! `/copy`'s sibling — the **whole** transcript where `/copy` is the last
//! reply — and, as a page, the `/donate` page's ([`super::donate`]): a
//! read-only composer-replacing picker with **no text entry**, two rows that
//! are the two answers, the hardware cursor hidden while its seat tracks the
//! highlighted `❯`, every key owned while open. The pure side decides *what*
//! the export is (the transcript on screen, rendered by `ui::export_text`)
//! and *where* it goes ([`ExportTarget`]); the boundary does the clipboard
//! write or the file write and raises the toast (`tui::export`).

use super::*;

/// The transient toast shown when `/export` finds no conversation to export
/// — the `/compact` `Nothing to compact` rule: a soft rejection the user
/// needn't keep, never an empty file.
pub const EXPORT_EMPTY_NOTICE: &str = "Nothing to export";

/// Where an export goes — the page's two rows, in the order they list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportTarget {
    /// The system clipboard, through `/copy`'s own path.
    Clipboard,
    /// A `conversation-YYYY-MM-DD-HHMMSS.txt` file in the working directory
    /// ([`export_file_name`]).
    File,
}

impl ExportTarget {
    /// The page's rows, in order: the clipboard first — the choice that
    /// leaves nothing behind — then the file.
    pub const ALL: [Self; 2] = [Self::Clipboard, Self::File];

    /// The row's label, the answer to "where?".
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Clipboard => "Copy to clipboard",
            Self::File => "Save to file",
        }
    }
}

/// The open `/export` page's state (`None` on [`App`] when closed): which
/// target the `❯` sits on. Nothing else — the rows are a const, and the page
/// takes no text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportPicker {
    /// Index of the highlighted row within [`ExportTarget::ALL`].
    pub selected: usize,
}

/// The name an export file takes: `conversation-YYYY-MM-DD-HHMMSS.txt` —
/// the date as ISO, the time as one run of digits, every field zero-padded
/// so the names sort as they were written. `date`/`time` come from the
/// boundary's clock (local time, like the rollout's own name —
/// `session::rollout_rel_path`). `dup` is how many names the boundary found
/// already taken: `0` is the plain name, `1` the same name closed by `-2`,
/// and so on, so two exports inside one second never overwrite each other
/// and the second file says it is the second.
#[must_use]
pub fn export_file_name(date: (i32, u32, u32), time: (u32, u32, u32), dup: u32) -> String {
    let (year, month, day) = date;
    let (hour, minute, second) = time;
    let suffix = if dup == 0 {
        String::new()
    } else {
        format!("-{}", dup + 1)
    };
    format!(
        "conversation-{year:04}-{month:02}-{day:02}-{hour:02}{minute:02}{second:02}{suffix}.txt"
    )
}

impl App {
    /// Is there a conversation to export? The transcript **on screen** —
    /// the viewed subagent's own inside its session view, else the main
    /// history (the `/copy` rule) — has at least one item. The live tail
    /// alone never counts: a turn records its user message before anything
    /// streams, so an empty history is an empty conversation.
    #[must_use]
    pub fn export_available(&self) -> bool {
        match self.viewed_agent() {
            Some(run) => !run.history.is_empty(),
            None => !self.history.is_empty(),
        }
    }

    /// Open the `/export` page, the highlight on the clipboard row. Abandons
    /// any `?` band / palette / file picker (they share the composer the page
    /// takes over) — every picker's open rule.
    pub fn open_export_picker(&mut self) {
        self.shortcuts_open = false;
        self.command_menu = None;
        self.file_search = None;
        self.skill_picker = None;
        self.backtrack = Backtrack::default();
        self.export_picker = Some(ExportPicker::default());
    }

    /// Dismiss the page (Esc, Ctrl+C, or a pick): the composer returns. No
    /// view change — it was never an overlay.
    pub fn close_export_picker(&mut self) {
        self.export_picker = None;
    }

    /// The highlighted target, if the page is open.
    #[must_use]
    pub fn highlighted_export_target(&self) -> Option<ExportTarget> {
        let picker = self.export_picker.as_ref()?;
        ExportTarget::ALL.get(picker.selected).copied()
    }

    /// Keys while the `/export` page is open — the `/donate` grammar. Owns
    /// **every** key (routed at the top of [`on_key`]): ↑/↓ move wrapping at
    /// the ends, Home/End jump, a digit jumps to its row **and picks it**,
    /// Enter picks the highlighted row, Esc and Ctrl+C close. A pick
    /// **closes the page**: the choice is the whole point of it, and unlike
    /// `/donate` there is no second thing to take from the same page.
    ///
    /// [`on_key`]: App::on_key
    pub(super) fn on_key_export_picker(&mut self, key: KeyEvent) -> Action {
        // Ctrl+C closes the page (never quits — the picker family's rule).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.close_export_picker();
            return Action::CloseExportPicker;
        }
        let len = ExportTarget::ALL.len();
        let Some(picker) = self.export_picker.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => picker.selected = wrap_step(picker.selected, len, -1),
            KeyCode::Down => picker.selected = wrap_step(picker.selected, len, 1),
            KeyCode::Home => picker.selected = 0,
            KeyCode::End => picker.selected = len.saturating_sub(1),
            KeyCode::Enter => {
                if let Some(target) = ExportTarget::ALL.get(picker.selected).copied() {
                    self.close_export_picker();
                    return Action::Export(target);
                }
            }
            // Digits jump to their absolute row and pick it (the `/donate`
            // page's jump-activate rule). A digit past the rows names
            // nothing and is ignored.
            KeyCode::Char(c @ '1'..='9')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let index = (c as usize) - ('1' as usize);
                if let Some(target) = ExportTarget::ALL.get(index).copied() {
                    self.close_export_picker();
                    return Action::Export(target);
                }
            }
            KeyCode::Esc => {
                self.close_export_picker();
                return Action::CloseExportPicker;
            }
            _ => {}
        }
        Action::None
    }
}
