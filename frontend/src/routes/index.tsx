import { createFileRoute, Link } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { AlertTriangle, ArrowUpRight, PlayCircle, ServerCog, ShieldAlert, ShieldCheck } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
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
import { fetchActivity, fetchNodes, fetchRuns, fetchComplianceTrend, fetchRunsTrend, fetchSummary } from "@/lib/api";
import { duration, pct, relTime } from "@/lib/format";
import type { ActivityType, FleetNode, Run } from "@/lib/mock/types";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/")({
  head: () => ({
    meta: [
      { title: "Fleet Dashboard — Spindle" },
      {
        name: "description",
        content:
          "Live fleet health: converge success rate, compliance trend, and the nodes that just flipped from passing to failing.",
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

  const {
    data: nodes,
    isLoading: nodesLoading,
    error: nodesError,
  } = useQuery<FleetNode[]>({
    queryKey: ["nodes"],
    queryFn: () => fetchNodes({ limit: 100 }),
  });

  const {
    data: runs,
    isLoading: runsLoading,
    error: runsError,
  } = useQuery<Run[]>({
    queryKey: ["runs", { limit: 50 }],
    queryFn: () => fetchRuns({ limit: 50 }),
  });

  const {
    data: summary,
    isLoading: summaryLoading,
  } = useQuery({
    queryKey: ["summary"],
    queryFn: fetchSummary,
    enabled: !!nodes,
  });

  const {
    data: complianceTrendItems,
    isLoading: complianceTrendLoading,
  } = useQuery({
    queryKey: ["compliance-trend"],
    queryFn: () => fetchComplianceTrend(30),
    enabled: !!nodes,
  });

  const {
    data: runsTrendItems,
    isLoading: runsTrendLoading,
  } = useQuery({
    queryKey: ["runs-trend"],
    queryFn: () => fetchRunsTrend(14),
    enabled: !!nodes,
  });

  const {
    data: activities,
    isLoading: activityLoading,
  } = useQuery({
    queryKey: ["activity", { limit: 100 }],
    queryFn: () => fetchActivity({ limit: 100 }),
    enabled: !!nodes,
  });

  const rangeMinutes = RANGES.find((r) => r.id === range)!.minutes;
  const now = Date.now();

  const events = useMemo(
    () =>
      (activities ?? []).filter(
        (e) => types.includes(e.type) && now - new Date(e.at).getTime() <= rangeMinutes * 60_000,
      ),
    [activities, types, rangeMinutes, now],
  );

  const fleetSummary = useMemo(() => {
    if (summary) {
      return summary;
    }
    // Fallback: compute from nodes (pre-summary-endpoint behavior)
    if (!nodes) return null;
    const total = nodes.length;
    const online = nodes.filter((n) => n.status !== "missing").length;
    const offline = nodes.filter((n) => n.status === "missing").length;
    const convergeSuccess = nodes.filter((n) => n.status === "success").length;
    const convergeFailed = nodes.filter((n) => n.status === "failed").length;
    const compliant = nodes.filter((n) => n.compliance === "compliant").length;
    const nonCompliant = nodes.filter((n) => n.compliance === "non-compliant").length;
    const unknownCompliance = nodes.filter((n) => n.compliance === "unknown").length;
    const flipped = nodes.filter((n) => n.flipped).map((n) => ({ id: n.id, name: n.name }));
    return { total, online, offline, convergeSuccess, convergeFailed, compliant, nonCompliant, unknownCompliance, flipped };
  }, [summary, nodes]);

  const passRate = 0;
  const convergeRate = fleetSummary
    ? Math.round((fleetSummary.convergeSuccess / Math.max(1, fleetSummary.convergeSuccess + fleetSummary.convergeFailed)) * 100)
    : 0;
  const recentFailures = (runs ?? []).filter((r) => r.status === "failed").slice(0, 5);
  const failingNodes = (nodes ?? []).filter((n) => n.compliance === "non-compliant");

  const toggleType = (t: ActivityType) =>
    setTypes((prev) => (prev.includes(t) ? prev.filter((x) => x !== t) : [...prev, t]));

  const loading = nodesLoading || runsLoading || activityLoading || summaryLoading;
  const error = nodesError || runsError;

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

      {error && (
        <div className="panel flex items-center gap-3 border-fail/40 bg-fail-soft/40 p-3">
          <ShieldAlert className="size-4 shrink-0 text-fail" />
          <p className="text-sm text-foreground">
            Unable to load fleet data. Check your API token and server connectivity.
          </p>
        </div>
      )}

      {fleetSummary && fleetSummary.flipped.length > 0 && (
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

      {loading && !fleetSummary && (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          {Array.from({ length: 4 }).map((_, i) => (
            <Panel key={i} title="" description="" bodyClassName="p-4">
              <div className="h-8 w-3/4 animate-pulse rounded bg-muted" />
              <div className="mt-2 h-6 w-1/2 animate-pulse rounded bg-muted" />
            </Panel>
          ))}
        </div>
      )}

      {fleetSummary && (
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
                <ServerCog className="size-3.5" /> Policy groups &middot; environments
              </span>
            }
          />
          <KpiCard
            label="Converge success (24h)"
            value={pct(convergeRate)}
            tone={convergeRate < 90 ? "warn" : "ok"}
            sub={
              <span className="num">
                {fleetSummary.convergeSuccess} ok &middot; <span className="text-fail">{fleetSummary.convergeFailed} failed</span>
              </span>
            }
            sparkTone={convergeRate < 90 ? "warn" : "ok"}
          />
          <KpiCard
            label="Control pass rate"
            value={pct(passRate)}
            tone={passRate < 85 ? "fail" : "ok"}
            sub={<span className="num text-fail">-22 pts vs 7d</span>}
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
      )}

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
                    to={e.href as any}
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
          >
            {complianceTrendItems && complianceTrendItems.length > 0 ? (
              <TrendChart
                data={complianceTrendItems.map((item) => ({
                  label: item.date,
                  passRate: item.passRate,
                }))}
                height={200}
              />
            ) : (
              <EmptyState title="Trend data coming soon" description="Compliance trend chart will render when scan data is available." />
            )}
          </Panel>
          <Panel title="Converge outcomes" description="Successful vs failed runs per day, last 14 days">
            {runsTrendItems && runsTrendItems.length > 0 ? (
              <ConvergeChart
                data={runsTrendItems.map((item) => ({
                  label: item.date,
                  success: item.success,
                  failed: item.failed,
                  rate: item.success + item.failed > 0 ? (item.success / (item.success + item.failed)) * 100 : 0,
                }))}
                height={200}
              />
            ) : (
              <EmptyState title="No trend data" description="Converge trend chart will render when run data is available." />
            )}
          </Panel>
        </div>
      </div>

      {recentFailures.length > 0 && (
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
          </Panel>

          <Panel
            title="Non-compliant nodes"
            description="Highest failing-control counts first"
            bodyClassName="p-0"
          >
            {failingNodes.length === 0 ? (
              <EmptyState
                icon={<ShieldCheck className="size-5" />}
                title="All nodes compliant"
                description="No non-compliant nodes in the fleet."
              />
            ) : (
              <ul className="divide-y divide-border/60">
                {failingNodes
                  .slice()
                  .sort((a, b) => b.failed - a.failed)
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
                            {n.environment} &middot; {n.policyGroup}
                          </div>
                        </div>
                        <span className="num shrink-0 text-xs text-fail">{n.failed} failing</span>
                      </Link>
                    </li>
                  ))}
              </ul>
            )}
          </Panel>
        </div>
      )}
    </div>
  );
}
