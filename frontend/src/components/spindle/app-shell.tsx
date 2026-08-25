import { Link, useNavigate } from "@tanstack/react-router";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  BookOpen,
  ChevronDown,
  LayoutDashboard,
  LifeBuoy,
  Moon,
  PlayCircle,
  Search,
  Server,
  ShieldCheck,
  Sun,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useNodes, useComplianceReports, useRuns, useComplianceProfiles, useCookbooks, getCurrentUser } from "@/lib/api";
import { cn } from "@/lib/utils";
import { StatusDot } from "./status";
import type { ActivityEvent } from "@/lib/mock/types";

const NAV = [
  { to: "/", label: "Dashboard", icon: LayoutDashboard, exact: true },
  { to: "/nodes", label: "Nodes", icon: Server },
  { to: "/runs", label: "Converge runs", icon: PlayCircle },
  { to: "/compliance", label: "Compliance", icon: ShieldCheck },
  { to: "/profiles", label: "Profiles", icon: Activity },
  { to: "/cookbooks", label: "Cookbooks", icon: BookOpen },
  // Settings moved to /__spindle-admin/settings (feature-flag admin page)
] as const;

function useTheme() {
  const [dark, setDark] = useState(true);
  useEffect(() => {
    const stored = window.localStorage.getItem("spindle-theme");
    const isDark = stored !== "light";
    setDark(isDark);
    document.documentElement.classList.toggle("dark", isDark);
  }, []);
  const toggle = () => {
    const next = !dark;
    setDark(next);
    document.documentElement.classList.toggle("dark", next);
    window.localStorage.setItem("spindle-theme", next ? "dark" : "light");
  };
  return { dark, toggle };
}

function GlobalSearch() {
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();

  const { data: nodes } = useNodes({ limit: 200 });
  const { data: runs } = useRuns({ limit: 100 });
  const { data: profiles } = useComplianceProfiles();
  const { data: cookbooks } = useCookbooks();

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        setOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const go = (to: string) => {
    setOpen(false);
    navigate({ to });
  };

  return (
    <>
      <button
        onClick={() => setOpen(true)}
        className="flex h-8 w-full max-w-md items-center gap-2 rounded-md border border-border bg-background/60 px-2.5 text-xs text-muted-foreground transition-colors hover:border-border-strong hover:text-foreground"
      >
        <Search className="size-3.5" />
        Search nodes, runs, controls, cookbooks…
        <kbd className="num ml-auto rounded border border-border bg-elevated px-1.5 py-px text-[10px]">⌘K</kbd>
      </button>
      <CommandDialog open={open} onOpenChange={setOpen}>
        <CommandInput placeholder="Search the fleet…" />
        <CommandList>
          <CommandEmpty>No results found.</CommandEmpty>
          <CommandGroup heading="Nodes">
            {(nodes ?? []).map((n) => (
              <CommandItem key={n.id} value={`${n.name} ${n.environment} ${n.platform}`} onSelect={() => go(`/nodes/${n.id}`)}>
                <StatusDot status={n.status} />
                <span className="font-mono text-xs">{n.name}</span>
                <span className="ml-auto text-[11px] text-muted-foreground">{n.environment}</span>
              </CommandItem>
            ))}
          </CommandGroup>
          <CommandGroup heading="Recent runs">
            {(runs ?? []).slice(0, 6).map((r) => (
              <CommandItem key={r.id} value={`${r.id} ${r.nodeName}`} onSelect={() => go(`/runs/${r.id}`)}>
                <StatusDot status={r.status} />
                <span className="font-mono text-xs">{r.id}</span>
                <span className="ml-auto text-[11px] text-muted-foreground">{r.nodeName}</span>
              </CommandItem>
            ))}
          </CommandGroup>
          <CommandGroup heading="Profiles">
            {(profiles ?? []).map((p) => (
              <CommandItem key={p.id} value={p.title} onSelect={() => go(`/profiles/${p.id}`)}>
                <ShieldCheck className="size-3.5 text-muted-foreground" />
                <span className="text-xs">{p.title}</span>
              </CommandItem>
            ))}
          </CommandGroup>
          <CommandGroup heading="Cookbooks">
            {(cookbooks ?? []).map((c: { name: string }) => (
              <CommandItem key={c.name} value={c.name} onSelect={() => go(`/cookbooks/${c.name}`)}>
                <BookOpen className="size-3.5 text-muted-foreground" />
                <span className="font-mono text-xs">{c.name}</span>
              </CommandItem>
            ))}
          </CommandGroup>
        </CommandList>
      </CommandDialog>
    </>
  );
}

export function AppShell({ children }: { children: ReactNode }) {
  const { dark, toggle } = useTheme();
  const user = getCurrentUser();

  const { data: nodes } = useNodes({ limit: 500 });
  const { data: scans } = useComplianceReports({ limit: 500 });

  // Count nodes that are either converged-failed OR compliance-failed.
  // The /v1/nodes endpoint provides converge "status" but NOT compliance
  // counts (passed_count/failed_count are absent from NodeSummary). We
  // derive compliance status from scan aggregate counts instead.
  const failing = useMemo(() => {
    const convergedFailed = (nodes ?? []).filter((n) => n.status === "failed").length;
    // Count unique nodes with non-zero failed_count in their latest scan
    const scanByNode = new Map<string, { latest: string; failed: number }>();
    for (const scan of scans ?? []) {
      const existing = scanByNode.get(scan.nodeId);
      if (!existing || scan.startedAt > existing.latest) {
        scanByNode.set(scan.nodeId, { latest: scan.startedAt, failed: scan.failed });
      }
    }
    const complianceFailed = Array.from(scanByNode.values()).filter((s) => s.failed > 0).length;
    return convergedFailed + complianceFailed;
  }, [nodes, scans]);

  return (
    <div className="flex min-h-screen bg-background">
      <aside className="sticky top-0 hidden h-screen w-60 shrink-0 flex-col border-r border-sidebar-border bg-sidebar lg:flex">
        <div className="flex h-14 items-center gap-2.5 border-b border-sidebar-border px-4">
          <div className="grid size-7 place-items-center rounded-md bg-primary text-primary-foreground">
            <svg viewBox="0 0 24 24" className="size-4" fill="none" stroke="currentColor" strokeWidth="2">
              <circle cx="12" cy="12" r="3" />
              <path d="M12 2v4M12 18v4M2 12h4M18 12h4M5 5l3 3M16 16l3 3M19 5l-3 3M8 16l-3 3" />
            </svg>
          </div>
          <div className="leading-tight">
            <div className="text-sm font-semibold tracking-tight">Spindle</div>
            <div className="text-[10px] tracking-wide text-muted-foreground uppercase">Fleet automation</div>
          </div>
        </div>
        <nav className="flex-1 space-y-0.5 px-2 py-3">
          {NAV.map((item) => (
            <Link
              key={item.to}
              to={item.to}
              activeOptions={{ exact: "exact" in item ? Boolean(item.exact) : false }}
              className="group flex items-center gap-2.5 rounded-md px-2.5 py-2 text-sm text-sidebar-foreground transition-colors hover:bg-sidebar-accent data-[status=active]:bg-sidebar-accent data-[status=active]:font-medium data-[status=active]:text-sidebar-accent-foreground"
            >
              <item.icon className="size-4 text-muted-foreground group-data-[status=active]:text-primary" />
              {item.label}
              {item.label === "Nodes" && failing > 0 && (
                <span className="num ml-auto rounded-full bg-fail-soft px-1.5 text-[10px] text-fail">{failing}</span>
              )}
            </Link>
          ))}
        </nav>
        <div className="border-t border-sidebar-border p-3">
          <div className="rounded-md border border-border bg-elevated/60 p-2.5">
            <div className="label-caps">Spindle server</div>
            <div className="mt-1 flex items-center gap-1.5 text-xs">
              <StatusDot status="success" />
              <span className="text-foreground">connected</span>
            </div>
          </div>
        </div>
      </aside>

      <div className="flex min-w-0 flex-1 flex-col">
        <header className="sticky top-0 z-30 flex h-14 items-center gap-3 border-b border-border bg-background/85 px-4 backdrop-blur">
          <div className="flex flex-1 items-center gap-3">
            <GlobalSearch />
          </div>
          <Button variant="ghost" size="icon" className="size-8" onClick={toggle} aria-label="Toggle theme">
            {dark ? <Sun className="size-4" /> : <Moon className="size-4" />}
          </Button>
          <Button variant="ghost" size="icon" className="size-8" aria-label="Help">
            <LifeBuoy className="size-4" />
          </Button>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button className="flex items-center gap-2 rounded-md py-1 pr-1.5 pl-1 transition-colors hover:bg-accent/50">
                <span className="grid size-7 place-items-center rounded-full bg-accent text-[11px] font-semibold text-accent-foreground">
                  {user.initials}
                </span>
                <span className="hidden text-left leading-tight sm:block">
                  <span className="block text-xs font-medium">{user.displayName}</span>
                </span>
                <ChevronDown className="size-3.5 text-muted-foreground" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-52">
              <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">
                {user.sub}
              </DropdownMenuLabel>
              <DropdownMenuSeparator />
              <DropdownMenuItem asChild className="text-xs">
                <Link to="/__spindle-admin/settings">Organization settings</Link>
              </DropdownMenuItem>
              <DropdownMenuItem className="text-xs">API documentation</DropdownMenuItem>
              <DropdownMenuItem className="text-xs">Keyboard shortcuts</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem className="text-xs text-fail">Sign out</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </header>

        <div className="mx-auto w-full max-w-[1600px] flex-1 px-4 py-5 sm:px-6 lg:px-8">{children}</div>

        <footer className={cn("mt-4 border-t border-border px-6 py-3 text-[11px] text-muted-foreground")}>
          Spindle &middot; fleet automation platform
        </footer>
      </div>
    </div>
  );
}
