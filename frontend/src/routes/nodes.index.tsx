import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Download } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { Sparkline } from "@/components/spindle/charts";
import { StatusPill, Tag } from "@/components/spindle/status";
import { KpiCard, PageHeader, Panel, EmptyState } from "@/components/spindle/ui-bits";
import { fetchNodes } from "@/lib/api";
import { relTime, toCsv, downloadFile } from "@/lib/format";
import type { FleetNode } from "@/lib/mock/types";
import { toast } from "sonner";

export const Route = createFileRoute("/nodes/")({
  head: () => ({
    meta: [
      { title: "Nodes — Spindle Fleet Inventory" },
      {
        name: "description",
        content:
          "Searchable inventory of every managed Linux node: platform, environment, policy group, converge status and last check-in.",
      },
      { property: "og:title", content: "Nodes — Spindle Fleet Inventory" },
      { property: "og:description", content: "Every managed Linux node with converge and compliance status." },
    ],
  }),
  component: NodesPage,
});

function NodesPage() {
  const navigate = useNavigate();
  const [env, setEnv] = useState<string[]>([]);
  const [plat, setPlat] = useState<string[]>([]);
  const [group, setGroup] = useState<string[]>([]);
  const [status, setStatus] = useState<string[]>([]);

  const {
    data: nodes,
    isLoading,
    error,
  } = useQuery<FleetNode[]>({
    queryKey: ["nodes", { limit: 100 }],
    queryFn: () => fetchNodes({ limit: 100 }),
  });

  const rows = useMemo(
    () =>
      (nodes ?? []).filter(
        (n) =>
          (env.length === 0 || env.includes(n.environment)) &&
          (plat.length === 0 || plat.includes(n.platform)) &&
          (group.length === 0 || group.includes(n.policyGroup)) &&
          (status.length === 0 || status.includes(n.status)),
      ),
    [nodes, env, plat, group, status],
  );

  const environments = useMemo(() => {
    if (!nodes) return [];
    return [...new Set(nodes.map((n) => n.environment))].sort();
  }, [nodes]);

  const platforms = useMemo(() => {
    if (!nodes) return [];
    return [...new Set(nodes.map((n) => n.platform))].sort();
  }, [nodes]);

  const policyGroups = useMemo(() => {
    if (!nodes) return [];
    return [...new Set(nodes.map((n) => n.policyGroup))].sort();
  }, [nodes]);

  const columns: Column<FleetNode>[] = [
    {
      key: "name",
      header: "Node",
      sortValue: (n) => n.name,
      cell: (n) => (
        <div className="min-w-0">
          <div className="num truncate text-xs font-medium text-foreground">{n.name}</div>
          <div className="num truncate text-[11px] text-muted-foreground">{n.ip}</div>
        </div>
      ),
    },
    {
      key: "platform",
      header: "Platform",
      sortValue: (n) => n.platform,
      cell: (n) => (
        <span className="text-xs capitalize">
          {n.platform} <span className="num text-muted-foreground">{n.platformVersion}</span>
        </span>
      ),
    },
    {
      key: "environment",
      header: "Environment",
      sortValue: (n) => n.environment,
      cell: (n) => <span className="text-xs capitalize">{n.environment}</span>,
    },
    {
      key: "policyGroup",
      header: "Policy group",
      sortValue: (n) => n.policyGroup,
      cell: (n) => (
        <div className="space-y-0.5">
          <div className="num text-xs">{n.policyGroup}</div>
          <div className="num text-[11px] text-muted-foreground">{n.policyName}</div>
        </div>
      ),
    },
    {
      key: "status",
      header: "Converge",
      sortValue: (n) => n.status,
      cell: (n) => <StatusPill status={n.status} />,
    },
    {
      key: "compliance",
      header: "Compliance",
      sortValue: (n) => n.compliance,
      cell: (n) => (
        <div className="flex items-center gap-2">
          <StatusPill status={n.compliance} />
          {n.controlsFailed > 0 && <span className="num text-[11px] text-fail">{n.controlsFailed} failing</span>}
        </div>
      ),
    },
    {
      key: "trend",
      header: "30d",
      sortable: false,
      cell: (n) => (
        <Sparkline data={n.complianceTrend} tone={n.compliance === "compliant" ? "ok" : "fail"} className="w-20" height={22} />
      ),
    },
    {
      key: "lastSeen",
      header: "Last seen",
      sortValue: (n) => n.lastSeen,
      className: "text-right",
      headerClassName: "text-right",
      cell: (n) => <span className="num text-[11px] text-muted-foreground">{relTime(n.lastSeen)}</span>,
    },
    {
      key: "tags",
      header: "Tags",
      sortable: false,
      cell: (n) => (
        <div className="flex flex-wrap gap-1">
          {n.tags.slice(0, 2).map((t) => (
            <Tag key={t}>{t}</Tag>
          ))}
        </div>
      ),
    },
  ];

  const fleetSummary = useMemo(() => {
    if (!nodes) return null;
    return {
      total: nodes.length,
      online: nodes.filter((n) => n.status !== "missing").length,
      offline: nodes.filter((n) => n.status === "missing").length,
      convergeSuccess: nodes.filter((n) => n.status === "success").length,
      convergeFailed: nodes.filter((n) => n.status === "failed").length,
      compliant: nodes.filter((n) => n.compliance === "compliant").length,
      nonCompliant: nodes.filter((n) => n.compliance === "non-compliant").length,
      unknownCompliance: nodes.filter((n) => n.compliance === "unknown" || n.compliance === "skipped").length,
      flipped: nodes.filter((n) => n.flipped),
    };
  }, [nodes]);

  if (error) {
    return (
      <div className="space-y-5">
        <PageHeader
          title="Nodes"
          breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Nodes" }]}
          description="Unable to load node inventory."
        />
        <Panel>
          <EmptyState
            title="Could not load nodes"
            description="Check your API token and server connectivity."
          />
        </Panel>
      </div>
    );
  }

  return (
    <div className="space-y-5">
      <PageHeader
        title="Nodes"
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Nodes" }]}
        description={nodes ? `${nodes.length} nodes managed by Spindle` : "Loading node inventory…"}
        actions={
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1.5 text-xs"
            disabled={!nodes || rows.length === 0}
            onClick={() => {
              downloadFile(
                "spindle-nodes.csv",
                toCsv(
                  rows.map((n) => ({
                    name: n.name,
                    platform: `${n.platform} ${n.platformVersion}`,
                    environment: n.environment,
                    policy_group: n.policyGroup,
                    converge: n.status,
                    compliance: n.compliance,
                    controls_failed: n.controlsFailed,
                    last_seen: n.lastSeen,
                  })),
                ),
                "text/csv",
              );
              toast.success("Exported node inventory (CSV)");
            }}
          >
            <Download className="size-3.5" /> Export
          </Button>
        }
      />

      {fleetSummary && (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <KpiCard label="Total" value={fleetSummary.total} sub="nodes" />
          <KpiCard label="Converge failed" value={fleetSummary.convergeFailed} tone="fail" sub="last run" />
          <KpiCard label="Missing / offline" value={fleetSummary.offline} tone="warn" sub="no check-in" />
          <KpiCard label="Non-compliant" value={fleetSummary.nonCompliant} tone="fail" sub="latest scan" />
        </div>
      )}

      <DataTable
        columns={columns}
        rows={rows}
        getRowKey={(n) => n.id}
        searchText={(n) => `${n.name} ${n.ip} ${n.platform} ${n.environment} ${n.policyGroup} ${n.tags.join(" ")}`}
        searchPlaceholder="Search hostname, IP, tag…"
        pageSize={10}
        initialSort={{ key: "name", dir: "asc" }}
        onRowClick={(n) => navigate({ to: "/nodes/$nodeId", params: { nodeId: n.id } })}
        filters={[
          { id: "env", label: "Environment", options: environments, selected: env, onChange: setEnv },
          { id: "platform", label: "Platform", options: platforms, selected: plat, onChange: setPlat },
          { id: "group", label: "Policy group", options: policyGroups, selected: group, onChange: setGroup },
          { id: "status", label: "Converge status", options: ["success", "failed", "missing"], selected: status, onChange: setStatus },
        ]}
        loading={isLoading}
        emptyTitle="No nodes match these filters"
      />
    </div>
  );
}
