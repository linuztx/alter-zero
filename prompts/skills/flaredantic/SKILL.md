---
name: flaredantic
description: Expose local services to the internet with Flaredantic tunnels. Use when sharing localhost, publishing a dev server, creating a public HTTPS URL, testing webhooks, forwarding TCP, or requesting Cloudflare, Serveo, or Microsoft tunnels.
---

# Flaredantic service exposure

Use Flaredantic (`flare`), not an unrelated tunneling tool, to share a local service. Upstream: https://github.com/linuztx/flaredantic. Read `reference.md` alongside this file for Python integration, implementation caveats, and troubleshooting.

## 1. Establish scope and safety

- Identify the intended local service, port, protocol, and whether the user wants a temporary share or persistent hosting. Inspect the project and listeners before asking questions that local evidence can answer. Ask when the intended service or exposure is ambiguous.
- Explain that this is a public tunnel to a running local machine, not deployment: the machine, app, network, and tunnel must stay running. Do not promise permanent URLs, production availability, unlimited usage, or provider quotas.
- Default to Cloudflare for HTTP apps and webhooks. Use Serveo for explicitly requested raw TCP; use Microsoft only when requested or deliberately chosen with the user.
- HTTPS at the public endpoint does not supply application authentication. Before launch, check for debug consoles, unprotected admin APIs, directory listings, secrets, and private data. Do not expose these casually. Ask for explicit confirmation and appropriate protection before sensitive exposure. Prefer a safe health route for verification.
- Do not open firewall/router ports, change app binding to `0.0.0.0`, disable authentication/TLS checks, create boot services, or upload code merely to create a tunnel. A loopback listener is sufficient. Database/SSH TCP exposure requires deliberate user intent and suitable authentication; raw TCP does not imply TLS.
- Microsoft uses `--allow-anonymous` and hosts the configured tunnel ID, potentially including other existing ports. Read the reference and check its port inventory before using it.

## 2. Check the local app and CLI

Use the actual port in place of `8080` below:

```bash
command -v flare
flare --help
ss -ltnp
curl --silent --show-error --max-time 10 --output /dev/null \
  --write-out 'local_http=%{http_code}\n' 'http://localhost:8080/'
```

- Verify that the listener belongs to the intended app. Use a known safe health path when available. For TCP use a protocol-appropriate probe, not HTTP.
- Start the app using its documented command if needed, retain its session handle, and verify readiness before starting the tunnel. Do not launch `python -m http.server` from an arbitrary project/home directory as a substitute: that can publish files.
- A legitimate 401/403/404 can be an expected app response, not a dead service. Record expected behavior rather than declaring success from any status.
- If `flare` is absent, prefer `pipx install flaredantic` for the CLI, or an isolated virtual environment. Do not use `sudo pip`, change system Python, or reinstall an already working copy. Python project integrations need Flaredantic installed in that project's environment; a pipx CLI install does not make it importable everywhere.
- First use may download provider binaries into `~/.flaredantic`. CLI startup timeout is not a download or login deadline. Do not print credentials or inspect account secret files.

## 3. Launch a tracked tunnel

Default HTTP share:

```bash
flare --port 8080 --tunnel cloudflare --timeout 60 --verbose
```

Other deliberate choices:

```bash
flare --port 8080 --tunnel serveo --timeout 60 --verbose
flare --port 8080 --tunnel microsoft --timeout 60 --verbose
flare --port 2222 --tcp --timeout 60 --verbose
```

- Run the command in a retained terminal/tool session (use Bash `wait: 0` for background execution), then inspect its output with session tools. Do not pipe through `head`, run inside a short-lived command substitution, or discard the process handle.
- The executable is `flare`, not `flaredantic`. `--tcp` forces Serveo even if `--tunnel` says otherwise. `-t` means timeout, not provider.
- Supported CLI flags are `-p/--port`, `-t/--timeout`, `-v/--verbose`, `--tunnel`, and `--tcp`. There are no CLI host, domain, auth, daemon, protocol, or tunnel-ID flags.
- Capture the exact endpoint from provider output. Cloudflare normally prints a random `https://….trycloudflare.com` URL; never use `api.trycloudflare.com` or fabricate an endpoint. Serveo TCP returns a `host:port`, not an HTTPS URL.
- Record the session ID and the selected provider/local port. Where practical record the exact wrapper and child PIDs for cleanup; never use global `pkill flare`, `pkill ssh`, or `pkill cloudflared`.

## 4. Verify external routing

For an HTTP endpoint actually printed by the tunnel, request the same safe path as the local probe:

```bash
curl --silent --show-error --connect-timeout 10 --max-time 20 \
  --output /dev/null --write-out 'public_http=%{http_code}\n' \
  'https://ACTUAL-PRINTED-HOST/health'
```

- Compare expected status/content with the local service. A provider interstitial, login page, or generic gateway response does not prove the intended app is accessible. Inspect a minimal safe body when needed without dumping private data.
- Allow a few bounded retries for DNS/routing initialization; diagnose continued failures instead of retrying indefinitely. Do not bypass TLS verification.
- Check the retained session is still running. A URL announcement alone does not prove the origin works, nor does a successful request guarantee continued availability.
- For TCP, verify the returned host and port with a protocol-appropriate check; distinguish transport connectivity from application authentication.
- If this environment cannot test public access, state that it is unverified; do not call the service live.

## 5. Report and stop predictably

Report the exact URL/host:port, provider, local port, verification result, session handle, and how to stop. Explain that it remains public while running and that the address may change on restart. Do not stop a requested active share immediately after verifying it.

For cleanup, send Ctrl+C to the retained CLI session so its context manager calls `stop()` and terminates the provider child. Confirm wrapper and child termination. A forced session kill is only a fallback after checking exact processes; SIGTERM/process-group termination does not guarantee Python context cleanup. Stop an app only if this task started it and the user no longer needs it. For Microsoft, stopping the host does not delete cloud-side tunnel or port resources; do not delete them without explicit intent.

## Scope

Creating this skill, explaining commands, or writing an integration is not authorization to expose a live service. Do not start a public tunnel as a documentation test. Persistent custom domains, protected production ingress, and authenticated provider policy are outside this CLI's supported configuration; explain the limitation rather than inventing flags.
