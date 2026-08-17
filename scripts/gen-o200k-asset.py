#!/usr/bin/env python3
"""Regenerate assets/o200k_base.cbpe from the canonical o200k_base.tiktoken.

The `tokenizer` module embeds the o200k_base BPE vocabulary as a compact
length-prefixed binary (`assets/o200k_base.cbpe`) instead of tiktoken's base64
text format, so the binary ships 1.7 MB rather than 3.6 MB and the loader can
point straight into the embedded bytes with no per-token heap allocations
(docs/tokenizer.md). This script converts the canonical text format into that
binary. Run it only when the vocabulary itself changes (it never has — the
encoding is frozen); the committed asset is the build input.

Usage:
    python3 scripts/gen-o200k-asset.py path/to/o200k_base.tiktoken

The canonical input is OpenAI tiktoken's published `o200k_base.tiktoken`
(MIT-licensed), as bundled by the `tiktoken-rs` crate:
    sha256 446a9538cb6c348e3516120d7c08b09f57c36495e2acfffe59a5bf8b0cfb1a2d

Input format:  one `{base64(token_bytes)} {rank}` pair per line, ranks dense
               and ascending (rank == line index) — asserted below.
Output format: for each rank in order, `[len: u8][token bytes: len]`. Rank is
               implicit in the order; the max token is 129 bytes, so a u8
               length always fits (asserted below).

`src/tokenizer.rs`'s differential tests verify the committed asset against
tiktoken-rs rank-by-rank and count-by-count, so a bad regeneration cannot land
silently.
"""

import base64
import hashlib
import sys
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    source = Path(sys.argv[1])
    out_path = Path(__file__).resolve().parent.parent / "assets" / "o200k_base.cbpe"

    text = source.read_bytes()
    print(f"input:  {source} (sha256 {hashlib.sha256(text).hexdigest()})")

    out = bytearray()
    for index, line in enumerate(text.decode("ascii").splitlines()):
        b64, rank = line.split(" ")
        assert int(rank) == index, f"ranks not dense at line {index}: {rank}"
        token = base64.b64decode(b64)
        assert 0 < len(token) < 256, f"token length {len(token)} at rank {index}"
        out.append(len(token))
        out += token

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_bytes(out)
    print(f"output: {out_path} ({len(out)} bytes, sha256 {hashlib.sha256(out).hexdigest()})")


if __name__ == "__main__":
    main()
