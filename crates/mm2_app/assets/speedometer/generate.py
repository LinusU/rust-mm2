"""Original HUD artwork. Run with Pillow; font path may be changed per platform."""
from pathlib import Path
from math import sin, cos, radians
from PIL import Image, ImageDraw, ImageFont
out=Path(__file__).resolve().parent
font='/System/Library/Fonts/Supplemental/Arial Bold.ttf'
im=Image.new('RGBA',(640,640)); d=ImageDraw.Draw(im)
d.ellipse((14,20,630,636),fill=(0,0,0,65))
d.ellipse((8,8,632,632),fill=(10,17,25,230),outline=(101,125,137,220),width=3)
d.ellipse((20,20,620,620),outline=(38,60,73,255),width=9)
d.arc((43,43,597,597),135,405,fill=(67,91,102,255),width=7)
d.arc((43,43,597,597),360,405,fill=(229,153,50,255),width=7)
for speed in range(0,301,10):
    a=radians(135+speed*.9)
    major=speed%20==0
    r1=274; r2=244 if major else 259
    color=(243,244,227,255) if speed<260 else (255,186,87,255)
    d.line((320+cos(a)*r1,320+sin(a)*r1,320+cos(a)*r2,320+sin(a)*r2),fill=color,width=7 if major else 3)
    if major:
        f=ImageFont.truetype(font,31)
        text=str(speed); b=d.textbbox((0,0),text,font=f)
        x=320+cos(a)*215-(b[2]-b[0])/2
        y=320+sin(a)*215-(b[3]-b[1])/2-b[1]
        d.text((x,y),text,font=f,fill=color)
def label(text,y,size,color):
    f=ImageFont.truetype(font,size); b=d.textbbox((0,0),text,font=f)
    d.text(((640-(b[2]-b[0]))/2,y-b[1]),text,font=f,fill=color)
label('KM/H',500,27,(155,187,197,255))
label('RPM',600,17,(113,140,151,255))
d.rounded_rectangle((226,425,414,491),radius=14,fill=(3,9,15,230),outline=(48,70,82,255),width=2)
d.ellipse((525,58,605,138),fill=(31,47,57,255),outline=(236,167,63,255),width=3)
d.rounded_rectangle((124,552,516,582),radius=6,fill=(2,8,13,255),outline=(58,80,91,255),width=2)
im.save(out/'dial.png')
needle=Image.new('RGBA',(640,640));d=ImageDraw.Draw(needle)
d.polygon([(313,360),(313,118),(320,67),(327,118),(327,360)],fill=(6,8,11,190))
d.polygon([(316,357),(316,116),(320,75),(324,116),(324,357)],fill=(255,105,29,255))
d.line((319,112,319,351),fill=(255,184,92,255),width=2)
d.ellipse((298,298,342,342),fill=(7,14,19,255),outline=(87,112,124,255),width=3)
d.ellipse((309,309,331,331),fill=(255,131,43,255))
needle.save(out/'needle.png')
