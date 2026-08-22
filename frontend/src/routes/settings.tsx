import { createFileRoute } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { DataTable, type Column } from "@/components/spindle/data-table";
import { StatusPill, Tag } from "@/components/spindle/status";
import { PageHeader, Panel, EmptyState } from "@/components/spindle/ui-bits";
import { fetchApiTokens, fetchNotificationRules, fetchRetentionPolicies, fetchTeams, fetchUsers } from "@/lib/api";
import { relTime } from "@/lib/format";
import type { ApiToken, NotificationRule, RetentionPolicy, Team, User } from "@/lib/mock/types";

export const Route = createFileRoute("/settings")({
  head: () => ({
    meta: [
      { title: "Settings — Spindle Organization" },
      {
        name: "description",
        content:
          "Manage Spindle users, teams, API tokens, alert routing and data retention policies for your fleet-automation organization.",
      },
      { property: "og:title", content: "Settings — Spindle Organization" },
      { property: "og:description", content: "Users, teams, tokens, notifications and retention policies." },
    ],
  }),
  component: SettingsPage,
});

function SettingsPage() {
  const queryClient = useQueryClient();

  const { data: usersData, isLoading: usersLoading, error: usersError } = useQuery<User[]>({
    queryKey: ["users"],
    queryFn: fetchUsers,
  });

  const { data: teamsData, isLoading: teamsLoading, error: teamsError } = useQuery<Team[]>({
    queryKey: ["teams"],
    queryFn: fetchTeams,
  });

  const { data: tokensData, isLoading: tokensLoading, error: tokensError } = useQuery<ApiToken[]>({
    queryKey: ["tokens"],
    queryFn: fetchApiTokens,
  });

  const { data: rules, isLoading: rulesLoading, error: rulesError } = useQuery<NotificationRule[]>({
    queryKey: ["notifications"],
    queryFn: fetchNotificationRules,
  });

  const { data: retentionData, isLoading: retentionLoading, error: retentionError } = useQuery<RetentionPolicy[]>({
    queryKey: ["retention"],
    queryFn: fetchRetentionPolicies,
  });

  const users = usersData ?? [];
  const teams = teamsData ?? [];
  const tokens = tokensData ?? [];
  const retention = retentionData ?? [];

  const [inviteName, setInviteName] = useState("");
  const [inviteEmail, setInviteEmail] = useState("");
  const [inviteRole, setInviteRole] = useState<User["role"]>("Operator");
  const [inviteOpen, setInviteOpen] = useState(false);

  const [tokenName, setTokenName] = useState("");
  const [tokenScope, setTokenScope] = useState<ApiToken["scope"]>("read");
  const [tokenOpen, setTokenOpen] = useState(false);

  useEffect(() => {
    if (usersError || teamsError || tokensError || rulesError || retentionError) {
      toast.error("Failed to load settings data");
    }
  }, [usersError, teamsError, tokensError, rulesError, retentionError]);

  const userColumns: Column<User>[] = [
    {
      key: "name",
      header: "User",
      sortValue: (u) => u.name,
      cell: (u) => (
        <div className="min-w-0">
          <div className="text-xs font-medium">{u.name}</div>
          <div className="num text-[11px] text-muted-foreground">{u.email}</div>
        </div>
      ),
    },
    { key: "role", header: "Role", sortValue: (u) => u.role, cell: (u) => <Tag>{u.role}</Tag> },
    {
      key: "teams",
      header: "Teams",
      sortable: false,
      cell: (u) => <span className="text-xs text-muted-foreground">{u.teams.join(", ")}</span>,
    },
    { key: "status", header: "Status", sortValue: (u) => u.status, cell: (u) => <StatusPill status={u.status} /> },
    {
      key: "lastActive",
      header: "Last active",
      sortValue: (u) => u.lastActive,
      cell: (u) => <span className="num text-[11px] text-muted-foreground">{relTime(u.lastActive)}</span>,
    },
    {
      key: "actions",
      header: "",
      sortable: false,
      className: "text-right",
      cell: (u) => (
        <Button
          variant="ghost"
          size="sm"
          className="h-7 px-2 text-xs text-muted-foreground hover:text-fail"
          onClick={() => {
            toast.info(`Remove ${u.name} (disabled in read-only mode)`);
          }}
        >
          <Trash2 className="size-3.5" />
        </Button>
      ),
    },
  ];

  const tokenColumns: Column<ApiToken>[] = [
    {
      key: "name",
      header: "Token",
      sortValue: (t) => t.name,
      cell: (t) => (
        <div className="min-w-0">
          <div className="num text-xs font-medium">{t.name}</div>
          <div className="num text-[11px] text-muted-foreground">{t.prefix}········</div>
        </div>
      ),
    },
    { key: "scope", header: "Scope", sortValue: (t) => t.scope, cell: (t) => <Tag>{t.scope}</Tag> },
    { key: "status", header: "Status", sortValue: (t) => t.status, cell: (t) => <StatusPill status={t.status} /> },
    {
      key: "lastUsed",
      header: "Last used",
      sortValue: (t) => t.lastUsed ?? "",
      cell: (t) => <span className="num text-[11px] text-muted-foreground">{t.lastUsed ? relTime(t.lastUsed) : "never"}</span>,
    },
    {
      key: "created",
      header: "Created",
      sortValue: (t) => t.createdAt,
      cell: (t) => <span className="num text-[11px] text-muted-foreground">{relTime(t.createdAt)}</span>,
    },
    {
      key: "actions",
      header: "",
      sortable: false,
      className: "text-right",
      cell: (t) => (
        <div className="flex justify-end gap-1">
          {t.status === "active" && (
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-2 text-xs text-muted-foreground hover:text-fail"
              onClick={() => toast.info(`Revoke ${t.name} (disabled in read-only mode)`)}
            >
              Revoke
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs text-muted-foreground hover:text-fail"
            onClick={() => toast.info("Delete (disabled in read-only mode)")}
          >
            <Trash2 className="size-3.5" />
          </Button>
        </div>
      ),
    },
  ];

  return (
    <div className="space-y-5">
      <PageHeader
        title="Settings"
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Settings" }]}
        description="Organization access, integrations and data lifecycle for your Spindle organization."
      />

      <Tabs defaultValue="users">
        <TabsList>
          <TabsTrigger value="users">Users</TabsTrigger>
          <TabsTrigger value="teams">Teams</TabsTrigger>
          <TabsTrigger value="tokens">API tokens</TabsTrigger>
          <TabsTrigger value="notifications">Notifications</TabsTrigger>
          <TabsTrigger value="retention">Data lifecycle</TabsTrigger>
        </TabsList>

        <TabsContent value="users" className="mt-4">
          <DataTable
            columns={userColumns}
            rows={users}
            getRowKey={(u) => u.id}
            searchText={(u) => `${u.name} ${u.email} ${u.role} ${u.teams.join(" ")}`}
            searchPlaceholder="Search users…"
            initialSort={{ key: "name", dir: "asc" }}
            pageSize={8}
            filters={[]}
            loading={usersLoading}
            emptyTitle="No users match"
          />
        </TabsContent>

        <TabsContent value="teams" className="mt-4">
          {teams.length === 0 && !teamsLoading ? (
            <Panel>
              <EmptyState title="No teams" description="Teams organize user access across environments." />
            </Panel>
          ) : (
            <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
              {teams.map((t) => (
                <Panel key={t.id} title={t.name} description={t.description}>
                  <div className="flex items-center justify-between">
                    <div className="flex flex-wrap gap-1">
                      {t.environments.map((e) => (
                        <Tag key={e}>{e}</Tag>
                      ))}
                    </div>
                    <span className="num text-xs text-muted-foreground">{t.members} members</span>
                  </div>
                </Panel>
              ))}
            </div>
          )}
        </TabsContent>

        <TabsContent value="tokens" className="mt-4">
          <DataTable
            columns={tokenColumns}
            rows={tokens}
            getRowKey={(t) => t.id}
            searchText={(t) => `${t.name} ${t.scope} ${t.status}`}
            searchPlaceholder="Search tokens…"
            initialSort={{ key: "created", dir: "desc" }}
            pageSize={8}
            loading={tokensLoading}
            emptyTitle="No tokens match"
          />
        </TabsContent>

        <TabsContent value="notifications" className="mt-4 space-y-3">
          {(rules ?? []).length === 0 && !rulesLoading ? (
            <Panel>
              <EmptyState title="No notification rules" description="Alert routing rules will appear here." />
            </Panel>
          ) : (
            (rules ?? []).map((r) => (
              <div key={r.id} className="panel flex flex-wrap items-center gap-4 p-4">
                <div className="min-w-56 flex-1">
                  <div className="text-sm font-medium">{r.name}</div>
                  <div className="num mt-0.5 text-[11px] text-muted-foreground">
                    {r.channel} → {r.target}
                  </div>
                </div>
                <Tag>{r.trigger}</Tag>
                <span className="num text-[11px] text-muted-foreground">
                  {r.lastFired ? `fired ${relTime(r.lastFired)}` : "never fired"}
                </span>
              </div>
            ))
          )}
        </TabsContent>

        <TabsContent value="retention" className="mt-4 space-y-3">
          {retention.length === 0 && !retentionLoading ? (
            <Panel>
              <EmptyState title="No retention policies" description="Data lifecycle policies will appear here." />
            </Panel>
          ) : (
            retention.map((p) => (
              <div key={p.id} className="panel flex flex-wrap items-center gap-4 p-4">
                <div className="min-w-56 flex-1">
                  <div className="text-sm font-medium">{p.dataset}</div>
                  <div className="mt-0.5 text-[11px] text-muted-foreground">{p.description}</div>
                </div>
                <div className="flex items-center gap-2">
                  <Label className="text-[11px] text-muted-foreground">Retain (days)</Label>
                  <Input
                    type="number"
                    value={p.retainDays}
                    className="num h-8 w-20 text-xs"
                    readOnly
                  />
                </div>
                <div className="flex items-center gap-2">
                  <Label className="text-[11px] text-muted-foreground">Archive</Label>
                  <Switch checked={p.archive} onCheckedChange={() => {}} disabled />
                </div>
                <span className="num text-[11px] text-muted-foreground">{p.estimatedSize}</span>
                <Switch checked={p.enabled} onCheckedChange={() => {}} disabled />
              </div>
            ))
          )}
        </TabsContent>
      </Tabs>
    </div>
  );
}
