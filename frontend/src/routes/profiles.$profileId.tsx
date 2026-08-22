import { createFileRoute, notFound } from "@tanstack/react-router";
import { useEffect, useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { SeverityBadge, StatusPill, Tag } from "@/components/spindle/status";
import { KpiCard, MetaGrid, PageHeader, Panel, EmptyState } from "@/components/spindle/ui-bits";
import { fetchComplianceProfiles } from "@/lib/api";
import { pct, relTime } from "@/lib/format";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/profiles/$profileId")({
  component: ProfileDetail,
});

function ProfileDetail() {
  const { profileId } = Route.useParams();

  const {
    data: profiles,
    isLoading,
    error,
  } = useQuery({
    queryKey: ["compliance-profiles"],
    queryFn: () => fetchComplianceProfiles(),
  });

  const profile = useMemo(() => profiles?.find((p) => p.id === profileId), [profiles, profileId]);

  useEffect(() => {
    if (error) {
      throw notFound();
    }
  }, [error]);

  if (isLoading || !profile) {
    return (
      <div className="space-y-5">
        <Panel title="" description="" bodyClassName="p-4">
          <div className="h-8 w-3/4 animate-pulse rounded bg-muted" />
          <div className="mt-4 h-4 w-1/2 animate-pulse rounded bg-muted" />
        </Panel>
      </div>
    );
  }

  const rows: any[] = [];

  const columns: Column<any>[] = [
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
    { key: "tests", header: "Tests", sortValue: (c) => c.passing + c.failing, cell: (c) => <span className="num text-xs">{c.passing + c.failing + c.skipped}</span> },
    { key: "failing", header: "Failing nodes", sortValue: (c) => c.failing, cell: (c) => <span className={cn("num text-xs", c.failing ? "text-fail" : "text-muted-foreground")}>{c.failing}</span> },
    { key: "passing", header: "Passing nodes", sortValue: (c) => c.passing, cell: (c) => <span className="num text-xs text-ok">{c.passing}</span> },
    {
      key: "status",
      header: "Status",
      sortValue: (c) => (c.failing ? 0 : 1),
      cell: (c) => <StatusPill status={c.failing ? "failed" : "passed"} size="sm" />,
    },
  ];

  return (
    <div className="space-y-5">
      <PageHeader
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Profiles", to: "/profiles" }, { label: profile.name }]}
        title={profile.title}
        description={profile.summary}
        meta={
          <div className="flex flex-wrap items-center gap-2 pt-1">
            <StatusPill status={profile.installed ? (profile.passRate > 0.85 ? "compliant" : "non-compliant") : "unknown"} {...(profile.installed ? {} : { label: "Not installed" })} />
            {profile.platforms.map((p) => (
              <Tag key={p}>{p}</Tag>
            ))}
          </div>
        }
      />

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <KpiCard label="Controls" value={profile.controlCount} sub="in profile" />
        <KpiCard label="Tests" value={profile.testCount} sub="individual checks" />
        <KpiCard label="Nodes scanned" value={profile.nodes} sub="last cycle" />
        <KpiCard
          label="Pass rate"
          value={profile.installed ? pct(profile.passRate * 100) : "—"}
          tone={profile.passRate > 0.85 ? "ok" : "fail"}
        />
      </div>

      <div className="panel p-4">
        <MetaGrid
          items={[
            { label: "Profile", value: <span className="num">{profile.name}</span> },
            { label: "Version", value: <span className="num">v{profile.version}</span> },
            { label: "Vendor", value: profile.vendor },
            { label: "Updated", value: relTime(profile.updatedAt) },
          ]}
        />
      </div>

      <Panel title="Controls" description="Every control in this profile with fleet-wide results" bodyClassName="p-4">
        {rows.length === 0 ? (
          <p className="py-10 text-center text-xs text-muted-foreground">
            This profile isn't installed yet — install it to evaluate its {profile.controlCount} controls against the fleet.
          </p>
        ) : (
          <DataTable
            columns={columns}
            rows={rows}
            getRowKey={(c) => c.id}
            searchText={(c) => `${c.id} ${c.title}`}
            searchPlaceholder="Search controls…"
            initialSort={{ key: "failing", dir: "desc" }}
            pageSize={10}
            density="compact"
            emptyTitle="No controls match"
          />
        )}
      </Panel>
    </div>
  );
}
