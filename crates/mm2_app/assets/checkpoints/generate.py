from PIL import Image, ImageDraw, ImageFont
from pathlib import Path
out=Path(__file__).resolve().parent
font='/System/Library/Fonts/Supplemental/Arial Bold.ttf'
for label,filename,color in [('CHECKPOINT','checkpoint',(9,66,110)),('FINISH','finish',(20,103,70))]:
    im=Image.new('RGB',(2048,320)); d=ImageDraw.Draw(im)
    for y in range(320):
        factor=1.18-.38*y/320
        d.line((0,y,2048,y),fill=tuple(int(v*factor) for v in color))
    d.rectangle((10,10,2037,309),outline=(76,157,187),width=3)
    d.rectangle((27,27,2020,292),outline=(5,22,37),width=3)
    for x in [44,1995]:
        d.rectangle((x,35,x+9,284),fill=(230,72,46))
    f=ImageFont.truetype(font,213); box=d.textbbox((0,0),label,font=f)
    x=(2048-(box[2]-box[0]))/2; y=(320-(box[3]-box[1]))/2-box[1]
    d.text((x+4,y+7),label,font=f,fill=(3,17,30),stroke_width=3)
    d.text((x,y),label,font=f,fill=(247,251,253))
    im.save(out/(filename+'.png'))
im=Image.new('RGB',(256,1024)); d=ImageDraw.Draw(im)
for x in range(256):
    d.line((x,0,x,1024),fill=(int(145+35*(1-abs(x-128)/128)),15,35))
d.rectangle((7,7,248,1016),outline=(238,184,76),width=5)
f=ImageFont.truetype(font,111)
for i,c in enumerate('MIDTOWN'):
    box=d.textbbox((0,0),c,font=f); x=(256-(box[2]-box[0]))/2
    y=64+i*128-box[1]
    d.text((x+2,y+4),c,font=f,fill=(70,5,15))
    d.text((x,y),c,font=f,fill=(250,249,237))
im.save(out/'tower.png')
