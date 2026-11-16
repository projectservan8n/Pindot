"""Builds the Pindot / Rusty logo files. Run: python3 build.py (then render with the logo-design skill's render_png.py)."""
import math, os
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen

PINK, INK, PAPER = "#FF2D95", "#15151B", "#F5F5F7"
R = 20                                   # corner softness
V = [(65, 65), (191, 109), (139, 139), (109, 191)]   # tip, right, inner notch, bottom

def body():
    n = len(V); norms = []; dirs = []
    for i in range(n):
        a, b = V[i], V[(i + 1) % n]; dx, dy = b[0] - a[0], b[1] - a[1]; l = math.hypot(dx, dy)
        dirs.append((dx / l, dy / l)); norms.append((dy / l, -dx / l))
    out = []
    for i in range(n):
        p, q = (i - 1) % n, i            # edge in, edge out at vertex i
        cross = dirs[p][0] * dirs[q][1] - dirs[p][1] * dirs[q][0]
        vx, vy = V[i]
        if cross > 0:                    # convex: round it
            a = (vx + R * norms[p][0], vy + R * norms[p][1]); b = (vx + R * norms[q][0], vy + R * norms[q][1])
            out.append(f"{'M' if not out else 'L'}{a[0]:.2f} {a[1]:.2f} A{R} {R} 0 0 1 {b[0]:.2f} {b[1]:.2f}")
        else:                            # the notch: where the two offset edges meet
            a = (vx + R * norms[p][0], vy + R * norms[p][1]); b = (vx + R * norms[q][0], vy + R * norms[q][1])
            t = ((b[0] - a[0]) * dirs[q][1] - (b[1] - a[1]) * dirs[q][0]) / cross
            out.append(f"L{a[0] + t * dirs[p][0]:.2f} {a[1] + t * dirs[p][1]:.2f}")
    return " ".join(out) + " Z"

def eye(x, y, w=16, h=28):
    r = w / 2
    return f"M{x - r} {y - h / 2 + r} a{r} {r} 0 0 1 {w} 0 v{h - w} a{r} {r} 0 0 1 -{w} 0 Z"

BODY = body()
RUSTY = f"{BODY} {eye(105, 119)} {eye(135, 119)}"

def mark(fill, d=RUSTY, extra=""):
    return f'<path fill="{fill}" fill-rule="evenodd" d="{d}"{extra}/>'

def svg(name, w, h, inner, title="Pindot logo"):
    open(name, "w").write(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}"><title>{title}</title>{inner}</svg>\n')

def wordmark(text="pindot", size=150, x0=262, base=178, track=-2):
    f = TTFont(os.path.expanduser("~/Library/Fonts/Poppins-SemiBold.ttf")); gs = f.getGlyphSet(); cmap = f.getBestCmap()
    s = size / f["head"].unitsPerEm; x = x0; parts = []
    for ch in text:
        g = cmap[ord(ch)]; pen = SVGPathPen(gs)
        gs[g].draw(TransformPen(pen, (s, 0, 0, -s, x, base)))
        parts.append(pen.getCommands()); x += gs[g].width * s + track
    return " ".join(parts), x - track

W, WEND = wordmark()
LW = round(WEND + 40)

for tag, c in (("black", "#000000"), ("white", "#FFFFFF"), ("pink", PINK)):
    svg(f"pindot-symbol-{tag}.svg", 256, 256, mark(c))
    svg(f"pindot-symbol-small-{tag}.svg", 256, 256, mark(c, BODY))
    svg(f"pindot-lockup-{tag}.svg", LW, 256, mark(c) + f'<path fill="{c}" d="{W}"/>')
svg("pindot-lockup-colour-on-light.svg", LW, 256, mark(PINK) + f'<path fill="{INK}" d="{W}"/>')
svg("pindot-lockup-colour-on-dark.svg", LW, 256, mark(PINK) + f'<path fill="{PAPER}" d="{W}"/>')

def tile(d, pad, rad, scale):
    off = 512 - 128 * scale
    return (f'<rect x="{pad}" y="{pad}" width="{1024 - 2 * pad}" height="{1024 - 2 * pad}" rx="{rad}" fill="{INK}"/>'
            f'<g transform="translate({off} {off}) scale({scale})">{mark(PINK, d)}</g>')
svg("pindot-app-icon.svg", 1024, 1024, tile(RUSTY, 100, 186, 2.75), "Pindot app icon")          # macOS: tile with margin
svg("pindot-app-icon-small.svg", 1024, 1024, tile(BODY, 100, 186, 2.9), "Pindot app icon")
svg("pindot-win-icon.svg", 1024, 1024, tile(RUSTY, 0, 220, 3.3), "Pindot icon")                 # Windows: tile fills the square
svg("pindot-win-icon-small.svg", 1024, 1024, tile(BODY, 0, 220, 3.5), "Pindot icon")
tr = lambda d: f'<g transform="translate(-41 -41) scale(1.32)">{mark("#000000", d)}</g>'
svg("pindot-tray.svg", 256, 256, tr(RUSTY), "Pindot tray icon")
svg("pindot-tray-plain.svg", 256, 256, tr(BODY), "Pindot tray icon")
print("lockup width", LW)
