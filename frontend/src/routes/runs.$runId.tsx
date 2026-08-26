import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Download, ExternalLink } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { StackedMeter } from "@/components/spindle/charts";
import { StatusDot, StatusPill } from "@/components/spindle/status";
import { CodeBlock, KpiCard, MetaGrid, PageHeader, Panel, EmptyState } from "@/components/spindle/ui-bits";
import { useRun, useNode } from "@/lib/api";
import { absTime, downloadFile, duration, relTime } from "@/lib/format";
import type { ResourceEvent } from "@/lib/mock/types";
import { toast } from "sonner";

export const Route = createFileRoute("/runs/$runId")({
  component: RunDetail,
});

function RunDetail() {
  const { runId } = Route.useParams();
  const navigate = useNavigate();

  const {
    data: run,
    isLoading: runLoading,
    error: runError,
  } = useRun(runId);

  const { data: node } = useNode(run?.nodeId ?? "");

  const [statusFilter, setStatusFilter] = useState<string[]>([]);
  const [typeFilter, setTypeFilter] = useState<string[]>([]);

  // Hoist all hooks BEFORE any conditional return. The useMemo handles
  // the not-yet-loaded case internally so the hook count is stable across
  // loading vs. loaded renders, preventing "Rendered more hooks than
  // during the previous render" (React 19 minified error #310).
  const resources = useMemo(
    () =>
      (run?.resources ?? []).filter(
        (r) =>
          (statusFilter.length === 0 || statusFilter.includes(r.status)) &&
          (typeFilter.length === 0 || typeFilter.includes(r.type)),
      ),
    [run?.resources, statusFilter, typeFilter],
  );

  if (runError) {
    return (
      <div className="space-y-5">
        <PageHeader
          title="Run not found"
          breadcrumbs={[
            { label: "Fleet", to: "/" },
            { label: "Converge runs", to: "/runs" },
            { label: "Not found" },
          ]}
          description="This converge run does not exist or you don't have access to it."
        />
        <Panel>
          <EmptyState title="Run not found" description="The requested converge run does not exist or has been removed." />
        </Panel>
      </div>
    );
  }

  if (runLoading || !run) {
    return (
      <div className="space-y-5">
        <Panel title="" description="" bodyClassName="p-4">
          <div className="h-8 w-3/4 animate-pulse rounded bg-muted" />
          <div className="mt-4 h-4 w-1/2 animate-pulse rounded bg-muted" />
        </Panel>
      </div>
    );
  }

  // Derive upToDate from the run summary counts, NOT from the (possibly paginated)
  // resource_events list. The API returns total_resource_count, updated_count,
  // failed_count, and skipped_count in the run summary. Resources that are
  // neither updated, failed, nor skipped are "up-to-date".
  const upToDate =
    run.totalResources - run.updatedResources - run.failedResources - (run.skippedResources ?? 0);
  const skipped = run.skippedResources ?? 0;

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
      sortValue: (r) => r.durationSec,
      className: "text-right",
      headerClassName: "text-right",
      cell: (r) => <span className="num text-xs">{duration(r.durationSec)}</span>,
    },
  ];

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

      {run.errorSummary && (
        <Panel
          title="Error log"
          description={run.errorSummary}
          actions={
            <Button
              variant="ghost"
              size="sm"
              className="h-7 text-xs"
              onClick={() => {
                downloadFile(`${run.id}-stacktrace.log`, run.errorLog ?? run.errorSummary ?? "", "text/plain");
                toast.success("Downloaded stacktrace");
              }}
            >
              Download log
            </Button>
          }
        >
          <CodeBlock content={run.errorLog ?? run.errorSummary ?? ""} />
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
              options: [...new Set((run.resources ?? []).map((r) => r.type))],
              selected: typeFilter,
              onChange: setTypeFilter,
            },
          ]}
          loading={false}
          emptyTitle={run.resources?.length === 0 ? "No resource events" : "No resource events match filters"}
          emptyDescription={run.status === "missing" ? "This node never reported a converge for this cycle." : "Clear filters to see all resources."}
        />
      </Panel>
    </div>
  );
}
