# The headless Kali container

`docker/` packages the published Alter Zero release on
[Kali Rolling](https://www.kali.org/docs/containers/official-kalilinux-docker-images/),
for Docker and Podman alike. This is the design: what was decided, the
measurement behind each decision, and what was tried and refused. How to *use*
it is [`docker/README.md`](../docker/README.md) (and the plain-text
`docker/build.txt`).

```
docker/Dockerfile      the image
docker/build.sh        resolve the latest release, build          [WORKSPACE_DIR]
docker/run.sh          create the container: workspace, ports, clipboard
docker/lib.sh          the names and engine choice both scripts share
docker/compose.yml     the same container, for Compose
docker/tests/          the scripts under a stub engine; the image under a real one
/.dockerignore         the build context, as an allowlist
```

## The release is downloaded, never built

The image installs the binary with the repository's own `install.sh`: the
archive for the image's architecture, its SHA-256 checked against the published
`SHA256SUMS`, refused on a mismatch. No Rust toolchain and no source enter the
image, and a build is one `apt-get install` plus one 9 MB download — about a
minute, against the several a release compile takes.

`install.sh` runs in a **stage of its own** (`release`), and the final stage
`COPY --from`s the one binary out of it, so neither the script nor its
temporary files reach the image.

### "Latest" is resolved outside the build

This is the decision the rest of `build.sh` exists for. A Dockerfile that asks
for the latest release inside a `RUN` gets an answer **once**: the layer is
cached on the instruction's text, which never changes, so every later build
reinstalls whatever release was newest the first time — silently, and
`docker build` reports success. So `build.sh` follows the repository's
`/releases/latest` redirect itself (`curl`, or `wget` where there is none — a
minimal Debian server ships only the latter), validates the tag, and passes it
as the `ALTER_ZERO_VERSION` build argument. The argument's value is part of the
layer's cache key: same release, cached; new release, exactly that layer and
the ones after it rebuild.

The tag is checked against `^v[0-9]+\.[0-9]+\.[0-9]+(-…)?$` before anything
else sees it, because it becomes both a build argument and part of a download
URL; and the redirect must land under the expected repository's
`/releases/tag/`. `ALTER_ZERO_INSTALL_BASE_URL` — `install.sh`'s own variable —
moves both the lookup and the in-build download to a fork.

The **tools layer comes first** for the same reason in mirror: it is the slow
one, and nothing about a new Alter Zero release should reinstall it.

### The build context is an allowlist

The context is the repository root, because the Dockerfile `COPY`s
`install.sh`, `LICENSE` and `TELEMETRY.md` from it. `/.dockerignore` is
therefore `**` followed by three `!` lines. Measured, the context sent to the
engine is **those three files and nothing else** — 39,574 bytes, which
buildkit reports as `transferring context: 39.75kB` — from a checkout holding
`.git`, a multi-gigabyte `target/`, and quite possibly a local `.env` of API
keys. (The `519B` that buildkit prints a step earlier is the `.dockerignore`
itself being loaded, not the context; they are separate `[internal] load`
steps and it is an easy number to quote by mistake.) A denylist would have to
be kept right forever; an allowlist is right by construction.

## What is in it, by measurement

"Lightweight" is a number. Installed size added to the base, each measured
alone with `apt-get install --no-install-recommends --assume-no`:

| Package | Installed | Verdict |
| --- | --- | --- |
| `git` (+ perl) | 120 MB | kept — `/diff` and the checkpoints shell out to it |
| `nmap` | 31 MB | kept — asked for by name |
| `python3` | 30 MB | kept — the agent's scripting language, and `http.server` is how the ports are demonstrated |
| `python3-venv` | 2.9 MB | kept — the only source of `pip` here; see *Python lives in a virtualenv* |
| `openssh-client` | 26 MB | kept — `git@…` remotes, and a Kali box without `ssh` would be a surprise |
| `curl` `wget` `ca-certificates` | 23 MB | kept — `install.sh` needs one |
| `bind9-dnsutils` | 14 MB | kept — `dig` |
| `iproute2` `iputils-ping` | 10 MB | kept — `ip`, `ss`, `ping` |
| `ripgrep` `jq` `file` `less` `nano` `tree` `xxd` `unzip` `whois` `netcat-openbsd` `socat` `traceroute` `net-tools` `openssl` `tini` `procps` `libcap2-bin` | < 6 MB each | kept |
| `ncurses-term` `kitty-terminfo` | 4.6 MB | kept — see *Terminal identity* |
| `binutils` | +33 MB | **left out** — `--with binutils` |
| `tcpdump` | +22 MB | **left out** — `--with tcpdump` |

Shared dependencies make the whole smaller than the sum: the default set is
55 MB to download. `build.sh --with "PKG …"` adds packages in a **layer of its
own after the tools**, so choosing extras never reinstalls the defaults; each
word is checked to be a package name, since the list is word-split into an
`apt-get install` and an option-shaped word would be apt's to interpret.

### Docs, man pages and translations are never unpacked

The first build measured 435 MB on disk, 45 MB more than the package sizes
predicted. `/usr/share` said why: of what *our* layer added, 22 MB was
`locale/`, 17 MB `doc/`, 8 MB `man/` — translations of interfaces nobody reads
in a headless container, and man pages with no `man` installed to read them. A
dpkg `path-exclude` file written **before** the install (the technique
`debian:slim` uses) keeps them from being unpacked at all: **435 → 393 MB**.
Each package's `copyright` file is kept. The rule stays in force for whatever
the user installs later; the README says how to lift it.

Final size: **409 MB on disk, 278 MB over the 131 MB Kali base** — the 393 MB
that leaves, plus the 16 MB of `python3-venv` and the virtualenv it builds.

## Root, and no user

Kali's image is root already, and the container stays that way: no `useradd`,
no `USER`. What root *means* is the engine's business:

| Engine | Container root is | Files it creates in a mounted folder |
| --- | --- | --- |
| rootless Podman / rootless Docker | the invoking user | the user's |
| rootful Docker | the machine's root | root's |

Verified under rootless Podman: a file written by container root appears on
the host owned by uid 1000. So **no `--userns` flag is passed**. A
`--userns=keep-id:uid=0,gid=0` was considered and refused: under rootless
Podman it restates the default mapping, and under rootful Podman it is an
error.

`run.sh` adds `--security-opt no-new-privileges` (root needs no setuid helper)
and never `--privileged`. `run.sh DIR -- ARGS…` hands anything after `--` to
the engine verbatim, which is how `--cap-add NET_ADMIN` or `--network host`
are asked for, deliberately and per container.

### One capability is granted: `NET_RAW`

The image ships a scanner, and as root `nmap localhost` is a SYN scan, which
opens a raw socket. The engines disagree about that by default, so the same
image behaved differently on each — which is precisely what the first line of
`docker/README.md` promises it does not:

```
$ podman run --rm alter-zero:kali nmap localhost
Couldn't open a raw socket. Error: (1) Operation not permitted
QUITTING!
$ docker run --rm alter-zero:kali nmap localhost
Nmap scan report for localhost (127.0.0.1) …
```

So `run.sh` passes `--cap-add NET_RAW`. The reasoning for granting it by
default rather than on request: it is the *more permissive of the two
engines' own defaults*, not an escalation past either; it is one named
capability, not `--privileged` and not `--cap-add ALL`; its reach is the
container's own network namespace, which under rootless Podman is a
slirp4netns/pasta namespace of the user's own; `no-new-privileges` still
applies; and a network-tools image whose flagship tool cannot run is simply
broken. `--no-net-raw` passes only `--cap-drop NET_RAW`, explicitly dropping
the capability on both engines at the cost of `nmap -sS`, `traceroute -I`
and `tcpdump` (`nmap -sT` is unaffected). Omitting `--cap-add` alone would
leave Docker's default grant in place.

It is a flag of its own because the `--` passthrough cannot do this one job.
Capabilities are lists, not last-one-wins flags, and the engines disagree
about a capability named in both, measured: given `--cap-add NET_RAW
--cap-drop NET_RAW`, Docker keeps it — silently — and Podman refuses the
container (`capability "CAP_NET_RAW" cannot be dropped and added`). So
`run.sh -- --cap-drop NET_RAW` is wrong twice, differently per engine, and
`run.sh`'s header and the README say so. `compose.yml` carries the same
`cap_add`, pinned to `run.sh`'s own default argv by `ComposeParity` in the
stub suite: "the same container" under `podman compose` would otherwise be
the one place the raw-socket refusal survived.

`docker/tests/smoke.sh` runs a real SYN scan in the container `run.sh`
creates, then checks that `--no-net-raw` removes the capability and prevents
opening a raw socket on both engines.

### The scanner that would not exec

Kali ships `/usr/lib/nmap/nmap` with **forced file capabilities**,
`cap_net_bind_service,cap_net_admin,cap_net_raw=eip`, so an unprivileged user
can run it. A container's default bounding set has no `NET_ADMIN`, and the
kernel refuses to exec a binary whose forced capabilities it cannot grant:

```
/usr/bin/nmap: 6: exec: /usr/lib/nmap/nmap: Operation not permitted
```

— as root too, and `no-new-privileges` makes no difference. Verified in a plain
`docker run` of the base image. The fix is `setcap -r`: without file
capabilities, root simply uses whatever the runtime grants. (Kali's
`/usr/bin/nmap` launcher is not the problem and is left alone: it adds
`--privileged` only for a *non-root* caller.)

One thing would silently undo it: the package's postinst re-applies the
capabilities, so an `apt upgrade` inside the container breaks the tool again.
`/etc/apt/apt.conf.d/90alter-zero-nmap-caps` is a `DPkg::Post-Invoke` hook that
strips them after every package run; `smoke_image.py` re-applies them by hand
and runs the hook's command to prove it.

What the engines grant **by default**, measured — the disagreement `run.sh`
closes by granting `NET_RAW` itself:

| | `NET_RAW` | `ping` | connect scan | raw-packet scan |
| --- | --- | --- | --- | --- |
| Docker | yes | works | works | works |
| Podman | **no** | works | works | `Couldn't open a raw socket` |
| either, through `run.sh` | yes | works | works | works |
| either, `run.sh --no-net-raw` | no | works | works | `Couldn't open a raw socket` |

## The container idles

`ENTRYPOINT ["tini","--"]`, `CMD ["sleep","infinity"]`. The container is a
place to `exec` into — once for `alter-zero`, again for a shell, in as many
terminals as wanted — rather than one program's lifetime, so quitting the app
never stops it. `tini` reaps what those sessions leave behind.

State lives in two places that outlive the container: `/workspace` (the user's
folder, or the `alter-zero-workspace` volume) and `/root` (the
`alter-zero-home` volume — sign-ins, settings, sessions, the telemetry install
id). Everything else is the container's writable layer and is lost on
replacement. `run.sh --replace` inspects the existing container and inherits
its workspace/home mounts, image name, published ports, clipboard setting
and `NET_RAW` choice. Explicit options override the inherited settings.
Port inheritance uses the configured mappings; an automatically allocated
host port remains automatic and may receive a different number.
Unsupported configurations are refused before removal; `--reset-config`
deliberately uses supplied options and defaults instead, so extra engine
flags must be supplied again. No `VOLUME` is declared in the image: an
undeclared mount would become an anonymous volume, which persists data nobody
can find and orphans it on `rm`.

**The home volume is shared by default** across containers (`--home-volume`
isolates one). That is the right default twice over: one `/login` serves every
project, and one person stays one telemetry install.

`run.sh` creates a workspace folder that does not exist — *as the invoking
user* — because an engine handed a missing path makes it itself, owned by root.
It refuses `/`, refuses a path containing `:` (the `-v` separator), and on an
SELinux host adds `:Z` while refusing to relabel the home directory itself.
Both the workspace and home are resolved to physical paths before comparison,
so a symlinked home cannot bypass that protection.

Workspace, port and clipboard validation runs before removal of an existing
container. A rejected launcher option leaves it intact. This is not a rollback
mechanism: a failure reported by the engine after removal can still require
retrying creation with corrected engine options.

A container that fails to start is removed again: a port already in use fails
*after* `create`, and the leftover would block the retry with a confusing
"already exists".

## Python lives in a virtualenv

`python3` and `pip` resolve to **`/opt/az-venv`** by default in your shell and
the agent's commands, without manual activation.

It is not a convenience. Kali marks its system Python **externally managed**
(PEP 668), so `pip install` there is refused, and the image ships no system
`pip` at all — `python3-venv` is what brings one, through `ensurepip`. Without
the venv the agent cannot install a Python package at all, and the obvious
workarounds are both wrong: `--break-system-packages` is the flag PEP 668
exists to discourage, and `apt install python3-…` only reaches what Kali
happens to package.

**How it is activated is the part worth writing down.** The obvious move is a
line in `.bashrc`, and it would have been wrong twice over. The agent runs its
own `bash` tool through **`sh -c`**, and this image's `/bin/sh` is **dash**,
which never reads `.bashrc` — that activation would have covered a human's
`exec -it … bash` and missed every command the agent itself runs. And
`/root` is a *named volume*: anything written to `/root/.bashrc` in the image
reaches a fresh volume once, by copy-up, and an upgrading user never. So the
image sets the environment instead:

```dockerfile
ENV VIRTUAL_ENV=/opt/az-venv \
    PATH=/opt/az-venv/bin:/usr/local/sbin:…
```

Child processes inherit these settings unless something clears or replaces
their environment.

That alone is not enough, and the gap is easy to miss because the documented
command does not hit it. A **login** shell runs `/etc/profile`, which on
Debian and Kali rewrites `PATH` for root outright:

```
/etc/profile:5:  PATH="/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
```

so `bash -l` used to find `/usr/bin/python3` and **no `pip`**, despite retaining
`VIRTUAL_ENV`. `su - root` has a separate problem: it clears `VIRTUAL_ENV` and
`PIP_CACHE_DIR` as well. Restoring `PATH` only when `VIRTUAL_ENV` is already
set fixes `bash -l` but misses `su -`.

**`/etc/profile.d/az-venv.sh`** handles both: it initializes and exports missing
or empty `VIRTUAL_ENV` and `PIP_CACHE_DIR` to `/opt/az-venv` and `/var/cache/pip`,
preserves nonempty overrides, and adds the venv to `PATH` only if absent.

**`/etc/bash.bashrc`** sources that same helper before the real `activate`
script adds `deactivate` and the prompt. The order matters: interactive login
Bash runs `/etc/bash.bashrc` from `/etc/profile` before the `profile.d` files,
so waiting for `profile.d` would leave an interactive `su -` without
`deactivate`. The block **strips the venv copy off `PATH` before activation**,
because `activate` prepends unconditionally. The match treats the path
literally, including spaces and pattern characters such as brackets, so a
custom virtualenv is added once and removed by `deactivate`. Kali's own `.bashrc` renders
`$VIRTUAL_ENV` into its `┌──(az-venv)(root㉿host)` prompt and sets
`VIRTUAL_ENV_DISABLE_PROMPT=1` so `activate` does not also prepend one — so
the prompt assertion checks the *rendered* prompt (`${PS1@P}`) rather than a
literal prefix, and holds whichever of the two mechanisms draws it.

Both hooks live outside the persistent `/root` volume. Rebuild the image and
recreate the container with it (`docker/run.sh --replace`) to receive these
changes while keeping the home volume; rebuilding alone does not update a
running container.

**pip's cache is `/var/cache/pip`, not `~/.cache/pip`.** `/root` is a named
volume, and under rootless Podman a volume whose host directory falls outside
the user's subuid range appears inside the container as `nobody` — container
root cannot write it, because `CAP_DAC_OVERRIDE` stops at the user-namespace
boundary — so pip prefaced every install with *"The directory
'/root/.cache/pip' or its parent directory is not owned or is not writable …
The cache has been disabled."* Reproduced by bind-mounting a host-root-owned
directory at `/root` (it reads `65534:65534` inside), and silenced by the
move: the cache is the one thing here with no business in the volume that
holds sign-ins, and in the image layer it is always container root's own.
It is lost on `run.sh --replace`, which is what a cache is for.

Note what this does **not** fix: a `/root` that container root cannot write is
a broken home volume, and Alter Zero's own `/root/.alter-zero` is on the far
side of it too. The cache move stops pip complaining about someone else's
problem; the volume still wants recreating.

Cost: `python3-venv` 2.9 MB plus the venv's own 13 MB, so the image goes
**393 → 409 MB**. The system interpreter is untouched at `/usr/bin/python3`,
and `apt install python3-…` still works for anyone who wants that instead.

What the venv does **not** get is persistence: `/opt` is not a volume, so a
runtime `pip install` writes to the container's writable layer. It survives
`stop`/`start` and is lost on
`run.sh --replace`, exactly like an `apt install` at runtime. Making it a
third volume was refused — it would mask the built venv on first run under
some engines, and the honest fix for a package you want permanently is to bake
it in with `RUN pip install ...` in a Dockerfile `FROM alter-zero:kali`.
`--with python3-...` installs distribution packages for the system interpreter,
not the virtualenv. The README says so where someone would hit it.

## Ports

The Dockerfile `EXPOSE`s 8080 and 8888 and `run.sh` publishes both — to
**`127.0.0.1`**. Nothing in the image listens; the ports are for what the user
or the agent starts. Loopback is the default because this is a root shell an
agent can start servers in, and Docker's published ports bypass `ufw`; `--bind
0.0.0.0` is one flag away for someone who means it. `--port` accepts `PORT`,
`HOST:CONTAINER`, either with `/udp`, or the engine's full form verbatim, and
replaces the default pair rather than adding to it.

## Terminal identity

The app picks its image protocol once, at startup, from the environment
(`docs/images.md` — it may never query the terminal, invariant 1). Inside a
container that environment is the image's `TERM=xterm-256color`, so pictures
fall to half-blocks. The documented command forwards the user's own:

```sh
docker exec -it -e TERM -e COLORTERM -e TERM_PROGRAM -e KITTY_WINDOW_ID -e TMUX \
  alter-zero-kali alter-zero
```

The list is what the detector reads, checked against the source rather than
assumed: `ImageStore::detect` reads `TERM`, `TERM_PROGRAM`, `KITTY_WINDOW_ID`
and `TMUX`, and `ratatui_image`'s env sniff reads `TERM_PROGRAM` for the
iTerm2 family. (`LC_TERMINAL`, `ITERM_SESSION_ID` and `WEZTERM_EXECUTABLE`
matter only for iTerm2/WezTerm *under tmux*, so they are a README footnote.)

Verified on the raw byte stream (`tmux pipe-pane`), resuming a rollout that
holds an image read — `smoke.sh` Phase 107's own technique, run inside the
container:

| `docker exec …` | kitty graphics escapes | half-block cells |
| --- | --- | --- |
| no `-e` flags | 0 | 35 |
| the flags, from a kitty terminal | **8** (37 KB of image data) | 5 — the banner mascot |
| the flags, inside tmux (`TMUX` forwarded) | 0 | 35 |

The third row is the app's multiplexer rule doing its job, not a defect;
`-e ALTER_ZERO_IMAGE_PROTOCOL=kitty` overrides it.

Forwarding `TERM` has a cost: programs in the container now look the user's
terminal up by name, and the Kali base knows only the xterm/tmux/screen
families. `kitty-terminfo` (0.1 MB) and `ncurses-term` (4.5 MB) add kitty,
Alacritty, foot, WezTerm, Rio and VTE. Ghostty's entry is in neither, so the
README carries the one-line `infocmp | tic` import, which lands in the home
volume. Alter Zero itself reads no terminfo.

## The clipboard

`exec -it` carries keystrokes. The desktop's selection is a separate
connection to the display server, which a container does not have — so Ctrl+V
finds nothing. `run.sh --clipboard` forwards the connection.

**Wayland**: the compositor's one socket, bind-mounted read-only at
`/run/alter-zero/wayland-0`, with `XDG_RUNTIME_DIR=/run/alter-zero` and
`WAYLAND_DISPLAY=wayland-0`. Never the runtime directory around it, which also
holds D-Bus, PipeWire and the keyring agent.

**X11 as well, not instead.** `clipboard::linux` tries Wayland first and falls
back to X11, "XWayland included, since a Wayland session with no data-control
protocol still serves its clipboard over X11" — which describes GNOME. A
Wayland-only forward gives such a desktop `MissingProtocol` and then nothing to
fall back to. So when `DISPLAY` names a local display, `/tmp/.X11-unix` is
mounted too, with the access cookie.

The cookie needs care. The server files it under the machine's **hostname**
(family `Local`), and the container has a different one, so a verbatim
`.Xauthority` matches nothing. `run.sh` rewrites the entry's family to the
wildcard — `xauth nlist | sed 's/^..../ffff/' | xauth nmerge` — into
`$XDG_STATE_HOME/alter-zero/docker/{name}/`, and mounts that **folder** rather
than the file, so a refreshed cookie is seen without a new container. (Giving
the container the host's hostname would also work, and would make the shell
prompt claim to be the host.) The file's mode is set with `chmod`, not `umask`:
a folder carrying a default ACL ignores the umask, which the test suite caught
as a world-readable cookie.

Sources are `--mount type=bind`, not `-v`. Given a source that does not exist,
`-v` **creates it as a root-owned folder** — in the runtime directory, at the
exact path the compositor then cannot put its socket. `--mount` fails instead.
For the same reason a `--clipboard` container gets **no restart policy**: its
sockets belong to a desktop login that does not exist yet at boot.

Both paths were driven end to end against real servers: an access-controlled
Xvfb (a cookie-less client is refused) and a headless sway with
wlr-data-control, each holding a PNG. Ctrl+V in the containerised TUI produced
`[Image #1]` and a file byte-identical to the one on the host clipboard.

### The one source change: say why, at once

A session naming no display used to fall through to `arboard`, whose X11 probe
could stall (`027-imagepaste.sh`'s own comment allowed ~4 s for it) before
reporting whatever it ran into. `clipboard::require_display` now refuses first
— pure over the two environment values, so it is tested without touching the
process environment, which edition 2024 makes `unsafe` and this crate forbids:

```
Failed to paste image: no desktop clipboard in this session (neither DISPLAY
nor WAYLAND_DISPLAY is set). Save the image file where this session can reach
it, then ask Alter Zero to read its path.
```

It names both variables because forwarding one is the fix, and the sentence is
pinned to `APP_NAME`. A display that is named but unreachable — a forwarded
socket gone stale — keeps arboard's cause whole and gains the same way around.
The image installs a *published* binary, so the container sees this wording
from the first release that carries it; the forwarding itself needs nothing
new.

## Telemetry is left alone

The image sets neither `ALTER_ZERO_TELEMETRY` nor `DO_NOT_TRACK`, and nothing
in `docker/` passes either. The app behaves as any install does: first-run
notice, one ping a day, its own off switches intact (`TELEMETRY.md`, a copy of
which ships at `/usr/share/doc/alter-zero/`). Building the image and starting
the idle container launch nothing and so send nothing.

`/etc/os-release` reads `ID=kali`, so the ping's `distro` is `kali`. The
install id is in `/root/.alter-zero/telemetry.json`, inside the home volume,
which is what keeps a recreated container — and every container sharing that
volume — one install rather than many.

This is tested rather than asserted. `smoke_image.py` launches the real CLI on
a PTY inside a container started with **`--network=none`**, beside a stub
collector on loopback named by `ALTER_ZERO_TELEMETRY_URL`. With no route out,
the stub is the only collector reachable, so the test can prove the ping is
sent (once, carrying `distro: kali`, and not again on a relaunch over the same
home) without one test run landing in the production numbers.

## Tests

| | Needs | Covers |
| --- | --- | --- |
| `docker/tests/test_scripts.py` | nothing | `build.sh` and `run.sh` under a `PATH` holding only symlinked coreutils and recording stubs named `docker`, `podman`, `curl`, `wget`, `xauth` — the exact argv the engine would get; a bug can never reach a real engine or the network |
| `docker/tests/smoke_image.py` | the image | run inside it, no network: root and no added user, tools, terminfo, the scanner and its dpkg hook, no listeners, telemetry on |
| `docker/tests/smoke.sh` | an engine | the above, then `run.sh` for real: files cross the mount both ways, a server inside answers on the published port, everything it made is removed |

They run in **`container.yml`**, a workflow of their own: the stub suite and
`shellcheck` first, then the image built and smoked on **both engines**. Its
trigger is the point — every push to `main` (the image comes from a *rolling*
base and the newest release, so that is the canary for upstream drift), but a
pull request only when it touches the packaging (`docker/**`, `install.sh`,
the allowlist, the two COPYed files). Nothing in `src/` can break it — it
installs a *published* release — so charging every unrelated pull request two
full Kali builds, against Docker Hub, a Kali mirror and github.com's release
redirect, would buy flake and nothing else.

## Refused

- **Compiling in the image.** Minutes per build and a toolchain in the image,
  to produce the binary the release workflow already published and checksummed.
- **A `latest` resolved inside a `RUN`.** Cached forever; see above.
- **A non-root user.** Asked for root, and a Kali user expects it. The
  ownership cost is real only under rootful Docker and is documented.
- **Publishing to `0.0.0.0` by default.** See *Ports*.
- **A wrapper for `exec`.** The command is the documentation: five `-e` flags a
  user can read, alias, and extend. `run.sh` prints it.
- **Kali metapackages.** `kali-linux-headless` is gigabytes. `--with` and
  `apt install` are the answer for the tool someone actually wants.
