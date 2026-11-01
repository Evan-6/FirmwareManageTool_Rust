// `<input type="number">` hands back "" for an empty box and "1e"/"-" for junk,
// and `Number("")` is 0 while `Number("1e")` is NaN — which React then renders as
// an empty field and we would post over IPC as `null`/`NaN`.
export function numberValue(raw: string, fallback: number, min = -Infinity, max = Infinity): number {
  if (raw.trim() === "") return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, value));
}

export function intValue(raw: string, fallback: number, min = -Infinity, max = Infinity): number {
  return Math.round(numberValue(raw, fallback, min, max));
}
