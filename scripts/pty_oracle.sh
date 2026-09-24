#!/usr/bin/env bash
# Differential check of the interactive-shell screen against tmux
# (docs/interactive-shell.md): run a program under the session's emulator
# with `examples/pty_oracle.rs`, recording every byte it wrote, replay those
# bytes into a 120x40 tmux pane, and diff the two screens and cursors. A
# difference is an escape sequence one emulator reads and the other doesn't.
#
#   cargo build --example pty_oracle
#   scripts/pty_oracle.sh 'btop' [steps.jsonl|-] [outdir]
#
# Exit status 0 when the screens agree, 1 when they differ.
set -euo pipefail

cmd=${1:?usage: pty_oracle.sh <command> [steps.jsonl|-] [outdir]}
steps=${2:--}
out=${3:-$(mktemp -d)}
root=$(cd "$(dirname "$0")/.." && pwd)
bin=$root/target/debug/examples/pty_oracle
mkdir -p "$out"

"$bin" "$cmd" "$steps" "$out/raw.bin" >"$out/ours.txt" 2>"$out/ours.err"

sock=pty-oracle-$$
conf=$out/tmux.conf
printf 'set -g status off\nset -g default-size 120x40\n' >"$conf"
# `raw -echo`: the recorded bytes reach tmux as written — no `\n` → `\r\n`,
# and the answers tmux sends to the queries in them are not echoed back.
tmux -L "$sock" -f "$conf" new-session -d -x 120 -y 40 \
    "stty raw -echo; cat '$out/raw.bin'; touch '$out/replayed'; sleep 60"
for _ in $(seq 100); do
    [ -e "$out/replayed" ] && break
    sleep 0.05
done
sleep 0.2
# `-e` marks line-drawing cells with SO/SI, which a plain capture
# would show as the letters that name them.
tmux -L "$sock" capture-pane -e -p -t 0 >"$out/tmux.txt"
tmux -L "$sock" display -p -t 0 '#{cursor_y} #{cursor_x} #{alternate_on}' >"$out/tmux.cursor"
tmux -L "$sock" kill-server 2>/dev/null || true

python3 - "$out" <<'EOF'
import sys, pathlib
out = pathlib.Path(sys.argv[1])
GRAPHICS = dict(zip("_`abcdefghijklmnopqrstuvwxyz{|}~",
                    " ◆▒␉␌␍␊°±␤␋┘┐┌└┼⎺⎻─⎼⎽├┤┴┬│≤≥π≠£·"))
def plain(line, shifted):
    """tmux's `-e` capture as text: escapes dropped, SO..SI cells drawn.
    The shift carries from one row into the next, so it is returned too."""
    text, i = [], 0
    while i < len(line):
        c = line[i]
        if c == "\x1b":
            i += 1
            if i < len(line) and line[i] == "[":
                i += 1
                while i < len(line) and not ("@" <= line[i] <= "~"):
                    i += 1
            elif i < len(line) and line[i] == "]":
                while i < len(line) and line[i] not in "\x07\x1b":
                    i += 1
                if i < len(line) and line[i] == "\x1b":
                    i += 1
            i += 1
            continue
        if c == "\x0e":
            shifted = True
        elif c == "\x0f":
            shifted = False
        else:
            text.append(GRAPHICS.get(c, c) if shifted else c)
        i += 1
    return "".join(text), shifted
def rows(name):
    lines, shifted = [], False
    for raw in (out / name).read_text(errors="replace").split("\n"):
        line, shifted = plain(raw, shifted)
        lines.append(line.rstrip())
    while lines and not lines[-1]:
        lines.pop()
    return lines
theirs, ours = rows("tmux.txt"), rows("ours.txt")
y, x, alt = (out / "tmux.cursor").read_text().split()
err = (out / "ours.err").read_text()
print(f"tmux cursor ({int(y) + 1}, {int(x) + 1}) alternate {alt == '1'}; ours: {err.strip()}")
differ = 0
for n in range(max(len(theirs), len(ours))):
    a = theirs[n] if n < len(theirs) else ""
    b = ours[n] if n < len(ours) else ""
    if a != b:
        differ += 1
        print(f"line {n + 1}:\n  tmux: {a!r}\n  ours: {b!r}")
print(f"{differ} line(s) differ ({out})")
sys.exit(1 if differ else 0)
EOF
