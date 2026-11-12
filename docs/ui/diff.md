# `/diff` — review the project's Git changes

`/diff` opens a full-screen, read-only review of the current Git worktree.
It works from the repository root or any directory within it, including a
linked worktree. Outside a Git worktree it shows the neutral info toast **Not
a Git repository.** and keeps the conversation open without entering the
full-screen viewer. It never initializes a repository.

## What the review shows

The **Git changes** header identifies the repository and summarizes its changes.
A file list on the left provides the changed paths, change categories, and
addition/deletion counts. The patch on the right shows the selected file with
old/new line numbers, highlighted additions and removals, and distinct hunk
headers. Focused pane borders and the footer make it clear which pane will
respond to navigation. Colors follow the active theme.

The initial **All** filter includes staged, unstaged, and untracked changes.
Staged patches compare the index to `HEAD`; unstaged patches compare the
working tree to the index. A file changed in both places has a separate entry
for each, so staged work and subsequent edits can be reviewed independently.
Untracked text files appear as additions. Git-ignored files are excluded.
Binary changes have a visible notice instead of a text preview. Large patches
and repositories use bounded previews, with explicit notices wherever content
has been truncated.

The review covers the current local changes, rather than commit history or
the difference from another branch. A clean repository opens with **Working
tree clean**. A filter or filename search with no results displays **No
matching files**.

## Navigation

The file list has focus when the review opens. The footer keeps the main
controls visible.

| Key | Action |
| --- | --- |
| `1`, `2`, `3`, `4` | Show All, Unstaged, Staged, or Untracked changes |
| `/` | Search filenames; type to filter the list |
| `Enter` while searching | Keep the filename filter and return to navigation |
| `Esc` while searching | Clear the filename filter and return to navigation |
| `Tab` | Switch focus between Files and Patch |
| `Enter` in Files | Focus the selected file's patch |
| `↑` / `↓`, `k` / `j` | Move through the focused pane |
| `PgUp` / `PgDn` | Move a page in the focused pane |
| `Home` / `End` | Jump to the start or end of the focused pane |
| `[` / `]` | Select the previous or next file |
| `n` / `N` | Jump to the next or previous hunk |
| `←` / `→` | Scroll the patch horizontally |
| `r` | Reload the current Git changes |
| `q`, `Esc`, `Ctrl+C` | Close the review and return to the conversation |

While the filename search is active, letters such as `q` and `r` are search
text. Finish searching before using the review's letter shortcuts.

## Loading and returning to chat

Git runs in a background worker, so opening or refreshing the review does not
block keyboard handling or a running conversation. The command first validates
the current directory as a Git worktree while keeping the conversation visible.
Only a valid Git worktree opens the alternate-screen review, where its changes
load in the background. Outside Git, the command shows **Not a Git repository.**
without opening a loading screen. Refreshing keeps the review open. Press `r`
after files change to obtain a new snapshot.

The viewer uses the terminal's alternate screen, preserving the inline
conversation and terminal scrollback. Closing it restores the conversation;
`Ctrl+C` closes the review rather than quitting the application. Terminal
resizes recalculate the pane layout; narrow terminals show the focused pane
at full width, with Tab switching between files and patch.

No review control stages, unstages, edits, restores, commits, or deletes files.

## Verification

`scripts/smoke/phases/120-diff.sh` exercises the command in an isolated tmux
terminal with staged, unstaged, and untracked fixtures. It covers filtering,
filename search, pane and hunk navigation, refresh, resizing, closing, a clean
repository, rejection outside Git, and a turn finishing while the review is
open. A raw terminal recording verifies that a rejected non-Git command never
enters the alternate screen, and a styled capture checks its neutral info toast.
Repository snapshots before and after
review verify that navigation leaves the index and working files unchanged.
