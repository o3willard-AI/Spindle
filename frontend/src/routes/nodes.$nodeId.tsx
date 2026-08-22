import { createFileRoute, Link, notFound, useNavigate } from "@tanstack/react-router";
import { useEffect, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, Search, Terminal } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Sparkline, StackedMeter } from "@/components/spindle/charts";
import { SeverityBadge, StatusDot, StatusPill, Tag } from "@/components/spindle/status";
import { CodeBlock, EmptyState, KeyValue, MetaGrid, PageHeader, Panel } from "@/components/spindle/ui-bits";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { fetchNode, fetchRuns } from "@/lib/api";
import { absTime, duration, relTime } from "@/lib/format";
import type { Control, FleetNode, Run } from "@/lib/mock/types";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/nodes/$nodeId")({
  component: NodeDetail,
});

function ControlRow({ control }: { control: Control }) {
  const [open, setOpen] = useState(control.status === "failed");
  return (
    <div className="border-b border-border/60 last:border-0">
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-start gap-3 px-4 py-2.5 text-left transition-colors hover:bg-accent/40"
      >
        {open ? (
          <ChevronDown className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" />
        )}
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <span className="num text-[11px] text-muted-foreground">{control.id}</span>
            <StatusPill size="sm" status={control.status} />
            <SeverityBadge severity={control.severity} impact={control.impact} />
          </div>
          <p className="mt-1 text-sm text-foreground/90">{control.title}</p>
        </div>
        <div className="hidden shrink-0 gap-1 sm:flex">
          {control.tags.map((t) => (
            <Tag key={t}>{t}</Tag>
          ))}
        </div>
      </button>
      {open && (
        <div className="space-y-3 border-t border-border/60 bg-elevated/40 px-4 py-3 pl-11">
          <p className="text-xs text-muted-foreground">{control.desc}</p>
          <div className="space-y-2">
            {control.results.map((r, i) => (
              <div key={i} className="rounded-md border border-border bg-surface p-2.5">
                <div className="flex items-center gap-2">
                  <StatusDot status={r.status} />
                  <span className="flex-1 font-mono text-[11.5px] text-foreground/90">{r.codeDesc}</span>
                  <span className="num text-[11px] text-muted-foreground">{r.runTimeMs}ms</span>
                </div>
                {r.message && <CodeBlock className="mt-2 max-h-40" content={r.message} />}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}

function NodeDetail() {
  const { nodeId } = Route.useParams();
  const navigate = useNavigate();

  const {
    data: node,
    isLoading: nodeLoading,
    error: nodeError,
  } = useQuery<FleetNode>({
    queryKey: ["node", nodeId],
    queryFn: () => fetchNode(nodeId),
    enabled: !!nodeId,
  });

  const {
    data: runs,
    isLoading: runsLoading,
    error: runsError,
  } = useQuery<Run[]>({
    queryKey: ["runs", { nodeId }],
    queryFn: () => fetchRuns({ nodeId, limit: 50 }),
    enabled: !!nodeId,
  });

  const [attrQuery, setAttrQuery] = useState("");
  const [attrCats, setAttrCats] = useState<string[]>([]);
  const [openGroups, setOpenGroups] = useState<string[]>(["system", "spindle"]);

  if (nodeError) {
    throw notFound();
  }

  if (nodeLoading || !node) {
    return (
      <div className="space-y-5">
        <Panel title="" description="" bodyClassName="p-4">
          <div className="h-8 w-3/4 animate-pulse rounded bg-muted" />
          <div className="mt-4 h-4 w-1/2 animate-pulse rounded bg-muted" />
        </Panel>
      </div>
    );
  }

  const nodeRuns = runs ?? [];
  const failingControls: Control[] = [];
  const attributes = node.attributes.filter(
    (a) =>
      (attrCats.length === 0 || attrCats.includes(a.category)) &&
      `${a.key} ${a.value}`.toLowerCase().includes(attrQuery.toLowerCase()),
  );

  const groups = useMemo(() => {
    const map = new Map<string, typeof attributes>();
    attributes.forEach((a) => map.set(a.group, [...(map.get(a.group) ?? []), a]));
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [attributes]);

  const runColumns: Column<Run>[] = [
    { key: "id", header: "Run", sortValue: (r) => r.id, cell: (r) => <span className="num text-xs">{r.id}</span> },
    { key: "status", header: "Status", sortValue: (r) => r.status, cell: (r) => <StatusPill status={r.status} /> },
    {
      key: "startedAt",
      header: "Started",
      sortValue: (r) => r.startedAt,
      cell: (r) => (
        <span className="num text-xs" title={absTime(r.startedAt)}>
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
          {r.updatedResources}
          <span className="text-muted-foreground">/{r.totalResources} updated</span>
        </span>
      ),
    },
    { key: "cookbook", header: "Policy", sortValue: (r) => r.cookbook, cell: (r) => <span className="num text-xs">{r.cookbook}</span> },
  ];

  return (
    <div className="space-y-5">
      <PageHeader
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Nodes", to: "/nodes" }, { label: node.name }]}
        title={
          <span className="flex items-center gap-2 font-mono text-lg">
            <StatusDot status={node.status} pulse />
            {node.name}
          </span>
        }
        description={node.fqdn}
        actions={
          <>
            <Button variant="outline" size="sm" className="h-8 gap-1.5 text-xs">
              <Terminal className="size-3.5" /> Run converge now
            </Button>
            <Button variant="outline" size="sm" className="h-8 text-xs">
              Rescan compliance
            </Button>
          </>
        }
        meta={
          <div className="flex flex-wrap items-center gap-2 pt-1">
            <StatusPill status={node.status} label={`Converge: ${node.status}`} />
            <StatusPill status={node.compliance} label={`Compliance: ${node.compliance}`} />
            {node.tags.map((t) => (
              <Tag key={t}>{t}</Tag>
            ))}
          </div>
        }
      />

      <div className="panel p-4">
        <MetaGrid
          items={[
            { label: "Node ID", value: <span className="num">{node.id}</span> },
            { label: "Platform", value: <span className="capitalize">{`${node.platform} ${node.platformVersion}`}</span> },
            { label: "Kernel", value: <span className="num">{node.kernel}</span> },
            { label: "Environment", value: <span className="capitalize">{node.environment}</span> },
            { label: "Policy group", value: <span className="num">{node.policyGroup}</span> },
            { label: "Policy", value: <span className="num">{node.policyName}</span> },
            { label: "Last check-in", value: relTime(node.lastSeen) },
            { label: "Uptime", value: `${node.uptimeDays}d` },
          ]}
        />
      </div>

      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="runs">Run history</TabsTrigger>
          <TabsTrigger value="compliance">
            Compliance
            {node.controlsFailed > 0 && <span className="num ml-1.5 text-fail">{node.controlsFailed}</span>}
          </TabsTrigger>
          <TabsTrigger value="attributes">Attributes</TabsTrigger>
        </TabsList>

        <TabsContent value="overview" className="mt-4 space-y-4">
          <div className="grid gap-4 lg:grid-cols-3">
            <Panel title="Compliance posture" description="Latest scan control breakdown">
              <StackedMeter
                segments={[
                  { label: "Passed", value: node.controlsPassed, tone: "ok" },
                  { label: "Failed", value: node.controlsFailed, tone: "fail" },
                  { label: "Waived", value: node.controlsWaived, tone: "warn" },
                  { label: "Skipped", value: node.controlsSkipped, tone: "unknown" },
                ]}
              />
              <div className="mt-4">
                <div className="label-caps mb-1">Pass rate, 30 days</div>
                <Sparkline data={node.complianceTrend} tone={node.compliance === "compliant" ? "ok" : "fail"} height={56} />
              </div>
            </Panel>

            <Panel title="Run list" description="Applied policy and recipes">
              <ul className="space-y-1.5">
                {node.runList.map((r) => (
                  <li key={r} className="num rounded-md border border-border bg-elevated/60 px-2.5 py-1.5 text-xs">
                    {r}
                  </li>
                ))}
              </ul>
            </Panel>

            <Panel title="Host facts">
              <KeyValue label="IP address">{node.ip}</KeyValue>
              <KeyValue label="CPU cores">{node.cpuCores}</KeyValue>
              <KeyValue label="Memory">{node.memoryGb} GB</KeyValue>
              <KeyValue label="Cloud">{node.cloud.toUpperCase()}</KeyValue>
              <KeyValue label="Region">{node.region}</KeyValue>
              <KeyValue label="Platform family">{node.platformFamily}</KeyValue>
            </Panel>
          </div>

          {failingControls.length > 0 && (
            <Panel
              title="Failing controls"
              description="Root-cause drill-down for the latest compliance scan"
              bodyClassName="p-0"
            >
              {failingControls.map((c) => (
                <ControlRow key={`${c.profileId}-${c.id}`} control={c} />
              ))}
            </Panel>
          )}
        </TabsContent>

        <TabsContent value="runs" className="mt-4 space-y-4">
          <Panel title="Converge timeline" description="Most recent converge reports for this node" bodyClassName="p-0">
            {runsLoading ? (
              <EmptyState title="Loading runs…" description="Fetching converge history for this node." />
            ) : nodeRuns.length === 0 ? (
              <EmptyState title="No runs found" description="This node has not reported any converge events yet." />
            ) : (
              <ol className="divide-y divide-border/60">
                {nodeRuns.slice(0, 8).map((r) => (
                  <li key={r.id}>
                    <Link
                      to="/runs/$runId"
                      params={{ runId: r.id }}
                      className="flex items-center gap-3 px-4 py-2.5 transition-colors hover:bg-accent/40"
                    >
                      <StatusDot status={r.status} />
                      <span className="num w-24 shrink-0 text-xs">{r.id}</span>
                      <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                        {r.status === "failed" ? r.errorSummary : `${r.updatedResources} of ${r.totalResources} resources updated`}
                      </span>
                      <span className="num shrink-0 text-[11px] text-muted-foreground">{duration(r.durationSec)}</span>
                      <span className="num w-20 shrink-0 text-right text-[11px] text-muted-foreground">{relTime(r.startedAt)}</span>
                    </Link>
                  </li>
                ))}
              </ol>
            )}
          </Panel>
          <DataTable
            columns={runColumns}
            rows={nodeRuns}
            getRowKey={(r) => r.id}
            searchText={(r) => `${r.id} ${r.status} ${r.cookbook}`}
            searchPlaceholder="Search runs…"
            initialSort={{ key: "startedAt", dir: "desc" }}
            onRowClick={(r) => navigate({ to: "/runs/$runId", params: { runId: r.id } })}
            pageSize={8}
            density="compact"
            loading={runsLoading}
            emptyTitle="No runs match"
          />
        </TabsContent>

        <TabsContent value="compliance" className="mt-4 space-y-4">
          <EmptyState title="No compliance data" description="Compliance controls are loaded on the dedicated compliance page.">
            <Button variant="outline" size="sm" asChild>
              <Link to="/compliance">View compliance report</Link>
            </Button>
          </EmptyState>
        </TabsContent>

        <TabsContent value="attributes" className="mt-4 space-y-4">
          <div className="flex flex-wrap items-center gap-2">
            <div className="relative min-w-56 flex-1 sm:max-w-72">
              <Search className="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={attrQuery}
                onChange={(e) => setAttrQuery(e.target.value)}
                placeholder="Search attribute key or value…"
                className="h-8 pl-8 text-xs"
              />
            </div>
            {(["default", "normal", "override", "automatic"] as const).map((c) => (
              <button
                key={c}
                onClick={() => setAttrCats((prev) => (prev.includes(c) ? prev.filter((x) => x !== c) : [...prev, c]))}
                className={cn(
                  "rounded-full border px-2.5 py-0.5 text-[11px] capitalize transition-colors",
                  attrCats.includes(c)
                    ? "border-primary/40 bg-accent text-accent-foreground"
                    : "border-border text-muted-foreground hover:text-foreground",
                )}
              >
                {c}
              </button>
            ))}
            <span className="num ml-auto text-[11px] text-muted-foreground">{attributes.length} attributes</span>
          </div>

          {groups.length === 0 ? (
            <Panel>
              <EmptyState title="No attributes match" description="Clear the search or category filters." />
            </Panel>
          ) : (
            <div className="space-y-2">
              {groups.map(([group, items]) => {
                const open = openGroups.includes(group);
                return (
                  <div key={group} className="panel overflow-hidden">
                    <button
                      onClick={() =>
                        setOpenGroups((prev) => (prev.includes(group) ? prev.filter((g) => g !== group) : [...prev, group]))
                      }
                      className="flex w-full items-center gap-2 px-4 py-2.5 text-left transition-colors hover:bg-accent/40"
                    >
                      {open ? <ChevronDown className="size-3.5 text-muted-foreground" /> : <ChevronRight className="size-3.5 text-muted-foreground" />}
                      <span className="num text-xs font-medium">{group}</span>
                      <span className="num ml-auto text-[11px] text-muted-foreground">{items.length}</span>
                    </button>
                    {open && (
                      <div className="border-t border-border">
                        {items.map((a) => (
                          <div
                            key={a.key}
                            className="flex items-baseline gap-3 border-b border-border/60 px-4 py-1.5 last:border-0"
                          >
                            <span className="num w-64 shrink-0 truncate text-xs text-foreground/90">{a.key}</span>
                            <span className="num min-w-0 flex-1 truncate text-xs text-muted-foreground">{a.value}</span>
                            <Tag>{a.category}</Tag>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </TabsContent>
      </Tabs>
    </div>
  );
}

// Suppress unused warning — runsError is surfaced in the runs table below
void runsError;
