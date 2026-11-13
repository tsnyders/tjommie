// The live stream fixture (spec 3.9): a page that repaints on every animation frame at 1280x800.
// Chromium's screencast emits a frame only when pixels change, so the counter and the background hue both change each frame.
export const STREAM_FIXTURE_PATH = "/stream-fixture";

export function streamFixtureHtml(): string {
  return `<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>Stream fixture</title>
<style>
html, body { margin: 0; width: 1280px; height: 800px; overflow: hidden; }
body { display: flex; align-items: center; justify-content: center; font: 160px monospace; color: #fff; }
</style>
</head>
<body><div id="counter">0</div>
<script>
let counter = 0;
const label = document.getElementById("counter");
function paint() {
  counter++;
  label.textContent = String(counter);
  document.body.style.background = "hsl(" + (counter % 360) + " 60% 40%)";
  requestAnimationFrame(paint);
}
requestAnimationFrame(paint);
</script>
</body>
</html>`;
}
