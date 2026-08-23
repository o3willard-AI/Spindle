import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Download, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { KpiCard, PageHeader, Panel, EmptyState } from "@/components/spindle/ui-bits";
import { useCookbooks } from "@/lib/api";
import { relTime, toCsv, downloadFile } from "@/lib/format";
import type { Cookbook } from "@/lib/mock/types";
import { toast } from "sonner";

export const Route = createFileRoute("/cookbooks/")({
  head: () => ({
    meta: [
      { title: "Cookbooks — Spindle Configuration Inventory" },
      {
        name: "description",
        content:
          "Cookbook inventory with versions, node counts and file contents for every policy applied to the fleet.",
      },
      { property: "og:title", content: "Cookbooks — Spindle Configuration Inventory" },
      {
        property: "og:description",
        content: "Cookbook versions, node counts and recipe contents.",
      },
    ],
  }),
  component: CookbooksPage,
});

function CookbooksPage() {
  const navigate = useNavigate();

  const {
    data: cookbooks,
    isLoading,
    error,
  } = useCookbooks();

  const [query, setQuery] = useState("");

  const columns: Column<Cookbook>[] = [
    {
      key: "name",
      header: "Cookbook",
      sortValue: (c) => c.name,
      cell: (c) => (
        <div className="min-w-0">
          <div className="num text-xs font-medium">{c.name}</div>
          <div className="truncate text-[11px] text-muted-foreground">{c.description || "(no description)"}</div>
        </div>
      ),
    },
    { key: "maintainer", header: "Maintainer", sortValue: (c) => c.maintainer, cell: (c) => <span className="text-xs text-muted-foreground">{c.maintainer || "—"}</span> },
    {
      key: "versions",
      header: "Versions",
      sortValue: (c) => c.versions.length,
      cell: (c) => (
        <span className="num text-xs">
          {c.versions.length > 0 ? c.versions[0]!.version : "—"}
          {c.versions.length > 1 && <span className="text-muted-foreground"> (+{c.versions.length - 1})</span>}
        </span>
      ),
    },
    { key: "nodes", header: "Nodes", sortValue: (c) => c.nodes, cell: (c) => <span className="num text-xs">{c.nodes}</span> },
    {
      key: "lastSeen",
      header: "Last applied",
      sortValue: (c) => c.lastSeen,
      className: "text-right",
      headerClassName: "text-right",
      cell: (c) => <span className="num text-[11px] text-muted-foreground">{relTime(c.lastSeen)}</span>,
    },
  ];

  if (error) {
    return (
      <div className="space-y-5">
        <PageHeader
          title="Cookbooks"
          breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Cookbooks" }]}
          description="Unable to load cookbook inventory."
        />
        <Panel>
          <EmptyState title="Could not load cookbooks" description="Check your API token and server connectivity." />
        </Panel>
      </div>
    );
  }

  const match = (c: Cookbook) =>
    `${c.name} ${c.maintainer} ${c.description}`.toLowerCase().includes(query.toLowerCase());

  const filtered = (cookbooks ?? []).filter(match);

  return (
    <div className="space-y-5">
      <PageHeader
        title="Cookbooks"
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Cookbooks" }]}
        description="Configuration code managed by Spindle."
        actions={
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1.5 text-xs"
            disabled={!cookbooks || filtered.length === 0}
            onClick={() => {
              downloadFile(
                "spindle-cookbooks.csv",
                toCsv(
                  filtered.map((c) => ({
                    name: c.name,
                    maintainer: c.maintainer,
                    description: c.description,
                    versions: c.versions.length,
                    nodes: c.nodes,
                    last_seen: c.lastSeen,
                  })),
                ),
                "text/csv",
              );
              toast.success("Exported cookbook inventory (CSV)");
            }}
          >
            <Download className="size-3.5" /> Export
          </Button>
        }
      />

      {cookbooks && cookbooks.length > 0 && (
        <div className="relative max-w-sm">
          <Search className="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search cookbooks…"
            className="h-8 pl-8 text-xs"
          />
        </div>
      )}

      {cookbooks && (
        <div className="grid gap-3 sm:grid-cols-3">
          <KpiCard label="Cookbooks" value={cookbooks.length} sub="in inventory" />
          <KpiCard label="Versions" value={cookbooks.reduce((a, c) => a + c.versions.length, 0)} sub="uploaded" />
          <KpiCard label="Nodes covered" value={cookbooks.reduce((a, c) => a + c.nodes, 0)} sub="fleet-wide" />
        </div>
      )}

      <DataTable
        columns={columns}
        rows={filtered}
        getRowKey={(c) => c.name}
        searchText={(c) => `${c.name} ${c.maintainer} ${c.description}`}
        searchPlaceholder="Search cookbooks…"
        initialSort={{ key: "name", dir: "asc" }}
        onRowClick={(c) => navigate({ to: "/cookbooks/$name", params: { name: c.name } })}
        pageSize={10}
        loading={isLoading}
        emptyTitle="No cookbooks match the search"
      />
    </div>
  );
}
