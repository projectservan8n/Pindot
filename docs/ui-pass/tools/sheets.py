#!/usr/bin/env python3
"""Contact sheets from docs/ui-pass screenshots. Usage: sheets.py <out dir>"""
import sys, os
from PIL import Image, ImageDraw, ImageFont
here = os.path.dirname(os.path.abspath(__file__)); ui = os.path.join(here, '..')
out = sys.argv[1]; os.makedirs(out, exist_ok=True)
FONT = os.path.join(ui, '../../app/src/fonts')
def font(sz):
    for p in ['/System/Library/Fonts/Supplemental/Arial Bold.ttf', '/System/Library/Fonts/Helvetica.ttc']:
        if os.path.exists(p): return ImageFont.truetype(p, sz)
    return ImageFont.load_default()
BG=(21,21,27); FG=(236,236,242); MUTED=(169,169,186); PINK=(255,45,149)
def sheet(items, cols, scale, path, title, cell_label_h=34, pad=16, head=64):
    w = int(820*scale); h = int(640*scale)
    rows = (len(items)+cols-1)//cols
    W = pad + cols*(w+pad); H = head + rows*(h+cell_label_h+pad) + pad
    im = Image.new('RGB', (W,H), BG); d = ImageDraw.Draw(im)
    d.text((pad,18), title, font=font(30), fill=FG)
    for i,(label,file) in enumerate(items):
        r,c = divmod(i,cols); x = pad + c*(w+pad); y = head + r*(h+cell_label_h+pad)
        col = PINK if label.startswith('NEW') else MUTED if label.startswith('BEFORE') else FG
        d.text((x,y+4), label, font=font(20), fill=col)
        s = Image.open(file).convert('RGB').resize((w,h), Image.LANCZOS)
        im.paste(s,(x,y+cell_label_h))
    assert W < 2400, W
    im.save(path); print(path, im.size)
A=os.path.join(ui,'after'); D=os.path.join(ui,'design/dark'); L=os.path.join(ui,'design/light')
names=[('chat-empty','Chat, empty'),('chat-conversation','Chat, conversation'),('chat-steps-expanded','Chat, steps open'),('chat-approval','Chat, approval'),
 ('chat-attachment','Chat, attachments'),('chat-error','Chat, error'),('chat-history','Chat, history'),('chat-not-ready','Chat, not ready'),
 ('activity','Activity'),('settings','Settings'),('setup-first-run','Setup'),('vault-list','Vault, list'),('vault-add','Vault, add'),
 ('vault-import-result','Vault, import result'),('vault-login-approval','Vault, login approval'),('vault-empty','Vault, empty')]
items=[]
for n,t in names:
    items += [('BEFORE (vault branch): '+t, f'{A}/{n}.png'), ('NEW: '+t, f'{D}/{n}.png')]
sheet(items, 4, 0.7, f'{out}/pindot-design-before-after.png', 'Pindot main window, dark. Left of each pair: before. Right: new design.')
light=[('NEW light: '+t, f'{L}/{n}.png') for n,t in names+[('settings-no-brain','Settings, no folder'),('vault-history','Vault, history'),('vault-import','Vault, import'),('activity-empty','Activity, empty')]]
sheet(light, 4, 0.7, f'{out}/pindot-design-light.png', 'Pindot main window, light theme')
cs=[('chat-conversation','Conversation'),('chat-steps-expanded','Steps'),('chat-approval','Approval'),('chat-attachment','Attachments'),('chat-error','Error and rejected file'),
 ('chat-dictation-listening','Dictation: listening'),('chat-dictation-transcribing','Dictation: transcribing'),('chat-dictation-error','Dictation: error'),('chat-dictation-done','Dictation: text inserted, not sent'),
 ('chat-voice-off','Chat with voice off'),('chat-empty','Empty'),('chat-not-ready','Not ready')]
items=[('NEW dark: '+t, f'{D}/{n}.png') for n,t in cs]+[('NEW light: '+t, f'{L}/{n}.png') for n,t in cs[5:9]]
sheet(items, 4, 0.7, f'{out}/pindot-design-chat-states.png', 'Pindot Chat states, including dictation')
