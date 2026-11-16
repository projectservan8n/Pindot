#!/usr/bin/env python3
"""Stitch frame strips from overlay-strips.sh into one labelled sheet.
usage: strips.py <dir> <out.png> scene[:crop=x,y,w,h] ...   (crop in CSS px, default whole frame)
"""
import sys, os, glob
from PIL import Image, ImageDraw, ImageFont
d, out = sys.argv[1], sys.argv[2]
def font(sz):
    for p in ['/System/Library/Fonts/Supplemental/Arial Bold.ttf', '/System/Library/Fonts/Helvetica.ttc']:
        if os.path.exists(p): return ImageFont.truetype(p, sz)
    return ImageFont.load_default()
rows = []
for spec in sys.argv[3:]:
    name, _, crop = spec.partition(':')
    files = sorted(glob.glob(f'{d}/{name}-[0-9][0-9].png')) or [f'{d}/{name}.png']
    ims = [Image.open(f).convert('RGBA') for f in files]
    s = ims[0].width / 520
    if crop:
        x, y, w, h = [int(float(v) * s) for v in crop.replace('crop=', '').split(',')]
        ims = [i.crop((x, y, x + w, y + h)) for i in ims]
    rows.append((name, ims))
cw = min(2400 // max(len(i) for _, i in rows) - 6, 300)
H = 0; sized = []
for name, ims in rows:
    k = cw / ims[0].width
    sized.append((name, [i.resize((cw, int(i.height * k)), Image.LANCZOS) for i in ims])); H += int(ims[0].height * k) + 30
W = max(len(i) for _, i in sized) * (cw + 6) + 6
sheet = Image.new('RGB', (W, H + 6), (21, 21, 27)); dr = ImageDraw.Draw(sheet); y = 4
for name, ims in sized:
    dr.text((8, y), name, font=font(18), fill=(236, 236, 242)); y += 26
    for j, i in enumerate(ims):
        bg = Image.new('RGBA', i.size, (138, 143, 152, 255)); bg.alpha_composite(i)
        sheet.paste(bg.convert('RGB'), (6 + j * (cw + 6), y))
    y += ims[0].height + 4
sheet.save(out); print(out, sheet.size)
