import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Download, Search, ShieldCheck } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { StatusPill, Tag } from "@/components/spindle/status";
import { EmptyState, PageHeader, Panel } from "@/components/spindle/ui-bits";
import { useComplianceProfiles, useComplianceReports } from "@/lib/api";
import { pct, relTime } from "@/lib/format";
import type { Profile, Scan } from "@/lib/mock/types";
import { toast } from "sonner";

export const Route = createFileRoute("/profiles/")({
  head: () => ({
    meta: [
      { title: "Compliance Profiles — Spindle" },
      {
        name: "description",
        content:
          "Installed and available compliance profiles — CIS benchmarks, STIG and app baselines — with control and test counts.",
      },
      { property: "og:title", content: "Compliance Profiles — Spindle" },
      { property: "og:description", content: "Browse installed and available CIS-style compliance profiles." },
    ],
  }),
  component: ProfilesPage,
});

interface ProfileCardProps {
  profile: Profile;
  controlCount: number;
  testCount: number;
  nodes: number;
  passRate: number;
  onOpen: () => void;
}

function ProfileCard({ profile, controlCount, testCount, nodes, passRate, onOpen }: ProfileCardProps) {
  return (
    <button
      onClick={onOpen}
      className="panel flex h-full flex-col p-4 text-left transition-colors hover:border-border-strong hover:bg-accent/30"
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="truncate text-sm font-semibold text-foreground">{profile.title}</h3>
          <p className="num mt-0.5 text-[11px] text-muted-foreground">
            {profile.name} · v{profile.version} · {profile.vendor}
          </p>
        </div>
        {passRate >= 0 ? (
          <StatusPill status={passRate > 0.85 ? "compliant" : "non-compliant"} />
        ) : (
          <StatusPill status="unknown" label="Not installed" />
        )}
      </div>
      <p className="mt-2 line-clamp-2 text-xs text-muted-foreground">{profile.summary || "(no description)"}</p>
      <div className="mt-3 flex flex-wrap gap-1">
        {profile.platforms.map((p) => (
          <Tag key={p}>{p}</Tag>
        ))}
      </div>
      <div className="mt-auto grid grid-cols-4 gap-2 border-t border-border pt-3">
        <div>
          <div className="label-caps">Controls</div>
          <div className="num text-sm">{controlCount}</div>
        </div>
        <div>
          <div className="label-caps">Tests</div>
          <div className="num text-sm">{testCount}</div>
        </div>
        <div>
          <div className="label-caps">Nodes</div>
          <div className="num text-sm">{nodes}</div>
        </div>
        <div>
          <div className="label-caps">Pass</div>
          <div className="num text-sm">{passRate >= 0 ? pct(passRate * 100) : "—"}</div>
        </div>
      </div>
      <div className="num mt-2 text-[11px] text-muted-foreground">Updated {relTime(profile.updatedAt)}</div>
    </button>
  );
}

function ProfilesPage() {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");

  const { data: profiles, isLoading, error } = useComplianceProfiles();
  const { data: scans, isLoading: scansLoading } = useComplianceReports({ limit: 500 });

  // Enrich profiles with control/test counts, node coverage, and pass rate from scan data
  const enriched = useMemo(() => {
    if (!profiles || !scans) return [];
    return profiles.map((p) => {
      const profileScans = scans.filter((s) =>
        s.profiles.some((prof) => prof.profileId === p.id),
      );
      const nodes = new Set<string>();
      let totalControls = 0;
      let totalTests = 0;
      let totalPassed = 0;
      let totalEvaluated = 0;

      for (const scan of profileScans) {
        nodes.add(scan.nodeId);
        for (const prof of scan.profiles.filter((pr) => pr.profileId === p.id)) {
          totalControls += prof.controls.length;
          for (const control of prof.controls) {
            const results = control.results ?? [];
            totalTests += results.length;
            totalPassed += results.filter((r) => r.status === "passed").length;
            totalEvaluated += results.filter((r) => r.status === "passed" || r.status === "failed").length;
          }
        }
      }

      const passRate = totalEvaluated > 0 ? totalPassed / totalEvaluated : 0;
      return {
        ...p,
        controlCount: totalControls,
        testCount: totalTests,
        nodes: nodes.size,
        passRate,
        scans: profileScans,
      };
    });
  }, [profiles, scans]);

  const match = (p: Profile) =>
    `${p.name} ${p.title} ${p.vendor} ${p.platforms.join(" ")}`.toLowerCase().includes(query.toLowerCase());

  const installed = useMemo(() => enriched.filter((p) => p.installed && match(p)), [enriched, query]);
  const available = useMemo(() => enriched.filter((p) => !p.installed && match(p)), [enriched, query]);

  if (error) {
    return (
      <div className="space-y-5">
        <PageHeader
          title="Compliance profiles"
          breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Profiles" }]}
          description="Unable to load profiles."
        />
        <Panel>
          <EmptyState title="Could not load profiles" description="Check your API token and server connectivity." />
        </Panel>
      </div>
    );
  }

  return (
    <div className="space-y-5">
      <PageHeader
        title="Compliance profiles"
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Profiles" }]}
        description="Audit baselines evaluated by Cinc Auditor on every scan cycle."
        actions={
          <Button size="sm" className="h-8 gap-1.5 text-xs" onClick={() => toast.success("Profile install dialog (read-only mode)")}>
            <Download className="size-3.5" /> Upload profile
          </Button>
        }
      />

      <div className="relative max-w-sm">
        <Search className="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search profiles by name, vendor or platform…"
          className="h-8 pl-8 text-xs"
        />
      </div>

      {scansLoading && !profiles && (
        <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
          {Array.from({ length: 6 }).map((_, i) => (
            <Panel key={i} title="" description="" bodyClassName="p-4">
              <div className="h-5 w-3/4 animate-pulse rounded bg-muted" />
              <div className="mt-2 h-3 w-1/2 animate-pulse rounded bg-muted" />
            </Panel>
          ))}
        </div>
      )}

      {profiles && (
        <Tabs defaultValue="installed">
          <TabsList>
            <TabsTrigger value="installed">
              Installed <span className="num ml-1.5 text-muted-foreground">{installed.length}</span>
            </TabsTrigger>
            <TabsTrigger value="available">
              Available <span className="num ml-1.5 text-muted-foreground">{available.length}</span>
            </TabsTrigger>
          </TabsList>

          <TabsContent value="installed" className="mt-4">
            {installed.length === 0 ? (
              <Panel>
                <EmptyState icon={<ShieldCheck className="size-5" />} title="No installed profiles match" description="Try a different search term." />
              </Panel>
            ) : (
              <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                {installed.map((p) => (
                  <ProfileCard
                    key={p.id}
                    profile={p}
                    controlCount={p.controlCount}
                    testCount={p.testCount}
                    nodes={p.nodes}
                    passRate={p.passRate}
                    onOpen={() => navigate({ to: "/profiles/$profileId", params: { profileId: p.id } })}
                  />
                ))}
              </div>
            )}
          </TabsContent>

          <TabsContent value="available" className="mt-4">
            {available.length === 0 ? (
              <Panel>
                <EmptyState title="No available profiles match" description="Try a different search term." />
              </Panel>
            ) : (
              <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                {available.map((p) => (
                  <ProfileCard
                    key={p.id}
                    profile={p}
                    controlCount={p.controlCount}
                    testCount={p.testCount}
                    nodes={p.nodes}
                    passRate={p.passRate}
                    onOpen={() => navigate({ to: "/profiles/$profileId", params: { profileId: p.id } })}
                  />
                ))}
              </div>
            )}
          </TabsContent>
        </Tabs>
      )}
    </div>
  );
}
