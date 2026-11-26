"""Frame-time stats for one PresentMon run (plan: "Measurement method").

    python -I tools/perf/frame-stats.py [--process <exe name>] <presentmon.csv> <markers.csv> [<trimmed.csv> [<trace.csv> [<cpu.csv> [<windows.csv>]]]]
    python -I tools/perf/frame-stats.py --self-test

Takes tjommie-desktop.exe (the default; --process names another) presents whose whole interval lies inside the scenario window
[start_qpc, end_qpc] written by the app, and reports msBetweenPresents p50 / p95 / max with
nearest-rank percentiles (every reported number is a frame that really happened).
Writes the in-window rows, trimmed to the useful columns, to <trimmed.csv> when given.
With the app's --trace CSV it also reports the app's own CPU work per frame (frame start to the
end of paint) and how late frames started; with a typeperf CSV, the system CPU load during the run.
With a windows CSV (start_qpc,end_qpc per motion), only presents inside a motion count: the idle
gaps between motion loops are not frame times. The app's own frame intervals (render start to
render start, from the trace) are reported for the same windows: PresentMon cannot resolve GPUI's
presents while the WebView2 child window is moved every frame (the panel motion), so those runs
are marked presentmon_unusable and judged on the app's intervals.
"""

import csv
import io
import json
import math
import sys

PROCESS = "tjommie-desktop.exe"  # the app's process name; --process overrides it
KEEP = ["QPCTime", "msBetweenPresents", "msBetweenDisplayChange", "msInPresentAPI", "msGPUActive", "Dropped", "PresentMode"]


def nearest_rank(sorted_values, q):
    return sorted_values[max(0, math.ceil(q * len(sorted_values)) - 1)]


def summarize(values):
    s = sorted(values)
    return {"n": len(s), "p50": nearest_rank(s, 0.50), "p95": nearest_rank(s, 0.95), "max": s[-1]}


def in_window(rows, start, end, freq, incomplete=None, process=None):
    process = (process or PROCESS).lower()
    for row in rows:
        if row["Application"].lower() != process:
            continue
        if not row.get("QPCTime") or not row.get("msBetweenPresents"):
            if incomplete is not None:
                incomplete.append(row)  # a line cut off when the capture stopped
            continue
        present = int(row["QPCTime"])
        began = present - float(row["msBetweenPresents"]) * freq / 1000
        if began >= start and present <= end:
            yield row


def inside_windows(rows, windows, freq):
    for row in rows:
        present = int(row["QPCTime"])
        began = present - float(row["msBetweenPresents"]) * freq / 1000
        if any(start <= began and present <= end for start, end in windows):
            yield row


def present_stats(rows):
    """msBetweenPresents stats. When PresentMon loses track of GPUI's presents it writes them at the end
    of the session, out of order and 0 ms apart: more than 1% such rows makes the capture unusable,
    and a stray one is dropped."""
    zero = sum(float(r["msBetweenPresents"]) <= 0 for r in rows)
    if not rows or zero * 100 > len(rows):
        return {"n": len(rows), "p50": None, "p95": None, "max": None, "presentmon_unusable": True, "presentmon_zero_rows": zero}
    result = summarize([float(r["msBetweenPresents"]) for r in rows if float(r["msBetweenPresents"]) > 0])
    result.update(presentmon_unusable=False, presentmon_zero_rows=zero)
    return result


def app_intervals(frames, windows, freq):
    """Render-start intervals (ms) where both frames fall inside the same motion window."""
    out = []
    for (a, _), (b, _) in zip(frames, frames[1:]):
        if any(start <= a and b <= end for start, end in windows):
            out.append((b - a) * 1000 / freq)
    return out


def trace_stats(trace_csv, freq):
    with open(trace_csv, newline="", encoding="utf-8-sig") as f:
        frames = [(int(r["begin_qpc"]), int(r["painted_qpc"])) for r in csv.DictReader(f)]
    if len(frames) < 2:
        sys.exit("no frames in the trace: the spike drew nothing (was its window covered or minimized?)")
    work = [(painted - begin) * 1000 / freq for begin, painted in frames if painted > begin]
    starts = [(b - a) * 1000 / freq for (a, _), (b, _) in zip(frames, frames[1:])]
    return {"work_p50": nearest_rank(sorted(work), 0.5), "work_p95": nearest_rank(sorted(work), 0.95), "work_max": max(work),
            "start_gap_p95": nearest_rank(sorted(starts), 0.95), "start_gap_max": max(starts)}


def cpu_values(text):
    """typeperf samples. The first is blank, and the last line is cut off when typeperf is stopped mid-write."""
    if not text.endswith("\n"):
        text = text[: text.rfind("\n") + 1]
    return [float(row[1]) for row in list(csv.reader(io.StringIO(text)))[1:] if len(row) > 1 and row[1].strip(' "')]


def cpu_stats(cpu_csv):
    with open(cpu_csv, newline="", encoding="utf-8-sig") as f:
        values = cpu_values(f.read())
    return {"cpu_mean": round(sum(values) / len(values), 1), "cpu_max": round(max(values), 1)} if values else {}


def main(pm_csv, markers_csv, trimmed=None, trace_csv=None, cpu_csv=None, windows_csv=None, process=None):
    with open(markers_csv, newline="", encoding="utf-8-sig") as f:
        marker = next(csv.DictReader(f))
    start, end, freq = int(marker["start_qpc"]), int(marker["end_qpc"]), int(marker["qpc_freq"])
    incomplete = []
    try:
        with open(pm_csv, newline="", encoding="utf-8-sig") as f:
            rows = list(in_window(csv.DictReader(f), start, end, freq, incomplete, process))
    except FileNotFoundError:
        rows = []  # PresentMon wrote nothing for this run; the app's own intervals still count
    windows = None
    if windows_csv:
        with open(windows_csv, newline="", encoding="utf-8-sig") as f:
            windows = [(int(r["start_qpc"]), int(r["end_qpc"])) for r in csv.DictReader(f)]
        rows = list(inside_windows(rows, windows, freq))
    result = present_stats(rows)
    result["dropped"] = sum(r["Dropped"] == "1" for r in rows)
    result["seconds"] = round((end - start) / freq, 3)
    result["app_frames"] = int(marker["frames"])
    result["incomplete_rows"] = len(incomplete)
    if trace_csv:
        result.update({k: round(v, 3) for k, v in trace_stats(trace_csv, freq).items()})
        if windows or trace_csv:
            with open(trace_csv, newline="", encoding="utf-8-sig") as f:
                frames = [(int(r["begin_qpc"]), int(r["painted_qpc"])) for r in csv.DictReader(f)]
            # No windows file (time-based scenarios, the scroll): the whole scenario is one window.
            gaps = sorted(app_intervals(frames, windows or [(start, end)], freq))
            if gaps:
                result.update({"app_interval_n": len(gaps), "app_interval_p50": round(nearest_rank(gaps, 0.5), 3),
                               "app_interval_p95": round(nearest_rank(gaps, 0.95), 3), "app_interval_max": round(gaps[-1], 3)})
    # What the verdict rests on: PresentMon, or the app's own intervals when PresentMon lost too many presents.
    judged = "app" if result["presentmon_unusable"] else "presentmon"
    result["judged_on"] = judged
    result["judged_p95"] = result.get("app_interval_p95") if judged == "app" else result["p95"]
    if cpu_csv:
        result.update(cpu_stats(cpu_csv))
    if trimmed:
        with open(trimmed, "w", newline="") as f:
            writer = csv.DictWriter(f, KEEP, extrasaction="ignore")
            writer.writeheader()
            writer.writerows(rows)
    print(json.dumps(result))


def self_test():
    assert nearest_rank([1, 2, 3, 4], 0.5) == 2
    assert nearest_rank(list(range(1, 101)), 0.95) == 95
    assert summarize([10.0] * 19 + [40.0]) == {"n": 20, "p50": 10.0, "p95": 10.0, "max": 40.0}
    assert summarize([10.0] * 18 + [40.0, 40.0])["p95"] == 40.0
    rows = [
        {"Application": "tjommie-desktop.exe", "QPCTime": "1000", "msBetweenPresents": "0.05"},  # began 500 < start
        {"Application": "tjommie-desktop.exe", "QPCTime": "1600", "msBetweenPresents": "0.05"},  # inside
        {"Application": "other.exe", "QPCTime": "1700", "msBetweenPresents": "0.01"},
        {"Application": "tjommie-desktop.exe", "QPCTime": "2600", "msBetweenPresents": "0.05"},  # after end
        {"Application": "tjommie-desktop.exe", "QPCTime": "", "msBetweenPresents": None},  # cut-off line
    ]
    cut = []
    assert [r["QPCTime"] for r in in_window(rows, 1000, 2000, 10_000_000, cut)] == ["1600"]
    assert len(cut) == 1
    motion = [{"QPCTime": "1100", "msBetweenPresents": "0.05"}, {"QPCTime": "1900", "msBetweenPresents": "0.05"}]
    assert [r["QPCTime"] for r in inside_windows(motion, [(1000, 1200)], 10_000_000)] == []  # began at 600
    assert [r["QPCTime"] for r in inside_windows(motion, [(1300, 1950)], 10_000_000)] == ["1900"]
    frames = [(1000, 0), (1100, 0), (1300, 0), (1400, 0)]
    assert app_intervals(frames, [(1000, 1150), (1290, 1400)], 1000) == [100.0, 100.0]  # gap 1100->1300 is idle
    stray = [{"msBetweenPresents": "10"}] * 199 + [{"msBetweenPresents": "0"}]
    assert present_stats(stray)["n"] == 199 and present_stats(stray)["presentmon_unusable"] is False
    assert present_stats([{"msBetweenPresents": "10"}] * 90 + [{"msBetweenPresents": "0"}] * 10)["presentmon_unusable"] is True
    assert present_stats([])["presentmon_unusable"] is True
    assert cpu_values('"h","cpu"\r\n"t1"," "\r\n"t2","100"\r\n"t3","4') == [100.0]  # "4" was cut off from e.g. "45.2"
    assert cpu_values('"h","cpu"\r\n"t1","50"\r\n') == [50.0]
    assert [r["QPCTime"] for r in in_window(rows, 1000, 2000, 10_000_000, None, "OTHER.exe")] == ["1700"]  # --process
    assert [r["QPCTime"] for r in in_window(rows, 1000, 2000, 10_000_000)] == ["1600"]  # the default process
    import os, tempfile
    with tempfile.TemporaryDirectory() as tmp:  # end to end: unusable PresentMon, so the app's intervals decide
        w = lambda name, text: (open(os.path.join(tmp, name), "w", newline="").write(text), os.path.join(tmp, name))[1]
        pm = w("pm.csv", "Application,QPCTime,msBetweenPresents,Dropped\ntjommie-desktop.exe,1500,0,0\n")
        mk = w("mk.csv", "start_qpc,end_qpc,qpc_freq,frames\n1000,2000,1000,3\n")
        tr = w("tr.csv", "begin_qpc,painted_qpc\n1000,1004\n1016,1020\n1050,1055\n")
        buf = io.StringIO()
        old, sys.stdout = sys.stdout, buf
        try:
            main(pm, mk, None, tr)
        finally:
            sys.stdout = old
        out = json.loads(buf.getvalue())
        assert out["judged_on"] == "app" and out["judged_p95"] == 34.0 and out["app_interval_n"] == 2, out
    print("self-test ok")


if __name__ == "__main__":
    args = sys.argv[1:]
    proc = None
    if args[:1] == ["--process"] and len(args) > 1:
        proc, args = args[1], args[2:]
    if args == ["--self-test"]:
        self_test()
    else:
        main(*args, process=proc)
