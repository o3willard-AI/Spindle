import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { StatusPill } from "@/components/spindle/status";
import { KpiCard, PageHeader } from "@/components/spindle/ui-bits";
import { ConvergeChart } from "@/components/spindle/charts";
import { Panel } from "@/components/spindle/ui-bits";
import { convergeSuccess14d, environments, nodes, runs } from "@/lib/mock/data";
import { absTime, duration, relTime } from "@/lib/format";
import type { Run } from "@/lib/mock/types";

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

  const rows = useMemo(
    () =>
      runs.filter(
        (r) =>
          (status.length === 0 || status.includes(r.status)) &&
          (env.length === 0 || env.includes(r.environment)) &&
          (node.length === 0 || node.includes(r.nodeName)),
      ),
    [status, env, node],
  );

  const failed = runs.filter((r) => r.status === "failed").length;
  const success = runs.filter((r) => r.status === "success").length;
  const missing = runs.filter((r) => r.status === "missing").length;

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

  return (
    <div className="space-y-5">
      <PageHeader
        title="Converge runs"
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Converge runs" }]}
        description={`${runs.length} converge reports from the last 32 hours`}
      />

      <div className="grid gap-4 lg:grid-cols-4">
        <div className="grid gap-3 sm:grid-cols-3 lg:col-span-2 lg:grid-cols-1">
          <KpiCard label="Failed" value={failed} tone="fail" sub="needs attention" />
          <KpiCard label="Successful" value={success} tone="ok" sub="converged clean" />
          <KpiCard label="Missing" value={missing} tone="warn" sub="no report received" />
        </div>
        <Panel className="lg:col-span-2" title="Daily converge outcomes" description="Successful vs failed runs, last 14 days">
          <ConvergeChart data={convergeSuccess14d} height={228} />
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
          { id: "node", label: "Node", options: nodes.map((n) => n.name), selected: node, onChange: setNode },
        ]}
        emptyTitle="No runs match these filters"
      />
    </div>
  );
}
