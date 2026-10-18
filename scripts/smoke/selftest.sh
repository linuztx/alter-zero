#!/usr/bin/env bash
# scripts/smoke/selftest.sh — the smoke suite's own tests over its pure readers.
#
#   bash scripts/smoke/selftest.sh
#
# The phase readers in `lib.sh` — `smoke_phase_id`, `smoke_phase_slug`,
# `smoke_phase_title`, `smoke_phase_tags` — and the `smoke_matches` /
# `smoke_selected` selector built on them are pure text: a file's name and the
# first lines of its header. They decide WHICH phases a query runs, so a reader
# that quietly returns nothing does not fail loudly, it runs the wrong suite.
#
# That is exactly what BSD `sed` did. It rejects the `{…;p}` form the readers
# used ("bad flag in substitute command"), so `smoke_phase_title` printed a sed
# error and returned nothing. Every query then matched on the file-name slug
# alone, which cost two things: 141 of the 144 phases became unreachable by any
# word that lives only in their prose title, and `121-slowstream` lost the
# `serial` tag that keeps a timing-sensitive phase out of the parallel pool.
#
# So these cases are fixtures, not the checkout's phases: a known file name and
# a known header block, asserted reader by reader, with the em dash and the
# tags line both present — the two shapes BSD `sed` used to swallow.
# Needs only bash, sed and grep, which the suite already requires.
set -uo pipefail
# shellcheck source-path=SCRIPTDIR
# shellcheck disable=SC1091
. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
set +e

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

_c_red='' _c_green='' _c_dim='' _c_off=''
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
	_c_red=$'\033[31m' _c_green=$'\033[32m' _c_dim=$'\033[2m' _c_off=$'\033[0m'
fi
PASS=0
FAILED=0
pass() { PASS=$((PASS + 1)); printf '  %sok%s  %s\n' "$_c_green" "$_c_off" "$*"; }
flunk() { FAILED=$((FAILED + 1)); printf '  %sFAIL%s %s\n' "$_c_red" "$_c_off" "$*"; }
section() { printf '%s%s%s\n' "$_c_dim" "$*" "$_c_off"; }
# expect_eq DESCRIPTION WANT GOT
expect_eq() {
	if [ "$2" = "$3" ]; then pass "$1"; else flunk "$1 — want '$2', got '$3'"; fi
}

# ---------------------------------------------------------------------------
# A fixture phase, written the way a real one is: the id in the file name, the
# title and the tags in the first lines. The em dash is the shape the reader's
# own comment calls out — under a byte-wise C locale a bracket matches one BYTE
# of it, which is why the pattern is an alternation.
# ---------------------------------------------------------------------------
mkdir -p "$T/phases"
PHASE="$T/phases/055-permission.sh"
{
	printf '#!/usr/bin/env bash\n'
	printf '# Phase 55 — the permission prompt, and what it says\n'
	printf '# smoke: tags=serial,slow\n'
	printf '\n. lib.sh\n'
} >"$PHASE"
# A title-less phase: the reader must say nothing, and the slug must stand in
# for the title at the call site (`smoke_begin`), not inside the reader.
BARE="$T/phases/107b-imagetransmit.sh"
{
	printf '#!/usr/bin/env bash\n'
	printf '# Phase 107b — image transmit\n'
} >"$BARE"

section "phase id, slug and title come off the file"
expect_eq "id strips the zero padding" "55" "$(smoke_phase_id "$PHASE")"
expect_eq "id keeps a letter suffix" "107b" "$(smoke_phase_id "$BARE")"
expect_eq "slug is the name after the id" "permission" "$(smoke_phase_slug "$PHASE")"
expect_eq "title keeps the text after the em dash" \
	"the permission prompt, and what it says" "$(smoke_phase_title "$PHASE")"
expect_eq "a title with a hyphen reads as the title" \
	"image transmit" "$(smoke_phase_title "$BARE")"

section "tags survive, and are comma-split"
# The shape `serial` needs: a timing-sensitive phase must not be scheduled into
# the parallel pool. A reader that returns nothing puts it there.
expect_eq "tags are space-separated" "serial slow" "$(smoke_phase_tags "$PHASE")"
expect_eq "a phase with no tags line has none" "" "$(smoke_phase_tags "$BARE")"

# A title the reader must not mistake for a phase header (no `# Phase N`).
OTHER="$T/phases/009-quit.sh"
{
	printf '#!/usr/bin/env bash\n'
	printf '# quit the app\n'
	printf '# Phase 9 — quit\n'
} >"$OTHER"
section "the header is found wherever it is, and only when it matches"
expect_eq "a later header line is still the title" "quit" "$(smoke_phase_title "$OTHER")"

# ---------------------------------------------------------------------------
# The selector built on those readers: this is where an empty title does real
# damage. A multi-word query is split on whitespace and ANY word may match, so
# with the title empty the only text left is the slug — and "the" is a
# substring of "theme".
# ---------------------------------------------------------------------------
THEME="$T/phases/111b-theme.sh"
{
	printf '#!/usr/bin/env bash\n'
	printf '# Phase 111b — the /theme picker\n'
} >"$THEME"

section "a multi-word query ORs its words, and the title is what makes them findable"
# Through the runner's own implementation, so a regression in either half fails
# here. `smoke_matches` reads slug AND title, so a word that lives only in the
# prose is answerable — and that is what an unreadable title takes away: with
# it empty, 141 of the 144 phases become unreachable by any word that is not
# already in the file name.
eval "$(sed -n '/^smoke_matches()/,/^}/p' "$SMOKE_LIB_DIR/../smoke.sh")" 2>/dev/null
eval "$(sed -n '/^smoke_selected()/,/^}/p' "$SMOKE_LIB_DIR/../smoke.sh")" 2>/dev/null
if ! declare -F smoke_matches >/dev/null || ! declare -F smoke_selected >/dev/null; then
	flunk "smoke_matches/smoke_selected could not be loaded from scripts/smoke.sh"
else
	expect_eq "a title word selects its phase" "0" \
		"$(smoke_matches "$PHASE" permission; echo $?)"
	expect_eq "an id selects its phase" "0" "$(smoke_matches "$PHASE" 55; echo $?)"
	expect_eq "a slug word selects its phase" "0" "$(smoke_matches "$THEME" theme; echo $?)"
	# A word only the title carries: `picker` is in neither slug.
	expect_eq "a word only the title carries selects its phase" "0" \
		"$(smoke_matches "$THEME" picker; echo $?)"
	# The selector splits a query into words and keeps a phase any word
	# answers — an OR, not a phrase match, and `the` is a substring of the slug
	# `theme`. Pinned so nobody turns it into an AND and silently narrows every
	# developer's selection. `smoke_selected` reads these two globals; the
	# directive keeps shellcheck from calling them dead.
	# shellcheck disable=SC2034
	ONLY=("the first turn") SKIP=()
	expect_eq "a multi-word query is an OR over its words" "0" \
		"$(smoke_selected "$THEME"; echo $?)"
fi

printf '\n%d passed, %d failed\n' "$PASS" "$FAILED"
[ "$FAILED" -eq 0 ]
