"""生成 Windows 安装程序（NSIS）用的两张位图：左侧大图 sidebar.bmp（164x314）和顶部横幅 header.bmp（150x57）。

用法：python scripts/gen_nsis_images.py
- 图标取 docs/logo/v4/app-icon-1024.png（发布仓库里没有 docs/logo 时，改用 src-tauri/icons/icon.png）。
- 先按 4 倍尺寸绘制再缩小，文字边缘更平滑；输出 24 位 BMP（NSIS 要求）。
"""

from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "src-tauri" / "nsis"
BG = (245, 240, 230)  # 与图标底色一致
ACCENT = (232, 68, 28)  # 主题色朱红 #E8441C
SUB = (138, 128, 112)
SCALE = 4

FONT_TITLE = ["C:/Windows/Fonts/segoeuib.ttf", "/System/Library/Fonts/Supplemental/Arial Bold.ttf"]
FONT_SUB = ["C:/Windows/Fonts/msyh.ttc", "/System/Library/Fonts/PingFang.ttc"]


def font(candidates: list[str], size: int) -> ImageFont.FreeTypeFont:
    for f in candidates:
        if Path(f).exists():
            return ImageFont.truetype(f, size)
    return ImageFont.load_default(size)


def icon(size: int) -> Image.Image:
    for p in [ROOT / "docs/logo/v4/app-icon-1024.png", ROOT / "src-tauri/icons/icon.png"]:
        if p.exists():
            return Image.open(p).convert("RGBA").resize((size, size), Image.LANCZOS)
    raise SystemExit("找不到图标文件")


def centered(draw: ImageDraw.ImageDraw, text: str, f, y: int, width: int, fill) -> None:
    w = draw.textbbox((0, 0), text, font=f)[2]
    draw.text(((width - w) // 2, y), text, font=f, fill=fill)


def sidebar() -> None:
    w, h = 164 * SCALE, 314 * SCALE
    im = Image.new("RGBA", (w, h), BG + (255,))
    ic = icon(110 * SCALE)
    im.alpha_composite(ic, ((w - ic.width) // 2, 62 * SCALE))
    d = ImageDraw.Draw(im)
    centered(d, "Beetle", font(FONT_TITLE, 30 * SCALE), 186 * SCALE, w, ACCENT)
    centered(d, "飞书 AI 机器人", font(FONT_SUB, 11 * SCALE), 232 * SCALE, w, SUB)
    centered(d, "查 bug · 审 PR", font(FONT_SUB, 11 * SCALE), 250 * SCALE, w, SUB)
    im.convert("RGB").resize((164, 314), Image.LANCZOS).save(OUT / "sidebar.bmp")


def header() -> None:
    w, h = 150 * SCALE, 57 * SCALE
    im = Image.new("RGBA", (w, h), BG + (255,))
    d = ImageDraw.Draw(im)
    f = font(FONT_TITLE, 22 * SCALE)
    box = d.textbbox((0, 0), "Beetle", font=f)
    d.text((12 * SCALE, (h - (box[3] - box[1])) // 2 - box[1]), "Beetle", font=f, fill=ACCENT)
    ic = icon(44 * SCALE)
    im.alpha_composite(ic, (w - ic.width - 8 * SCALE, (h - ic.height) // 2))
    im.convert("RGB").resize((150, 57), Image.LANCZOS).save(OUT / "header.bmp")


if __name__ == "__main__":
    sidebar()
    header()
    print(f"已生成 {OUT / 'sidebar.bmp'}、{OUT / 'header.bmp'}")
