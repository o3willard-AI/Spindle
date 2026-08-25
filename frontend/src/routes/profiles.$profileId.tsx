import { createFileRoute } from "@tanstack/react-router";
import { useMemo } from "react";
import { useComplianceReports, useComplianceProfiles, useControlRollups } from "@/lib/api";
import { SeverityBadge, StatusPill, Tag } from "@/components/spindle/status";
import { KpiCard, MetaGrid, PageHeader, Panel, EmptyState } from "@/components/spindle/ui-bits";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { pct, relTime } from "@/lib/format";
import type { Control, Profile } from "@/lib/mock/types";
import { cn } from "@/lib/utils";

interface ControlRollupRow {
  id: string;
  title: string;
  severity: Control["severity"];
  impact: number;
  failing: number;
  passing: number;
  skipped: number;
  failingNodes: string[];
  passingNodes: string[];
}

export const Route = createFileRoute("/profiles/$profileId")({
  component: ProfileDetail,
});

function ProfileDetail() {
  const { profileId } = Route.useParams();

  const { data: profiles, isLoading: profilesLoading, error: profilesError } = useComplianceProfiles();
  const { data: scans, isLoading: scansLoading, error: scansError } = useComplianceReports({ limit: 500 });

  const { data: controlRollups } = useControlRollups();

  const profile = useMemo(() => profiles?.find((p) => p.id === profileId), [profiles, profileId]);

  // Aggregate control results from the server-provided /v1/compliance/controls
  // endpoint (useControlRollups), filtered to this profile. The /v1/compliance/reports
  // list endpoint does NOT populate per-profile control arrays (controls: []),
  // so we cannot derive control-level rollups from the reports list alone.
  const controlRows = useMemo(() => {
    if (!controlRollups || !profile) return [];
    // Filter control rollups to this profile only
    const profileRollups = controlRollups.filter((r) => r.profileId === profile.id);
    return profileRollups.map((r): ControlRollupRow => ({
      id: r.id,
      title: r.title,
      severity: r.severity ?? "medium",
      impact: r.impact,
      failing: r.failing,
      passing: r.passing,
      skipped: r.warnings,
      failingNodes: r.nodes.filter((_, i) => i < r.failing),
      passingNodes: r.nodes.filter((_, i) => i < r.passing),
    }));
  }, [controlRollups, profile]);

  if (profilesLoading || scansLoading) {
    return (
      <div className="space-y-5">
        <Panel title="" description="" bodyClassName="p-4">
          <div className="h-8 w-3/4 animate-pulse rounded bg-muted" />
          <div className="mt-4 h-4 w-1/2 animate-pulse rounded bg-muted" />
        </Panel>
      </div>
    );
  }

  if (profilesError || scansError || !profile) {
    return (
      <div className="space-y-5">
        <PageHeader
          title="Profile not found"
          breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Profiles", to: "/profiles" }, { label: "Not found" }]}
          description="This compliance profile is not in this Spindle organization."
        />
        <Panel>
          <EmptyState title="Profile not found" description="The requested profile does not exist or has been removed." />
        </Panel>
      </div>
    );
  }

  // Compute aggregate stats for this profile from scan-level aggregate counts.
  // The reports list endpoint populates passed_count/failed_count/warning_count
  // per report but leaves profile.controls empty. Derive everything from the
  // aggregate counts instead of iterating the (empty) control arrays.
  const stats = useMemo(() => {
    if (!scans || !profile) return { nodes: 0, totalControls: 0, totalTests: 0, passRate: -1 };
    const profileScans = scans.filter((s) => s.profiles.some((p) => p.profileId === profile.id));
    const nodes = new Set<string>();
    let totalPassed = 0;
    let totalFailed = 0;
    let totalTests = 0;
    for (const scan of profileScans) {
      nodes.add(scan.nodeId);
      // scan.profiles only has the matching profile's entry; counts are at scan level
      totalPassed += scan.passed;
      totalFailed += scan.failed;
      totalTests += scan.passed + scan.failed + scan.warnings;
    }
    const totalEvaluated = totalPassed + totalFailed;
    const passRate = totalEvaluated > 0 ? totalPassed / totalEvaluated : -1;
    return { nodes: nodes.size, totalControls: 0, totalTests, passRate };
  }, [scans, profile]);

  const columns: Column<ControlRollupRow>[] = [
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
    { key: "tests", header: "Tests", sortValue: (c) => c.passing + c.failing + c.skipped, cell: (c) => <span className="num text-xs">{c.passing + c.failing + c.skipped}</span> },
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
        description={profile.summary || ""}
        meta={
          <div className="flex flex-wrap items-center gap-2 pt-1">
            <StatusPill status={stats.passRate >= 0 ? (stats.passRate > 0.85 ? "compliant" : "non-compliant") : "unknown"} {...(stats.passRate >= 0 ? {} : { label: "Not scanned" })} />
            {profile.platforms.map((p) => (
              <Tag key={p}>{p}</Tag>
            ))}
          </div>
        }
      />

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <KpiCard label="Controls" value={stats.totalControls} sub="in profile" />
        <KpiCard label="Tests" value={stats.totalTests} sub="individual checks" />
        <KpiCard label="Nodes scanned" value={stats.nodes} sub="last cycle" />
        <KpiCard
          label="Pass rate"
          value={stats.passRate >= 0 ? pct(stats.passRate * 100) : "—"}
          tone={stats.passRate >= 0 ? (stats.passRate > 0.85 ? "ok" : "fail") : "warn"}
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
        {controlRows.length === 0 ? (
          <p className="py-10 text-center text-xs text-muted-foreground">
            This profile hasn't been evaluated against any nodes yet.
          </p>
        ) : (
          <DataTable
            columns={columns}
            rows={controlRows}
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
