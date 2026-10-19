# Flaredantic implementation reference

Studied upstream commit `a25069806abbd1eb0bb9f47126fdbf2570b304c9` (2026-10-19). Local `flare --help` and `pipx list --short` verified CLI flags and installed version 0.1.5. Recheck installed behavior before relying on future versions.

Sources:
- https://github.com/linuztx/flaredantic
- https://github.com/linuztx/flaredantic/blob/a25069806abbd1eb0bb9f47126fdbf2570b304c9/flaredantic/cli.py
- Provider implementations: `flaredantic/tunnel/{cloudflare,serveo,microsoft}/tunnel.py`
- Configurations: `flaredantic/base/config.py` and provider `config.py` files

## Provider behavior and caveats

| Provider | Actual implementation | Operational consequence |
| --- | --- | --- |
| Cloudflare | Runs downloaded/cached `cloudflared tunnel --protocol quic --url http://localhost:PORT`; Termux uses `http2` | Quick HTTP tunnel; no CLI upstream-host/HTTPS-origin override. QUIC connectivity can fail even if a URL is announced. |
| Serveo HTTP | Runs `ssh -T -R 80:localhost:PORT … serveo.net`; parses `https://….serveousercontent.com` | Requires local SSH client and provider availability. |
| Serveo TCP | Runs SSH with `-R 0:localhost:PORT`; parses allocated port into `serveousercontent.com:PORT` | Remote port is allocated, not necessarily equal to the local port. |
| Microsoft | Downloads `devtunnel`; checks login, creates/reuses tunnel and HTTP port, then runs `devtunnel host ID --allow-anonymous` | Provider login is not visitor authentication. Hosting an existing ID may expose its other ports too. |

Cloudflare defaults to caching at `~/.flaredantic/cloudflared` (`cloudflared.exe` on Windows). The downloader uses GitHub's latest release when no cached file exists; it does not automatically update an existing binary or verify checksums. Microsoft also uses the configured binary directory. Do not silently remove cached binaries or switch versions during unrelated work.

Serveo uses `StrictHostKeyChecking=accept-new` and its own configured known-hosts path. Never disable host-key checking or erase a changed key automatically. Verify a mismatch independently. Availability is not guaranteed for any provider, regardless of README claims.

Microsoft defaults to `tunnel_id="flaredantic"`, `device_login=True`. Login invokes GitHub device flow (`devtunnel user login -g -d`). Let the user complete the displayed login flow; do not guess codes or credentials. Before hosting an existing tunnel, use the installed `devtunnel` help to find and run its port-inventory command. If the ID contains unrelated ports, use a fresh, unique ID through the Python API instead of hosting all of them. The `flare` CLI cannot set an ID. Startup checks create resources if absent; host shutdown leaves those resources intact. Account logout and deletion need separate, intentional cleanup.

The URL-extraction thread stops reading provider output after finding an endpoint. Subsequent low-level logs may not appear, and an idle wrapper does not guarantee a healthy child. Verify routing independently; if long-running use fails, investigate the exact child process and pipe/log behavior rather than assuming a daemon health monitor exists.

## Python integration

Install Flaredantic in the app's own virtual environment. Start and verify the local app first. Keep the context alive for as long as the URL should work:

```python
import signal
from flaredantic import FlareConfig, FlareTunnel


def stop(signum, frame):
    raise SystemExit(0)


signal.signal(signal.SIGINT, stop)
signal.signal(signal.SIGTERM, stop)

with FlareTunnel(FlareConfig(port=8080, timeout=60, verbose=True)) as tunnel:
    print(tunnel.tunnel_url, flush=True)
    while True:
        signal.pause()  # Linux/main thread; no busy loop
```

Alternative classes exported at package root:

```python
from flaredantic import ServeoConfig, ServeoTunnel
from flaredantic import MicrosoftConfig, MicrosoftTunnel

# Construct only the selected provider, and keep it in a live context.
serveo_config = ServeoConfig(port=8080, timeout=60, verbose=True)
tcp_config = ServeoConfig(port=2222, timeout=60, verbose=True, tcp=True)
microsoft_config = MicrosoftConfig(
    port=8080,
    timeout=60,
    verbose=True,
    tunnel_id="replace-with-a-unique-task-id",
    device_login=True,
)
```

Shared config fields: `port` (required), `bin_dir` (`pathlib.Path`, default `~/.flaredantic`), `timeout` (30), `verbose` (False). Serveo adds `ssh_dir` (default `~/.flaredantic/ssh`) and `tcp` (False). Microsoft adds `tunnel_id` and `device_login`. These are dataclasses, not Pydantic models; validate port range and user input explicitly.

`start()` returns the endpoint; `stop()` tears down the subprocess. Prefer context management, or `try/finally` around manual lifecycle. Construct a new tunnel object after stopping: provider stop events are not reset for restart. Async context methods exist but call synchronous start/stop, so they block the event loop; offload deliberately for asynchronous apps.

## Failure checklist

- **Connection refused / gateway error:** verify the app, port, safe local route, HTTP protocol, and local hostname resolution. These provider implementations forward to localhost HTTP; do not invent a `--host` flag.
- **Timeout waiting for URL:** separate binary download, provider connection, URL parsing, and origin readiness. Raising `--timeout` only changes the extraction-thread wait. Downloads and Microsoft login are outside it.
- **Cloudflare URL prints but fails:** inspect the exact provider process and network constraints. This version hardcodes QUIC outside Termux; there is no protocol switch in `flare`. Do not fake `TERMUX_VERSION` to change protocol. Explain the limit and offer an explicitly chosen supported alternative or deliberate source change.
- **Serveo unavailable:** report it; offer Cloudflare for HTTP. Cloudflare HTTP is not an equivalent replacement for raw TCP.
- **Microsoft waits for login:** show the requested user action privately and wait. Do not promise its timeout limits login.
- **Process exited after printing URL:** inspect session output and child status. Verify exact-process cleanup before retrying to avoid orphaned tunnels.
- **Custom domain, auth, persistent hosting:** unsupported by these CLI flags. Do not claim that a random quick-tunnel address is a durable cloud deployment.
