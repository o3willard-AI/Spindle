import { createFileRoute, Link } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { AlertTriangle, ArrowUpRight, PlayCircle, ServerCog, ShieldAlert, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ConvergeChart, Sparkline, StackedMeter, TrendChart } from "@/components/spindle/charts";
import { StatusDot, StatusPill } from "@/components/spindle/status";
import { EmptyState, KpiCard, Panel, PageHeader } from "@/components/spindle/ui-bits";
import {
  activity,
  complianceTrend30d,
  convergeSuccess14d,
  fleetSummary,
  nodes,
  runs,
} from "@/lib/mock/data";
import { duration, pct, relTime } from "@/lib/format";
import type { ActivityType } from "@/lib/mock/types";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/")({
  head: () => ({
    meta: [
      { title: "Fleet Dashboard — Spindle" },
      {
        name: "description",
        content:
          "Live fleet health for 10 managed Linux nodes: converge success rate, compliance trend, and the nodes that just flipped from passing to failing.",
      },
      { property: "og:title", content: "Fleet Dashboard — Spindle" },
      {
        property: "og:description",
        content: "Converge success rate, compliance trend and recent failures across your Linux fleet.",
      },
    ],
  }),
  component: Dashboard,
});

const TYPES: Array<{ id: ActivityType; label: string }> = [
  { id: "converge", label: "Converges" },
  { id: "scan", label: "Compliance scans" },
  { id: "node", label: "Node changes" },
];

const RANGES = [
  { id: "1h", label: "Last hour", minutes: 60 },
  { id: "6h", label: "Last 6 hours", minutes: 360 },
  { id: "24h", label: "Last 24 hours", minutes: 1440 },
  { id: "7d", label: "Last 7 days", minutes: 10080 },
];

function Dashboard() {
  const [types, setTypes] = useState<ActivityType[]>(["converge", "scan", "node"]);
  const [range, setRange] = useState("24h");

  const now = new Date("2026-08-22T18:40:00.000Z").getTime();
  const rangeMinutes = RANGES.find((r) => r.id === range)!.minutes;

  const events = useMemo(
    () =>
      activity.filter(
        (e) => types.includes(e.type) && now - new Date(e.at).getTime() <= rangeMinutes * 60_000,
      ),
    [types, rangeMinutes, now],
  );

  const passRate = complianceTrend30d.at(-1)!.passRate;
  const convergeRate = Math.round(
    (fleetSummary.convergeSuccess / (fleetSummary.convergeSuccess + fleetSummary.convergeFailed)) * 100,
  );
  const recentFailures = runs.filter((r) => r.status === "failed").slice(0, 5);
  const failingNodes = nodes.filter((n) => n.compliance === "non-compliant");

  const toggleType = (t: ActivityType) =>
    setTypes((prev) => (prev.includes(t) ? prev.filter((x) => x !== t) : [...prev, t]));

  return (
    <div className="space-y-5">
      <PageHeader
        title="Fleet dashboard"
        description="Configuration and compliance posture across all managed nodes."
        actions={
          <>
            <Button variant="outline" size="sm" className="h-8 text-xs" asChild>
              <Link to="/runs">All converge runs</Link>
            </Button>
            <Button size="sm" className="h-8 text-xs" asChild>
              <Link to="/compliance">Compliance report</Link>
            </Button>
          </>
        }
      />

      {fleetSummary.flipped.length > 0 && (
        <div className="panel flex flex-wrap items-center gap-3 border-fail/40 bg-fail-soft/40 p-3">
          <ShieldAlert className="size-4 shrink-0 text-fail" />
          <p className="text-sm text-foreground">
            <span className="font-medium">{fleetSummary.flipped.length} nodes flipped from compliant to non-compliant</span>{" "}
            <span className="text-muted-foreground">in the last 2 hours.</span>
          </p>
          <div className="flex flex-wrap gap-1.5">
            {fleetSummary.flipped.map((n) => (
              <Link
                key={n.id}
                to="/nodes/$nodeId"
                params={{ nodeId: n.id }}
                className="num inline-flex items-center gap-1.5 rounded-full border border-fail/30 bg-surface px-2 py-0.5 text-[11px] transition-colors hover:border-fail"
              >
                <StatusDot status="failed" pulse />
                {n.name}
              </Link>
            ))}
          </div>
          <Button variant="ghost" size="sm" className="ml-auto h-7 text-xs" asChild>
            <Link to="/compliance">Investigate</Link>
          </Button>
        </div>
      )}

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <KpiCard
          label="Managed nodes"
          value={fleetSummary.total}
          sub={
            <span className="flex items-center gap-2">
              <span className="flex items-center gap-1">
                <StatusDot status="success" /> {fleetSummary.online} online
              </span>
              <span className="flex items-center gap-1">
                <StatusDot status="missing" /> {fleetSummary.offline} missing
              </span>
            </span>
          }
          footer={
            <span className="flex items-center gap-1.5">
              <ServerCog className="size-3.5" /> 5 policy groups · 3 environments
            </span>
          }
        />
        <KpiCard
          label="Converge success (24h)"
          value={pct(convergeRate)}
          tone={convergeRate < 90 ? "warn" : "ok"}
          sub={
            <span className="num">
              {fleetSummary.convergeSuccess} ok · <span className="text-fail">{fleetSummary.convergeFailed} failed</span>
            </span>
          }
          spark={convergeSuccess14d.map((d) => d.rate)}
          sparkTone={convergeRate < 90 ? "warn" : "ok"}
        />
        <KpiCard
          label="Control pass rate"
          value={pct(passRate)}
          tone={passRate < 85 ? "fail" : "ok"}
          sub={<span className="num text-fail">-22 pts vs 7d</span>}
          spark={complianceTrend30d.map((d) => d.passRate)}
          sparkTone={passRate < 85 ? "fail" : "ok"}
        />
        <KpiCard
          label="Non-compliant nodes"
          value={fleetSummary.nonCompliant}
          tone="fail"
          sub={<span className="num">{fleetSummary.compliant} compliant</span>}
          footer={
            <StackedMeter
              segments={[
                { label: "Compliant", value: fleetSummary.compliant, tone: "ok" },
                { label: "Failing", value: fleetSummary.nonCompliant, tone: "fail" },
                { label: "Unknown", value: fleetSummary.unknownCompliance, tone: "unknown" },
              ]}
            />
          }
        />
      </div>

      <div className="grid gap-4 xl:grid-cols-3">
        <Panel
          className="xl:col-span-2"
          title="Activity timeline"
          description="Converge reports, compliance scans and inventory changes."
          actions={
            <div className="flex flex-wrap items-center gap-1.5">
              {TYPES.map((t) => (
                <button
                  key={t.id}
                  onClick={() => toggleType(t.id)}
                  className={cn(
                    "rounded-full border px-2 py-0.5 text-[11px] transition-colors",
                    types.includes(t.id)
                      ? "border-primary/40 bg-accent text-accent-foreground"
                      : "border-border text-muted-foreground hover:text-foreground",
                  )}
                >
                  {t.label}
                </button>
              ))}
              <Select value={range} onValueChange={setRange}>
                <SelectTrigger className="h-7 w-[130px] text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {RANGES.map((r) => (
                    <SelectItem key={r.id} value={r.id} className="text-xs">
                      {r.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          }
          bodyClassName="p-0"
        >
          {events.length === 0 ? (
            <EmptyState
              icon={<AlertTriangle className="size-5" />}
              title="No events in this window"
              description="Widen the time range or re-enable an event type to see fleet activity."
            />
          ) : (
            <ol className="scroll-thin max-h-[560px] divide-y divide-border/60 overflow-y-auto">
              {events.map((e) => (
                <li key={e.id}>
                  <Link
                    to={e.href}
                    className="flex items-start gap-3 px-4 py-2.5 transition-colors hover:bg-accent/40"
                  >
                    <span className="mt-1.5">
                      <StatusDot status={e.status} pulse={e.status === "fail"} />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="flex flex-wrap items-center gap-2">
                        <span className="num text-xs text-foreground">{e.nodeName}</span>
                        <StatusPill
                          size="sm"
                          status={e.status}
                          label={e.type === "converge" ? "converge" : e.type === "scan" ? "scan" : "inventory"}
                        />
                      </span>
                      <span className="mt-0.5 block truncate text-sm text-foreground/90">{e.title}</span>
                      <span className="num mt-0.5 block text-[11px] text-muted-foreground">{e.detail}</span>
                    </span>
                    <span className="num shrink-0 text-[11px] text-muted-foreground">{relTime(e.at)}</span>
                  </Link>
                </li>
              ))}
            </ol>
          )}
        </Panel>

        <div className="space-y-4">
          <Panel
            title="Compliance trend"
            description="Control pass rate, last 30 days"
            actions={<Sparkline data={complianceTrend30d.map((d) => d.passRate)} tone="ok" className="w-20" height={24} />}
          >
            <TrendChart data={complianceTrend30d} height={172} />
          </Panel>

          <Panel title="Converge outcomes" description="Successful vs failed runs per day, last 14 days">
            <ConvergeChart data={convergeSuccess14d} height={172} />
          </Panel>
        </div>
      </div>

      <div className="grid gap-4 xl:grid-cols-3">
        <Panel
          className="xl:col-span-2"
          title="Recent failures"
          description="Latest failed converge runs — click through to the failing resource."
          actions={
            <Button variant="ghost" size="sm" className="h-7 text-xs" asChild>
              <Link to="/runs">View all</Link>
            </Button>
          }
          bodyClassName="p-0"
        >
          {recentFailures.length === 0 ? (
            <EmptyState icon={<ShieldCheck className="size-5" />} title="No failures in range" />
          ) : (
            <ul className="divide-y divide-border/60">
              {recentFailures.map((r) => (
                <li key={r.id}>
                  <Link
                    to="/runs/$runId"
                    params={{ runId: r.id }}
                    className="flex items-center gap-3 px-4 py-2.5 transition-colors hover:bg-accent/40"
                  >
                    <PlayCircle className="size-4 shrink-0 text-fail" />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-center gap-2">
                        <span className="num text-xs text-foreground">{r.nodeName}</span>
                        <StatusPill size="sm" status="failed" />
                        <span className="num text-[11px] text-muted-foreground">{r.id}</span>
                      </div>
                      <p className="mt-0.5 truncate font-mono text-[11px] text-muted-foreground">{r.errorSummary}</p>
                    </div>
                    <div className="num hidden shrink-0 text-right text-[11px] text-muted-foreground sm:block">
                      <div>{relTime(r.startedAt)}</div>
                      <div>{duration(r.durationSec)}</div>
                    </div>
                    <ArrowUpRight className="size-3.5 shrink-0 text-muted-foreground" />
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </Panel>

        <Panel
          title="Non-compliant nodes"
          description="Highest failing-control counts first"
          bodyClassName="p-0"
        >
          <ul className="divide-y divide-border/60">
            {failingNodes
              .slice()
              .sort((a, b) => b.controlsFailed - a.controlsFailed)
              .map((n) => (
                <li key={n.id}>
                  <Link
                    to="/nodes/$nodeId"
                    params={{ nodeId: n.id }}
                    className="flex items-center gap-3 px-4 py-2.5 transition-colors hover:bg-accent/40"
                  >
                    <div className="min-w-0 flex-1">
                      <div className="num truncate text-xs text-foreground">{n.name}</div>
                      <div className="mt-0.5 text-[11px] text-muted-foreground">
                        {n.environment} · {n.policyGroup}
                      </div>
                    </div>
                    <Sparkline data={n.complianceTrend} tone="fail" className="w-16" height={22} />
                    <span className="num shrink-0 text-xs text-fail">{n.controlsFailed} failing</span>
                  </Link>
                </li>
              ))}
          </ul>
        </Panel>
      </div>
    </div>
  );
}
