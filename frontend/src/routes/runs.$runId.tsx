import { createFileRoute, Link, notFound } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Download, ExternalLink } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { StackedMeter } from "@/components/spindle/charts";
import { StatusDot, StatusPill } from "@/components/spindle/status";
import { CodeBlock, KpiCard, MetaGrid, PageHeader, Panel } from "@/components/spindle/ui-bits";
import { nodeById, runById } from "@/lib/mock/data";
import { absTime, downloadFile, duration, ms, relTime } from "@/lib/format";
import type { ResourceEvent } from "@/lib/mock/types";
import { toast } from "sonner";

export const Route = createFileRoute("/runs/$runId")({
  loader: ({ params }) => {
    const run = runById(params.runId);
    if (!run) throw notFound();
    return { id: run.id, nodeName: run.nodeName, status: run.status };
  },
  head: ({ loaderData }) => {
    if (!loaderData) {
      return { meta: [{ title: "Run not found — Spindle" }, { name: "robots", content: "noindex" }] };
    }
    const title = `Run ${loaderData.id} on ${loaderData.nodeName} — Spindle`;
    const description = `Resource-level converge report (${loaderData.status}) for ${loaderData.nodeName}, including error log output.`;
    return {
      meta: [
        { title },
        { name: "description", content: description },
        { property: "og:title", content: title },
        { property: "og:description", content: description },
      ],
    };
  },
  component: RunDetail,
});

function RunDetail() {
  const { runId } = Route.useParams();
  const run = runById(runId)!;
  const node = nodeById(run.nodeId);
  const [statusFilter, setStatusFilter] = useState<string[]>([]);
  const [typeFilter, setTypeFilter] = useState<string[]>([]);

  const resources = useMemo(
    () =>
      run.resources.filter(
        (r) =>
          (statusFilter.length === 0 || statusFilter.includes(r.status)) &&
          (typeFilter.length === 0 || typeFilter.includes(r.type)),
      ),
    [run.resources, statusFilter, typeFilter],
  );

  const columns: Column<ResourceEvent>[] = [
    { key: "type", header: "Type", sortValue: (r) => r.type, cell: (r) => <span className="num text-xs">{r.type}</span> },
    {
      key: "name",
      header: "Name",
      sortValue: (r) => r.name,
      cell: (r) => <span className="num block max-w-80 truncate text-xs text-foreground">{r.name}</span>,
    },
    { key: "action", header: "Action", sortValue: (r) => r.action, cell: (r) => <span className="num text-xs text-muted-foreground">{r.action}</span> },
    { key: "cookbook", header: "Cookbook", sortValue: (r) => r.cookbook, cell: (r) => <span className="num text-xs">{r.cookbook}</span> },
    { key: "status", header: "Status", sortValue: (r) => r.status, cell: (r) => <StatusPill status={r.status} size="sm" /> },
    {
      key: "delta",
      header: "Detail",
      sortable: false,
      cell: (r) => <span className="text-[11px] text-muted-foreground">{r.delta ?? "—"}</span>,
    },
    {
      key: "duration",
      header: "Duration",
      sortValue: (r) => r.durationMs,
      className: "text-right",
      headerClassName: "text-right",
      cell: (r) => <span className="num text-xs">{ms(r.durationMs)}</span>,
    },
  ];

  const upToDate = run.resources.filter((r) => r.status === "up-to-date").length;
  const skipped = run.resources.filter((r) => r.status === "skipped").length;

  return (
    <div className="space-y-5">
      <PageHeader
        breadcrumbs={[
          { label: "Fleet", to: "/" },
          { label: "Converge runs", to: "/runs" },
          { label: run.id },
        ]}
        title={
          <span className="flex items-center gap-2">
            <StatusDot status={run.status} pulse />
            <span className="num">{run.id}</span>
          </span>
        }
        description={
          <span className="num">
            {run.nodeName} · started {absTime(run.startedAt)} ({relTime(run.startedAt)})
          </span>
        }
        actions={
          <>
            {node && (
              <Button variant="outline" size="sm" className="h-8 gap-1.5 text-xs" asChild>
                <Link to="/nodes/$nodeId" params={{ nodeId: node.id }}>
                  <ExternalLink className="size-3.5" /> Node detail
                </Link>
              </Button>
            )}
            <Button
              variant="outline"
              size="sm"
              className="h-8 gap-1.5 text-xs"
              onClick={() => {
                downloadFile(`${run.id}.json`, JSON.stringify(run, null, 2), "application/json");
                toast.success(`Downloaded ${run.id}.json`);
              }}
            >
              <Download className="size-3.5" /> Run report (JSON)
            </Button>
          </>
        }
        meta={
          <div className="flex flex-wrap items-center gap-2 pt-1">
            <StatusPill status={run.status} />
            <span className="num text-xs text-muted-foreground">{run.cookbook}</span>
          </div>
        }
      />

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <KpiCard label="Duration" value={duration(run.durationSec)} sub="wall clock" />
        <KpiCard label="Resources" value={run.totalResources} sub="in run list" />
        <KpiCard label="Updated" value={run.updatedResources} tone="ok" sub="converged changes" />
        <KpiCard
          label="Failed"
          value={run.failedResources}
          tone={run.failedResources ? "fail" : "ok"}
          sub={run.failedResources ? "blocking" : "none"}
        />
      </div>

      <div className="panel p-4">
        <MetaGrid
          items={[
            { label: "Node", value: <span className="num">{run.nodeName}</span> },
            { label: "Environment", value: <span className="capitalize">{run.environment}</span> },
            { label: "Policy", value: <span className="num">{run.cookbook}</span> },
            { label: "Cinc Client", value: <span className="num">18.4.12</span> },
            { label: "Started", value: <span className="num">{absTime(run.startedAt)}</span> },
            { label: "Run list", value: <span className="num truncate">{run.runList.join(", ")}</span> },
          ]}
        />
        <div className="mt-4 max-w-md">
          <StackedMeter
            segments={[
              { label: "Updated", value: run.updatedResources, tone: "ok" },
              { label: "Failed", value: run.failedResources, tone: "fail" },
              { label: "Skipped", value: skipped, tone: "warn" },
              { label: "Up to date", value: upToDate, tone: "unknown" },
            ]}
          />
        </div>
      </div>

      {run.errorLog && (
        <Panel
          title="Error log"
          description={run.errorSummary}
          actions={
            <Button
              variant="ghost"
              size="sm"
              className="h-7 text-xs"
              onClick={() => {
                downloadFile(`${run.id}-stacktrace.log`, run.errorLog!, "text/plain");
                toast.success("Downloaded stacktrace");
              }}
            >
              Download log
            </Button>
          }
        >
          <CodeBlock content={run.errorLog} />
        </Panel>
      )}

      <Panel title="Resource events" description="Every resource evaluated during this converge" bodyClassName="p-4">
        <DataTable
          columns={columns}
          rows={resources}
          getRowKey={(r) => r.id}
          searchText={(r) => `${r.type} ${r.name} ${r.action} ${r.cookbook}`}
          searchPlaceholder="Search resource name or type…"
          initialSort={{ key: "duration", dir: "desc" }}
          pageSize={12}
          density="compact"
          filters={[
            {
              id: "status",
              label: "Status",
              options: ["updated", "up-to-date", "skipped", "failed"],
              selected: statusFilter,
              onChange: setStatusFilter,
            },
            {
              id: "type",
              label: "Resource type",
              options: [...new Set(run.resources.map((r) => r.type))],
              selected: typeFilter,
              onChange: setTypeFilter,
            },
          ]}
          emptyTitle="No resource events"
          emptyDescription={run.status === "missing" ? "This node never reported a converge for this cycle." : "Clear filters to see all resources."}
        />
      </Panel>
    </div>
  );
}
