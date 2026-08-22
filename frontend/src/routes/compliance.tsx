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
import { fetchComplianceReports, fetchNodes } from "@/lib/api";
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

  const controlRows: ControlRollup[] = [];

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
          <KpiCard label="Skipped / unknown" value={nodes?.filter((n) => n.compliance === "skipped" || n.compliance === "unknown").length ?? 0} tone="warn" sub="not audited" />
          <KpiCard label="Control pass rate" value={pct(0)} tone="ok" />
        </div>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <KpiCard label="Installed profiles" value={0} sub="available" />
          <KpiCard label="Profiles failing" value={0} tone="fail" sub="below 85% pass" />
          <KpiCard label="Controls evaluated" value={0} sub="latest scan cycle" />
          <KpiCard label="Failing controls" value={0} tone="fail" sub="0 waived" />
        </div>
      )}

      <div className="grid gap-4 xl:grid-cols-3">
        <Panel className="xl:col-span-2" title="Control pass rate" description="Fleet-wide, last 30 days">
          <EmptyState title="Trend data coming soon" description="Compliance trend chart will render when scan data is available." />
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
            columns={[]}
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
                options: [],
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
