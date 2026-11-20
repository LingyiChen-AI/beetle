"""生成 macOS 用的 src-tauri/icons/icon.icns。

用法：python scripts/gen_macos_icon.py

macOS 的 app 图标有固定网格：1024 画布里主体只占居中的 824x824（四边各留 100 透明边），
不是铺满画布。量过 Calculator / Notes / Mail / Reminders，都是 80.5%。图标铺满画布的话，
Dock、启动台、Command+Tab 切换器里就会比别家的 app 明显大一圈——Beetle 之前就是这样。

只改 icns。Windows 的 .ico 和 Linux 的 png 按各自平台惯例保持满幅，不要套这个网格。

依赖 rsvg-convert（brew install librsvg）把 SVG 渲染成位图；没有就报错退出，
别用低分辨率 PNG 放大顶替。
"""

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SVG = ROOT / "src" / "assets" / "beetle-8.svg"
OUT = ROOT / "src-tauri" / "icons" / "icon.icns"

CANVAS = 1024
BODY = 824  # Apple 的 macOS 图标网格：主体边长 = 画布的 80.5%
# iconutil 认的文件名与边长
VARIANTS = [
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
]


def render_body(dst: Path) -> Image.Image:
    """把 SVG 渲染成 BODY x BODY 的位图。"""
    rsvg = shutil.which("rsvg-convert")
    if not rsvg:
        sys.exit("需要 rsvg-convert：brew install librsvg")
    if not SVG.is_file():
        sys.exit(f"找不到图标源文件 {SVG}")
    subprocess.run(
        [rsvg, "-w", str(BODY), "-h", str(BODY), "-o", str(dst), str(SVG)],
        check=True,
    )
    return Image.open(dst).convert("RGBA")


def main() -> None:
    if sys.platform != "darwin":
        sys.exit("iconutil 只有 macOS 上有，请在 macOS 上跑这个脚本")
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        body = render_body(tmp / "body.png")
        full = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
        off = (CANVAS - BODY) // 2
        full.paste(body, (off, off), body)

        iconset = tmp / "icon.iconset"
        iconset.mkdir()
        for name, size in VARIANTS:
            full.resize((size, size), Image.LANCZOS).save(iconset / name)

        OUT.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["iconutil", "-c", "icns", "-o", str(OUT), str(iconset)], check=True)
    print(f"已生成 {OUT.relative_to(ROOT)}（主体 {BODY}/{CANVAS} = {BODY / CANVAS:.1%}）")


if __name__ == "__main__":
    main()
