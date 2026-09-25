"""Extract flat magenta key fields, recover edge alpha and remove key spill.

Pillow + NumPy only. Source pixels are preserved away from the edge band.
"""
from pathlib import Path
import argparse
import json

import numpy as np
from PIL import Image, ImageFilter

def extract(source: Path, destination: Path):
    img = Image.open(source).convert("RGB")
    rgb = np.asarray(img).astype(np.float32)
    # Estimate actual generated key color from all four corners.
    corners = np.concatenate([rgb[:24,:24].reshape(-1,3), rgb[:24,-24:].reshape(-1,3),
                              rgb[-24:,:24].reshape(-1,3), rgb[-24:,-24:].reshape(-1,3)])
    key = np.median(corners, axis=0)
    r, g, b = rgb[:,:,0], rgb[:,:,1], rgb[:,:,2]
    key_like = (r-g > 70) & (b-g > 70) & (r > 170) & (b > 140)
    rough = Image.fromarray((~key_like).astype(np.uint8)*255)
    core = np.asarray(rough.filter(ImageFilter.MinFilter(7))) > 0
    reach = np.asarray(rough.filter(ImageFilter.MaxFilter(9))) > 0
    # March nearby solid object colors through the antialias band. These give
    # a local foreground estimate for solving C = alpha*F + (1-alpha)*key.
    colors = rgb.copy()
    known = core.copy()
    for _ in range(14):
        sums = np.zeros_like(rgb)
        counts = np.zeros(known.shape, dtype=np.float32)
        for dy, dx in ((-1,0),(1,0),(0,-1),(0,1),(-1,-1),(-1,1),(1,-1),(1,1)):
            ys = slice(max(0,dy), rgb.shape[0]+min(0,dy))
            xs = slice(max(0,dx), rgb.shape[1]+min(0,dx))
            yn = slice(max(0,-dy), rgb.shape[0]+min(0,-dy))
            xn = slice(max(0,-dx), rgb.shape[1]+min(0,-dx))
            valid = known[yn,xn]
            sums[ys,xs] += colors[yn,xn]*valid[:,:,None]
            counts[ys,xs] += valid
        take = ~known & reach & (counts > 0)
        if not np.any(take):
            break
        colors[take] = sums[take]/counts[take,None]
        known[take] = True
    vector = colors-key
    denom = np.maximum(np.sum(vector*vector,axis=2),1)
    alpha = np.clip(np.sum((rgb-key)*vector,axis=2)/denom,0,1)
    alpha[core] = 1
    alpha[~reach] = 0
    alpha[alpha < 0.015] = 0
    alpha[alpha > 0.99] = 1
    recovered = np.clip((rgb-key*(1-alpha[:,:,None]))/np.maximum(alpha[:,:,None],0.001),0,255)
    recovered[core] = rgb[core]
    # At very low coverage unmixing amplifies quantization noise; the nearest
    # solid color is a better edge estimate than a pink or black fringe.
    tiny = (alpha>0) & (alpha<0.12)
    recovered[tiny] = colors[tiny]
    recovered[alpha==0] = 0
    rgba = np.dstack((np.rint(recovered).astype(np.uint8), np.rint(alpha*255).astype(np.uint8)))
    out = Image.fromarray(rgba)
    destination.parent.mkdir(parents=True,exist_ok=True)
    out.save(destination)
    return {"source":str(source),"size":list(img.size),"key":key.tolist(),
            "alpha_bbox":out.getbbox(),"transparent_pixels":int((alpha==0).sum()),
            "partial_alpha_pixels":int(((alpha>0)&(alpha<1)).sum())}

if __name__ == "__main__":
    parser=argparse.ArgumentParser()
    parser.add_argument("source",type=Path)
    parser.add_argument("destination",type=Path)
    args=parser.parse_args()
    print(json.dumps(extract(args.source,args.destination),indent=2))
