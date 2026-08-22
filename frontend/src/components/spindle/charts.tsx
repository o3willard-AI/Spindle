import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { cn } from "@/lib/utils";

export function Sparkline({
  data,
  tone = "ok",
  className,
  height = 32,
}: {
  data: number[];
  tone?: "ok" | "fail" | "warn" | "info";
  className?: string;
  height?: number;
}) {
  const series = data.map((v, i) => ({ i, v }));
  const color = `var(--${tone})`;
  return (
    <div className={cn("w-full", className)} style={{ height }}>
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={series} margin={{ top: 2, right: 0, bottom: 0, left: 0 }}>
          <defs>
            <linearGradient id={`spark-${tone}`} x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor={color} stopOpacity={0.35} />
              <stop offset="100%" stopColor={color} stopOpacity={0} />
            </linearGradient>
          </defs>
          <Area
            type="monotone"
            dataKey="v"
            stroke={color}
            strokeWidth={1.5}
            fill={`url(#spark-${tone})`}
            isAnimationActive={false}
            dot={false}
          />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}

const tooltipStyle = {
  backgroundColor: "var(--popover)",
  border: "1px solid var(--border)",
  borderRadius: "8px",
  fontSize: "12px",
  color: "var(--popover-foreground)",
  boxShadow: "var(--shadow-panel)",
};

export function TrendChart({
  data,
  height = 200,
}: {
  data: Array<{ label: string; passRate: number }>;
  height?: number;
}) {
  return (
    <div style={{ height }}>
      <ResponsiveContainer width="100%" height="100%">
        <AreaChart data={data} margin={{ top: 8, right: 8, bottom: 0, left: -18 }}>
          <defs>
            <linearGradient id="trend-fill" x1="0" y1="0" x2="0" y2="1">
              <stop offset="0%" stopColor="var(--ok)" stopOpacity={0.3} />
              <stop offset="100%" stopColor="var(--ok)" stopOpacity={0} />
            </linearGradient>
          </defs>
          <CartesianGrid stroke="var(--border)" strokeDasharray="2 4" vertical={false} />
          <XAxis
            dataKey="label"
            tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
            tickLine={false}
            axisLine={false}
            interval={4}
          />
          <YAxis
            domain={[50, 100]}
            tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
            tickLine={false}
            axisLine={false}
            width={38}
          />
          <Tooltip contentStyle={tooltipStyle} formatter={(v: number) => [`${v}%`, "Pass rate"]} />
          <Area
            type="monotone"
            dataKey="passRate"
            stroke="var(--ok)"
            strokeWidth={2}
            fill="url(#trend-fill)"
            isAnimationActive={false}
          />
        </AreaChart>
      </ResponsiveContainer>
    </div>
  );
}

export function ConvergeChart({
  data,
  height = 200,
}: {
  data: Array<{ label: string; success: number; failed: number; rate: number }>;
  height?: number;
}) {
  return (
    <div style={{ height }}>
      <ResponsiveContainer width="100%" height="100%">
        <BarChart data={data} margin={{ top: 8, right: 8, bottom: 0, left: -18 }} barCategoryGap="28%">
          <CartesianGrid stroke="var(--border)" strokeDasharray="2 4" vertical={false} />
          <XAxis
            dataKey="label"
            tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
            tickLine={false}
            axisLine={false}
          />
          <YAxis
            tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
            tickLine={false}
            axisLine={false}
            width={38}
          />
          <Tooltip contentStyle={tooltipStyle} cursor={{ fill: "var(--muted)", opacity: 0.4 }} />
          <Bar dataKey="success" stackId="a" fill="var(--ok)" radius={[0, 0, 0, 0]} isAnimationActive={false} />
          <Bar dataKey="failed" stackId="a" fill="var(--fail)" radius={[2, 2, 0, 0]} isAnimationActive={false} />
        </BarChart>
      </ResponsiveContainer>
    </div>
  );
}

export function MiniLine({
  data,
  height = 120,
}: {
  data: Array<{ label: string; value: number }>;
  height?: number;
}) {
  return (
    <div style={{ height }}>
      <ResponsiveContainer width="100%" height="100%">
        <LineChart data={data} margin={{ top: 8, right: 8, bottom: 0, left: -24 }}>
          <CartesianGrid stroke="var(--border)" strokeDasharray="2 4" vertical={false} />
          <XAxis dataKey="label" tick={{ fill: "var(--muted-foreground)", fontSize: 10 }} tickLine={false} axisLine={false} interval={4} />
          <YAxis tick={{ fill: "var(--muted-foreground)", fontSize: 10 }} tickLine={false} axisLine={false} width={38} />
          <Tooltip contentStyle={tooltipStyle} />
          <Line type="monotone" dataKey="value" stroke="var(--info)" strokeWidth={2} dot={false} isAnimationActive={false} />
        </LineChart>
      </ResponsiveContainer>
    </div>
  );
}

export function StackedMeter({
  segments,
  className,
}: {
  segments: Array<{ label: string; value: number; tone: "ok" | "fail" | "warn" | "unknown" }>;
  className?: string;
}) {
  const total = segments.reduce((a, s) => a + s.value, 0) || 1;
  const toneBg = { ok: "bg-ok", fail: "bg-fail", warn: "bg-warn", unknown: "bg-unknown" };
  return (
    <div className={cn("space-y-2", className)}>
      <div className="flex h-2 w-full overflow-hidden rounded-full bg-muted">
        {segments.map((s) => (
          <div
            key={s.label}
            className={cn(toneBg[s.tone], "transition-all")}
            style={{ width: `${(s.value / total) * 100}%` }}
            title={`${s.label}: ${s.value}`}
          />
        ))}
      </div>
      <div className="flex flex-wrap gap-x-4 gap-y-1">
        {segments.map((s) => (
          <div key={s.label} className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <span className={cn("size-1.5 rounded-full", toneBg[s.tone])} />
            {s.label}
            <span className="num text-foreground">{s.value}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
