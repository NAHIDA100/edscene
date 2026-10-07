"""Fill the release package's ``sounds/`` with ASCII names.

The broadcast files the artist shipped are named after the events in Chinese
(``解锁返回桌面.mp3``). Those names are what the resolver matches by stem, so
they work locally — but a zip entry written in the system codepage comes out as
mojibake on a machine that isn't Chinese, and then nothing resolves. The
resolver also accepts each event's id, so the distributed pack uses those.

Run from the repository root::

    python packaging/windows/pack_sounds.py "D:\\works\\eds" "pkg\\edscene-0.1.0-windows-x64\\sounds"
"""

import os
import shutil
import sys

# (factory Chinese name, event id) — ids come from eds_core::event::EventKind.
PACK = [
    ("解锁返回桌面", "unlock"),
    ("开机进入桌面", "boot_desktop"),
    ("锁屏", "lock"),
    ("屏保恢复", "screensaver_restore"),
    ("断开电源（电源适配器）", "ac_lost"),
    ("电源恢复接入", "ac_restored"),
    ("低电量模式", "battery_saver"),
    ("电池电量低于5%", "battery_low"),
]


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    src, dst = sys.argv[1], sys.argv[2]
    os.makedirs(dst, exist_ok=True)
    for f in os.listdir(dst):
        os.remove(os.path.join(dst, f))
    for zh, ident in PACK:
        s = os.path.join(src, zh + ".mp3")
        if not os.path.isfile(s):
            print(f"缺少素材：{s}")
            return 1
        shutil.copy2(s, os.path.join(dst, ident + ".mp3"))
    print(f"{len(PACK)} 个素材已按事件 id 复制到 {dst}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
