import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { StatusPill } from "@/components/spindle/status";
import { KpiCard, PageHeader, Panel, EmptyState } from "@/components/spindle/ui-bits";
import { ConvergeChart } from "@/components/spindle/charts";
import { useRuns, useNodes, useRunsTrend } from "@/lib/api";
import { absTime, duration, relTime } from "@/lib/format";
import type { FleetNode, Run } from "@/lib/mock/types";

export const Route = createFileRoute("/runs/")({
  head: () => ({
    meta: [
      { title: "Converge Runs — Spindle" },
      {
        name: "description",
        content:
          "Every Cinc Client converge report across the fleet with status, duration, resource counts and failure drill-down.",
      },
      { property: "og:title", content: "Converge Runs — Spindle" },
      { property: "og:description", content: "Converge reports with resource events and error logs." },
    ],
  }),
  component: RunsPage,
});

function RunsPage() {
  const navigate = useNavigate();
  const [status, setStatus] = useState<string[]>([]);
  const [env, setEnv] = useState<string[]>([]);
  const [node, setNode] = useState<string[]>([]);

  const {
    data: runs,
    isLoading,
    error,
  } = useRuns({ limit: 200 });

  const { data: nodes } = useNodes({ limit: 100 });

  const { data: runsTrendItems } = useRunsTrend(14);

  // Build a nodeId → {name, environment} map for enriching run list
  const nodeMap = useMemo(() => {
    if (!nodes) return new Map();
    const m = new Map<string, { name: string; environment: string }>();
    for (const n of nodes) {
      m.set(n.id, { name: n.name, environment: n.environment });
    }
    return m;
  }, [nodes]);

  const enrichedRuns = useMemo(() => {
    if (!runs) return [];
    return runs.map((r) => ({
      ...r,
      nodeName: nodeMap.get(r.nodeId)?.name ?? r.nodeName,
      environment: nodeMap.get(r.nodeId)?.environment ?? r.environment,
    }));
  }, [runs, nodeMap]);

  const rows = useMemo(
    () =>
      (enrichedRuns ?? []).filter(
        (r) =>
          (status.length === 0 || status.includes(r.status)) &&
          (env.length === 0 || env.includes(r.environment)) &&
          (node.length === 0 || node.includes(r.nodeName)),
      ),
    [enrichedRuns, status, env, node],
  );

  const failed = (runs ?? []).filter((r) => r.status === "failed").length;
  const success = (runs ?? []).filter((r) => r.status === "success").length;
  const missing = (runs ?? []).filter((r) => r.status === "missing").length;

  const environments = useMemo(() => {
    if (!runs) return [];
    return [...new Set(runs.map((r) => r.environment))].sort();
  }, [runs]);

  const nodeNames = useMemo(() => {
    if (!runs) return [];
    return [...new Set(runs.map((r) => r.nodeName))].sort();
  }, [runs]);

  const columns: Column<Run>[] = [
    {
      key: "node",
      header: "Node",
      sortValue: (r) => r.nodeName,
      cell: (r) => (
        <div className="min-w-0">
          <div className="num truncate text-xs font-medium">{r.nodeName}</div>
          <div className="num text-[11px] text-muted-foreground">{r.id}</div>
        </div>
      ),
    },
    { key: "status", header: "Status", sortValue: (r) => r.status, cell: (r) => <StatusPill status={r.status} /> },
    {
      key: "startedAt",
      header: "Started",
      sortValue: (r) => r.startedAt,
      cell: (r) => (
        <span className="num text-xs text-muted-foreground" title={absTime(r.startedAt)}>
          {relTime(r.startedAt)}
        </span>
      ),
    },
    { key: "duration", header: "Duration", sortValue: (r) => r.durationSec, cell: (r) => <span className="num text-xs">{duration(r.durationSec)}</span> },
    {
      key: "resources",
      header: "Resources",
      sortValue: (r) => r.totalResources,
      cell: (r) => (
        <span className="num text-xs">
          {r.totalResources}
          <span className="text-muted-foreground"> total · </span>
          <span className="text-ok">{r.updatedResources} upd</span>
          {r.failedResources > 0 && <span className="text-fail"> · {r.failedResources} fail</span>}
        </span>
      ),
    },
    { key: "cookbook", header: "Policy / cookbook", sortValue: (r) => r.cookbook, cell: (r) => <span className="num text-xs">{r.cookbook}</span> },
    {
      key: "environment",
      header: "Environment",
      sortValue: (r) => r.environment,
      cell: (r) => <span className="text-xs capitalize">{r.environment}</span>,
    },
    {
      key: "error",
      header: "Failure",
      sortable: false,
      cell: (r) =>
        r.errorSummary ? (
          <span className="block max-w-72 truncate font-mono text-[11px] text-fail">{r.errorSummary}</span>
        ) : (
          <span className="text-[11px] text-muted-foreground">—</span>
        ),
    },
  ];

  if (error) {
    return (
      <div className="space-y-5">
        <PageHeader
          title="Converge runs"
          breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Converge runs" }]}
          description="Unable to load run history."
        />
        <Panel>
          <EmptyState title="Could not load runs" description="Check your API token and server connectivity." />
        </Panel>
      </div>
    );
  }

  return (
    <div className="space-y-5">
      <PageHeader
        title="Converge runs"
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Converge runs" }]}
        description={runs ? `${runs.length} converge reports` : "Loading run history…"}
      />

      <div className="grid gap-4 lg:grid-cols-4">
        <div className="grid gap-3 sm:grid-cols-3 lg:col-span-2 lg:grid-cols-1">
          <KpiCard label="Failed" value={failed} tone="fail" sub="needs attention" />
          <KpiCard label="Successful" value={success} tone="ok" sub="converged clean" />
          <KpiCard label="Missing" value={missing} tone="warn" sub="no report received" />
        </div>
        <Panel className="lg:col-span-2" title="Daily converge outcomes" description="Successful vs failed runs, last 14 days">
          {runsTrendItems && runsTrendItems.length > 0 ? (
            <ConvergeChart
              data={runsTrendItems.map((item) => ({
                label: item.date,
                success: item.success,
                failed: item.failed,
                rate: item.success + item.failed > 0 ? (item.success / (item.success + item.failed)) * 100 : 0,
              }))}
              height={228}
            />
          ) : (
            <EmptyState title="No trend data" description="Converge trend chart will render when run data is available." />
          )}
        </Panel>
      </div>

      <DataTable
        columns={columns}
        rows={rows}
        getRowKey={(r) => r.id}
        searchText={(r) => `${r.id} ${r.nodeName} ${r.cookbook} ${r.errorSummary ?? ""}`}
        searchPlaceholder="Search run ID, node, cookbook…"
        initialSort={{ key: "startedAt", dir: "desc" }}
        onRowClick={(r) => navigate({ to: "/runs/$runId", params: { runId: r.id } })}
        pageSize={12}
        density="compact"
        filters={[
          { id: "status", label: "Status", options: ["success", "failed", "missing"], selected: status, onChange: setStatus },
          { id: "env", label: "Environment", options: environments, selected: env, onChange: setEnv },
          { id: "node", label: "Node", options: nodeNames, selected: node, onChange: setNode },
        ]}
        loading={isLoading}
        emptyTitle="No runs match these filters"
      />
    </div>
  );
}
