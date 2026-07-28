//! The composer's editing seams around the [`TextArea`](crate::textarea::TextArea):
//! bracketed-paste placeholders, Ctrl+V image attachments, and the `!` shell
//! mode. See `docs/paste.md`, `docs/image-paste.md`, `docs/shell-command.md`.

use super::*;

/// The notice shown when Enter is pressed on a bare `!` (no command) — codex's
/// `Prefix a command with ! to run it locally`.
pub const SHELL_EMPTY_NOTICE: &str = "Type a command after ! to run it locally (e.g. !ls)";

/// The shell command in `input`, if it is a **`!`-prefixed** line: a leading
/// `!` followed by the rest of the line (spaces and all — unlike
/// [`command_query`], a shell command obviously contains whitespace). `Some("")`
/// for a lone `!`. This is what flips the composer into "shell mode" (the red
/// footer hint) and, on Enter from an idle composer, runs the rest locally. A
/// port of codex's `is_bash_shell_command`; see `docs/shell-command.md`.
#[must_use]
pub fn shell_query(input: &str) -> Option<&str> {
    input.strip_prefix('!')
}

/// How many earlier occurrences of the placeholder at `span` precede it in
/// `text` — the ordinal pairing a placeholder occurrence to its list entry
/// (occurrences in text order correspond to `(placeholder, value)` pairs in
/// list order; see `paste::distribute_images`).
fn occurrence_ordinal(text: &str, span: &Range<usize>) -> usize {
    let placeholder = &text[span.clone()];
    text[..span.start].matches(placeholder).count()
}

/// Remove and return the value of the pair backing the `ordinal`-th occurrence
/// of `placeholder`; `None` when no pair sits at that ordinal (the occurrence
/// was an unbacked marker recalled as plain text).
fn remove_nth_pair<V>(
    pairs: &mut Vec<(String, V)>,
    placeholder: &str,
    ordinal: usize,
) -> Option<V> {
    let pos = pairs
        .iter()
        .enumerate()
        .filter(|(_, (ph, _))| ph == placeholder)
        .map(|(i, _)| i)
        .nth(ordinal)?;
    Some(pairs.remove(pos).1)
}

impl App {
    /// Handle one key press and report what the event loop should do.
    ///
    /// Sending is disabled while a reply streams; quitting (Ctrl+C) and the
    /// tool-view toggle (Ctrl+O) always work, from either screen. Other keys are
    /// dispatched to the active [`View`].
    /// Handle a bracketed-paste event. A paste over
    /// [`crate::paste::LARGE_PASTE_CHAR_THRESHOLD`] characters is shown in the
    /// composer as a compact `[Pasted Content N chars]` placeholder, with the
    /// real text remembered in [`pasted`] for `take_input` to splice back in on
    /// send; a smaller paste is inserted verbatim, indistinguishable from typing
    /// it. The loop only calls this in the conversation view (the Ctrl+O overlay
    /// has no composer, like typing there). See `docs/paste.md`.
    ///
    /// [`pasted`]: App::pasted
    pub fn on_paste(&mut self, pasted: &str) {
        // An open Ctrl+R search owns *every* key ([`on_key`] routes them all to
        // on_key_search) — a bracketed paste is input too, so it extends the
        // query (readline's paste-into-isearch) instead of editing the doomed
        // preview underneath (the next rerun/cancel set_texts over the
        // composer). Control characters would corrupt the single-row search
        // line, so they flatten to spaces. See `docs/history-search.md`.
        //
        // [`on_key`]: App::on_key
        if self.history_search.is_some() {
            let sanitised = pasted.replace(|c: char| c.is_control(), " ");
            self.edit_search_query(|query| query.push_str(&sanitised));
            return;
        }
        // Terminals such as iTerm2 send CR (or CRLF) for newlines in a paste;
        // normalise to LF so the char count and stored text match the display.
        let pasted = pasted.replace("\r\n", "\n").replace('\r', "\n");
        // An edit may change the active /command or @token, like the Char arm.
        let had_query = command_query(self.input.text()).is_some();
        let had_token = self.in_at_token();
        let char_count = pasted.chars().count();
        if char_count > crate::paste::LARGE_PASTE_CHAR_THRESHOLD {
            let placeholder = crate::paste::next_paste_placeholder(char_count, &self.pasted);
            self.input.insert_str(&placeholder);
            self.pasted.push((placeholder, pasted));
        } else {
            // Control characters other than '\n' (tabs above all) reach us only
            // via a paste, and they break the cursor math: unicode-width counts
            // '\t' as one column while ratatui renders zero cells, drifting the
            // hardware cursor right of the text. Sanitise what the composer will
            // actually render; the large-paste branch above keeps its payload
            // verbatim since only the placeholder is displayed.
            self.input
                .insert_str(&pasted.replace(|c: char| c.is_control() && c != '\n', " "));
        }
        self.refresh_command_menu(had_query);
        self.sync_shell_mode();
        self.refresh_file_search(had_token);
    }

    /// Take the composer draft, splicing every large-paste placeholder back to
    /// its real text and clearing [`pasted`] (the composer is now empty). The one
    /// choke point every send/queue path uses in place of a bare
    /// `self.input.take()`, so the model — and the committed record of what was
    /// sent — receives the real content, never a `[Pasted Content N chars]`
    /// placeholder. See `docs/paste.md`.
    ///
    /// [`pasted`]: App::pasted
    pub(super) fn take_input(&mut self) -> String {
        let text = self.input.take();
        // Taking the draft consumes its attachments too (the idle submit and
        // queue paths stage image paths out first); whatever is still attached
        // here is a *drop* — record it so the boundary can remove the temp
        // file. Image placeholders are *not* expanded — they stay in `text`,
        // only the paths travel a separate channel. See `docs/image-paste.md`.
        self.discard_attachments();
        if self.pasted.is_empty() {
            return text;
        }
        let expanded = crate::paste::expand_pastes(&text, &self.pasted);
        self.pasted.clear();
        expanded
    }

    /// If the cursor is on an atomic placeholder — a large-paste
    /// `[Pasted Content N chars]` **or** a Ctrl+V `[Image #N]` — delete the
    /// **whole** placeholder atomically (one keystroke removes it, not one
    /// character) and drop its remembered text/path, returning `true`. Text
    /// pastes are tried first, then images; both are matched by string.
    /// `backward` is Backspace (vs Delete; see [`crate::paste::placeholder_to_delete`]
    /// for the cursor rules). Returns `false` when the cursor isn't on a
    /// placeholder, leaving the keypress to the normal per-grapheme edit. See
    /// `docs/paste.md` and `docs/image-paste.md`.
    pub(super) fn delete_placeholder(&mut self, backward: bool) -> bool {
        if let Some(span) = crate::paste::placeholder_to_delete(
            self.input.text(),
            self.input.cursor(),
            &self.pasted,
            backward,
        ) {
            let ordinal = occurrence_ordinal(self.input.text(), &span);
            let placeholder = self.delete_span(span);
            remove_nth_pair(&mut self.pasted, &placeholder, ordinal);
            return true;
        }
        if let Some(span) = crate::paste::placeholder_to_delete(
            self.input.text(),
            self.input.cursor(),
            &self.images,
            backward,
        ) {
            let ordinal = occurrence_ordinal(self.input.text(), &span);
            let placeholder = self.delete_span(span);
            // Only the deleted occurrence's own pair goes (occurrences in
            // text order pair with list entries in order — the string-keyed
            // scheme's convention): a merged batch's duplicate-named pairs
            // keep backing their other occurrences. The dropped attachment's
            // temp file is orphaned now — hand its path to the boundary for
            // removal (docs/image-paste.md).
            if let Some(path) = remove_nth_pair(&mut self.images, &placeholder, ordinal) {
                self.discarded_images.push(path);
            }
            return true;
        }
        false
    }

    /// Splice the byte `span` out of the composer, returning the text it held —
    /// the shared half of [`delete_placeholder`] across the text/image lists.
    fn delete_span(&mut self, span: Range<usize>) -> String {
        let removed = self.input.text()[span.clone()].to_string();
        self.input.replace_range(span, "");
        removed
    }

    /// Attach a Ctrl+V-pasted image: insert an `[Image #N]` placeholder at the
    /// cursor and remember its temp-PNG `path` in [`images`] (codex's
    /// `attach_image`). The placeholder stays in the message text on send; the
    /// path is delivered separately (see [`take_submission_images`]). The loop
    /// calls this at the I/O boundary after [`crate::clipboard`] reads the
    /// image. See `docs/image-paste.md`.
    ///
    /// [`images`]: App::images
    /// [`take_submission_images`]: App::take_submission_images
    pub fn attach_image(&mut self, path: PathBuf) {
        // Like `on_paste`, an insert next to a `/command` or `@token` re-derives
        // the same menu/shell/file-search state every composer edit runs.
        let had_query = command_query(self.input.text()).is_some();
        let had_token = self.in_at_token();
        let placeholder = crate::paste::next_image_placeholder(&self.images);
        self.input.insert_str(&placeholder);
        self.images.push((placeholder, path));
        self.refresh_command_menu(had_query);
        self.sync_shell_mode();
        self.refresh_file_search(had_token);
    }

    /// Take the image attachments staged by the last idle submit (codex's
    /// `take_recent_submission_images`), as `(placeholder, path)` pairs: the
    /// loop threads them into `start_turn`, which records each path onto the
    /// message carrying its placeholder and hands the bare paths to
    /// [`crate::stream::ReplySource::spawn`]. See `docs/image-paste.md`.
    pub fn take_submission_images(&mut self) -> Vec<(String, PathBuf)> {
        std::mem::take(&mut self.submission_images)
    }

    /// Move every still-attached image into the discarded list — the shared
    /// tail of the drop paths (a `take_input` with nothing staged out first).
    pub(super) fn discard_attachments(&mut self) {
        self.discarded_images
            .extend(self.images.drain(..).map(|(_, path)| path));
    }

    /// Drain the temp-PNG paths of attachments dropped since the last drain —
    /// the boundary deletes these files after handling each key event (the
    /// pure core records the drops, the file I/O stays in `main.rs`). See
    /// `docs/image-paste.md`.
    pub fn take_discarded_images(&mut self) -> Vec<PathBuf> {
        std::mem::take(&mut self.discarded_images)
    }

    /// Count `count` attached images into the live token tally as uploaded input
    /// (arrow ↑), like [`count_user_input`] does for the text, so the status
    /// reflects the images during the backend's pre-stream pause. No-op when no
    /// turn is in flight. See `docs/image-paste.md`.
    ///
    /// [`count_user_input`]: App::count_user_input
    pub fn count_input_images(&mut self, count: usize) {
        if let Some(status) = self.status.as_mut() {
            status.tokens += count * IMAGE_INPUT_TOKENS;
            status.arrow = TokenArrow::Up;
        }
    }

    /// Absorb a leading `!` out of the textarea into [`shell_mode`] — codex's
    /// `sync_bash_mode_from_text`, run after every edit that could produce one:
    /// the bang lives in the flag (rendered as the `! ` prompt), never in the
    /// text. Only ever *enters* the mode; leaving it is an explicit gesture
    /// (Backspace/Esc on empty, or submitting). No-op when already in the mode
    /// or when the text doesn't start with `!`.
    ///
    /// [`shell_mode`]: App::shell_mode
    pub(super) fn sync_shell_mode(&mut self) {
        if self.shell_mode {
            return;
        }
        // An agent session view has no `!` shell — the draft chats with the
        // agent, so a leading bang is literal text (docs/agent-tool.md).
        if self.agent_view.is_some() {
            return;
        }
        if let Some(rest) = shell_query(self.input.text()) {
            let rest = rest.to_string();
            self.shell_mode = true;
            // Only the 1-byte bang leaves the text — keep the cursor where the
            // user had it (shifted back past the removed prefix) instead of
            // letting a plain set_text teleport it to the end.
            let cursor = self.input.cursor().saturating_sub(1);
            self.input.set_text_with_cursor(&rest, cursor);
            self.command_menu = None; // the draft is a literal command now
        }
    }

    /// Drop image pairs no longer anchored to a placeholder occurrence in the
    /// composer (keeping at most one pair per occurrence, in order) — the
    /// reconcile a search-snapshot restore needs when an async Ctrl+V
    /// completion landed while the Ctrl+R search owned the composer: its
    /// placeholder went with the preview text, so the pair must not linger
    /// invisibly and ride the next submission. Orphaned temp files go to the
    /// boundary's discard list (docs/image-paste.md).
    pub(super) fn reconcile_images_with_input(&mut self) {
        let text = self.input.text().to_string();
        let mut kept: Vec<(String, PathBuf)> = Vec::new();
        for (placeholder, path) in std::mem::take(&mut self.images) {
            let occurrences = text.matches(placeholder.as_str()).count();
            let backed = kept.iter().filter(|(ph, _)| *ph == placeholder).count();
            if backed < occurrences {
                kept.push((placeholder, path));
            } else {
                self.discarded_images.push(path);
            }
        }
        self.images = kept;
    }
}
