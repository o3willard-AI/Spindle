import { createFileRoute } from "@tanstack/react-router";
import { Shield, ShieldCheck, ShieldOff, Database, Layers, Users, Bell, Key } from "lucide-react";
import { PageHeader, Panel } from "@/components/spindle/ui-bits";

export const Route = createFileRoute("/__spindle-admin/settings")({
  head: () => ({
    meta: [
      { title: "Settings — Spindle Organization" },
      {
        name: "description",
        content:
          "Organization settings. Admin features are gated behind the /__spindle-admin path.",
      },
      { property: "og:title", content: "Settings — Spindle Organization" },
      { property: "og:description", content: "Users, teams, tokens, notifications and retention policies." },
    ],
  }),
  component: SettingsPage,
});

/**
 * Admin settings surface at the secret URL `/__spindle-admin/settings`.
 *
 * These are inert Turnstile stubs — no API calls are made. Each section
 * displays "integration pending" until the corresponding admin endpoints
 * are implemented behind JWT auth. This intentionally avoids mock data.
 */

function StubPanel({
  title,
  icon: Icon,
  description,
}: {
  title: string;
  icon: React.ComponentType<{ className?: string }>;
  description: string;
}) {
  return (
    <Panel title={title} bodyClassName="p-4">
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <Icon className="size-4" />
        <span>{description}</span>
      </div>
    </Panel>
  );
}

function SettingsPage() {
  return (
    <div className="space-y-5">
      <PageHeader
        title="Settings"
        breadcrumbs={[{ label: "Fleet", to: "/" }, { label: "Settings" }]}
        description="Organization access, integrations and data lifecycle for your Spindle organization."
      />

      <StubPanel
        title="Users"
        icon={Users}
        description="Integration pending — user management will be available via the OIDC/SAML identity provider."
      />

      <StubPanel
        title="Teams"
        icon={Layers}
        description="Integration pending — teams are managed through identity provider scope mappings."
      />

      <StubPanel
        title="API tokens"
        icon={Key}
        description="Integration pending — API tokens are provisioned via SPINDLE_INGEST_TOKEN at the server level."
      />

      <StubPanel
        title="Notifications"
        icon={Bell}
        description="Integration pending — notification routing will be exposed via admin API endpoints."
      />

      <StubPanel
        title="Data lifecycle"
        icon={Database}
        description="Integration pending — retention policies are enforced server-side via config (retention.auto_cleanup)."
      />

      <Panel title="System health" bodyClassName="p-4">
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <ShieldOff className="size-4" />
          <span>Integration pending — health probes are not exposed on this admin surface.</span>
        </div>
      </Panel>

      <Panel title="Compliance waivers" bodyClassName="p-4">
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <ShieldCheck className="size-4" />
          <span>Integration pending — waiver management will be available via admin API endpoints.</span>
        </div>
      </Panel>
    </div>
  );
}
