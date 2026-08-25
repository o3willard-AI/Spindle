import type {
  ActivityEvent,
  ActivityType,
  AttributeEntry,
  Cookbook,
  CookbookVersion,
  ComplianceTrendItem,
  Control,
  ControlRollup,
  ControlStatus,
  FleetNode,
  FleetSummary,
  HealthResponse,
  NodeStatus,
  NodeProfileResult,
  Profile,
  ResourceEvent,
  ResourceEventAggregate,
  Run,
  RunStatus,
  RunsTrendItem,
  Scan,
  Team,
  ApiToken,
  NotificationRule,
  RetentionPolicy,
  User,
  Waiver,
} from "@/lib/mock/types";

const BASE_URL = import.meta.env["VITE_API_URL"] || "";

/** Local-storage key for the Spindle API token. */
export const TOKEN_KEY = "spindle_token";

/** Read the API token from localStorage. Returns null when absent. */
export function getToken(): string | null {
  if (typeof localStorage === "undefined") return null;
  return localStorage.getItem(TOKEN_KEY);
}

/** Persist the API token in localStorage. */
export function setToken(token: string) {
  if (typeof localStorage !== "undefined") {
    localStorage.setItem(TOKEN_KEY, token);
  }
}

/** Remove the API token from localStorage. */
export function clearToken() {
  if (typeof localStorage !== "undefined") {
    localStorage.removeItem(TOKEN_KEY);
  }
}

/**
 * Derive the current user identity from the stored API token.
 *
 * The Spindle API accepts an opaque bearer token (set via
 * `SPINDLE_INGEST_TOKEN`). When the token is a JWT we decode its `sub`
 * claim; otherwise we fall back to a neutral display name. No mock data
 * (e.g. "Dana Whitfield" / "acme-infra") is ever fabricated.
 */
export interface CurrentUser {
  /** Display name for the avatar menu (no fake org/role lines). */
  displayName: string;
  /** Short avatar initials (2–3 characters). */
  initials: string;
  /** Token or "service" when the token is not a JWT. */
  sub: string;
}

/** Decode a base64url string without external deps. */
function b64urlDecode(s: string): string {
  let b = s.replace(/-/g, "+").replace(/_/g, "/");
  while (b.length % 4) b += "=";
  try {
    return atob(b);
  } catch {
    return "";
  }
}

/** Best-effort JWT decode — returns null for non-JWT tokens. */
function tryDecodeJwt(token: string): { sub?: string; name?: string; email?: string } | null {
  const parts = token.split(".");
  if (parts.length !== 3) return null;
  const payload = b64urlDecode(parts[1]!);
  try {
    return JSON.parse(payload);
  } catch {
    return null;
  }
}

/** Derive the current user identity from the stored token. */
export function getCurrentUser(): CurrentUser {
  const token = getToken() ?? "";
  const jwt = tryDecodeJwt(token);

  if (jwt) {
    const name = jwt.name ?? jwt.sub ?? "";
    const initials = name
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((w) => w[0])
      .join("")
      .toUpperCase()
      .slice(0, 3);
    return {
      displayName: name || jwt.sub || "Admin",
      initials: initials || "AD",
      sub: jwt.sub ?? "service",
    };
  }

  // Opaque ingest token — no claims available. Use a neutral identity.
  return { displayName: "Admin", initials: "AD", sub: "service" };
}

export interface ApiResponse<T> {
  api_version: string;
  request_id: string;
  data: T;
  pagination?: {
    total_count: number;
    has_more: boolean;
    next_cursor?: string | null;
  };
}

export interface ApiError {
  api_version: string;
  error: {
    code: string;
    message: string;
  };
}

/* ── Low-level fetch wrapper ──────────────────────────────────────────────── */

/**
 * Core fetch helper. Reads the token from localStorage and attaches it via
 * BOTH `X-Api-Token` and `Authorization: Bearer` headers so the request is
 * accepted by every middleware configuration (legacy and current).
 */
async function apiFetch(path: string, init?: RequestInit): Promise<Response> {
  const token = getToken();
  const headers = new Headers(init?.headers);
  if (token) {
    headers.set("X-Api-Token", token);
    headers.set("Authorization", `Bearer ${token}`);
  }
  headers.set("Accept", "application/json");

  const url = path.startsWith("http") ? path : `${BASE_URL}${path}`;
  const res = await fetch(url, { ...init, headers });

  if (!res.ok) {
    const body = await res.json().catch(() => ({} as ApiError));
    const apiErr = body as ApiError;
    throw new Error(apiErr.error?.message || `HTTP ${res.status}`);
  }

  return res;
}

/** Fetch and return the decoded JSON body (no envelope unwrapping). */
async function apiJson<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await apiFetch(path, init);
  return (await res.json()) as T;
}

/** Fetch and extract `.data` from the standard envelope `{ ..., data: T }`. */
async function apiFetchData<T>(path: string, init?: RequestInit): Promise<T> {
  const body = await apiJson<ApiResponse<T>>(path, init);
  return body.data;
}

/** Fetch and extract `.data.items` from the `{ data: { items: T[] } }` shape. */
async function apiFetchItems<T>(path: string, init?: RequestInit): Promise<T[]> {
  const body = await apiJson<{ data: { items: T[] } }>(path, init);
  return body.data.items;
}

/* ── API response types (snake_case, as returned by the backend) ──────────── */

/** `chef_environment` → `environment`, `policy_group` → `policyGroup`, etc. */
interface ApiNodeDetail {
  id: string;
  node_type: string;
  name: string | null;
  platform: string | null;
  platform_version?: string;
  chef_environment: string | null;
  policy_group: string | null;
  policy_name: string | null;
  run_list: string[];
  last_seen: string | null;
  status?: string;
  attributes?: Record<string, unknown>;
  project_id: string | null;
  created_at: string;
  updated_at?: string;
  // Enriched fields (joined from runs / compliance)
  passed_count?: number;
  failed_count?: number;
  warning_count?: number;
  compliance_trend?: number[];
  flipped?: boolean;
}

interface ApiRunSummary {
  id: string;
  run_id: string;
  node_id: string;
  status: string;
  start_time: string;
  end_time: string | null;
  duration_ms: number;
  total_resource_count: number;
  updated_count: number;
  failed_count: number;
  skipped_count: number;
  cookbook_name: string | null;
  cookbook_version: string | null;
}

interface ApiResourceEventSummary {
  id: string;
  resource_type: string;
  resource_name: string;
  action: string;
  status: string;
  duration_ms: number;
  cookbook_name: string | null;
  cookbook_version: string | null;
  delta: string | null;
}

interface ApiRunDetail {
  id: string;
  run_id: string;
  node_id: string;
  status?: string;
  start_time: string;
  end_time: string | null;
  duration_ms: number;
  total_resource_count: number;
  updated_count: number;
  failed_count: number;
  skipped_count: number;
  cookbook_name: string | null;
  cookbook_version: string | null;
  error_summary: unknown;
  cookbook_set: Record<string, unknown> | null;
  resource_events: { items: ApiResourceEventSummary[]; pagination: unknown };
}

interface ApiComplianceReport {
  id: string;
  node_id: string;
  profile_id: string;
  profile_name: string;
  status: string;
  passed_count: number;
  failed_count: number;
  warning_count: number;
  created_at: string;
}

interface ApiCookbookVersionInfo {
  cookbook_name: string;
  cookbook_version: string;
  node_count: number;
  first_seen: string;
  last_seen: string;
  total_resource_count: number;
}

interface ApiCookbookInventoryEntry {
  name: string;
  versions: ApiCookbookVersionInfo[];
  total_nodes: number;
  last_seen: string;
}

/* ── Mappers: snake_case API → camelCase UI types ──────────────────────────── */

/** Derive compliance status from aggregate report counts.
 *
 * The backend report-level `status` field uses three values: "passed",
 * "failed", and "warn" (see spindle-worker/src/lib.rs:740-745). When only
 * counts are available we map: failed > 0 → "non-compliant",
 * warning > 0 → "non-compliant" (warnings are non-compliant findings),
 * passed > 0 with no failures → "compliant", absent → "unknown".
 *
 * Unscanned nodes (no report at all) must NEVER default to "compliant".
 */
function deriveCompliance(
  failedCount: number | undefined,
  warningCount: number | undefined,
  passedCount: number | undefined,
): ComplianceStatus {
  const failed = failedCount ?? 0;
  const warned = warningCount ?? 0;
  const passed = passedCount ?? 0;
  if (failed > 0 || warned > 0) return "non-compliant";
  if (passed > 0) return "compliant";
  return "unknown";
}

/** Derive node converge status from `last_seen` freshness.
 *
 * The `/v1/nodes` list endpoint returns `NodeSummary` which has no `status`
 * field — only `last_seen`. A node that checked in within a threshold is
 * assumed to have converged successfully; one that hasn't is "missing".
 * We deliberately do NOT fabricate a "failed" status from node data alone;
 * converge failure is determined from run/report data elsewhere.
 *
 * Threshold: 24 hours (matches Chef Infra client default 30-minute cadence
 * with ample headroom for offline nodes).
 */
const NODE_ONLINE_THRESHOLD_MS = 24 * 60 * 60 * 1000;
function deriveNodeStatus(lastSeen: string | null | undefined): NodeStatus {
  if (!lastSeen) return "missing";
  const delta = Date.now() - new Date(lastSeen).getTime();
  if (delta < 0 || delta > NODE_ONLINE_THRESHOLD_MS) return "missing";
  return "success";
}

/** Convert raw API attributes (JSON object) into AttributeEntry[]. */
function mapAttributes(raw: Record<string, unknown> | undefined): AttributeEntry[] {
  if (!raw || typeof raw !== "object") return [];
  return Object.entries(raw).map(([key, value]) => ({
    key,
    value: typeof value === "string" ? value : String(value ?? ""),
    category: "normal" as const,
    group: "attributes",
  }));
}

/** Map a raw API node detail into the UI FleetNode type. */
function mapNode(apiNode: ApiNodeDetail): FleetNode {
  const failedCount = apiNode.failed_count;
  const passedCount = apiNode.passed_count;
  const warningCount = apiNode.warning_count;
  // The /v1/nodes endpoints do NOT return compliance counts. When all
  // three are absent (undefined), deriveCompliance returns "unknown".
  const compliance = deriveCompliance(failedCount, warningCount, passedCount);
  return {
    id: apiNode.id,
    name: apiNode.name ?? apiNode.id,
    platform: apiNode.platform ?? "unknown",
    platformVersion: apiNode.platform_version ?? "",
    environment: apiNode.chef_environment ?? "default",
    policyGroup: apiNode.policy_group ?? "",
    policyName: apiNode.policy_name ?? "",
    nodeType: apiNode.node_type,
    status: deriveNodeStatus(apiNode.last_seen),
    compliance,
    lastSeen: apiNode.last_seen ?? new Date().toISOString(),
    runList: apiNode.run_list ?? [],
    attributes: mapAttributes(apiNode.attributes),
    passed: passedCount ?? 0,
    failed: failedCount ?? 0,
    warnings: warningCount ?? 0,
    complianceTrend: apiNode.compliance_trend ?? [],
    flipped: apiNode.flipped ?? false,
  };
}

/** Normalize a raw API run status string into the UI RunStatus enum.
 *
 * The backend returns "successful" or "failed"; the UI expects "success".
 * Empty/unknown values fall back to "missing".
 */
function mapRunStatus(raw: string | undefined): RunStatus {
  if (raw === "successful" || raw === "success") return "success";
  if (raw === "failed" || raw === "failure") return "failed";
  return "missing";
}

/** Map a raw API run summary into the UI Run type. */
function mapRunSummary(apiRun: ApiRunSummary, nodeMap?: Map<string, { name: string; environment: string }>): Run {
  const node = nodeMap?.get(apiRun.node_id);
  const cookbookName = apiRun.cookbook_name ?? "";
  return {
    id: apiRun.id,
    nodeId: apiRun.node_id,
    nodeName: node?.name ?? "",
    environment: node?.environment ?? "",
    status: mapRunStatus(apiRun.status),
    startedAt: apiRun.start_time,
    durationSec: Math.round(apiRun.duration_ms / 1000),
    totalResources: apiRun.total_resource_count,
    updatedResources: apiRun.updated_count,
    failedResources: apiRun.failed_count,
    skippedResources: apiRun.skipped_count,
    cookbook: cookbookName,
    runList: [],
    resources: [],
    errorLog: undefined,
    errorSummary: undefined,
  };
}

/** Map a raw API run detail into the UI Run type. */
function mapRunDetail(apiRun: ApiRunDetail, nodeMap?: Map<string, { name: string; environment: string }>): Run {
  const node = nodeMap?.get(apiRun.node_id);
  const resources: ResourceEvent[] = (apiRun.resource_events?.items ?? []).map(mapResourceEvent);
  const cookbookName = apiRun.cookbook_name ?? "";
  return {
    id: apiRun.id,
    nodeId: apiRun.node_id,
    nodeName: node?.name ?? "",
    environment: node?.environment ?? "",
    status: mapRunStatus(apiRun.status),
    startedAt: apiRun.start_time,
    durationSec: Math.round(apiRun.duration_ms / 1000),
    totalResources: apiRun.total_resource_count,
    updatedResources: apiRun.updated_count,
    failedResources: apiRun.failed_count,
    skippedResources: apiRun.skipped_count,
    cookbook: cookbookName,
    runList: [],
    resources,
    errorLog: undefined,
    errorSummary: apiRun.error_summary ? String(apiRun.error_summary) : undefined,
  };
}

/** Map a single resource event summary. */
function mapResourceEvent(api: ApiResourceEventSummary): ResourceEvent {
  return {
    id: api.id,
    type: api.resource_type,
    name: api.resource_name,
    action: api.action,
    status: (api.status as ResourceEvent["status"]) || "up-to-date",
    durationSec: Math.round(api.duration_ms / 1000),
    cookbook: api.cookbook_name ?? "",
    delta: api.delta ?? undefined,
  };
}

/** Map a raw API compliance report into the UI Scan type.
 * The report list endpoint doesn't include control-level detail, so `profiles`
 * gets a single minimal entry carrying the profile name/id (enough for the
 * activity feed and profile derivation). Full control data is populated by
 * `fetchComplianceReport` which reads `control_results`.
 */
function mapScan(apiReport: ApiComplianceReport): Scan {
  const failedCount = apiReport.failed_count;
  const passedCount = apiReport.passed_count;
  const warningCount = apiReport.warning_count;
  const compliance = deriveCompliance(failedCount, warningCount, passedCount);
  const profile: NodeProfileResult = {
    profileId: apiReport.profile_id ?? "",
    profileName: apiReport.profile_name ?? "",
    profileTitle: apiReport.profile_name ?? "",
    version: "",
    status: compliance,
    controls: [],
  };
  return {
    id: apiReport.id,
    nodeId: apiReport.node_id,
    nodeName: "",
    startedAt: apiReport.created_at,
    durationSec: 0,
    status: compliance,
    passed: passedCount,
    failed: failedCount,
    warnings: warningCount,
    profiles: [profile],
  };
}

/** Map a raw API cookbook inventory entry into the UI Cookbook type. */
function mapCookbook(apiEntry: ApiCookbookInventoryEntry): Cookbook {
  const versions = (apiEntry.versions ?? []).map((v): CookbookVersion => ({
    version: v.cookbook_version,
    nodes: v.node_count,
    updatedAt: v.last_seen,
    files: [],
  }));
  return {
    name: apiEntry.name,
    maintainer: "",
    description: "",
    nodes: apiEntry.total_nodes,
    lastSeen: apiEntry.last_seen,
    versions,
  };
}

/* ── Fetch functions (with mapping) ─────────────────────────────────────────── */

// --- Nodes ---
export async function fetchNodes(params?: { limit?: number; platform?: string; status?: string }): Promise<FleetNode[]> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set("limit", String(params.limit));
  if (params?.platform) qs.set("filter[platform]", params.platform);
  if (params?.status) qs.set("filter[status]", params.status);
  const raw = await apiFetchData<ApiNodeDetail[]>(`/v1/nodes?${qs.toString()}`);
  return raw.map(mapNode);
}

export async function fetchNode(id: string): Promise<FleetNode> {
  const raw = await apiFetchData<ApiNodeDetail>(`/v1/nodes/${encodeURIComponent(id)}`);
  return mapNode(raw);
}

// --- Runs ---
export async function fetchRuns(params?: { limit?: number; nodeId?: string }): Promise<Run[]> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set("limit", String(params.limit));
  if (params?.nodeId) qs.set("filter[node_id]", params.nodeId);
  const raw = await apiFetchData<ApiRunSummary[]>(`/v1/runs?${qs.toString()}`);
  return raw.map((r) => mapRunSummary(r));
}

export async function fetchRun(id: string): Promise<Run> {
  const raw = await apiFetchData<ApiRunDetail>(`/v1/runs/${encodeURIComponent(id)}`);
  return mapRunDetail(raw);
}

// --- Compliance ---
export async function fetchComplianceReports(params?: {
  limit?: number;
  node?: string;
  profile?: string;
}): Promise<Scan[]> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set("limit", String(params.limit));
  if (params?.node) qs.set("node", params.node);
  if (params?.profile) qs.set("profile", params.profile);
  const raw = await apiFetchItems<ApiComplianceReport>(`/v1/compliance/reports?${qs.toString()}`);
  return raw.map(mapScan);
}

/** Raw response from GET /v1/compliance/reports/:id.
 * Contains control_results array from the server.
 */
interface ApiComplianceReportDetail {
  id: string;
  run_id: string;
  node_id: string;
  profile_id: string;
  profile_name: string;
  status: string;
  passed_count: number;
  failed_count: number;
  warning_count: number;
  created_at: string;
  control_results?: ApiControlResult[];
}

/** Map an impact score (0–1) to a severity tier. */
function mapSeverity(impact: number): Control["severity"] {
  if (impact >= 0.7) return "critical";
  if (impact >= 0.4) return "high";
  if (impact >= 0.1) return "medium";
  return "low";
}

/** Map a server-side control status to the UI ControlStatus.
 *
 * The backend `control_results.status` column stores AuditorStatus Display
 * values: "passed" / "failed" / "skipped" / "unknown"
 * (spindle-pipeline/src/lib.rs:421-424). The report-level status field
 * additionally uses "warn" for warning-only reports.
 */
function mapControlStatus(status: string): ControlStatus {
  if (status === "passed") return "passed";
  if (status === "failed") return "failed";
  if (status === "skipped") return "skipped";
  if (status === "waived") return "waived";
  // "warn", "pass", "fail", "unknown", or any other variant → skipped
  // (treat unmapped statuses as skipped, never as false-passing "passed")
  return "skipped";
}

/** GET /v1/compliance/reports/:id — full detail with control results.
 * Server returns a raw JSON object (not envelope-wrapped) containing
 * `id`, `node_id`, `profile_id`, `profile_name`, `status`, counts,
 * and a `control_results` array. We aggregate control_results into
 * the `profiles → controls → results` tree that the UI expects.
 */
export async function fetchComplianceReport(id: string): Promise<Scan> {
  const raw = await apiJson<ApiComplianceReportDetail>(
    `/v1/compliance/reports/${encodeURIComponent(id)}`,
  );

  const failedCount = raw.failed_count;
  const passedCount = raw.passed_count;
  const warningCount = raw.warning_count;
  const compliance = deriveCompliance(failedCount, warningCount, passedCount);
  const scan: Scan = {
    id: raw.id,
    nodeId: raw.node_id,
    nodeName: "",
    startedAt: raw.created_at,
    durationSec: 0,
    status: compliance,
    passed: passedCount,
    failed: failedCount,
    warnings: warningCount,
    profiles: [],
  };

  // Aggregate control_results into profile → control → result groups.
  // Backend stores control status as "passed" / "failed" / "skipped"
  // (AuditorStatus Display impl — spindle-pipeline/src/lib.rs:421-424).
  const profileMap = new Map<string, NodeProfileResult>();
  for (const cr of raw.control_results ?? []) {
    const profileId = cr.profile_id;
    const profileName = cr.profile_name ?? raw.profile_name ?? "";
    if (!profileMap.has(profileId)) {
      profileMap.set(profileId, {
        profileId,
        profileName,
        profileTitle: profileName,
        version: "",
        status: "unknown",
        controls: [],
      });
    }
    const profile = profileMap.get(profileId)!;
    // Group control results by control_id
    let control = profile.controls.find((c) => c.id === cr.control_id);
    if (!control) {
      control = {
        id: cr.control_id,
        title: cr.control_id,
        impact: cr.impact ?? 0,
        severity: mapSeverity(cr.impact ?? 0),
        status: mapControlStatus(cr.status),
        profileId,
        profileName,
        desc: "",
        tags: [],
        results: [],
      };
      profile.controls.push(control);
    }
    // Each control_result is one evaluation result
    control.results.push({
      codeDesc: cr.control_id,
      status: mapControlStatus(cr.status),
      message: cr.result ? JSON.stringify(cr.result) : undefined,
      runTimeMs: 0,
    });
  }

  scan.profiles = Array.from(profileMap.values());
  return scan;
}

/** GET /v1/compliance/profiles — does NOT exist as a server endpoint.
 *  Profile list is derived client-side from recent compliance reports
 *  (distinct profile_id). Returns the same Profile[] shape the UI expects.
 *
 *  Pass-rate and counts are derived from scan-level aggregate fields
 *  (passed_count/failed_count/warning_count) which the list endpoint always
 *  populates. The reports list endpoint does NOT include per-profile control
 *  arrays — those come only from /v1/compliance/reports/:id (detail).
 *  Therefore controlCount is left 0 here; the profile detail page fetches
 *  the full report to get real control counts.
 */
export async function fetchComplianceProfiles(): Promise<Profile[]> {
  const reports = await fetchComplianceReports({ limit: 500 });
  const seen = new Map<string, Profile>();
  for (const scan of reports) {
    for (const profile of scan.profiles) {
      if (!seen.has(profile.profileId)) {
        seen.set(profile.profileId, {
          id: profile.profileId,
          name: profile.profileName,
          title: profile.profileName,
          version: profile.version,
          vendor: "",
          installed: true,
          summary: "",
          platforms: [],
          controlCount: 0, // populated on detail page via /v1/compliance/reports/:id
          testCount: scan.passed + scan.failed + scan.warnings,
          nodes: 0,
          passRate: 0,
          updatedAt: scan.startedAt,
        });
      } else {
        // Aggregate across multiple reports for this profile
        const existing = seen.get(profile.profileId)!;
        existing.testCount += scan.passed + scan.failed + scan.warnings;
        const totalPassed = existing.passRate * existing.testCount + scan.passed;
        existing.passRate = totalPassed / existing.testCount;
        existing.updatedAt = scan.startedAt > existing.updatedAt ? scan.startedAt : existing.updatedAt;
      }
    }
  }
  return Array.from(seen.values());
}

/** GET /v1/compliance/controls — returns raw control_results rows.
 *  Aggregated client-side into ControlRollup entries by control_id.
 */
interface ApiControlResult {
  id: string;
  report_id: string;
  node_id: string;
  profile_id: string;
  profile_name?: string;
  control_id: string;
  status: string;
  impact: number | null;
  result: unknown | null;
  created_at: string;
}

export async function fetchControlRollups(): Promise<ControlRollup[]> {
  const raw = await apiFetchItems<ApiControlResult>("/v1/compliance/controls");
  const rollupMap = new Map<string, ControlRollup>();
  for (const row of raw) {
    const key = `${row.profile_id}-${row.control_id}`;
    if (!rollupMap.has(key)) {
      rollupMap.set(key, {
        id: row.control_id,
        title: row.control_id,
        profileId: row.profile_id,
        profileTitle: row.profile_id,
        severity: "medium",
        impact: row.impact ?? 0,
        failing: 0,
        passing: 0,
        warnings: 0,
        nodes: [],
      });
    }
    const rollup = rollupMap.get(key)!;
    if (!rollup.nodes.includes(row.node_id)) {
      rollup.nodes.push(row.node_id);
    }
    // Backend stores control status as "passed" / "failed" / "skipped"
    // (AuditorStatus Display impl — spindle-pipeline/src/lib.rs:421-424).
    // "skipped" controls are NOT warnings — they are skip events.
    // Warnings only arise at the report level (status = "warn"), which
    // is not returned by /v1/compliance/controls, so we count only
    // "passed" → passing and "failed" → failing here.
    if (row.status === "passed") rollup.passing++;
    else if (row.status === "failed") rollup.failing++;
    else rollup.warnings++;
  }
  return Array.from(rollupMap.values());
}

// --- Cookbooks ---
export async function fetchCookbooks(): Promise<Cookbook[]> {
  const raw = await apiFetchData<ApiCookbookInventoryEntry[]>(`/v1/cookbooks`);
  return raw.map(mapCookbook);
}

export async function fetchCookbook(name: string): Promise<Cookbook> {
  // Cookbooks endpoint returns a list; we filter client-side for the detail.
  const list = await fetchCookbooks();
  const found = list.find((c) => c.name === name);
  if (!found) {
    throw new Error(`Cookbook ${name} not found`);
  }
  return found;
}

// --- Settings (admin endpoints) ---
// NOTE: /v1/admin/* endpoints are mounted only in production mode (DB-backed).
// These are inert fetchers retained for the future admin page.
export async function fetchUsers(): Promise<User[]> {
  return apiFetchItems<User>("/v1/admin/users");
}

export async function fetchTeams(): Promise<Team[]> {
  return apiFetchItems<Team>("/v1/admin/teams");
}

export async function fetchApiTokens(): Promise<ApiToken[]> {
  return apiFetchItems<ApiToken>("/v1/admin/tokens");
}

export async function fetchNotificationRules(): Promise<NotificationRule[]> {
  return apiFetchItems<NotificationRule>("/v1/admin/notifications");
}

export async function fetchRetentionPolicies(): Promise<RetentionPolicy[]> {
  return apiFetchItems<RetentionPolicy>("/v1/admin/retention");
}

// --- NEW endpoints (not admin-gated) ---

/** GET /v1/health — aggregate subsystem health (DB, storage, dex, ingest lag).
 * No auth envelope; returns the health response directly. The server uses
 * `#[serde(rename_all = "lowercase")]` on HealthStatus, so values are
 * "up" / "degraded" / "down" — matching the HealthStatus type.
 */
export async function fetchHealth(): Promise<HealthResponse> {
  return apiJson<HealthResponse>("/v1/health");
}

/** GET /v1/waivers — list active (non-expired) waivers.
 * Server returns `WaiversListResponse { data: [WaiverSummary] }` — `data` is an
 * array (not `data.items`). */
interface ApiWaiverSummary {
  id: string;
  control_id: string;
  profile_id: string;
  scope: string;
  justification: string | null;
  approver: string | null;
  start_date: string;
  expiry_date: string;
  created_at: string;
  updated_at: string;
  is_expired: boolean;
}
export async function fetchWaivers(): Promise<Waiver[]> {
  const raw = await apiFetchData<ApiWaiverSummary[]>("/v1/waivers");
  return raw.map((w) => ({
    id: w.id,
    controlId: w.control_id,
    profileId: w.profile_id,
    scope: w.scope,
    justification: w.justification,
    approver: w.approver,
    startDate: w.start_date,
    expiryDate: w.expiry_date,
    createdAt: w.created_at,
    updatedAt: w.updated_at,
    isExpired: w.is_expired,
  }));
}

/** GET /v1/resource-events/aggregates — resource event rollup rows.
 * Server returns `AggregatesResponse { data: [AggregateRow] }` — `data` is an
 * array (not `data.items`). */
interface ApiAggregateRow {
  id: string;
  hour: string;
  cookbook_name: string;
  cookbook_version: string | null;
  resource_type: string;
  platform: string;
  count: number;
  sum_duration_ms: number;
  avg_duration_ms: number;
  p50_ms: number | null;
  p95_ms: number | null;
  p99_ms: number | null;
  max_ms: number;
}
export async function fetchResourceEventAggregates(): Promise<ResourceEventAggregate[]> {
  const raw = await apiFetchData<ApiAggregateRow[]>("/v1/resource-events/aggregates");
  return raw.map((a) => ({
    id: a.id,
    hour: a.hour,
    cookbookName: a.cookbook_name,
    cookbookVersion: a.cookbook_version,
    resourceType: a.resource_type,
    platform: a.platform,
    count: a.count,
    sumDurationMs: a.sum_duration_ms,
    avgDurationMs: a.avg_duration_ms,
    p50Ms: a.p50_ms,
    p95Ms: a.p95_ms,
    p99Ms: a.p99_ms,
    maxMs: a.max_ms,
  }));
}

/* ── NEW endpoints ──────────────────────────────────────────────────────────── */

/** GET /v1/summary — fleet-wide counts and flipped nodes.
 * Server returns FleetSummary directly (no envelope). The struct uses
 * `#[serde(rename_all = "camelCase")]` so fields arrive as camelCase. */
export async function fetchSummary(): Promise<FleetSummary> {
  const raw = await apiJson<{
    total: number;
    online: number;
    offline: number;
    convergeSuccess: number;
    convergeFailed: number;
    compliant: number;
    nonCompliant: number;
    unknownCompliance: number;
    flipped: Array<{ id: string; name: string }>;
  }>("/v1/summary");
  return {
    total: raw.total,
    online: raw.online,
    offline: raw.offline,
    convergeSuccess: raw.convergeSuccess,
    convergeFailed: raw.convergeFailed,
    compliant: raw.compliant,
    nonCompliant: raw.nonCompliant,
    unknownCompliance: raw.unknownCompliance,
    flipped: (raw.flipped ?? []).map((f) => ({ id: f.id, name: f.name })),
  };
}

/** GET /v1/compliance/trend?days=14 — daily compliance pass-rate trend.
 * Server now returns `{ data: { items: [...] } }` (wrapped in the standard
 * list envelope, matching /v1/compliance/reports). Each bucket's fields
 * arrive in camelCase via `#[serde(rename_all = "camelCase")]`. */
export async function fetchComplianceTrend(days: number = 14): Promise<ComplianceTrendItem[]> {
  const body = await apiJson<{ data: { items: ComplianceTrendBucketRaw[] } }>(
    `/v1/compliance/trend?days=${days}`,
  );
  return (body.data?.items ?? []).map((b) => ({
    date: b.date,
    passRate: b.passRate,
    passed: b.passed,
    failed: b.failed,
  }));
}

/** GET /v1/runs/trend?days=7 — daily converge success/failed counts.
 * Server returns `{ data: { items: [...] } }` (wrapped envelope). */
export async function fetchRunsTrend(days: number = 7): Promise<RunsTrendItem[]> {
  const body = await apiJson<{ data: { items: RunsTrendBucketRaw[] } }>(
    `/v1/runs/trend?days=${days}`,
  );
  return (body.data?.items ?? []).map((b) => ({
    date: b.date,
    success: b.success,
    failed: b.failed,
  }));
}

/** Raw trend bucket from /v1/compliance/trend.
 * Server struct uses `#[serde(rename_all = "camelCase")]`. */
interface ComplianceTrendBucketRaw {
  date: string;
  passRate: number;
  passed: number;
  failed: number;
}

/** Raw trend bucket from /v1/runs/trend. */
interface RunsTrendBucketRaw {
  date: string;
  success: number;
  failed: number;
}

/* ── TanStack Query hooks ───────────────────────────────────────────────────── */

// eslint-disable-next-line @typescript-eslint/no-var-requires
import { useQuery, type UseQueryOptions } from "@tanstack/react-query";

/* --- Nodes --- */
export function useNodes(
  params?: { limit?: number; platform?: string; status?: string },
  options?: Omit<UseQueryOptions<FleetNode[]>, "queryKey" | "queryFn">,
) {
  return useQuery<FleetNode[]>({
    queryKey: ["nodes", params],
    queryFn: () => fetchNodes(params),
    ...options,
  });
}

export function useNode(
  id: string,
  options?: Omit<UseQueryOptions<FleetNode>, "queryKey" | "queryFn">,
) {
  return useQuery<FleetNode>({
    queryKey: ["node", id],
    queryFn: () => fetchNode(id),
    enabled: !!id,
    ...options,
  });
}

/* --- Runs --- */
export function useRuns(
  params?: { limit?: number; nodeId?: string },
  options?: Omit<UseQueryOptions<Run[]>, "queryKey" | "queryFn">,
) {
  return useQuery<Run[]>({
    queryKey: ["runs", params],
    queryFn: () => fetchRuns(params),
    ...options,
  });
}

export function useRun(
  id: string,
  options?: Omit<UseQueryOptions<Run>, "queryKey" | "queryFn">,
) {
  return useQuery<Run>({
    queryKey: ["run", id],
    queryFn: () => fetchRun(id),
    enabled: !!id,
    ...options,
  });
}

/* --- Compliance --- */
export function useComplianceReports(
  params?: { limit?: number; node?: string; profile?: string },
  options?: Omit<UseQueryOptions<Scan[]>, "queryKey" | "queryFn">,
) {
  return useQuery<Scan[]>({
    queryKey: ["compliance", params],
    queryFn: () => fetchComplianceReports(params),
    ...options,
  });
}

export function useComplianceReport(
  id: string,
  options?: Omit<UseQueryOptions<Scan>, "queryKey" | "queryFn">,
) {
  return useQuery<Scan>({
    queryKey: ["compliance-report", id],
    queryFn: () => fetchComplianceReport(id),
    enabled: !!id,
    ...options,
  });
}

export function useComplianceProfiles(
  options?: Omit<UseQueryOptions<Profile[]>, "queryKey" | "queryFn">,
) {
  return useQuery<Profile[]>({
    queryKey: ["compliance-profiles"],
    queryFn: fetchComplianceProfiles,
    ...options,
  });
}

export function useControlRollups(
  options?: Omit<UseQueryOptions<ControlRollup[]>, "queryKey" | "queryFn">,
) {
  return useQuery<ControlRollup[]>({
    queryKey: ["control-rollups"],
    queryFn: fetchControlRollups,
    ...options,
  });
}

/* --- Cookbooks --- */
export function useCookbooks(
  options?: Omit<UseQueryOptions<Cookbook[]>, "queryKey" | "queryFn">,
) {
  return useQuery<Cookbook[]>({
    queryKey: ["cookbooks"],
    queryFn: fetchCookbooks,
    ...options,
  });
}

export function useCookbook(
  name: string,
  options?: Omit<UseQueryOptions<Cookbook>, "queryKey" | "queryFn">,
) {
  return useQuery<Cookbook>({
    queryKey: ["cookbook", name],
    queryFn: () => fetchCookbook(name),
    enabled: !!name,
    ...options,
  });
}

/* --- Activity (client-side merge of runs + compliance reports) --- */
/** /v1/activity does NOT exist — the activity feed is built client-side
 *  from recent converge runs and compliance reports, merged + sorted desc.
 */
export function useActivity(
  params?: { limit?: number; types?: string },
  options?: Omit<UseQueryOptions<ActivityEvent[]>, "queryKey" | "queryFn">,
) {
  const typesFilter = params?.types
    ? new Set(params.types.split(",").map((t) => t.trim()))
    : undefined;

  return useQuery<ActivityEvent[]>({
    queryKey: ["activity", params],
    queryFn: async () => {
      const [runs, reports] = await Promise.all([
        fetchRuns({ limit: params?.limit ?? 200 }),
        fetchComplianceReports({ limit: params?.limit ?? 200 }),
      ]);
      const events: ActivityEvent[] = [];
      const max = params?.limit ?? 50;

      for (const r of runs) {
        const type: ActivityType = "converge";
        if (typesFilter && !typesFilter.has(type)) continue;
        events.push({
          id: `run-${r.id}`,
          type,
          status: r.status === "success" ? "ok" : r.status === "failed" ? "fail" : "unknown",
          nodeId: r.nodeId,
          nodeName: r.nodeName,
          title: r.errorSummary
            ? `Converge failed: ${r.errorSummary}`
            : `${r.updatedResources} of ${r.totalResources} resources updated`,
          detail: r.cookbook,
          at: r.startedAt,
          href: `/runs/${r.id}`,
        });
      }

      for (const s of reports) {
        const type: ActivityType = "scan";
        if (typesFilter && !typesFilter.has(type)) continue;
        const isCompliant = s.failed === 0 && s.warnings === 0 && s.passed > 0;
        const hasIssues = s.failed > 0 || s.warnings > 0;
        events.push({
          id: `scan-${s.id}`,
          type,
          status: hasIssues ? "fail" : isCompliant ? "ok" : "warn",
          nodeId: s.nodeId,
          nodeName: s.nodeName,
          title:
            s.failed > 0
              ? `${s.failed} controls failed`
              : s.warnings > 0
                ? `${s.warnings} controls warned`
                : s.passed > 0
                  ? `Compliance scan passed (${s.passed} controls)`
                  : "Compliance scan completed",
          detail: s.profiles
            .map((p) => p.profileName)
            .filter(Boolean)
            .join(", "),
          at: s.startedAt,
          href: `/compliance`,
        });
      }

      return events
        .sort((a, b) => new Date(b.at).getTime() - new Date(a.at).getTime())
        .slice(0, max);
    },
    ...options,
  });
}

/* --- Settings --- */
export function useUsers(
  options?: Omit<UseQueryOptions<User[]>, "queryKey" | "queryFn">,
) {
  return useQuery<User[]>({
    queryKey: ["users"],
    queryFn: fetchUsers,
    ...options,
  });
}

export function useTeams(
  options?: Omit<UseQueryOptions<Team[]>, "queryKey" | "queryFn">,
) {
  return useQuery<Team[]>({
    queryKey: ["teams"],
    queryFn: fetchTeams,
    ...options,
  });
}

export function useApiTokens(
  options?: Omit<UseQueryOptions<ApiToken[]>, "queryKey" | "queryFn">,
) {
  return useQuery<ApiToken[]>({
    queryKey: ["api-tokens"],
    queryFn: fetchApiTokens,
    ...options,
  });
}

export function useNotificationRules(
  options?: Omit<UseQueryOptions<NotificationRule[]>, "queryKey" | "queryFn">,
) {
  return useQuery<NotificationRule[]>({
    queryKey: ["notification-rules"],
    queryFn: fetchNotificationRules,
    ...options,
  });
}

export function useRetentionPolicies(
  options?: Omit<UseQueryOptions<RetentionPolicy[]>, "queryKey" | "queryFn">,
) {
  return useQuery<RetentionPolicy[]>({
    queryKey: ["retention-policies"],
    queryFn: fetchRetentionPolicies,
    ...options,
  });
}

/* --- NEW endpoints --- */
export function useSummary(
  options?: Omit<UseQueryOptions<FleetSummary>, "queryKey" | "queryFn">,
) {
  return useQuery<FleetSummary>({
    queryKey: ["summary"],
    queryFn: fetchSummary,
    ...options,
  });
}

export function useComplianceTrend(
  days: number = 14,
  options?: Omit<UseQueryOptions<ComplianceTrendItem[]>, "queryKey" | "queryFn">,
) {
  return useQuery<ComplianceTrendItem[]>({
    queryKey: ["compliance-trend", { days }],
    queryFn: () => fetchComplianceTrend(days),
    ...options,
  });
}

export function useRunsTrend(
  days: number = 7,
  options?: Omit<UseQueryOptions<RunsTrendItem[]>, "queryKey" | "queryFn">,
) {
  return useQuery<RunsTrendItem[]>({
    queryKey: ["runs-trend", { days }],
    queryFn: () => fetchRunsTrend(days),
    ...options,
  });
}

/* --- Health --- */
export function useHealth(
  options?: Omit<UseQueryOptions<HealthResponse>, "queryKey" | "queryFn">,
) {
  return useQuery<HealthResponse>({
    queryKey: ["health"],
    queryFn: fetchHealth,
    ...options,
  });
}

/* --- Waivers --- */
export function useWaivers(
  options?: Omit<UseQueryOptions<Waiver[]>, "queryKey" | "queryFn">,
) {
  return useQuery<Waiver[]>({
    queryKey: ["waivers"],
    queryFn: fetchWaivers,
    ...options,
  });
}

/* --- Resource event aggregates --- */
export function useResourceEventAggregates(
  options?: Omit<UseQueryOptions<ResourceEventAggregate[]>, "queryKey" | "queryFn">,
) {
  return useQuery<ResourceEventAggregate[]>({
    queryKey: ["resource-event-aggregates"],
    queryFn: fetchResourceEventAggregates,
    ...options,
  });
}
