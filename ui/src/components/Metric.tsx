import type { ReactNode } from "react";
import { Sparkline } from "./Sparkline";

interface Props {
  label: string;
  value: ReactNode;
  hint?: string;
  values?: number[];
  colour?: string;
  suffix?: string;
}

export function Metric({ label, value, hint, values, colour, suffix }: Props) {
  return (
    <div className="metric">
      <div className="metric-head">
        <span>{label}</span>
        {suffix && <span className="metric-suffix">{suffix}</span>}
      </div>
      <div className="metric-value">{value}</div>
      {values && <Sparkline values={values} colour={colour} />}
      {hint && <div className="metric-hint">{hint}</div>}
    </div>
  );
}
