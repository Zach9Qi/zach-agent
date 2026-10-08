"""生成公开、确定性的联调附件；需要 Pillow、reportlab 和本地中文字体。"""

import argparse
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.cidfonts import UnicodeCIDFont
from reportlab.pdfgen import canvas


def generate(font_path: str) -> None:
    """保持四份附件各自包含独立数值，避免提示词泄漏预期答案。"""
    root = Path(__file__).parent
    for label, value in [("a", 137), ("b", 263)]:
        image = Image.new("RGB", (640, 360), "#f4f7fb")
        draw = ImageDraw.Draw(image)
        draw.rounded_rectangle((28, 28, 612, 332), radius=20, fill="white", outline="#bcc9d8", width=2)
        draw.text((65, 65), f"图片 {label.upper()} 的记录", font=ImageFont.truetype(font_path, 34), fill="#23364d")
        draw.text((65, 140), str(value), font=ImageFont.truetype(font_path, 90), fill="#165acb")
        draw.text((65, 270), "请读取本卡片中的数值", font=ImageFont.truetype(font_path, 24), fill="#596b80")
        image.save(root / f"image-{label}.png")

    pdfmetrics.registerFont(UnicodeCIDFont("STSong-Light"))
    for label, value in [("a", 421), ("b", 89)]:
        document = canvas.Canvas(str(root / f"report-{label}.pdf"), pagesize=(595, 420), invariant=1)
        document.setTitle(f"联调报告 {label.upper()}")
        document.setFont("STSong-Light", 26)
        document.drawString(50, 340, f"报告 {label.upper()} 的记录")
        document.setFont("STSong-Light", 18)
        document.drawString(50, 280, "本报告提供一个独立数值，请与其他附件分别核对。")
        document.setFont("Helvetica-Bold", 64)
        document.drawString(50, 170, str(value))
        document.setFont("STSong-Light", 14)
        document.drawString(50, 85, "用途：真实 API 附件读取与跨附件汇总联调。")
        document.showPage()
        document.save()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--font", required=True, help="用于图片的中文 TrueType 字体路径")
    generate(parser.parse_args().font)
