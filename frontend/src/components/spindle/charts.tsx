import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Line,
  LineChart,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { cn } from "@/lib/utils";

/**
 * Sparkline — a lightweight plain-SVG area sparkline with no recharts dependency.
 *
 * For a ~22–56 px-tall sparkline inside a KpiCard, recharts' ResponsiveContainer
 * + AreaChart is pure overhead and a correctness liability — a plain SVG path
 * is cheaper, faster, and immune to ResizeObserver measurement drift.
 */
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
  if (!data || data.length === 0) {
    return <div className={cn("w-full", className)} style={{ height }} />;
  }

  const width = 120;
  const pad = 2;
  const chartW = width - pad * 2;
  const chartH = Math.max(1, height - 4);

  const min = Math.min(...data);
  const max = Math.max(...data, min + 1); // guard against flat data
  const range = max - min || 1;

  const pts = data.map((v, i) => {
    const x = pad + (i / Math.max(1, data.length - 1)) * chartW;
    const y = pad + ((max - v) / range) * chartH;
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });

  const areaPts = `${pts.join(" ")} ${pad + chartW},${pad + chartH} ${pad},${pad + chartH} ${pts[0]}`;

  const color = `var(--${tone})`;

  return (
    <div className={cn("w-full", className)} style={{ height, width: "100%" }}>
      <svg
        viewBox={`0 0 ${width} ${height}`}
        width="100%"
        height={height}
        preserveAspectRatio="none"
        role="img"
        aria-label={`Trend sparkline: ${data.length} data points`}
      >
        <defs>
          <linearGradient id={`spark-fill-${tone}`} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={color} stopOpacity={0.35} />
            <stop offset="100%" stopColor={color} stopOpacity={0} />
          </linearGradient>
        </defs>
        <polygon points={areaPts} fill={`url(#spark-fill-${tone})`} />
        <polyline
          points={pts.join(" ")}
          fill="none"
          stroke={color}
          strokeWidth={1.5}
          strokeLinejoin="round"
          strokeLinecap="round"
        />
      </svg>
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

/**
 * SimpleAreaChart — a recharts AreaChart at a fixed numeric width/height.
 * No ResponsiveContainer, no ResizeObserver.  The parent passes a known
 * height; width is fixed at 600 (the dashboard panel width).
 */
function SimpleAreaChart({
  data,
  width = 600,
  height = 200,
}: {
  data: Array<{ label: string; passRate: number }>;
  width?: number;
  height?: number;
}) {
  return (
    <AreaChart
      data={data}
      width={width}
      height={height}
      margin={{ top: 8, right: 8, bottom: 0, left: -18 }}
    >
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
      <Tooltip
        contentStyle={tooltipStyle}
        formatter={(v: number) => [`${v}%`, "Pass rate"]}
      />
      <Area
        type="monotone"
        dataKey="passRate"
        stroke="var(--ok)"
        strokeWidth={2}
        fill="url(#trend-fill)"
        isAnimationActive={false}
      />
    </AreaChart>
  );
}

/**
 * SimpleBarChart — a recharts BarChart at a fixed numeric width/height.
 */
function SimpleBarChart({
  data,
  width = 600,
  height = 200,
}: {
  data: Array<{ label: string; success: number; failed: number; rate: number }>;
  width?: number;
  height?: number;
}) {
  return (
    <BarChart
      data={data}
      width={width}
      height={height}
      margin={{ top: 8, right: 8, bottom: 0, left: -18 }}
      barCategoryGap="28%"
    >
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
      <Tooltip
        contentStyle={tooltipStyle}
        cursor={{ fill: "var(--muted)", opacity: 0.4 }}
      />
      <Bar
        dataKey="success"
        stackId="a"
        fill="var(--ok)"
        radius={[0, 0, 0, 0]}
        isAnimationActive={false}
      />
      <Bar
        dataKey="failed"
        stackId="a"
        fill="var(--fail)"
        radius={[2, 2, 0, 0]}
        isAnimationActive={false}
      />
    </BarChart>
  );
}

/**
 * SimpleLineChart — a recharts LineChart at a fixed numeric width/height.
 */
function SimpleLineChart({
  data,
  width = 600,
  height = 120,
}: {
  data: Array<{ label: string; value: number }>;
  width?: number;
  height?: number;
}) {
  return (
    <LineChart
      data={data}
      width={width}
      height={height}
      margin={{ top: 8, right: 8, bottom: 0, left: -24 }}
    >
      <CartesianGrid stroke="var(--border)" strokeDasharray="2 4" vertical={false} />
      <XAxis
        dataKey="label"
        tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
        tickLine={false}
        axisLine={false}
        interval={4}
      />
      <YAxis
        tick={{ fill: "var(--muted-foreground)", fontSize: 10 }}
        tickLine={false}
        axisLine={false}
        width={38}
      />
      <Tooltip contentStyle={tooltipStyle} />
      <Line
        type="monotone"
        dataKey="value"
        stroke="var(--info)"
        strokeWidth={2}
        dot={false}
        isAnimationActive={false}
      />
    </LineChart>
  );
}

export function TrendChart({
  data,
  height = 200,
}: {
  data: Array<{ label: string; passRate: number }>;
  height?: number;
}) {
  if (!data || data.length === 0) {
    return null;
  }
  return <SimpleAreaChart data={data} width={600} height={height} />;
}

export function ConvergeChart({
  data,
  height = 200,
}: {
  data: Array<{ label: string; success: number; failed: number; rate: number }>;
  height?: number;
}) {
  if (!data || data.length === 0) {
    return null;
  }
  return <SimpleBarChart data={data} width={600} height={height} />;
}

export function MiniLine({
  data,
  height = 120,
}: {
  data: Array<{ label: string; value: number }>;
  height?: number;
}) {
  if (!data || data.length === 0) {
    return null;
  }
  return <SimpleLineChart data={data} width={600} height={height} />;
}

export function StackedMeter({
  segments,
  className,
}: {
  segments: Array<{ label: string; value: number; tone: "ok" | "fail" | "warn" | "unknown" }>;
  className?: string;
}) {
  const total = segments.reduce((a, s) => a + s.value, 0) || 1;
  const toneBg = {
    ok: "bg-ok",
    fail: "bg-fail",
    warn: "bg-warn",
    unknown: "bg-unknown",
  };
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
          <div
            key={s.label}
            className="flex items-center gap-1.5 text-xs text-muted-foreground"
          >
            <span className={cn("size-1.5 rounded-full", toneBg[s.tone])} />
            {s.label}
            <span className="num text-foreground">{s.value}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
