import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Download, FileJson } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { TrendChart } from "@/components/spindle/charts";
import { SeverityBadge, StatusPill } from "@/components/spindle/status";
import { KpiCard, PageHeader, Panel, EmptyState } from "@/components/spindle/ui-bits";
import { useNodes, useComplianceReports, useComplianceTrend, useControlRollups, useComplianceProfiles } from "@/lib/api";
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

  const { data: nodes, isLoading: nodesLoading, error: nodesError } = useNodes({ limit: 100 });

  const { data: scans, isLoading: scansLoading, error: scansError } = useComplianceReports({ limit: 100 });

  const { data: complianceTrendItems } = useComplianceTrend(30, { enabled: !!nodes });

  const { data: controlRollups } = useControlRollups();
  const { data: profiles } = useComplianceProfiles();

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
    return nodes
      .map((n) => {
        // Enrich node with compliance data from scans (server doesn't include it in node list)
        const nodeScans = scans.filter((s) => s.nodeId === n.id);
        const totalFailed = nodeScans.reduce((sum, s) => sum + s.failed, 0);
        const compliance = (totalFailed > 0 ? "non-compliant" : nodeScans.length > 0 ? "compliant" : "unknown") as FleetNode["compliance"];
        return { ...n, compliance };
      })
      .filter((n) => {
        const nodeProfiles = scans.find((s) => s.nodeId === n.id)?.profiles.map((p) => p.profileName) ?? [];
        return (
          (nodeEnv.length === 0 || nodeEnv.includes(n.environment)) &&
          (nodePlat.length === 0 || nodePlat.includes(n.platform)) &&
          (nodeStatus.length === 0 || nodeStatus.includes(n.compliance)) &&
          (nodeProfile.length === 0 || nodeProfile.some((p) => nodeProfiles.includes(p)))
        );
      });
  }, [nodes, scans, nodeEnv, nodePlat, nodeStatus, nodeProfile]);

  // Build control rollups from compliance report data (for CSV export)
  const localControlRollups: ControlRollup[] = useMemo(() => {
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

  // Use server-aggregated rollups when available, fall back to local rollup
  const controlRows = (controlRollups ?? localControlRollups);

  // Derive per-node compliance status from scans (node list doesn't include compliance)
  const nodeCompliance = useMemo(() => {
    if (!scans || scans.length === 0) return { compliant: 0, nonCompliant: 0, unknown: 0 };
    const seen = new Map<string, { passed: number; failed: number }>();
    for (const scan of scans) {
      const key = scan.nodeId;
      const existing = seen.get(key);
      if (existing) {
        existing.passed += scan.passed;
        existing.failed += scan.failed;
      } else {
        seen.set(key, { passed: scan.passed, failed: scan.failed });
      }
    }
    let compliant = 0, nonCompliant = 0, unknown = 0;
    if (nodes) {
      for (const node of nodes) {
        const agg = seen.get(node.id);
        if (agg) {
          if (agg.failed > 0) nonCompliant++;
          else if (agg.passed > 0) compliant++;
          else unknown++;
        } else {
          unknown++;
        }
      }
    }
    return { compliant, nonCompliant, unknown };
  }, [scans, nodes]);

  // Compute compliance pass rate from trend
  const compliancePassRate = useMemo(() => {
    if (!complianceTrendItems || complianceTrendItems.length === 0) return 0;
    const latest = complianceTrendItems[complianceTrendItems.length - 1];
    return latest?.passRate ?? 0;
  }, [complianceTrendItems]);

  // Profile view stats
  const profileStats = useMemo(() => {
    if (!profiles) return { profliesFailing: 0, totalControls: 0, failingControls: 0, warned: 0 };
    const profliesFailing = profiles.filter((p) => p.passRate < 0.85).length;
    let totalControls = 0, failingControls = 0, warned = 0;
    for (const rollup of controlRows) {
      totalControls += rollup.failing + rollup.passing + rollup.warnings;
      failingControls += rollup.failing;
      warned += rollup.warnings;
    }
    return { profliesFailing, totalControls, failingControls, warned };
  }, [profiles, controlRows]);

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
          <KpiCard label="Compliant nodes" value={nodeCompliance?.compliant ?? 0} tone="ok" sub={`of ${nodes?.length ?? 0}`} />
          <KpiCard label="Non-compliant nodes" value={nodeCompliance?.nonCompliant ?? 0} tone="fail" sub="action needed" />
          <KpiCard label="Skipped / unknown" value={nodeCompliance?.unknown ?? 0} tone="warn" sub="not audited" />
          <KpiCard label="Control pass rate" value={pct(compliancePassRate)} tone={compliancePassRate < 85 ? "fail" : "ok"} spark={complianceTrendItems?.map((i) => i.passRate) ?? []} />
        </div>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <KpiCard label="Installed profiles" value={profiles?.length ?? 0} sub="available" />
          <KpiCard label="Profiles failing" value={profileStats.profliesFailing} tone="fail" sub="below 85% pass" />
          <KpiCard label="Controls evaluated" value={profileStats.totalControls} sub="latest scan cycle" />
          <KpiCard label="Failing controls" value={profileStats.failingControls} tone="fail" sub={`${profileStats.warned} warned`} />
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
            <EmptyState title="No compliance trend data" description="Compliance trend chart will render when scan data is available." />
          )}
        </Panel>
        <Panel
          title="Top failures"
          description="Where non-compliance is concentrated"
          bodyClassName="p-0"
        >
          {(() => {
            const failingRollups = (controlRows ?? [])
              .filter((c) => c.failing > 0)
              .sort((a, b) => b.failing - a.failing)
              .slice(0, 8);
            if (failingRollups.length === 0) {
              return (
                <EmptyState title="No failing controls" description="All controls are passing." />
              );
            }
            return (
              <ul className="divide-y divide-border/60">
                {failingRollups.map((c) => (
                  <li key={`${c.profileId}-${c.id}`} className="px-4 py-2.5">
                    <div className="flex items-start justify-between gap-3">
                      <div className="min-w-0">
                        <div className="num text-[11px] text-muted-foreground">{c.id}</div>
                        <div className="max-w-sm truncate text-xs text-foreground">{c.title}</div>
                      </div>
                      <div className="text-right">
                        <span className="num text-xs text-fail">{c.failing} failing</span>
                        <span className="num block text-[11px] text-muted-foreground">{c.nodes.length} nodes</span>
                      </div>
                    </div>
                  </li>
                ))}
              </ul>
            );
          })()}
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
            loading={!controlRows.length && !scans?.length}
            emptyTitle="No controls match"
          />
        </TabsContent>
      </Tabs>
    </div>
  );
}
