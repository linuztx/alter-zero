#!/usr/bin/env bash
# Phase 165 — the .agents/ directory: skills from every project directory and ~/.agents, a marketplace found in the project

. "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
smoke_begin

# THE .agents/ DIRECTORY (docs/agents-dir.md), read where Codex reads it, on
# the real binary in a project this phase plants and launched two levels
# below its root:
#
# - skills: the walk reads every directory from the cwd up to the project
#   root, where it read only the cwd and the root, and `~/.agents/skills`.
#   Skills sit at every level, each named `zz-…` so one search lists them:
#
#     outer/.agents/skills/zz-outer            above the repository: never read
#     outer/proj/.agents/skills/zz-repo        the project root (it holds `.git`)
#     outer/proj/pkg/.agents/skills/zz-lint    between the root and the cwd
#     ~/.agents/skills/zz-notes                personal
#
#   and `zz-shared` both in the package and at home, where the nearer wins.
#   `/skills` must hold exactly the built-ins and those four, show the
#   package's `zz-shared`, and the `$` picker offers the personal one;
# - plugins: `proj/.agents/plugins/marketplace.json` is found without being
#   added. `/plugins` lists it on a tab of its own whose header says where
#   it was found and whose hint offers no Ctrl+R, and installing its plugin
#   adds it: the header then reads as any added directory's, the plugin
#   loads in place, and plugins.json records the marketplace.
S165="${S}_agentsdir"
AD_OUTER="$SMOKE_TMP/outer"
AD_ROOT="$AD_OUTER/proj"
AD_CWD="$AD_ROOT/pkg/src"
AD_HOME="$SMOKE_TMP/adhome"
mkdir -p "$AD_CWD" "$AD_HOME" "$AD_ROOT/.agents/plugins" "$AD_ROOT/plugins/tool/.claude-plugin"
git init -q "$AD_ROOT"
plant_skill() { # ROOT NAME DESCRIPTION
	mkdir -p "$1/$2"
	printf -- '---\nname: %s\ndescription: %s\n---\nBody of %s.\n' "$2" "$3" "$2" >"$1/$2/SKILL.md"
}
plant_skill "$AD_OUTER/.agents/skills" zz-outer "Above the repository, never read"
plant_skill "$AD_ROOT/.agents/skills" zz-repo "Kept at the project root"
plant_skill "$AD_ROOT/pkg/.agents/skills" zz-lint "Lint the package the way its Codex setup does"
plant_skill "$AD_HOME/.agents/skills" zz-notes "Keep notes, from the personal agents folder"
plant_skill "$AD_ROOT/pkg/.agents/skills" zz-shared "The package's own copy"
plant_skill "$AD_HOME/.agents/skills" zz-shared "The personal copy"
# The built-ins are seeded into the config home on the first launch, so the
# header counts them beside the four planted skills the walk must find.
AD_BUILTINS="$(find "$SMOKE_ROOT/prompts/skills" -mindepth 2 -maxdepth 2 -name SKILL.md | wc -l | tr -d ' ')"
AD_TOTAL=$((AD_BUILTINS + 4))
cat >"$AD_ROOT/.agents/plugins/marketplace.json" <<'JSON'
{ "name": "projkit", "plugins": [
  { "name": "tool", "source": "./plugins/tool", "description": "The project's own tool" } ] }
JSON
cat >"$AD_ROOT/plugins/tool/.claude-plugin/plugin.json" <<'JSON'
{ "name": "tool", "version": "0.3.0" }
JSON
# The suite's base environment minus ALTER_ZERO_SKILLS_DIR, which REPLACES
# the roots and would hide every row this phase is about; HOME is this
# phase's own, holding `~/.agents/skills`.
AD_ENV="$(printf '%s' "$CFG_ENV_NOHIST" | sed 's#ALTER_ZERO_SKILLS_DIR=[^ ]*##')"
APP_AD="env $AD_ENV HOME=$AD_HOME ALTER_ZERO_STARTUP_DELAY_MS=$SMOKE_STARTUP_MS $BIN_ABS"
launch -c "$AD_CWD" "$S165" 100 40 "$APP_AD"

# Exactly the built-ins and the four the walk should find.
type_text "$S165" "/skills"
sleep 0.3
keys "$S165" Enter
ad_menu="$(wait_pane 5 "$S165" -F "skills · ")"
dump "/skills launched in outer/proj/pkg/src" "$ad_menu"
expect_has "$ad_menu" -F "$AD_TOTAL skills · $AD_TOTAL enabled" \
	"the walk did not find exactly the $AD_BUILTINS built-ins and the four .agents skills"
# Searching narrows the list to the planted skills: all of them fit the
# window, so a wrongly read `zz-outer` would show among them.
type_text "$S165" "zz-"
ad_found="$(wait_pane 5 "$S165" -F "zz-shared")"
dump "the planted skills, searched" "$ad_found"
for name in zz-repo zz-lint zz-notes zz-shared; do
	expect_has "$ad_found" -F "$name" "/skills does not list $name"
done
expect_lacks "$ad_found" -F "zz-outer" "a skill above the project root was read"
# The one match is highlighted, so its description is the line under the list.
type_text "$S165" "shared"
ad_shared="$(wait_pane 5 "$S165" -F "copy")"
dump "the name the package and the home folder share" "$ad_shared"
expect_has "$ad_shared" -F "The package's own copy" "the nearer root did not win zz-shared"
expect_lacks "$ad_shared" -F "The personal copy" "the personal zz-shared was not shadowed"
# The first Esc clears the search, the second closes the page.
keys "$S165" Escape
sleep 0.3
keys "$S165" Escape
wait_for 3 "$S165" -E "^❯ *$" || fail "/skills did not close"

# The personal one, from ~/.agents/skills, in the `$` picker too.
type_text "$S165" '$zz-no'
ad_mention="$(wait_pane 5 "$S165" -E "→ zz-notes +Keep notes")"
dump "the \$ picker over ~/.agents/skills" "$ad_mention"
expect_has "$ad_mention" -E "→ zz-notes +Keep notes, from the personal agents folder" "~/.agents/skills was not read"
keys "$S165" C-u
sleep 0.3

# The project's marketplace, found and not added.
type_text "$S165" "/plugins"
sleep 0.3
keys "$S165" Enter
ad_page="$(wait_pane 5 "$S165" -F " projkit (1) ")"
dump "/plugins with a marketplace found in the project" "$ad_page"
expect_has "$ad_page" -F " All (1)    Installed (0)    projkit (1)    + Add" "the found marketplace has no tab"
keys "$S165" Right
sleep 0.2
keys "$S165" Right
ad_tab="$(wait_pane 5 "$S165" -F "found in this project, not added")"
dump "the found marketplace's tab" "$ad_tab"
expect_has "$ad_tab" -F "found in this project, not added · $AD_ROOT" "the header does not say where the marketplace was found"
expect_has "$ad_tab" -E "^  → tool +\+ available +The project's own tool" "the found marketplace's plugin is not offered"
expect_lacks "$ad_tab" -F "Ctrl+R remove" "a marketplace never added offers to be removed"

# Installing its plugin adds it.
keys "$S165" Enter
wait_for 3 "$S165" -F "❯ 1. Install" || fail "tool's page did not open"
keys "$S165" Enter
ad_installed="$(wait_pane 10 "$S165" -E "^  ✔ enabled in this directory · tool@projkit")"
dump "tool installed" "$ad_installed"
expect_has "$ad_installed" -E "Loads from: " "the found marketplace's plugin does not load in place"
keys "$S165" Escape
ad_added="$(wait_pane 5 "$S165" -F "Ctrl+R remove")"
dump "the marketplace, added by the install" "$ad_added"
expect_lacks "$ad_added" -F "not added" "the marketplace still reads as found after its plugin was installed"
expect_has "$ad_added" -F "$AD_ROOT" "the added marketplace does not name its directory"
tmux kill-session -t "$S165" 2>/dev/null

note "plugins.json on disk"
cat "$SMOKE_CFG/plugins.json" 2>/dev/null || echo "(no file)"
grep -qF '"projkit"' "$SMOKE_CFG/plugins.json" 2>/dev/null || fail "plugins.json does not record the marketplace the install added"
grep -qF "$AD_ROOT" "$SMOKE_CFG/plugins.json" 2>/dev/null || fail "plugins.json does not record the project as the marketplace's directory"
[ ! -e "$SMOKE_CFG/plugins/installed/projkit/tool" ] || fail "the found marketplace's plugin was copied, not loaded in place"
