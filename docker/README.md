# Alter Zero on headless Kali Linux

A small [Kali Rolling](https://www.kali.org/docs/containers/official-kalilinux-docker-images/)
container with Alter Zero and a handful of command-line tools already in it.
It works the same with **Docker** and **Podman**.

- **Nothing is compiled.** The image downloads the latest published release,
  SHA-256-verified by the repository's own [`install.sh`](../install.sh). A
  build is about a minute.
- **It runs as root**, as Kali's own image does. No user is added.
- **It is small on purpose**: no desktop, no Kali metapackage, no compiler.
  About 280 MB on top of the Kali base.
- **Ports 8080 and 8888** are published to your machine, for whatever you or
  the agent start on them.
- **Telemetry is left exactly as it is** in an ordinary install. See
  [Telemetry](#telemetry).

## Quick start

From the repository root:

```sh
docker/build.sh ~/projects/site
```

That builds the image, then creates a container whose `/workspace` **is** that
folder. It ends by printing the command that opens Alter Zero:

```sh
docker exec -it \
  -e TERM -e COLORTERM -e TERM_PROGRAM \
  -e KITTY_WINDOW_ID -e TMUX \
  alter-zero-kali alter-zero
```

Run it from your own terminal. Then `/login` to connect a provider and
`/model` to choose a model. For a ChatGPT subscription pick **Device code
login (headless)**, since the container has no browser.

With **Podman**, say so once and swap the word in the commands:

```sh
docker/build.sh --engine podman ~/projects/site

podman exec -it \
  -e TERM -e COLORTERM -e TERM_PROGRAM \
  -e KITTY_WINDOW_ID -e TMUX \
  alter-zero-kali alter-zero
```

`--engine` also reads `CONTAINER_ENGINE`, and with neither it picks Docker
when installed, else Podman.

> **Why the five `-e` flags?** A bare `-e NAME` forwards that variable's value
> from *your* terminal. They are what let Alter Zero recognise kitty, Ghostty,
> iTerm2, WezTerm and friends and draw **real pictures** instead of coloured
> half-blocks. See [Pictures](#pictures-in-your-terminal).

The two steps, separately:

```sh
docker/build.sh                   # build the image only
docker/run.sh ~/projects/site     # create the container on that folder
docker/run.sh                     # ...or on a named volume instead
```

Both take `--help`.

The container name is `alter-zero-kali`, which Docker and Podman commands
use. Its Linux hostname is `az-kali`, as shown in the shell prompt.
`--name` changes only the container name; the hostname stays `az-kali`.

## Everyday use

```sh
docker exec -it alter-zero-kali bash      # a root shell in /workspace
docker stop alter-zero-kali               # stop it
docker start alter-zero-kali              # start it again
docker rm -f alter-zero-kali              # remove it (your data is kept, see below)
```

The container idles; you `exec` into it as many times as you like, in as many
terminals as you like. Quitting Alter Zero does not stop the container.

A shell alias saves the typing:

```sh
alias az='docker exec -it -e TERM -e COLORTERM -e TERM_PROGRAM -e KITTY_WINDOW_ID -e TMUX alter-zero-kali alter-zero'
az                         # open it
az --continue              # reopen the last conversation in /workspace
az "fix the failing test"  # start on a prompt
```

## Your files: the workspace

`/workspace` is where you land and where the agent works.

**A folder on your machine** is mounted there when you name one. Edits show up
on both sides at once:

```sh
docker/run.sh ~/projects/site
docker/run.sh --replace ~/projects/other     # move the container to another folder
```

A folder that does not exist is created for you. The path is resolved, so a
relative one is fine.

**Several projects** each get a container of their own. They share the one
home volume, so you sign in once:

```sh
docker/run.sh --name az-site --port 8081:8080 ~/projects/site
docker/run.sh --name az-api  --port 8082:8080 ~/projects/api
docker exec -it -e TERM -e COLORTERM -e TERM_PROGRAM -e KITTY_WINDOW_ID -e TMUX az-api alter-zero
```

**With no folder**, `/workspace` is the named volume `alter-zero-workspace`.
Copy files in and out with the engine:

```sh
docker cp ./report.pdf alter-zero-kali:/workspace/
docker cp alter-zero-kali:/workspace/notes.md ./
```

### Who owns the files

The container runs as root, and what that means on your side depends on the
engine:

| Engine | Root inside is… | Files it creates in your folder |
| --- | --- | --- |
| Podman, rootless (the default) | **you** | yours |
| Docker, rootless | **you** | yours |
| Docker, the usual rootful daemon | the machine's root | root's. Reclaim them with `sudo chown -R "$USER": ~/projects/site` |

On an SELinux machine `run.sh` labels the mounted folder for the container
(`:Z`). It refuses to do that to your home directory itself; mount a folder
inside it.

### What is kept

| Where | What | Survives `rm -f` / `--replace` |
| --- | --- | --- |
| `/workspace` | your folder, or the `alter-zero-workspace` volume | yes |
| `/root` | the `alter-zero-home` volume: sign-ins, settings, sessions, shell history, SSH keys | yes |
| everything else | packages you `apt install`ed, files under `/tmp`, `/opt` | **no** |

`--replace` keeps the existing workspace and home mounts, image name, port
mappings, clipboard setting, PID limit and `NET_RAW` choice. Options you supply
override those settings. The launcher checks its inputs before removing the
old container; an invalid workspace, port, IP address or desktop session
leaves it running.
Other files in the container's writable layer are still lost on replacement.
To delete the volumes as well: `docker volume rm alter-zero-home
alter-zero-workspace`.

To switch from a host folder back to a named workspace, use
`docker/run.sh --replace --workspace-volume alter-zero-workspace`.

## Ports

Nothing in the image listens. `run.sh` publishes **8080** and **8888** to
`127.0.0.1` on your machine so that a server started inside is reachable:

```sh
docker exec -it alter-zero-kali python3 -m http.server 8080 --bind 0.0.0.0
# now open http://localhost:8080 on your machine
```

The server must listen on **`0.0.0.0`** inside the container. One bound to the
container's own `127.0.0.1` is not reachable through a published port.

```sh
docker/run.sh --port 3000                      # 3000 instead of the default pair
docker/run.sh --port 8080 --port 5173          # several
docker/run.sh --port 9000:8080                 # your 9000 -> the container's 8080
docker/run.sh --port 5353/udp                  # UDP
docker/run.sh --bind 0.0.0.0                   # reachable from your network
docker/run.sh --no-ports                       # publish nothing
```

`--bind 0.0.0.0` exposes those ports to every machine that can reach yours,
and Docker's published ports bypass `ufw`. Keep the default unless you mean
it. Ports are fixed when the container is created, so changing them means
`--replace`. Outbound connections need no published port.

From inside, your machine is `host.docker.internal` (Docker Desktop) or
`host.containers.internal` (Podman).

## Pictures in your terminal

Alter Zero draws pasted screenshots and image reads as real pictures where the
terminal can: the kitty protocol (kitty, Ghostty), iTerm2's (iTerm2, WezTerm,
VS Code, Rio, Warp), or sixel. It decides **once, at startup, from the
environment**, and inside a container that environment is the image's generic
`TERM=xterm-256color` unless you forward yours:

```sh
docker exec -it \
  -e TERM -e COLORTERM -e TERM_PROGRAM \
  -e KITTY_WINDOW_ID -e TMUX \
  alter-zero-kali alter-zero
```

Without the flags you get half-blocks. Nothing needs rebuilding: quit and
reopen with the flags.

<details>
<summary><strong>Inside tmux or screen, and forcing a protocol</strong></summary>

Under a multiplexer Alter Zero deliberately falls back to half-blocks: `TMUX`
(forwarded above, on purpose) tells it the outer terminal's identity may be
stale, and tmux drops graphics unless passthrough is on. If yours is set up
for it (`set -g allow-passthrough on`), say which protocol to use:

```sh
docker exec -it \
  -e TERM -e COLORTERM -e TERM_PROGRAM -e KITTY_WINDOW_ID -e TMUX \
  -e ALTER_ZERO_IMAGE_PROTOCOL=kitty \
  alter-zero-kali alter-zero
```

The values are `kitty`, `iterm2`, `sixel` and `halfblocks`. iTerm2 and WezTerm
*inside tmux* are recognised by three more variables; add
`-e LC_TERMINAL -e ITERM_SESSION_ID -e WEZTERM_EXECUTABLE`.

</details>

<details>
<summary><strong>"unknown terminal type" from less, nano or git</strong></summary>

Forwarding `TERM` means programs in the container look your terminal up by
name. The image carries the entries for kitty, Alacritty, foot, WezTerm, Rio
and the VTE terminals. For one it lacks (Ghostty's `xterm-ghostty`, today),
copy yours in once. It lands in the home volume, so it stays:

```sh
infocmp -x "$TERM" | docker exec -i alter-zero-kali tic -x -o /root/.terminfo -
```

Alter Zero itself never needs the entry.

</details>

## Pasting images with Ctrl+V

`docker exec -it` carries your keystrokes, not your desktop's clipboard, so in
a plain container Ctrl+V has nothing to read. Create the container with
`--clipboard` and it can:

```sh
docker/run.sh --clipboard ~/projects/site
```

Run that **from a terminal on your own desktop**. It forwards what your session
has: the Wayland compositor's socket, the X11 socket with its access cookie, or
both. Both matters: GNOME's compositor has no clipboard protocol for
command-line programs, so its clipboard is reached through XWayland, and Alter
Zero falls back to that by itself. Then copy a screenshot, press Ctrl+V, and
`[Image #1]` appears.

What this grants: a program in the container can talk to your display server —
read and set the clipboard, and on X11 see other windows and keystrokes. It is
off unless you ask. Only the one socket is mounted, never your whole runtime
directory.

<details>
<summary><strong>Limits, and what to do over SSH</strong></summary>

- **It follows your login session.** After you log out and in, or reboot, the
  sockets are new ones. Recreate the container: `docker/run.sh --replace
  --clipboard ~/projects/site`. For the same reason a clipboard container is
  not restarted automatically at boot; `docker start alter-zero-kali` it.
- **X11 needs `xauth`** on your machine to hand over the cookie. `run.sh` says
  so if it is missing.
- **Copy the picture, not the file.** A file copied in a file manager puts a
  *path* on the clipboard, and that path does not exist in the container.
- **Text copy needs none of this.** `/copy` reaches your clipboard through the
  terminal itself (OSC 52).

**Over SSH, or on macOS and Windows**, there is no desktop socket to forward.
Put the picture in the workspace and ask for it by path:

```sh
docker cp ./screenshot.png alter-zero-kali:/workspace/
```

```text
read /workspace/screenshot.png and tell me what is wrong with the layout
```

The `read` tool sends the image to a vision-capable model and draws it inline,
exactly as a paste would.

</details>

## What is inside

| | |
| --- | --- |
| **Alter Zero** | the latest release, at `/usr/local/bin/alter-zero` |
| Shell and files | `bash` `git` `ssh` `curl` `wget` `jq` `rg` `file` `less` `nano` `tree` `xxd` `unzip` |
| Python | `python3` and `pip` from a virtualenv at `/opt/az-venv`, active by default — see [Python](#python) |
| Network | `nmap` `nc` `socat` `whois` `dig` `nslookup` `ping` `traceroute` `ip` `ss` `ifconfig` `netstat` `openssl` |
| Terminal | terminfo for kitty, Alacritty, foot, WezTerm, Rio, VTE |

Left out because they are large and not everyone wants them: `binutils`
(`strings`, `objdump`; +33 MB), `tcpdump` (+22 MB), a compiler, man pages,
translations.

**Add your own**, baked into the image:

```sh
docker/build.sh --with "binutils tcpdump"
```

or for the life of one container: `apt update && apt install -y sqlmap`.
Package docs, man pages and translations are skipped by a dpkg rule. To get a
package's pages back, delete `/etc/dpkg/dpkg.cfg.d/90alter-zero-slim` and
reinstall it.

### Privileges

The container gets the engine's default capabilities plus **`NET_RAW`**, and
`no-new-privileges`. It is never `--privileged` and never uses host
networking.

`NET_RAW` is there because the image ships a scanner: as root, `nmap
localhost` is a SYN scan, and that opens a raw socket. Docker grants the
capability by default and Podman 4.x does not, so without it the same image
answers `Couldn't open a raw socket` on one engine and scans on the other.
It is one named capability, scoped to the container's own network namespace.
`--no-net-raw` explicitly drops it on both engines, and `nmap -sS`,
`traceroute -I` and `tcpdump` go with it (`nmap -sT` still works — a connect
scan needs nothing special). Use that
flag rather than `-- --cap-drop NET_RAW`: next to the `--cap-add` that `run.sh`
passes, Docker silently keeps the capability and Podman refuses to create the
container.

Anything after `--` goes straight to `docker run`, for whatever else a job
needs:

```sh
docker/run.sh ~/projects/site -- --cap-add NET_ADMIN
docker/run.sh ~/projects/site -- --network host
```

Only scan machines you are authorised to test.

## Python

`python3` and `pip` come from a **virtualenv at `/opt/az-venv`**, active by
default in your shell and the agent's commands:

```sh
docker exec -it alter-zero-kali bash
┌──(az-venv)(root㉿az-kali)-[/workspace]
└─# pip install requests        # just works
```

The image exports `VIRTUAL_ENV` and puts the venv first on `PATH`. Shell hooks
also restore the defaults for login shells, including `bash -l` and
`su - root`, so these need no manual activation. Nonempty `VIRTUAL_ENV` and
`PIP_CACHE_DIR` overrides are preserved. Recreate the container using a rebuilt
image to receive updated hooks; your existing home volume can stay.

This is not decoration. Kali marks its system Python **externally managed**
(PEP 668), so a plain `pip install` there is refused, and the image ships no
system `pip` at all — without the venv, the agent simply cannot install a
Python package. Distribution packages stay untouched: `/usr/bin/python3` is
still the system interpreter, and `apt install python3-…` still works.

pip caches downloads in `/var/cache/pip`, not under `/root` — a home volume
the container cannot write (rootless Podman, a volume outside your subuid
range) would otherwise make pip disable its cache and say so on every install.

An interactive shell also gets `deactivate`, if you want the system
interpreter for a moment. Or just call it by path: `/usr/bin/python3`.

> **Packages you install at runtime live in the container's writable layer.** They
> survive `stop`/`start`, and they are lost when the container is replaced
> (`docker/run.sh --replace`) — the same as anything else you `apt install` at
> runtime. For a package you want permanently in the virtualenv, add
> `RUN pip install numpy` to your own Dockerfile built `FROM alter-zero:kali`.
> `docker/build.sh --with python3-numpy` instead installs NumPy for the system
> interpreter, `/usr/bin/python3`.

## Updating

A new Alter Zero release, or a fresher Kali base:

```sh
docker/build.sh               # resolves the latest release again
docker/run.sh --replace       # use the rebuilt image with existing launch settings
```

For example, a container created with `--clipboard --no-ports` keeps both
settings on replacement. Run the command from your desktop so clipboard
sockets can be refreshed. Use `--no-clipboard` to disable forwarding or
`--net-raw` to re-enable raw sockets after previously dropping them.
If a port was configured for automatic allocation (an empty or zero host
port), the engine may assign a different host port on replacement.

Only the settings managed by `run.sh` are inherited. Custom environment
overrides, CPU limits and other detected unsupported settings stop replacement
before the old container is removed. Environment defaults are checked against
the image that created the container, even if its tag now points to a newer
image.
`--replace --reset-config` intentionally starts from the launcher's defaults;
pass the complete workspace, volume, port and other options you want to keep,
including any engine options after `--`.

The PID limit is inherited; override it with `-- --pids-limit NUMBER`.

Errors reported only by the engine, such as a port already in use, can still
prevent startup after the old container has been removed. Correct the error
and retry with the intended configuration.

`build.sh` looks the latest release up **every time**, before the engine
consults its layer cache. That is the whole reason to build with it rather
than a bare `docker build`, whose cached download layer would go on
installing the release it first saw. To pin one: `docker/build.sh --version
v0.4.0`.

`alter-zero update` inside the container also works, until the container is
recreated.

## Compose

[`compose.yml`](compose.yml) describes the same container, for those who
prefer it. Build the image first:

```sh
docker/build.sh
ALTER_ZERO_WORKSPACE=~/projects/site docker compose -f docker/compose.yml up -d
docker exec -it -e TERM -e COLORTERM -e TERM_PROGRAM -e KITTY_WINDOW_ID -e TMUX \
  alter-zero-kali alter-zero
docker compose -f docker/compose.yml down        # the volumes are kept; never add --volumes
```

Without `ALTER_ZERO_WORKSPACE`, `/workspace` is the named volume. `podman
compose` takes the same file.

## Telemetry

The image changes nothing about it. It sets neither `ALTER_ZERO_TELEMETRY` nor
`DO_NOT_TRACK`, so Alter Zero behaves as any install does: the first launch
shows its notice, and it sends one anonymous ping a day — a random install id,
the app version, `linux`, the architecture, and the distribution, which here
reads `kali`. Never a prompt, a path, a model name or a key.
[`TELEMETRY.md`](../TELEMETRY.md) is the complete statement; a copy ships in
the image at `/usr/share/doc/alter-zero/`.

Building the image and starting the idle container send nothing. Only
launching `alter-zero` does.

The install id lives in `/root/.alter-zero/telemetry.json`, which is in the
home volume. That is what keeps one person counted as one install: every
container sharing `alter-zero-home` is the same install, and recreating a
container does not mint a new one.

Your own off switches still work, and are yours to use: `/settings` →
**Telemetry**, or per run `docker exec -it -e ALTER_ZERO_TELEMETRY=0 …`.

## Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| Pictures are coloured blocks | The `-e` flags are missing from `docker exec`, or you are inside tmux. See [Pictures](#pictures-in-your-terminal). |
| `Failed to paste image: no desktop clipboard…` | The container has no display forwarded. Create it with `--clipboard`, or `docker cp` the picture in and ask Alter Zero to read it. |
| Paste worked yesterday, not today | You logged out and in; the sockets changed. `docker/run.sh --replace --clipboard …` |
| `a container called alter-zero-kali already exists` | Enter it, or recreate it with `--replace`. Nothing is removed unless you say so. |
| `docker could not start …` | Usually 8080 or 8888 is taken on your machine. `--port 18080:8080`, or `--no-ports`. |
| `container … is not running` | `docker start alter-zero-kali`. |
| `nmap: Operation not permitted` | Kali ships it with file capabilities a container cannot grant. The image strips them and re-strips after every `apt` run; by hand: `setcap -r /usr/lib/nmap/nmap`. |
| `Couldn't open a raw socket … Operation not permitted` | A SYN scan needs `NET_RAW`. `docker/run.sh` grants it, so this means the container was made another way, or with `--no-net-raw`. Recreate it: `docker/run.sh --replace …`, or use `nmap -sT`. See [Privileges](#privileges). |
| Files in my folder belong to root | Rootful Docker. See [Who owns the files](#who-owns-the-files). |

## Testing

```sh
python3 -m unittest discover -s docker/tests -p 'test_*.py' -v   # the scripts, no engine, no network
docker/tests/smoke.sh                                            # the built image + run.sh, for real
docker/tests/smoke.sh --engine podman
```

The smoke test launches the real CLI inside a container **with no network**
and a stub collector beside it, which is how it proves telemetry is still on
without a single test ping reaching production. The design, the measurements
and what was verified are in [`docs/docker.md`](../docs/docker.md).
