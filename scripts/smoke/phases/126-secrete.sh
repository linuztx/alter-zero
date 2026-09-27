#!/usr/bin/env bash
# Phase 126 — the `/secrete` page and placeholder secrets

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# the `/secrete` page (docs/secrets.md): the user stores a credential — a
# name, a value, a line of context — and the agent uses it by placeholder,
# `<secrete:NAME>`, without the value ever reaching the screen or the model.
# Driven end to end: the palette entry; the empty page; the form (a typed
# name normalized to ROOT-style and previewing its placeholder, the value
# PASTED and shown one dot per character, the context); the save — a toast,
# the list row with its fixed mask, `secrets.json` owner-only; a relaunch
# that lists it again; a `!` command whose placeholder EXPANDS for the shell
# (proved by the value's hash, which only the real value produces) and whose
# echoed value comes back REDACTED; the placeholder in the Ctrl+D reminder;
# the edit form keeping the value unloaded; and a two-step delete. Through
# all of it the value itself must never appear on screen, scrollback
# included.
S126="${S}_secrete"
VALUE="s3cr3t-demo-value-42"
VALUE_SHA="$(printf '%s' "$VALUE" | sha256sum | cut -c1-16)"
FILE="$SMOKE_CFG/secrets.json"
never_shows_value() { # $1 label
	local screen
	screen="$(pane "$S126" -S -400)"
	if printf '%s\n' "$screen" | grep -qF "$VALUE"; then
		dump "the value leaked ($1)" "$screen"
		fail "the secret's value is on screen ($1)"
	fi
}

launch "$S126" 90 40
type_text "$S126" "/secrete"
palette="$(wait_pane 5 "$S126" -F "Store credentials the agent uses but never sees")" ||
	fail "/secrete is missing from the slash-command palette"
dump "the palette filtered to /secrete" "$palette"
keys "$S126" Enter
empty="$(wait_pane 5 "$S126" -F "+ Add a secret")"
dump "the page with no secrets" "$empty"
for expect in "Secrets" "Credentials the agent uses by placeholder" "No secrets yet" \
	"❯ + Add a secret" "enter add a secret"; do
	expect_has "$empty" -F "$expect" "the empty page is missing '$expect'"
done

# The form: a name typed the way a person would say it.
keys "$S126" Enter
wait_for 5 "$S126" -F "New secret" || fail "Enter on the add row did not open the form"
type_text "$S126" "demo token"
form="$(wait_pane 5 "$S126" -F "Use it as <secrete:DEMO_TOKEN>")" ||
	fail "the typed name did not normalize and preview its placeholder"
dump "the form with a name" "$form"
expect_has "$form" -F "DEMO_TOKEN" "the name did not normalize to DEMO_TOKEN"
keys "$S126" Enter
# The value, PASTED (bracketed): into the field, one dot per character.
tmux set-buffer -b secret "$VALUE"
tmux paste-buffer -p -b secret -t "$S126"
dots="$(printf '%*s' "${#VALUE}" '' | sed 's/ /•/g')"
masked="$(wait_pane 5 "$S126" -F "$dots")" || fail "the pasted value is not masked one dot per character"
dump "the form with the value pasted" "$masked"
never_shows_value "the value field"
keys "$S126" Enter
type_text "$S126" "Demo token for the smoke test"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S126" Enter
saved="$(wait_pane 5 "$S126" -F "<secrete:DEMO_TOKEN>  ••••••••  Demo token for the smoke test")" ||
	fail "the saved secret is not listed with its fixed mask and context"
dump "the list after the save" "$saved"
expect_has "$saved" -F "Saved <secrete:DEMO_TOKEN>" "no confirming toast"
never_shows_value "the list"

# On disk: owner-only, the value stored (it is the one place it lives).
if [ ! -f "$FILE" ]; then
	fail "secrets.json was not written"
else
	expect_eq "$(stat -c %a "$FILE")" "600" "secrets.json is not owner-only"
	expect_file_has "$FILE" -F "\"name\": \"DEMO_TOKEN\"" "secrets.json lacks the name"
	expect_file_has "$FILE" -F "$VALUE" "secrets.json lacks the value"
fi

# A relaunch lists it again.
keys "$S126" Escape
sleep 0.3
keys "$S126" C-c
wait_gone 5 "$S126" || fail "the app did not quit"
launch "$S126" 90 40
type_text "$S126" "/secrete"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S126" Enter
relisted="$(wait_pane 5 "$S126" -F "<secrete:DEMO_TOKEN>")" || fail "the secret did not survive a relaunch"
dump "the list after a relaunch" "$relisted"
keys "$S126" Escape
wait_for 5 "$S126" -E '^❯' || fail "Esc did not bring the composer back"

# A `!` command: the placeholder expands for the shell — only the real value
# hashes to VALUE_SHA — and the echoed value comes back as the placeholder.
submit "$S126" "!printf '%s' '<secrete:DEMO_TOKEN>' | sha256sum | cut -c1-16"
hashed="$(wait_pane 10 "$S126" -F "$VALUE_SHA")" || fail "the placeholder did not expand for the ! command"
dump "the ! command hashing the expanded value" "$hashed"
submit "$S126" "!printf 'token=%s\\n' '<secrete:DEMO_TOKEN>'"
echoed="$(wait_pane 10 "$S126" -F "token=<secrete:DEMO_TOKEN>")" || fail "the echoed value was not redacted"
dump "the ! command echoing the value" "$echoed"
never_shows_value "a ! command's output"

# The reminder names the placeholder and its context — never the value.
keys "$S126" C-d
sleep 0.8
keys "$S126" Home
sleep 0.5
context="$(pane "$S126")"
dump "the derived context" "$context"
expect_has "$context" -F "The user's secrets, as placeholders" "the reminder has no secrets section"
expect_has "$context" -F "<secrete:DEMO_TOKEN>: Demo token for the smoke test" "the reminder does not list the secret"
if printf '%s\n' "$context" | grep -qF "$VALUE"; then
	fail "the value is in the derived context"
fi
keys "$S126" q
wait_for 5 "$S126" -E '^❯' || fail "the context view did not close"

# Editing never loads the value back.
type_text "$S126" "/secrete"
sleep "$SMOKE_TYPE_SETTLE"
keys "$S126" Enter
wait_for 5 "$S126" -F "<secrete:DEMO_TOKEN>" || fail "the page did not reopen"
keys "$S126" Enter
edit="$(wait_pane 5 "$S126" -F "Edit <secrete:DEMO_TOKEN>")" || fail "Enter on the row did not open the edit form"
dump "the edit form" "$edit"
expect_has "$edit" -F "leave empty to keep the current value" "the edit form loaded the value"
keys "$S126" Escape
sleep 0.3

# A delete asks first.
keys "$S126" d
asked="$(wait_pane 5 "$S126" -F "Delete <secrete:DEMO_TOKEN>?")" || fail "d did not ask before deleting"
dump "the delete question" "$asked"
keys "$S126" d
gone="$(wait_pane 5 "$S126" -F "No secrets yet")" || fail "the second d did not delete"
dump "the list after the delete" "$gone"
expect_has "$gone" -F "Deleted <secrete:DEMO_TOKEN>" "no confirming toast for the delete"
if grep -qF "DEMO_TOKEN" "$FILE" 2>/dev/null; then
	fail "the deleted secret is still in secrets.json"
fi
never_shows_value "the whole session"
keys "$S126" Escape
tmux kill-session -t "$S126" 2>/dev/null
