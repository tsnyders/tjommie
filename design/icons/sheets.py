"""Native-size review sheets. All icon resizing uses premultiplied Lanczos."""
from pathlib import Path
import argparse
from PIL import Image, ImageDraw, ImageFont

ROOT=Path(__file__).resolve().parent
SIZES=(18,24,40,96)
ROSTER=[('chief-of-staff','Chief of Staff'),('pnet-apply','Pnet apply'),
        ('cv-writer','CV Writer'),('inbox-triage','Inbox Triage'),
        ('night-shift','Night Shift'),('careers24-scout','Careers24 scout')]

def font(size,bold=False):
    return ImageFont.truetype('C:/Windows/Fonts/segoeuib.ttf' if bold else 'C:/Windows/Fonts/segoeui.ttf',size)

def resize(img,size):
    return img.convert('RGBa').resize((size,size),Image.Resampling.LANCZOS).convert('RGBA')

def paste_icon(canvas,img,x,y,size):
    small=resize(img,size)
    canvas.paste(small,(round(x-size/2),round(y-size/2)),small)

def grid(entries,destination,title,subtitle):
    # The 18/24/40/96 px samples below are their true pixel sizes in this PNG.
    width,panel,rowheight=1248,624,128
    height=176+len(entries)*rowheight+44
    sheet=Image.new('RGB',(width,height),'#E4E4E1')
    d=ImageDraw.Draw(sheet)
    d.text((28,18),title,font=font(28,True),fill='#202321')
    d.text((28,59),subtitle,font=font(14),fill='#555956')
    for j,(bg,fg,muted) in enumerate((('#111111','#F5F5F5','#AAAAAA'),('#F5F5F5','#202321','#656A67'))):
        x0=j*panel
        d.rectangle((x0,100,x0+panel-1,height-1),fill=bg)
        d.text((x0+26,119),'DARK RAIL  #111111' if j==0 else 'LIGHT RAIL  #F5F5F5',font=font(13,True),fill=fg)
        centers=[x0+244,x0+322,x0+408,x0+540]
        for s,x in zip(SIZES,centers):
            d.text((x,156),f'{s} px',anchor='mm',font=font(13),fill=muted)
        for i,(path,name) in enumerate(entries):
            y=176+i*rowheight+rowheight/2
            d.text((x0+26,y),name,anchor='lm',font=font(16,True),fill=fg)
            img=Image.open(path).convert('RGBA')
            for s,x in zip(SIZES,centers): paste_icon(sheet,img,x,y,s)
        d.text((x0+26,height-24),'Native pixel sizes  /  premultiplied-alpha Lanczos',font=font(12),fill=muted)
    destination.parent.mkdir(parents=True,exist_ok=True)
    sheet.save(destination)

if __name__=='__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('kind',choices=['heroes','final'])
    args=parser.parse_args()
    if args.kind=='heroes':
        grid([(ROOT/'explorations/folded-couriers/pnet-apply.png','Folded Couriers'),
              (ROOT/'explorations/workshop-totems/pnet-apply.png','Workshop Totems')],
             ROOT/'explorations/comparison.png','Grokbot / Two ways to get work done',
             'Pnet apply hero study  /  same role, different construction and metaphor')
    else:
        grid([(ROOT/'final'/f'{id}.png',name) for id,name in ROSTER],ROOT/'contact-sheet.png',
             'Grokbot / Workshop Totems','Six useful objects with an inner life  /  native-size legibility review')
