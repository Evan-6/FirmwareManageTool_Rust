// Tiny SVG sparkline. Values are already reduced by the caller (delta for
// monotonic counters, raw for gauges), so this stays a pure renderer.
interface Props {
  values: number[];
  colour?: string;
  height?: number;
  filled?: boolean;
}

export function Sparkline({ values, colour = "var(--accent)", height = 38, filled = true }: Props) {
  if (values.length < 2) {
    return <div className="sparkline empty" style={{ height }} />;
  }
  const width = 240;
  const max = Math.max(...values, 1);
  const min = Math.min(...values, 0);
  const span = max - min || 1;
  const step = width / (values.length - 1);
  const points = values.map((value, index) => {
    const x = index * step;
    const y = height - ((value - min) / span) * (height - 4) - 2;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
  const area = `M0,${height} L${points.join(" L")} L${width},${height} Z`;
  return (
    <svg className="sparkline" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none">
      {filled && <path d={area} fill={colour} opacity="0.14" />}
      <polyline points={points.join(" ")} fill="none" stroke={colour} strokeWidth="1.6" />
    </svg>
  );
}
