"""Run one user-requested Nano Banana image job with a verbatim prompt file."""
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent
AGY = Path(r"C:\Users\tyrel\AppData\Local\agy\bin\agy.exe")

def main():
    prompt_path = (ROOT / sys.argv[1]).resolve()
    output_path = (ROOT / sys.argv[2]).resolve()
    if not prompt_path.is_relative_to(ROOT) or not output_path.is_relative_to(ROOT):
        raise SystemExit("All job paths must be inside design/icons")
    if output_path.exists():
        raise SystemExit(f"Refusing to overwrite existing generation: {output_path}")
    output_path.parent.mkdir(parents=True, exist_ok=True)
    log_path = ROOT / "logs" / (prompt_path.stem + ".log")
    log_path.parent.mkdir(parents=True, exist_ok=True)
    prompt = prompt_path.read_text(encoding="utf-8").strip()
    started = time.time()
    print(f"Generating {output_path.relative_to(ROOT)}", flush=True)
    with log_path.open("w", encoding="utf-8") as log:
        result = subprocess.run(
            [str(AGY), "--dangerously-skip-permissions", "-p", prompt],
            cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
            encoding="utf-8", errors="replace", check=False,
        )
    response = log_path.read_text(encoding="utf-8")
    print(response, flush=True)
    print(f"Exit code: {result.returncode}; elapsed: {time.time()-started:.1f}s", flush=True)
    if result.returncode != 0:
        raise SystemExit(result.returncode)
    if not output_path.is_file():
        raise SystemExit(f"AGY OUTPUT MISSING: {output_path}")
    from PIL import Image
    with Image.open(output_path) as img:
        img.verify()
    print(f"Verified image file: {output_path}", flush=True)

if __name__ == "__main__":
    main()
