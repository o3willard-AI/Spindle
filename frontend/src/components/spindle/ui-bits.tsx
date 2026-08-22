import { Link } from "@tanstack/react-router";
import { ChevronRight } from "lucide-react";
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { Sparkline } from "./charts";

export function PageHeader({
  title,
  description,
  breadcrumbs,
  actions,
  meta,
}: {
  title: ReactNode;
  description?: ReactNode;
  breadcrumbs?: Array<{ label: string; to?: string }>;
  actions?: ReactNode;
  meta?: ReactNode;
}) {
  return (
    <header className="space-y-3">
      {breadcrumbs && breadcrumbs.length > 0 && (
        <nav aria-label="Breadcrumb" className="flex items-center gap-1 text-xs text-muted-foreground">
          {breadcrumbs.map((b, i) => (
            <span key={`${b.label}-${i}`} className="inline-flex items-center gap-1">
              {i > 0 && <ChevronRight className="size-3 opacity-50" />}
              {b.to ? (
                <Link to={b.to} className="transition-colors hover:text-foreground">
                  {b.label}
                </Link>
              ) : (
                <span className="text-foreground">{b.label}</span>
              )}
            </span>
          ))}
        </nav>
      )}
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <h1 className="truncate text-xl font-semibold text-foreground">{title}</h1>
          {description && <p className="text-sm text-muted-foreground">{description}</p>}
          {meta}
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
    </header>
  );
}

export function Panel({
  title,
  description,
  actions,
  children,
  className,
  bodyClassName,
  footer,
}: {
  title?: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  bodyClassName?: string;
  footer?: ReactNode;
}) {
  return (
    <section className={cn("panel flex flex-col", className)}>
      {(title || actions) && (
        <div className="flex items-start justify-between gap-3 border-b border-border px-4 py-3">
          <div className="min-w-0">
            {title && <h2 className="text-sm font-semibold text-foreground">{title}</h2>}
            {description && <p className="mt-0.5 text-xs text-muted-foreground">{description}</p>}
          </div>
          {actions && <div className="flex shrink-0 items-center gap-1.5">{actions}</div>}
        </div>
      )}
      <div className={cn("flex-1 p-4", bodyClassName)}>{children}</div>
      {footer && <div className="border-t border-border px-4 py-2.5">{footer}</div>}
    </section>
  );
}

export function KpiCard({
  label,
  value,
  sub,
  tone = "neutral",
  spark,
  sparkTone,
  footer,
}: {
  label: string;
  value: ReactNode;
  sub?: ReactNode;
  tone?: "neutral" | "ok" | "fail" | "warn";
  spark?: number[];
  sparkTone?: "ok" | "fail" | "warn" | "info";
  footer?: ReactNode;
}) {
  const valueTone = {
    neutral: "text-foreground",
    ok: "text-ok",
    fail: "text-fail",
    warn: "text-warn",
  }[tone];
  return (
    <div className="panel relative overflow-hidden p-4">
      <div className="label-caps">{label}</div>
      <div className="mt-2 flex items-end justify-between gap-2">
        <div className={cn("num text-[28px] leading-none font-semibold", valueTone)}>{value}</div>
        {sub && <div className="pb-0.5 text-xs text-muted-foreground">{sub}</div>}
      </div>
      {spark && <Sparkline data={spark} tone={sparkTone ?? "ok"} className="mt-3" />}
      {footer && <div className="mt-3 border-t border-border pt-2 text-xs text-muted-foreground">{footer}</div>}
    </div>
  );
}

export function MetaGrid({ items }: { items: Array<{ label: string; value: ReactNode }> }) {
  return (
    <dl className="grid grid-cols-2 gap-x-6 gap-y-3 sm:grid-cols-3 lg:grid-cols-4">
      {items.map((it) => (
        <div key={it.label} className="min-w-0">
          <dt className="label-caps">{it.label}</dt>
          <dd className="mt-0.5 truncate text-sm text-foreground">{it.value}</dd>
        </div>
      ))}
    </dl>
  );
}

export function EmptyState({
  title,
  description,
  icon,
  action,
}: {
  title: string;
  description?: string;
  icon?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="flex flex-col items-center justify-center px-6 py-14 text-center">
      {icon && <div className="mb-3 text-muted-foreground">{icon}</div>}
      <p className="text-sm font-medium text-foreground">{title}</p>
      {description && <p className="mt-1 max-w-sm text-xs text-muted-foreground">{description}</p>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

export function KeyValue({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4 border-b border-border/60 py-1.5 last:border-0">
      <span className="text-xs text-muted-foreground">{label}</span>
      <span className="num truncate text-xs text-foreground">{children}</span>
    </div>
  );
}

export function CodeBlock({ content, className }: { content: string; className?: string }) {
  return (
    <pre
      className={cn(
        "scroll-thin max-h-[28rem] overflow-auto rounded-md border border-border bg-background p-3 font-mono text-[11.5px] leading-relaxed text-foreground/90",
        className,
      )}
    >
      {content}
    </pre>
  );
}
