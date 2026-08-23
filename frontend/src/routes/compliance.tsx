import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Download, FileJson } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { TrendChart } from "@/components/spindle/charts";
import { SeverityBadge, StatusPill } from "@/components/spindle/status";
import { KpiCard, PageHeader, Panel, EmptyState } from "@/components/spindle/ui-bits";
import { fetchComplianceReports, fetchNodes, fetchComplianceTrend } from "@/lib/api";
import { downloadFile, pct, relTime, toCsv } from "@/lib/format";
import type { ControlRollup, FleetNode, Scan } from "@/lib/mock/types";
import { cn } from "@/lib/utils";
import { toast } from "sonner";

export const Route = createFileRoute("/compliance")({
  head: () => ({
    meta: [
      { title: "Compliance — Spindle Continuous Auditing" },
      {
        name: "description",
        content:
          "Continuous compliance across the fleet: pass/fail/warning breakdown, control pass-rate trend and the controls failing on the most nodes.",
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

  const { data: nodes, isLoading: nodesLoading, error: nodesError } = useQuery<FleetNode[]>({
    queryKey: ["nodes", { limit: 100 }],
    queryFn: () => fetchNodes({ limit: 100 }),
  });

  const { data: scans, isLoading: scansLoading, error: scansError } = useQuery<Scan[]>({
    queryKey: ["compliance", { limit: 100 }],
    queryFn: () => fetchComplianceReports({ limit: 100 }),
  });

  const { data: complianceTrendItems } = useQuery({
    queryKey: ["compliance-trend"],
    queryFn: () => fetchComplianceTrend(30),
    enabled: !!nodes,
  });

  const loading = nodesLoading || scansLoading;

  if (nodesError || scansError) {
    return (
      <div className="space-y-5">
        <PageHeader
          title="Compliance"
          breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Compliance" }]}
          description="Unable to load compliance data."
        />
        <Panel>
          <EmptyState title="Could not load compliance data" description="Check your API token and server connectivity." />
        </Panel>
      </div>
    );
  }

  const nodeRows = useMemo(() => {
    if (!nodes || !scans) return [];
    return nodes.filter((n) => {
      const nodeProfiles = scans.find((s) => s.nodeId === n.id)?.profiles.map((p) => p.profileName) ?? [];
      return (
        (nodeEnv.length === 0 || nodeEnv.includes(n.environment)) &&
        (nodePlat.length === 0 || nodePlat.includes(n.platform)) &&
        (nodeStatus.length === 0 || nodeStatus.includes(n.compliance)) &&
        (nodeProfile.length === 0 || nodeProfile.some((p) => nodeProfiles.includes(p)))
      );
    });
  }, [nodes, scans, nodeEnv, nodePlat, nodeStatus, nodeProfile]);

  // Build control rollups from compliance report data
  const controlRows: ControlRollup[] = useMemo(() => {
    if (!scans || scans.length === 0) return [];
    const rollupMap = new Map<string, ControlRollup>();
    for (const scan of scans) {
      for (const profile of scan.profiles) {
        for (const control of profile.controls) {
          const key = `${profile.profileId}-${control.id}`;
          if (!rollupMap.has(key)) {
            rollupMap.set(key, {
              id: control.id,
              title: control.title,
              profileId: profile.profileId,
              profileTitle: profile.profileName,
              severity: control.severity,
              impact: control.impact,
              failing: 0,
              passing: 0,
              warnings: 0,
              nodes: [],
            });
          }
          const rollup = rollupMap.get(key)!;
          rollup.nodes.push(scan.nodeId);
          const allResults = control.results ?? [];
          rollup.failing += allResults.filter((r) => r.status === "failed").length;
          rollup.passing += allResults.filter((r) => r.status === "passed").length;
          rollup.warnings += allResults.filter((r) => r.status === "skipped").length;
        }
      }
    }
    return Array.from(rollupMap.values());
  }, [scans]);

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
      sortValue: (n) => n.passed,
      cell: (n) => <span className="num text-xs text-ok">{n.passed}</span>,
    },
    {
      key: "failed",
      header: "Failed",
      sortValue: (n) => n.failed,
      cell: (n) => <span className={cn("num text-xs", n.failed ? "text-fail" : "text-muted-foreground")}>{n.failed}</span>,
    },
    {
      key: "warnings",
      header: "Warnings",
      sortValue: (n) => n.warnings,
      cell: (n) => <span className="num text-xs text-warn">{n.warnings}</span>,
    },
    {
      key: "rate",
      header: "Pass rate",
      sortValue: (n) => n.passed / Math.max(1, n.passed + n.failed),
      cell: (n) => (
        <span className="num text-xs">
          {pct((n.passed / Math.max(1, n.passed + n.failed)) * 100)}
        </span>
      ),
    },
    {
      key: "trend",
      header: "30d",
      sortable: false,
      cell: (n) => <TrendChart data={n.complianceTrend.map((v, i) => ({ label: String(i), passRate: v }))} height={20} />,
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

  const exportJson = () => {
    downloadFile(
      "spindle-compliance-report.json",
      JSON.stringify({ generatedAt: new Date().toISOString(), scans }, null, 2),
      "application/json",
    );
    toast.success("Compliance report exported (JSON)");
  };

  const exportCsv = () => {
    downloadFile(
      "spindle-compliance-controls.csv",
      toCsv(
        controlRows.map((c) => ({
          control: c.id,
          title: c.title,
          profile: c.profileId,
          severity: c.severity,
          failing_nodes: c.failing,
          passing_nodes: c.passing,
          warnings: c.warnings,
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
          <KpiCard label="Compliant nodes" value={nodes?.filter((n) => n.compliance === "compliant").length ?? 0} tone="ok" sub={`of ${nodes?.length ?? 0}`} />
          <KpiCard label="Non-compliant nodes" value={nodes?.filter((n) => n.compliance === "non-compliant").length ?? 0} tone="fail" sub="action needed" />
          <KpiCard label="Skipped / unknown" value={nodes?.filter((n) => n.compliance === "unknown").length ?? 0} tone="warn" sub="not audited" />
          <KpiCard label="Control pass rate" value={pct(0)} tone="ok" />
        </div>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <KpiCard label="Installed profiles" value={0} sub="available" />
          <KpiCard label="Profiles failing" value={0} tone="fail" sub="below 85% pass" />
          <KpiCard label="Controls evaluated" value={0} sub="latest scan cycle" />
          <KpiCard label="Failing controls" value={0} tone="fail" sub="0 warned" />
        </div>
      )}

      <div className="grid gap-4 xl:grid-cols-3">
        <Panel className="xl:col-span-2" title="Control pass rate" description="Fleet-wide, last 30 days">
          {complianceTrendItems && complianceTrendItems.length > 0 ? (
            <TrendChart
              data={complianceTrendItems.map((item) => ({ label: item.date, passRate: item.passRate }))}
              height={228}
            />
          ) : (
            <EmptyState title="Trend data coming soon" description="Compliance trend chart will render when scan data is available." />
          )}
        </Panel>
        <Panel
          title="Top failures"
          description="Where non-compliance is concentrated"
        >
          <EmptyState title="No failing controls" description="All controls are passing." />
        </Panel>
      </div>

      <Tabs defaultValue="nodes">
        <TabsList>
          <TabsTrigger value="nodes">Nodes</TabsTrigger>
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
              { id: "env", label: "Environment", options: ["production", "staging", "development"], selected: nodeEnv, onChange: setNodeEnv },
              { id: "plat", label: "Platform", options: ["ubuntu", "rhel", "debian", "amazon", "sles"], selected: nodePlat, onChange: setNodePlat },
              {
                id: "status",
                label: "Compliance",
                options: ["compliant", "non-compliant", "skipped", "unknown"],
                selected: nodeStatus,
                onChange: setNodeStatus,
              },
            ]}
            loading={loading}
            emptyTitle="No nodes match these filters"
          />
        </TabsContent>
        <TabsContent value="controls" className="mt-4">
          <DataTable
            columns={[
              {
                key: "id",
                header: "Control",
                sortValue: (c) => c.id,
                cell: (c) => (
                  <div className="min-w-0">
                    <div className="num text-[11px] text-muted-foreground">{c.id}</div>
                    <div className="max-w-lg truncate text-xs">{c.title}</div>
                  </div>
                ),
              },
              { key: "severity", header: "Severity", sortValue: (c) => c.impact, cell: (c) => <SeverityBadge severity={c.severity} impact={c.impact} /> },
              { key: "tests", header: "Tests", sortValue: (c) => c.passing + c.failing + c.warnings, cell: (c) => <span className="num text-xs">{c.passing + c.failing + c.warnings}</span> },
              { key: "failing", header: "Failing nodes", sortValue: (c) => c.failing, cell: (c) => <span className={cn("num text-xs", c.failing ? "text-fail" : "text-muted-foreground")}>{c.failing}</span> },
              { key: "passing", header: "Passing nodes", sortValue: (c) => c.passing, cell: (c) => <span className="num text-xs text-ok">{c.passing}</span> },
              {
                key: "status",
                header: "Status",
                sortValue: (c) => (c.failing ? 0 : 1),
                cell: (c) => <StatusPill status={c.failing ? "failed" : "passed"} size="sm" />,
              },
            ]}
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
                options: Array.from(new Set(controlRows.map((c) => c.profileId))),
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
            loading={false}
            emptyTitle="No controls match"
          />
        </TabsContent>
      </Tabs>
    </div>
  );
}
