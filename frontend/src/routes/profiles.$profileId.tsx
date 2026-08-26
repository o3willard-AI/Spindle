import { createFileRoute } from "@tanstack/react-router";
import { useMemo } from "react";
import { useComplianceReports, useComplianceProfiles } from "@/lib/api";
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

interface ControlRollupIntermediate extends Omit<ControlRollupRow, "failingNodes" | "passingNodes"> {
  failingNodes: Set<string>;
  passingNodes: Set<string>;
}

export const Route = createFileRoute("/profiles/$profileId")({
  component: ProfileDetail,
});

function ProfileDetail() {
  const { profileId } = Route.useParams();

  const { data: profiles, isLoading: profilesLoading, error: profilesError } = useComplianceProfiles();
  const { data: scans, isLoading: scansLoading, error: scansError } = useComplianceReports({ limit: 500 });

  const profile = useMemo(() => profiles?.find((p) => p.id === profileId), [profiles, profileId]);

  // Aggregate control results across all scans for this profile
  const controlRows = useMemo(() => {
    if (!scans || !profile) return [];
    // Collect all control evaluations for this profile across all scans
    const controlMap = new Map<string, ControlRollupIntermediate>();

    for (const scan of scans) {
      for (const prof of scan.profiles) {
        if (prof.profileId !== profile.id) continue;
        for (const control of prof.controls) {
          const key = control.id;
          if (!controlMap.has(key)) {
            controlMap.set(key, {
              id: control.id,
              title: control.title,
              severity: control.severity ?? "medium",
              impact: control.impact,
              failing: 0,
              passing: 0,
              skipped: 0,
              failingNodes: new Set(),
              passingNodes: new Set(),
            });
          }
          const rollup = controlMap.get(key)!;
          for (const result of control.results ?? []) {
            if (result.status === "passed") {
              rollup.passing++;
              rollup.passingNodes.add(scan.nodeId);
            } else if (result.status === "failed") {
              rollup.failing++;
              rollup.failingNodes.add(scan.nodeId);
            } else if (result.status === "skipped") {
              rollup.skipped++;
            }
          }
        }
      }
    }

    return Array.from(controlMap.values()).map((c) => ({
      ...c,
      failingNodes: Array.from(c.failingNodes),
      passingNodes: Array.from(c.passingNodes),
    }));
  }, [scans, profile]);

  // Compute aggregate stats for this profile
  const stats = useMemo(() => {
    if (!scans || !profile) return { nodes: 0, totalControls: 0, totalTests: 0, passRate: 0 };
    const profileScans = scans.filter((s) => s.profiles.some((p) => p.profileId === profile.id));
    const nodes = new Set<string>();
    let totalControls = 0;
    let totalTests = 0;
    let passed = 0;
    let evaluated = 0;
    for (const scan of profileScans) {
      nodes.add(scan.nodeId);
      for (const prof of scan.profiles.filter((p) => p.profileId === profile.id)) {
        totalControls += prof.controls.length;
        for (const control of prof.controls) {
          const results = control.results ?? [];
          totalTests += results.length;
          passed += results.filter((r) => r.status === "passed").length;
          evaluated += results.filter((r) => r.status === "passed" || r.status === "failed").length;
        }
      }
    }
    const passRate = evaluated > 0 ? passed / evaluated : 0;
    return { nodes: nodes.size, totalControls, totalTests, passRate };
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

  return (
    <div className="space-y-5">
      <PageHeader
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Profiles", to: "/profiles" }, { label: profile.name }]}
        title={profile.title}
        description={profile.summary || ""}
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
        <KpiCard label="Controls" value={stats.totalControls} sub="in profile" />
        <KpiCard label="Tests" value={stats.totalTests} sub="individual checks" />
        <KpiCard label="Nodes scanned" value={stats.nodes} sub="last cycle" />
        <KpiCard
          label="Pass rate"
          value={stats.passRate > 0 ? pct(stats.passRate * 100) : "—"}
          tone={stats.passRate > 0.85 ? "ok" : "fail"}
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
