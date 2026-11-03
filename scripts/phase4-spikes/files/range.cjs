// P0 HTTP adapter only. RPC reads remain bounded even when an HTTP range is open-ended.
function httpRange(header, size) {
  if (header === null || header === undefined) return null;
  const match = /^bytes=(\d*)-(\d*)$/.exec(header);
  if (!match || (!match[1] && !match[2]) || size === 0) throw new RangeError("Unsatisfiable range");
  const first = match[1] ? Number(match[1]) : null;
  const last = match[2] ? Number(match[2]) : null;
  if (first !== null && !Number.isSafeInteger(first) || last !== null && !Number.isSafeInteger(last)) throw new RangeError("Unsatisfiable range");
  const start = first ?? Math.max(0, size - last);
  const end = first === null || last === null ? size - 1 : Math.min(last, size - 1);
  if (start < 0 || start >= size || end < start || first === null && last === 0) throw new RangeError("Unsatisfiable range");
  return { start, end };
}

module.exports = { httpRange };
