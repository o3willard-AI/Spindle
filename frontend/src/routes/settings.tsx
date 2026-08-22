import { createFileRoute } from "@tanstack/react-router";
import { useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
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
import { PageHeader, Panel } from "@/components/spindle/ui-bits";
import {
  apiTokens as seedTokens,
  notificationRules as seedRules,
  retentionPolicies as seedRetention,
  teams as seedTeams,
  users as seedUsers,
} from "@/lib/mock/data";
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
  const [users, setUsers] = useState<User[]>(seedUsers);
  const [teams, setTeams] = useState<Team[]>(seedTeams);
  const [tokens, setTokens] = useState<ApiToken[]>(seedTokens);
  const [rules, setRules] = useState<NotificationRule[]>(seedRules);
  const [retention, setRetention] = useState<RetentionPolicy[]>(seedRetention);

  const [inviteName, setInviteName] = useState("");
  const [inviteEmail, setInviteEmail] = useState("");
  const [inviteRole, setInviteRole] = useState<User["role"]>("Operator");
  const [inviteOpen, setInviteOpen] = useState(false);

  const [tokenName, setTokenName] = useState("");
  const [tokenScope, setTokenScope] = useState<ApiToken["scope"]>("read");
  const [tokenOpen, setTokenOpen] = useState(false);

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
            setUsers((prev) => prev.filter((x) => x.id !== u.id));
            toast.success(`Removed ${u.name}`);
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
          <div className="num text-[11px] text-muted-foreground">{t.prefix}••••••••</div>
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
              onClick={() => {
                setTokens((prev) => prev.map((x) => (x.id === t.id ? { ...x, status: "revoked" } : x)));
                toast.success(`Revoked ${t.name}`);
              }}
            >
              Revoke
            </Button>
          )}
          <Button
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs text-muted-foreground hover:text-fail"
            onClick={() => {
              setTokens((prev) => prev.filter((x) => x.id !== t.id));
              toast.success("Token deleted");
            }}
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
        description="Organization access, integrations and data lifecycle for acme-infra."
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
            toolbarRight={
              <Dialog open={inviteOpen} onOpenChange={setInviteOpen}>
                <DialogTrigger asChild>
                  <Button size="sm" className="h-8 gap-1.5 text-xs">
                    <Plus className="size-3.5" /> Invite user
                  </Button>
                </DialogTrigger>
                <DialogContent>
                  <DialogHeader>
                    <DialogTitle>Invite a user</DialogTitle>
                    <DialogDescription>They'll receive an email invitation to join acme-infra.</DialogDescription>
                  </DialogHeader>
                  <div className="space-y-3">
                    <div className="space-y-1.5">
                      <Label className="text-xs">Full name</Label>
                      <Input value={inviteName} onChange={(e) => setInviteName(e.target.value)} placeholder="Alex Chen" className="h-8 text-xs" />
                    </div>
                    <div className="space-y-1.5">
                      <Label className="text-xs">Email</Label>
                      <Input value={inviteEmail} onChange={(e) => setInviteEmail(e.target.value)} placeholder="alex@spindle.io" className="h-8 text-xs" />
                    </div>
                    <div className="space-y-1.5">
                      <Label className="text-xs">Role</Label>
                      <Select value={inviteRole} onValueChange={(v) => setInviteRole(v as User["role"])}>
                        <SelectTrigger className="h-8 text-xs">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {["Admin", "Operator", "Viewer"].map((r) => (
                            <SelectItem key={r} value={r} className="text-xs">
                              {r}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                  <DialogFooter>
                    <Button
                      size="sm"
                      className="text-xs"
                      disabled={!inviteName || !inviteEmail}
                      onClick={() => {
                        setUsers((prev) => [
                          ...prev,
                          {
                            id: `u${prev.length + 1}-${inviteEmail}`,
                            name: inviteName,
                            email: inviteEmail,
                            role: inviteRole,
                            teams: ["Platform Infra"],
                            lastActive: new Date("2026-08-22T18:40:00.000Z").toISOString(),
                            status: "invited",
                          },
                        ]);
                        toast.success(`Invitation sent to ${inviteEmail}`);
                        setInviteName("");
                        setInviteEmail("");
                        setInviteOpen(false);
                      }}
                    >
                      Send invitation
                    </Button>
                  </DialogFooter>
                </DialogContent>
              </Dialog>
            }
          />
        </TabsContent>

        <TabsContent value="teams" className="mt-4">
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
                <div className="mt-3 flex gap-1.5">
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-7 text-xs"
                    onClick={() => {
                      setTeams((prev) => prev.map((x) => (x.id === t.id ? { ...x, members: x.members + 1 } : x)));
                      toast.success(`Added a member to ${t.name}`);
                    }}
                  >
                    Add member
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    className="h-7 text-xs text-muted-foreground hover:text-fail"
                    onClick={() => {
                      setTeams((prev) => prev.filter((x) => x.id !== t.id));
                      toast.success(`Deleted ${t.name}`);
                    }}
                  >
                    Delete
                  </Button>
                </div>
              </Panel>
            ))}
            <button
              onClick={() => {
                const n = teams.length + 1;
                setTeams((prev) => [
                  ...prev,
                  { id: `t${n}`, name: `New team ${n}`, description: "Describe this team's ownership.", members: 1, environments: ["staging"] },
                ]);
                toast.success("Team created");
              }}
              className="panel flex min-h-32 flex-col items-center justify-center gap-2 border-dashed text-xs text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground"
            >
              <Plus className="size-4" />
              Create team
            </button>
          </div>
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
            toolbarRight={
              <Dialog open={tokenOpen} onOpenChange={setTokenOpen}>
                <DialogTrigger asChild>
                  <Button size="sm" className="h-8 gap-1.5 text-xs">
                    <Plus className="size-3.5" /> New token
                  </Button>
                </DialogTrigger>
                <DialogContent>
                  <DialogHeader>
                    <DialogTitle>Create API token</DialogTitle>
                    <DialogDescription>Tokens authenticate automation against the Spindle API v3.</DialogDescription>
                  </DialogHeader>
                  <div className="space-y-3">
                    <div className="space-y-1.5">
                      <Label className="text-xs">Name</Label>
                      <Input value={tokenName} onChange={(e) => setTokenName(e.target.value)} placeholder="terraform-provisioner" className="h-8 text-xs" />
                    </div>
                    <div className="space-y-1.5">
                      <Label className="text-xs">Scope</Label>
                      <Select value={tokenScope} onValueChange={(v) => setTokenScope(v as ApiToken["scope"])}>
                        <SelectTrigger className="h-8 text-xs">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {["read", "write", "admin"].map((s) => (
                            <SelectItem key={s} value={s} className="text-xs">
                              {s}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  </div>
                  <DialogFooter>
                    <Button
                      size="sm"
                      className="text-xs"
                      disabled={!tokenName}
                      onClick={() => {
                        setTokens((prev) => [
                          {
                            id: `tk-${prev.length + 1}`,
                            name: tokenName,
                            prefix: "spn_live_" + tokenName.slice(0, 4).toUpperCase(),
                            scope: tokenScope,
                            createdAt: new Date("2026-08-22T18:40:00.000Z").toISOString(),
                            lastUsed: null,
                            expiresAt: null,
                            status: "active",
                          },
                          ...prev,
                        ]);
                        toast.success("Token created — copy it now, it won't be shown again");
                        setTokenName("");
                        setTokenOpen(false);
                      }}
                    >
                      Create token
                    </Button>
                  </DialogFooter>
                </DialogContent>
              </Dialog>
            }
          />
        </TabsContent>

        <TabsContent value="notifications" className="mt-4 space-y-3">
          {rules.map((r) => (
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
              <div className="flex items-center gap-2">
                <Switch
                  checked={r.enabled}
                  onCheckedChange={(v) => {
                    setRules((prev) => prev.map((x) => (x.id === r.id ? { ...x, enabled: v } : x)));
                    toast.success(`${r.name} ${v ? "enabled" : "disabled"}`);
                  }}
                />
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs text-muted-foreground hover:text-fail"
                  onClick={() => {
                    setRules((prev) => prev.filter((x) => x.id !== r.id));
                    toast.success("Notification rule deleted");
                  }}
                >
                  <Trash2 className="size-3.5" />
                </Button>
              </div>
            </div>
          ))}
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1.5 text-xs"
            onClick={() => {
              const n = rules.length + 1;
              setRules((prev) => [
                ...prev,
                {
                  id: `n${n}`,
                  name: `New alert rule ${n}`,
                  channel: "Slack",
                  target: "#infra-alerts",
                  trigger: "converge-failure",
                  enabled: true,
                  lastFired: null,
                },
              ]);
              toast.success("Notification rule created");
            }}
          >
            <Plus className="size-3.5" /> Add notification rule
          </Button>
        </TabsContent>

        <TabsContent value="retention" className="mt-4 space-y-3">
          {retention.map((p) => (
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
                  onChange={(e) =>
                    setRetention((prev) =>
                      prev.map((x) => (x.id === p.id ? { ...x, retainDays: Number(e.target.value) } : x)),
                    )
                  }
                  className="num h-8 w-20 text-xs"
                />
              </div>
              <div className="flex items-center gap-2">
                <Label className="text-[11px] text-muted-foreground">Archive</Label>
                <Switch
                  checked={p.archive}
                  onCheckedChange={(v) => setRetention((prev) => prev.map((x) => (x.id === p.id ? { ...x, archive: v } : x)))}
                />
              </div>
              <span className="num text-[11px] text-muted-foreground">{p.estimatedSize}</span>
              <div className="flex items-center gap-2">
                <Switch
                  checked={p.enabled}
                  onCheckedChange={(v) => {
                    setRetention((prev) => prev.map((x) => (x.id === p.id ? { ...x, enabled: v } : x)));
                    toast.success(`${p.dataset} policy ${v ? "enabled" : "disabled"}`);
                  }}
                />
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs text-muted-foreground hover:text-fail"
                  onClick={() => {
                    setRetention((prev) => prev.filter((x) => x.id !== p.id));
                    toast.success("Retention policy removed");
                  }}
                >
                  <Trash2 className="size-3.5" />
                </Button>
              </div>
            </div>
          ))}
          <Button variant="outline" size="sm" className="h-8 text-xs" onClick={() => toast.success("Retention changes saved")}>
            Save lifecycle policies
          </Button>
        </TabsContent>
      </Tabs>
    </div>
  );
}
