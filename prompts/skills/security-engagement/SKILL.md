---
name: security-engagement
description: Plan and run an authorized security assessment of a web target — recon,
  attack-surface mapping, validation, impact, and a written finding. Use when asked
  to pentest a site, review an application's attack surface, reproduce a
  vulnerability, or turn notes into a report.
---

# Security engagement

One loop: **map → confirm → prove → write it down.** Everything below assumes the
target is yours or you have written permission to test it. Stop at the smallest
proof that establishes impact; a second request that changes state is a second
risk you did not need to take.

## Placeholders

Fill these from the task and carry them through every command. Never invent a
value you were not given — leave the placeholder and say which one is missing.

| Token | Meaning |
| --- | --- |
| `TARGET` | host or base URL under test |
| `DOMAIN` | registrable domain (recon scope) |
| `WORDLIST` | wordlist path for content discovery |
| `OAST` | out-of-band callback host you control |
| `ROLE_A` / `ROLE_B` | two accounts at different privilege levels |
| `RESULT_DIR` | where raw output and evidence go |

## 1. Map the outside

```bash
subfinder -d DOMAIN -all -silent > RESULT_DIR/subs.txt
httpx -l RESULT_DIR/subs.txt -sc -title -td -silent -o RESULT_DIR/live.txt
whatweb -a 3 TARGET | tee RESULT_DIR/fingerprint.txt
gau DOMAIN | anew RESULT_DIR/urls.txt
```

Read `live.txt` before anything else: a staging host, an admin panel on a
non-standard port, or a framework version is usually the whole engagement.

## 2. Map the application

```bash
ffuf -u TARGET/FUZZ -w WORDLIST -mc 200,204,301,302,403 -t 40 -rate 30 \
     -o RESULT_DIR/ffuf.json
grep -oE '"/?(api|v[0-9])/[a-zA-Z0-9_/?=&.-]+' RESULT_DIR/js/*.js \
  | sort -u > RESULT_DIR/endpoints.txt
```

Then walk the app as a user: log in as `ROLE_A`, click everything, and keep the
proxy history. The endpoints the JavaScript names are the ones the UI hides.

## 3. Confirm, one class at a time

| Class | First probe | Confirm with |
| --- | --- | --- |
| Injection | `' and 1=1 -- -` vs `' and 1=2 -- -` | boolean/delay difference, then `sqlmap -u URL --batch --level 3 --risk 2` |
| XSS | `<script>alert(1)</script>`, `<img src=x onerror=alert(1)>` | stored copy re-rendered for a second user |
| SSRF | `url=http://127.0.0.1:PORT`, `file:///etc/hostname`, `gopher://` | callback to `OAST` |
| Access control | same request as `ROLE_B`, and with the id incremented | 200 vs 403, or another user's object |
| Upload | double extension, wrong `Content-Type`, `.htaccess` | the uploaded file is served and parsed |
| Deserialization | framework-specific gadget header in the body | command output or DNS hit on `OAST` |

For blind classes, `OAST` is the evidence: a DNS or HTTP hit with the request id
in the subdomain is worth more than a timing guess.

## 4. Prove it without collateral

- One request that demonstrates the effect; no destructive payloads, no
  `DROP`, no writes outside a record you created.
- Save the exact request and response (`RESULT_DIR/evidence/<id>.txt`), plus a
  screenshot of the rendered result when the finding is visual.
- Re-run the proof once from a clean session, so the finding is reproducible and
  not a cached response.

## 5. Report

One finding per heading, in this order:

```
### [severity] Title — endpoint
Impact: what an attacker gains, in one sentence.
Reproduction: numbered steps, exact request, expected vs observed.
Evidence: file names under evidence/.
Fix: the specific change, not "validate input".
```

Severity follows demonstrated impact, not the class of bug: an admin-only stored
XSS is not critical because it is XSS. A finding you could not confirm goes in a
separate "unconfirmed" section with what would settle it.

## Output contract

Lead with the finding list and the count by severity, then the detail. Every
command you recommend must be runnable as written with the placeholders filled;
every claim points at evidence in `RESULT_DIR`.
