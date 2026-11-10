---
name: reverse-engineering
description: Take an unknown binary or app apart — triage, static analysis, dynamic
  tracing, patch and re-sign, then verify the change. Use when asked to reverse,
  crack, unpack, bypass a licence check, patch a binary or hook an app (IDA, Hopper,
  Ghidra, Frida, jadx, apktool, LLDB, otool).
---

# Reverse engineering

One loop: **identify → read → run → patch → prove.** Never patch before you can
name the function you are patching, and never claim a crack without running the
original check afterwards.

## Placeholders

| Token | Meaning |
| --- | --- |
| `SAMPLE` | binary, app bundle, APK, IPA or dylib under analysis |
| `BIN` | the Mach-O / ELF / PE inside it |
| `CHECK_FN` | the function that decides pass/fail |
| `OFFSET` / `PATCH_BYTE` | file offset and the bytes written there |
| `SERIAL` / `LICENSE_BLOB` | input the check consumes |
| `PACKAGE` | Android package id |

Work on a copy: `cp -a SAMPLE work/` and keep the original untouched.

## 1. Identify

```bash
file BIN; otool -hv BIN; otool -L BIN          # Mach-O
readelf -h BIN; readelf -d BIN                 # ELF
codesign -dvvv SAMPLE 2>&1                     # signature, entitlements
nm -m BIN | head -50; strings -a BIN | grep -iE 'licen|serial|trial|expire'
```

Packed? Look for high entropy sections, a tiny `__TEXT` and a fat `__DATA`, or
an entry point that jumps into a stub. Then unpack before reading further:
UPX (`upx -d`), or dump the process after it maps itself (`frida-dump`, or
`lldb` + `memory read` on the region that turns executable).

## 2. Read

| Target | Tool |
| --- | --- |
| Native, GUI | Hopper or IDA — xrefs on the strings from step 1 |
| Native, free | Ghidra headless: `analyzeHeadless /tmp/gh proj -import BIN` |
| Swift / ObjC | `class-dump`, Hopper's ObjC view; look for `isLicensed`, `validate` |
| Android | `apktool d SAMPLE`, `jadx -d out SAMPLE`; check `smali` for the check |
| JS / Electron | `asar extract app.asar out`, then read the main bundle |

Find `CHECK_FN` by following the failure path backwards: the string shown on a
bad serial is usually one or two calls from the branch that decides.

## 3. Run

```bash
# macOS / iOS, once the check is located
lldb -- BIN
(lldb) b CHECK_FN
(lldb) run SERIAL
(lldb) register read x0 x1; memory read --format s $x1   # args on arm64

# Cross-platform hook
frida -f SAMPLE -l hook.js --no-pause
```

```js
// hook.js — print the check's arguments and force its verdict
const addr = Module.findBaseAddress('BIN').add(OFFSET);
Interceptor.attach(addr, {
  onEnter(args) { console.log('arg0', args[0].readUtf8String()); },
  onLeave(ret) { console.log('ret', ret.toInt32()); ret.replace(ptr(1)); },
});
```

Android: push `frida-server` matching the ABI, `adb shell su -c ./frida-server &`,
then `frida -U -f PACKAGE -l hook.js`. For Java-level checks use
`Java.perform` and hook the class directly instead of the native layer.

## 4. Patch

- **File patch**: `printf '\x..\x..' | dd of=BIN bs=1 seek=OFFSET conv=notrunc`
  after mapping the virtual address to a file offset (`otool -l` `__TEXT` vmaddr
  and fileoff; `readelf -S` for ELF).
- **Branch flip**: turn the conditional into an unconditional pass
  (`b.eq` → `b`, `je` → `jmp`, or `nop` the call) — then check the registers the
  caller expects.
- **Re-sign (macOS)**: `codesign --force --deep --sign - SAMPLE` and strip the
  quarantine bit. On iOS the binary also needs the right entitlements and a
  valid `LC_CODE_SIGNATURE` slot.
- **Repack (Android)**: `apktool b`, `zipalign -f 4`, `apksigner sign --ks`,
  then reinstall. Patch `smali` before rebuilding, not the dex afterwards.

Keep `OFFSET`, the original bytes and the patched bytes in a note; a patch you
cannot undo is a patch you cannot verify.

## 5. Prove

Run the original check against `SERIAL` again and show the transition: the
failure output before, the success state after, same input, same machine. For a
UI check, screenshot both states. For a server-verified licence, say plainly that
the client check is bypassed and the server call still exists — that is the real
limit of the patch.

## Output contract

Lead with the mechanism in one paragraph: where the check lives, what it
compares, what the patch changes. Then the exact commands and offsets, the
verification transcript, and the files you produced under `work/`. Byte values
in hex, addresses as `0x…`, and every claim tied to a command that was run.
