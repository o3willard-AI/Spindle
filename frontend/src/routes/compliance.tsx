import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Download, FileJson } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { Sparkline, StackedMeter, TrendChart } from "@/components/spindle/charts";
import { SeverityBadge, StatusPill } from "@/components/spindle/status";
import { KpiCard, PageHeader, Panel } from "@/components/spindle/ui-bits";
import {
  complianceTrend30d,
  controlRollups,
  environments,
  nodes,
  platforms,
  profiles,
  scans,
  type ControlRollup,
} from "@/lib/mock/data";
import { downloadFile, pct, relTime, toCsv } from "@/lib/format";
import type { FleetNode, Profile } from "@/lib/mock/types";
import { cn } from "@/lib/utils";
import { toast } from "sonner";

export const Route = createFileRoute("/compliance")({
  head: () => ({
    meta: [
      { title: "Compliance — Spindle Continuous Auditing" },
      {
        name: "description",
        content:
          "Continuous compliance across the fleet: pass/fail/skip/waived breakdown, control pass-rate trend and the controls failing on the most nodes.",
      },
      { property: "og:title", content: "Compliance — Spindle Continuous Auditing" },
      {
        property: "og:description",
        content: "CIS-style profile and control results for every managed Linux node.",
      },
    ],
  }),
  component: CompliancePage,
});

type View = "node" | "profile";
type FailureDim = "platform" | "environment" | "profile" | "control";

const installed = profiles.filter((p) => p.installed);

function totals() {
  let passed = 0;
  let failed = 0;
  let skipped = 0;
  let waived = 0;
  scans.forEach((s) =>
    s.profiles.forEach((p) =>
      p.controls.forEach((c) => {
        if (c.status === "failed") failed += 1;
        else if (c.status === "skipped") skipped += 1;
        else if (c.status === "waived") waived += 1;
        else passed += 1;
      }),
    ),
  );
  waived = nodes.reduce((a, n) => a + n.controlsWaived, 0);
  return { passed, failed, skipped, waived };
}

function FailureBars({ dim }: { dim: FailureDim }) {
  const rows = useMemo(() => {
    const map = new Map<string, number>();
    if (dim === "control") {
      controlRollups
        .filter((c) => c.failing > 0)
        .slice(0, 8)
        .forEach((c) => map.set(`${c.id} · ${c.title}`, c.failing));
    } else {
      nodes.forEach((n) => {
        if (n.controlsFailed === 0) return;
        if (dim === "platform") map.set(n.platform, (map.get(n.platform) ?? 0) + n.controlsFailed);
        if (dim === "environment") map.set(n.environment, (map.get(n.environment) ?? 0) + n.controlsFailed);
      });
      if (dim === "profile") {
        scans.forEach((s) =>
          s.profiles.forEach((p) => {
            const f = p.controls.filter((c) => c.status === "failed").length;
            if (f) map.set(p.profileName, (map.get(p.profileName) ?? 0) + f);
          }),
        );
      }
    }
    return [...map.entries()].sort((a, b) => b[1] - a[1]);
  }, [dim]);

  const max = Math.max(1, ...rows.map((r) => r[1]));

  if (rows.length === 0) {
    return <p className="py-8 text-center text-xs text-muted-foreground">No failing controls in this dimension.</p>;
  }

  return (
    <ul className="space-y-2.5">
      {rows.map(([label, value]) => (
        <li key={label} className="space-y-1">
          <div className="flex items-baseline justify-between gap-3">
            <span className="truncate text-xs text-foreground/90 capitalize">{label}</span>
            <span className="num text-xs text-fail">{value}</span>
          </div>
          <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
            <div className="h-full rounded-full bg-fail" style={{ width: `${(value / max) * 100}%` }} />
          </div>
        </li>
      ))}
    </ul>
  );
}

function CompliancePage() {
  const navigate = useNavigate();
  const [view, setView] = useState<View>("node");
  const [dim, setDim] = useState<FailureDim>("control");

  const [nodeEnv, setNodeEnv] = useState<string[]>([]);
  const [nodePlat, setNodePlat] = useState<string[]>([]);
  const [nodeStatus, setNodeStatus] = useState<string[]>([]);
  const [nodeProfile, setNodeProfile] = useState<string[]>([]);

  const [ctrlProfile, setCtrlProfile] = useState<string[]>([]);
  const [ctrlSeverity, setCtrlSeverity] = useState<string[]>([]);
  const [ctrlState, setCtrlState] = useState<string[]>([]);

  const t = totals();
  const total = t.passed + t.failed + t.skipped + t.waived;
  const passRate = (t.passed / total) * 100;

  const nodeRows = useMemo(
    () =>
      nodes.filter((n) => {
        const nodeProfiles = scans.find((s) => s.nodeId === n.id)?.profiles.map((p) => p.profileName) ?? [];
        return (
          (nodeEnv.length === 0 || nodeEnv.includes(n.environment)) &&
          (nodePlat.length === 0 || nodePlat.includes(n.platform)) &&
          (nodeStatus.length === 0 || nodeStatus.includes(n.compliance)) &&
          (nodeProfile.length === 0 || nodeProfile.some((p) => nodeProfiles.includes(p)))
        );
      }),
    [nodeEnv, nodePlat, nodeStatus, nodeProfile],
  );

  const controlRows = useMemo(
    () =>
      controlRollups.filter(
        (c) =>
          (ctrlProfile.length === 0 || ctrlProfile.includes(c.profileId)) &&
          (ctrlSeverity.length === 0 || ctrlSeverity.includes(c.severity)) &&
          (ctrlState.length === 0 ||
            (ctrlState.includes("failing") && c.failing > 0) ||
            (ctrlState.includes("passing") && c.failing === 0)),
      ),
    [ctrlProfile, ctrlSeverity, ctrlState],
  );

  const nodeColumns: Column<FleetNode>[] = [
    {
      key: "name",
      header: "Node",
      sortValue: (n) => n.name,
      cell: (n) => <span className="num text-xs font-medium">{n.name}</span>,
    },
    { key: "status", header: "Status", sortValue: (n) => n.compliance, cell: (n) => <StatusPill status={n.compliance} /> },
    {
      key: "passed",
      header: "Passed",
      sortValue: (n) => n.controlsPassed,
      cell: (n) => <span className="num text-xs text-ok">{n.controlsPassed}</span>,
    },
    {
      key: "failed",
      header: "Failed",
      sortValue: (n) => n.controlsFailed,
      cell: (n) => <span className={cn("num text-xs", n.controlsFailed ? "text-fail" : "text-muted-foreground")}>{n.controlsFailed}</span>,
    },
    {
      key: "skipped",
      header: "Skipped",
      sortValue: (n) => n.controlsSkipped,
      cell: (n) => <span className="num text-xs text-muted-foreground">{n.controlsSkipped}</span>,
    },
    {
      key: "waived",
      header: "Waived",
      sortValue: (n) => n.controlsWaived,
      cell: (n) => <span className="num text-xs text-warn">{n.controlsWaived}</span>,
    },
    {
      key: "rate",
      header: "Pass rate",
      sortValue: (n) => n.controlsPassed / Math.max(1, n.controlsPassed + n.controlsFailed),
      cell: (n) => (
        <span className="num text-xs">
          {pct((n.controlsPassed / Math.max(1, n.controlsPassed + n.controlsFailed)) * 100)}
        </span>
      ),
    },
    {
      key: "trend",
      header: "30d",
      sortable: false,
      cell: (n) => <Sparkline data={n.complianceTrend} tone={n.compliance === "compliant" ? "ok" : "fail"} className="w-20" height={20} />,
    },
    {
      key: "scanned",
      header: "Last scan",
      sortValue: (n) => n.lastSeen,
      className: "text-right",
      headerClassName: "text-right",
      cell: (n) => <span className="num text-[11px] text-muted-foreground">{relTime(n.lastSeen)}</span>,
    },
  ];

  const profileColumns: Column<Profile>[] = [
    { key: "title", header: "Profile", sortValue: (p) => p.title, cell: (p) => (
      <div className="min-w-0">
        <div className="truncate text-xs font-medium">{p.title}</div>
        <div className="num text-[11px] text-muted-foreground">{p.name} v{p.version}</div>
      </div>
    ) },
    { key: "vendor", header: "Vendor", sortValue: (p) => p.vendor, cell: (p) => <span className="text-xs text-muted-foreground">{p.vendor}</span> },
    { key: "nodes", header: "Nodes", sortValue: (p) => p.nodes, cell: (p) => <span className="num text-xs">{p.nodes}</span> },
    { key: "controls", header: "Controls", sortValue: (p) => p.controlCount, cell: (p) => <span className="num text-xs">{p.controlCount}</span> },
    { key: "tests", header: "Tests", sortValue: (p) => p.testCount, cell: (p) => <span className="num text-xs">{p.testCount}</span> },
    {
      key: "rate",
      header: "Pass rate",
      sortValue: (p) => p.passRate,
      cell: (p) => (
        <div className="flex items-center gap-2">
          <div className="h-1.5 w-20 overflow-hidden rounded-full bg-muted">
            <div className={cn("h-full rounded-full", p.passRate > 0.85 ? "bg-ok" : "bg-fail")} style={{ width: `${p.passRate * 100}%` }} />
          </div>
          <span className="num text-xs">{pct(p.passRate * 100)}</span>
        </div>
      ),
    },
    {
      key: "status",
      header: "Status",
      sortValue: (p) => (p.passRate > 0.85 ? "compliant" : "non-compliant"),
      cell: (p) => <StatusPill status={p.passRate > 0.85 ? "compliant" : "non-compliant"} />,
    },
  ];

  const controlColumns: Column<ControlRollup>[] = [
    { key: "id", header: "Control", sortValue: (c) => c.id, cell: (c) => (
      <div className="min-w-0">
        <div className="num text-[11px] text-muted-foreground">{c.id}</div>
        <div className="max-w-96 truncate text-xs">{c.title}</div>
      </div>
    ) },
    { key: "profile", header: "Profile", sortValue: (c) => c.profileTitle, cell: (c) => <span className="num text-xs">{c.profileId}</span> },
    { key: "severity", header: "Severity", sortValue: (c) => c.impact, cell: (c) => <SeverityBadge severity={c.severity} impact={c.impact} /> },
    {
      key: "failing",
      header: "Failing nodes",
      sortValue: (c) => c.failing,
      cell: (c) => <span className={cn("num text-xs", c.failing ? "text-fail" : "text-muted-foreground")}>{c.failing}</span>,
    },
    { key: "passing", header: "Passing", sortValue: (c) => c.passing, cell: (c) => <span className="num text-xs text-ok">{c.passing}</span> },
    { key: "skipped", header: "Skipped", sortValue: (c) => c.skipped, cell: (c) => <span className="num text-xs text-muted-foreground">{c.skipped}</span> },
    {
      key: "nodes",
      header: "Affected nodes",
      sortable: false,
      cell: (c) => (
        <span className="num block max-w-72 truncate text-[11px] text-muted-foreground">
          {c.nodes.length ? c.nodes.join(", ") : "—"}
        </span>
      ),
    },
  ];

  const exportJson = () => {
    downloadFile(
      "spindle-compliance-report.json",
      JSON.stringify({ generatedAt: "2026-08-22T18:40:00Z", totals: t, scans }, null, 2),
      "application/json",
    );
    toast.success("Compliance report exported (JSON)");
  };

  const exportCsv = () => {
    downloadFile(
      "spindle-compliance-controls.csv",
      toCsv(
        controlRollups.map((c) => ({
          control: c.id,
          title: c.title,
          profile: c.profileId,
          severity: c.severity,
          failing_nodes: c.failing,
          passing_nodes: c.passing,
          affected: c.nodes.join(" "),
        })),
      ),
      "text/csv",
    );
    toast.success("Control rollup exported (CSV)");
  };

  return (
    <div className="space-y-5">
      <PageHeader
        title="Compliance"
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Compliance" }]}
        description="Continuous auditing results from Cinc Auditor across every managed node."
        actions={
          <>
            <div className="flex items-center rounded-md border border-border p-0.5">
              {(["node", "profile"] as View[]).map((v) => (
                <button
                  key={v}
                  onClick={() => setView(v)}
                  className={cn(
                    "rounded px-2.5 py-1 text-xs capitalize transition-colors",
                    view === v ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:text-foreground",
                  )}
                >
                  {v} status
                </button>
              ))}
            </div>
            <Button variant="outline" size="sm" className="h-8 gap-1.5 text-xs" onClick={exportJson}>
              <FileJson className="size-3.5" /> JSON
            </Button>
            <Button variant="outline" size="sm" className="h-8 gap-1.5 text-xs" onClick={exportCsv}>
              <Download className="size-3.5" /> CSV
            </Button>
          </>
        }
      />

      {view === "node" ? (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <KpiCard label="Compliant nodes" value={nodes.filter((n) => n.compliance === "compliant").length} tone="ok" sub={`of ${nodes.length}`} />
          <KpiCard label="Non-compliant nodes" value={nodes.filter((n) => n.compliance === "non-compliant").length} tone="fail" sub="action needed" />
          <KpiCard label="Skipped / unknown" value={nodes.filter((n) => n.compliance === "skipped" || n.compliance === "unknown").length} tone="warn" sub="not audited" />
          <KpiCard label="Control pass rate" value={pct(passRate)} tone={passRate > 85 ? "ok" : "fail"} spark={complianceTrend30d.map((d) => d.passRate)} sparkTone={passRate > 85 ? "ok" : "fail"} />
        </div>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <KpiCard label="Installed profiles" value={installed.length} sub={`${profiles.length - installed.length} available`} />
          <KpiCard label="Profiles failing" value={installed.filter((p) => p.passRate <= 0.85).length} tone="fail" sub="below 85% pass" />
          <KpiCard label="Controls evaluated" value={total} sub="latest scan cycle" />
          <KpiCard label="Failing controls" value={t.failed} tone="fail" sub={`${t.waived} waived`} />
        </div>
      )}

      <div className="grid gap-4 xl:grid-cols-3">
        <Panel className="xl:col-span-2" title="Control pass rate" description="Fleet-wide, last 30 days">
          <TrendChart data={complianceTrend30d} height={210} />
          <div className="mt-4">
            <StackedMeter
              segments={[
                { label: "Passed", value: t.passed, tone: "ok" },
                { label: "Failed", value: t.failed, tone: "fail" },
                { label: "Waived", value: t.waived, tone: "warn" },
                { label: "Skipped", value: t.skipped, tone: "unknown" },
              ]}
            />
          </div>
        </Panel>

        <Panel
          title="Top failures"
          description="Where non-compliance is concentrated"
          actions={
            <div className="flex flex-wrap items-center gap-1">
              {(["platform", "environment", "profile", "control"] as FailureDim[]).map((d) => (
                <button
                  key={d}
                  onClick={() => setDim(d)}
                  className={cn(
                    "rounded-full border px-2 py-0.5 text-[11px] capitalize transition-colors",
                    dim === d ? "border-primary/40 bg-accent text-accent-foreground" : "border-border text-muted-foreground hover:text-foreground",
                  )}
                >
                  {d}
                </button>
              ))}
            </div>
          }
        >
          <FailureBars dim={dim} />
        </Panel>
      </div>

      <Tabs defaultValue="nodes">
        <TabsList>
          <TabsTrigger value="nodes">Nodes</TabsTrigger>
          <TabsTrigger value="profiles">Profiles</TabsTrigger>
          <TabsTrigger value="controls">Controls</TabsTrigger>
        </TabsList>

        <TabsContent value="nodes" className="mt-4">
          <DataTable
            columns={nodeColumns}
            rows={nodeRows}
            getRowKey={(n) => n.id}
            searchText={(n) => `${n.name} ${n.environment} ${n.platform}`}
            searchPlaceholder="Search nodes…"
            initialSort={{ key: "failed", dir: "desc" }}
            onRowClick={(n) => navigate({ to: "/nodes/$nodeId", params: { nodeId: n.id } })}
            filters={[
              { id: "env", label: "Environment", options: environments, selected: nodeEnv, onChange: setNodeEnv },
              { id: "plat", label: "Platform", options: platforms, selected: nodePlat, onChange: setNodePlat },
              {
                id: "status",
                label: "Compliance",
                options: ["compliant", "non-compliant", "skipped", "unknown"],
                selected: nodeStatus,
                onChange: setNodeStatus,
              },
              {
                id: "profile",
                label: "Profile",
                options: installed.map((p) => p.name),
                selected: nodeProfile,
                onChange: setNodeProfile,
              },
            ]}
          />
        </TabsContent>

        <TabsContent value="profiles" className="mt-4">
          <DataTable
            columns={profileColumns}
            rows={installed}
            getRowKey={(p) => p.id}
            searchText={(p) => `${p.title} ${p.name} ${p.vendor}`}
            searchPlaceholder="Search profiles…"
            initialSort={{ key: "rate", dir: "asc" }}
            onRowClick={(p) => navigate({ to: "/profiles/$profileId", params: { profileId: p.id } })}
            pageSize={8}
          />
        </TabsContent>

        <TabsContent value="controls" className="mt-4">
          <DataTable
            columns={controlColumns}
            rows={controlRows}
            getRowKey={(c) => `${c.profileId}-${c.id}`}
            searchText={(c) => `${c.id} ${c.title} ${c.profileId} ${c.nodes.join(" ")}`}
            searchPlaceholder="Search control ID or title…"
            initialSort={{ key: "failing", dir: "desc" }}
            pageSize={12}
            density="compact"
            filters={[
              {
                id: "profile",
                label: "Profile",
                options: installed.map((p) => p.id),
                selected: ctrlProfile,
                onChange: setCtrlProfile,
              },
              {
                id: "severity",
                label: "Severity",
                options: ["critical", "high", "medium", "low"],
                selected: ctrlSeverity,
                onChange: setCtrlSeverity,
              },
              { id: "state", label: "State", options: ["failing", "passing"], selected: ctrlState, onChange: setCtrlState },
            ]}
            emptyTitle="No controls match"
          />
        </TabsContent>
      </Tabs>
    </div>
  );
}
