import { cn } from "@/lib/utils";
import type { StatusKind } from "@/lib/mock/types";

const KIND_MAP: Record<string, StatusKind> = {
  success: "ok",
  passed: "ok",
  compliant: "ok",
  ok: "ok",
  updated: "ok",
  active: "ok",
  enabled: "ok",
  failed: "fail",
  fail: "fail",
  "non-compliant": "fail",
  revoked: "fail",
  missing: "warn",
  warn: "warn",
  offline: "warn",
  invited: "warn",
  suspended: "warn",
  waived: "warn",
  skipped: "unknown",
  unknown: "unknown",
  "up-to-date": "unknown",
  disabled: "unknown",
};

export const statusKind = (status: string): StatusKind => KIND_MAP[status] ?? "unknown";

const TONE: Record<StatusKind, string> = {
  ok: "bg-ok-soft text-ok border-ok/30",
  fail: "bg-fail-soft text-fail border-fail/30",
  warn: "bg-warn-soft text-warn border-warn/35",
  unknown: "bg-unknown-soft text-muted-foreground border-border",
};

const DOT: Record<StatusKind, string> = {
  ok: "bg-ok",
  fail: "bg-fail",
  warn: "bg-warn",
  unknown: "bg-unknown",
};

export function StatusDot({ status, pulse }: { status: string; pulse?: boolean }) {
  const kind = statusKind(status);
  return (
    <span className="relative inline-flex size-2 shrink-0">
      {pulse && kind === "fail" && (
        <span className={cn("absolute inset-0 animate-ping rounded-full opacity-70", DOT[kind])} />
      )}
      <span className={cn("relative size-2 rounded-full", DOT[kind])} />
    </span>
  );
}

const LABELS: Record<string, string> = {
  success: "Success",
  failed: "Failed",
  missing: "Missing",
  passed: "Passed",
  skipped: "Skipped",
  waived: "Waived",
  compliant: "Compliant",
  "non-compliant": "Non-compliant",
  unknown: "Unknown",
  "up-to-date": "Up to date",
  updated: "Updated",
};

export function StatusPill({
  status,
  label,
  className,
  dot = true,
  size = "md",
}: {
  status: string;
  label?: string;
  className?: string;
  dot?: boolean;
  size?: "sm" | "md";
}) {
  const kind = statusKind(status);
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-full border font-medium whitespace-nowrap",
        size === "sm" ? "px-1.5 py-px text-[11px]" : "px-2 py-0.5 text-xs",
        TONE[kind],
        className,
      )}
    >
      {dot && <span className={cn("size-1.5 rounded-full", DOT[kind])} />}
      {label ?? LABELS[status] ?? status}
    </span>
  );
}

const SEVERITY_TONE: Record<string, string> = {
  critical: "bg-fail-soft text-fail border-fail/30",
  high: "bg-warn-soft text-warn border-warn/35",
  medium: "bg-info-soft text-info border-info/30",
  low: "bg-unknown-soft text-muted-foreground border-border",
};

export function SeverityBadge({ severity, impact }: { severity: string; impact?: number }) {
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 rounded-md border px-1.5 py-px text-[11px] font-medium capitalize",
        SEVERITY_TONE[severity] ?? SEVERITY_TONE["low"],
      )}
    >
      {severity}
      {impact !== undefined && <span className="num opacity-70">{impact.toFixed(1)}</span>}
    </span>
  );
}

export function Tag({ children }: { children: React.ReactNode }) {
  return (
    <span className="rounded border border-border bg-elevated px-1.5 py-px text-[11px] text-muted-foreground">
      {children}
    </span>
  );
}
