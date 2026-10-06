---
name: yt-dlp
description: Download videos or playlists with yt-dlp; fetch subtitles/captions, extract audio, choose formats or quality, and troubleshoot access restrictions. Use when asked to save online media, transcripts, or audio.
---

# yt-dlp media workflow

Use `yt-dlp` for media the user is authorized to access. Work in the user's requested output directory; otherwise ask where to save if ambiguous, or use the current working directory when appropriate. Quote URLs and paths. Never print cookies, tokens, or signed media URLs in the response.

## Before downloading

1. Check `command -v yt-dlp; yt-dlp --version; command -v ffmpeg; command -v ffprobe`. `ffmpeg`/`ffprobe` are needed for merging, remuxing, subtitle conversion and audio extraction. If unavailable, explain the limitation rather than promising a merged or converted file.
2. Clarify only missing consequential choices (single video versus whole playlist, output destination, format or language) if they cannot be inferred. A URL with a playlist parameter is **not** consent to download the entire playlist; use `--no-playlist` unless a playlist was requested.
3. Inspect availability when necessary: `yt-dlp --no-playlist -F 'URL'` (formats), `yt-dlp --no-playlist --list-subs 'URL'` (manual and auto subtitles). For a playlist preview use `yt-dlp --flat-playlist --print '%(playlist_index)s %(title)s [%(id)s]' 'URL'`; large playlists can have substantial size and duration. Avoid dumping excessive listings into the reply.
4. Use `-P 'DIR' -o '%(title).180B [%(id)s].%(ext)s'` for single items; for playlists use `-o '%(playlist_index)03d - %(title).180B [%(id)s].%(ext)s'` and `--download-archive 'DIR/downloaded.txt'` for resumable repeated runs. Preserve existing files unless the user explicitly wants replacement. Shell-quote all interpolated values, never build executable shell text out of untrusted video titles.

## Recipes (replace URL and DIR; run only what the user asked)

| Goal | yt-dlp arguments to add after `-P 'DIR'` and before `'URL'` |
|---|---|
| Single video, best available | `--no-playlist -f 'bv*+ba/b' -o '%(title).180B [%(id)s].%(ext)s'` |
| Entire playlist | `--yes-playlist --download-archive 'DIR/downloaded.txt' -o '%(playlist_index)03d - %(title).180B [%(id)s].%(ext)s'` |
| Specific playlist entries | `--yes-playlist -I '1:5,10'` (combine with playlist output template) |
| At most 1080p | `--no-playlist -f 'bv*[height<=1080]+ba/b[height<=1080]'` (inspect `-F` if no matching format; fallback may differ in quality) |
| Prefer MP4-compatible streams | `--no-playlist -f 'bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b' --merge-output-format mp4` (fallback may not be MP4 if merging unsupported codecs; verify actual file) |
| Exact listed format ID | `--no-playlist -f 'VIDEO_ID+AUDIO_ID'` or `-f 'FORMAT_ID'` from `-F` |
| Best available audio, no re-encode | `--no-playlist -f 'ba/b'` (saves source audio or fallback container, not necessarily MP3) |
| MP3 audio | `--no-playlist -x --audio-format mp3 --audio-quality 0` (transcoding is lossy; do not imply improved source quality) |
| Lossless conversion | `--no-playlist -x --audio-format flac` (conversion cannot restore lost quality) |
| Manual subtitle only, no media | `--no-playlist --skip-download --write-subs --sub-langs 'en.*' --sub-format 'srt/best'` |
| Auto captions only, no media | `--no-playlist --skip-download --write-auto-subs --sub-langs 'en.*' --sub-format 'srt/best'` |
| Prefer manual, use auto if needed | Check `--list-subs` first; use `--write-subs --write-auto-subs` only if both are wanted or after verifying no manual subtitles match. |
| Convert captions to SRT | Add `--convert-subs srt` (needs ffmpeg); `--sub-format srt/best` alone does **not** convert formats. |
| Include captions in video | Add `--write-subs --sub-langs 'en.*' --embed-subs` (supported containers: mp4/webm/mkv; availability varies). |

Use `-S 'res:720'` to **prefer** a resolution (not enforce a ceiling); use `-f` filters to impose ceilings. `--merge-output-format mp4` affects merging, not arbitrary transcoding; `--remux-video mp4` only remuxes compatible streams. Never assume subtitles exist, translations are available, or every platform offers the requested resolution. For language selection inspect the exact tags in `--list-subs` first. To download subtitles across a playlist, replace `--no-playlist` with `--yes-playlist`, add the playlist filename template and optionally the archive; archive entries may skip already downloaded videos, so avoid an existing archive when fetching newly requested sidecars.

## Access problems and restrictions

- Read the actual error first; if extractor behavior appears outdated, check installed version and official yt-dlp updates appropriate to the package manager. Retry inspection with `-v` only if needed, redact sensitive headers/URLs before quoting logs. Slow down rather than evade rate limits: `--sleep-requests 1 --sleep-interval 5 --max-sleep-interval 10`; obey site policies.
- For age-gated/login-required media the user is entitled to access, prefer `--cookies-from-browser firefox` (or the browser they identify), or `--cookies 'PRIVATE_COOKIES_FILE'` when they provide it. Ask before accessing browser cookies; do not request passwords, copy cookie contents into chat, or store secrets in the download folder. Browser profile/keyring support varies.
- For region-specific content, `--proxy 'http://host:port'` routes traffic via a **user-provided, authorized** proxy. `--geo-verification-proxy` changes verification only; it is not a general download proxy. No guarantee of access; comply with licensing and applicable restrictions. If request failures depend on client fingerprints, inspect `yt-dlp --list-impersonate-targets` and try `--impersonate CLIENT` only if supported. Site-specific `--extractor-args` need the extractor's documented syntax and should be tried only in response to an identified issue.
- Do not claim yt-dlp can decrypt DRM, bypass payment/access controls, or guarantee access to private/deleted/unavailable media. Do not attempt to obtain other people's credentials or circumvent controls when authorization is absent. Explain the limitation and suggest official access or a non-DRM source supplied by the user.

## Finish

Check the command's exit status, inspect actual created files (`find 'DIR' -maxdepth 1 -type f` or targeted `ls`) and, if relevant, `ffprobe` for streams/duration. Report file paths, actual format/quality and missing items or failures; do not call a run successful based solely on yt-dlp progress output.
