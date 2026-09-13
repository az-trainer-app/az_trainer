"""
Embed artwork into a game config as base64, so configs are self-contained.

    python embed_art.py <image> <config.js>

Replaces any existing `export const art` in the config. Keeps the whole thing
portable: no Steam install, no appid, no network at runtime.

Anything the trainer can decode works (jpg/png). Aim for roughly a 300x140
image -- the header renders at 138x65, so more than ~2x that is wasted bytes
in a text file.
"""

import base64
import os
import re
import sys


def main():
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    img_path, cfg_path = sys.argv[1], sys.argv[2]

    if not os.path.isfile(img_path):
        print(f"! no such image: {img_path}")
        return 1
    if not os.path.isfile(cfg_path):
        print(f"! no such config: {cfg_path}")
        return 1

    raw = open(img_path, "rb").read()
    if raw[:3] == b"\xff\xd8\xff":
        mime = "image/jpeg"
    elif raw[:4] == b"\x89PNG":
        mime = "image/png"
    else:
        print("! not a jpg or png")
        return 1

    b64 = base64.b64encode(raw).decode("ascii")
    line = f"export const art = 'data:{mime};base64,{b64}';\n"

    src = open(cfg_path, encoding="utf-8").read()
    if re.search(r"^export const art = ", src, re.M):
        src = re.sub(r"^export const art = .*\n", line, src, count=1, flags=re.M)
        action = "replaced"
    else:
        # place it right after the title export, or after the process export
        anchor = re.search(r"^export const (?:title|process) = .*\n", src, re.M)
        if not anchor:
            print("! config has no `export const process`/`title` to anchor to")
            return 1
        at = anchor.end()
        src = src[:at] + line + src[at:]
        action = "added"

    open(cfg_path, "w", encoding="utf-8").write(src)
    print(f"{action} art: {len(raw):,} bytes -> {len(b64):,} base64 chars")
    print(f"  {os.path.basename(img_path)} -> {os.path.basename(cfg_path)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
