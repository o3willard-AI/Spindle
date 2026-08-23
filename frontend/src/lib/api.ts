import type {
  ActivityEvent,
  AttributeEntry,
  Cookbook,
  CookbookVersion,
  ControlRollup,
  FleetNode,
  FleetSummary,
  NodeStatus,
  Profile,
  ResourceEvent,
  Run,
  RunStatus,
  Scan,
  Team,
  ApiToken,
  NotificationRule,
  RetentionPolicy,
  User,
  ComplianceTrendResponse,
  RunsTrendResponse,
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
  platform_version: string;
  chef_environment: string | null;
  policy_group: string | null;
  policy_name: string | null;
  run_list: string[];
  last_seen: string | null;
  status: string;
  attributes: Record<string, unknown>;
  project_id: string | null;
  created_at: string;
  updated_at: string;
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
  status: string;
  start_time: string;
  end_time: string | null;
  duration_ms: number;
  total_resource_count: number;
  updated_count: number;
  failed_count: number;
  cookbook_name: string | null;
  cookbook_version: string | null;
  error_summary: string | null;
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

/** Derive node compliance status from the latest report's failed_count. */
function deriveCompliance(failedCount: number | undefined): "compliant" | "non-compliant" {
  return (failedCount ?? 0) > 0 ? "non-compliant" : "compliant";
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
  const failedCount = apiNode.failed_count ?? 0;
  const passedCount = apiNode.passed_count ?? 0;
  return {
    id: apiNode.id,
    name: apiNode.name ?? apiNode.id,
    platform: apiNode.platform ?? "unknown",
    platformVersion: apiNode.platform_version,
    environment: apiNode.chef_environment ?? "default",
    policyGroup: apiNode.policy_group ?? "",
    policyName: apiNode.policy_name ?? "",
    nodeType: apiNode.node_type,
    status: (apiNode.status as NodeStatus) || "missing",
    compliance: deriveCompliance(failedCount),
    lastSeen: apiNode.last_seen ?? new Date().toISOString(),
    runList: apiNode.run_list ?? [],
    attributes: mapAttributes(apiNode.attributes),
    passed: passedCount,
    failed: failedCount,
    warnings: apiNode.warning_count ?? 0,
    complianceTrend: apiNode.compliance_trend ?? [],
    flipped: apiNode.flipped ?? false,
  };
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
    status: (apiRun.status as RunStatus) || "missing",
    startedAt: apiRun.start_time,
    durationSec: Math.round(apiRun.duration_ms / 1000),
    totalResources: apiRun.total_resource_count,
    updatedResources: apiRun.updated_count,
    failedResources: apiRun.failed_count,
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
    status: (apiRun.status as RunStatus) || "missing",
    startedAt: apiRun.start_time,
    durationSec: Math.round(apiRun.duration_ms / 1000),
    totalResources: apiRun.total_resource_count,
    updatedResources: apiRun.updated_count,
    failedResources: apiRun.failed_count,
    cookbook: cookbookName,
    runList: [],
    resources,
    errorLog: undefined,
    errorSummary: apiRun.error_summary ?? undefined,
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

/** Map a raw API compliance report into the UI Scan type. */
function mapScan(apiReport: ApiComplianceReport): Scan {
  const failedCount = apiReport.failed_count;
  return {
    id: apiReport.id,
    nodeId: apiReport.node_id,
    nodeName: "",
    startedAt: apiReport.created_at,
    durationSec: 0,
    status: deriveCompliance(failedCount) === "non-compliant" ? "non-compliant" : "compliant",
    passed: apiReport.passed_count,
    failed: failedCount,
    warnings: apiReport.warning_count,
    profiles: [],
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
  if (params?.platform) qs.set("platform", params.platform);
  if (params?.status) qs.set("status", params.status);
  const raw = await apiFetchItems<ApiNodeDetail>(`/v1/nodes?${qs.toString()}`);
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
  if (params?.nodeId) qs.set("node_id", params.nodeId);
  const raw = await apiFetchItems<ApiRunSummary>(`/v1/runs?${qs.toString()}`);
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

export async function fetchComplianceReport(id: string): Promise<Scan> {
  const raw = await apiJson<ApiComplianceReport>(`/v1/compliance/reports/${encodeURIComponent(id)}`);
  return {
    ...mapScan(raw),
    id: raw.id,
  };
}

export async function fetchComplianceProfiles(): Promise<Profile[]> {
  return apiFetchData<Profile[]>("/v1/compliance/profiles");
}

export async function fetchControlRollups(): Promise<ControlRollup[]> {
  return apiFetchData<ControlRollup[]>("/v1/compliance/controls");
}

// --- Cookbooks ---
export async function fetchCookbooks(): Promise<Cookbook[]> {
  const raw = await apiFetchItems<ApiCookbookInventoryEntry>("/v1/cookbooks");
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

// --- Activity ---
export async function fetchActivity(params?: { limit?: number; types?: string }): Promise<ActivityEvent[]> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set("limit", String(params.limit));
  if (params?.types) qs.set("types", params.types);
  return apiFetchItems<ActivityEvent>(`/v1/activity?${qs.toString()}`);
}

// --- Settings (admin endpoints) ---
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

/* ── NEW endpoints ──────────────────────────────────────────────────────────── */

/** GET /v1/summary — fleet-wide counts and flipped nodes. */
export async function fetchSummary(): Promise<FleetSummary> {
  const raw = await apiJson<FleetSummary & { flipped: Array<{ id: string; name: string }> }>(
    "/v1/summary",
  );
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

/** GET /v1/compliance/trend?days=14 — daily compliance pass-rate trend. */
export async function fetchComplianceTrend(days: number = 14): Promise<ComplianceTrendResponse["data"]["items"]> {
  const res = await apiJson<ComplianceTrendResponse>(
    `/v1/compliance/trend?days=${days}`,
  );
  return res.data.items;
}

/** GET /v1/runs/trend?days=7 — daily converge success/failed counts. */
export async function fetchRunsTrend(days: number = 7): Promise<RunsTrendResponse["data"]["items"]> {
  const res = await apiJson<RunsTrendResponse>(
    `/v1/runs/trend?days=${days}`,
  );
  return res.data.items;
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

/* --- Activity --- */
export function useActivity(
  params?: { limit?: number; types?: string },
  options?: Omit<UseQueryOptions<ActivityEvent[]>, "queryKey" | "queryFn">,
) {
  return useQuery<ActivityEvent[]>({
    queryKey: ["activity", params],
    queryFn: () => fetchActivity(params),
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
  options?: Omit<UseQueryOptions<ComplianceTrendResponse["data"]["items"]>, "queryKey" | "queryFn">,
) {
  return useQuery({
    queryKey: ["compliance-trend", { days }],
    queryFn: () => fetchComplianceTrend(days),
    ...options,
  });
}

export function useRunsTrend(
  days: number = 7,
  options?: Omit<UseQueryOptions<RunsTrendResponse["data"]["items"]>, "queryKey" | "queryFn">,
) {
  return useQuery({
    queryKey: ["runs-trend", { days }],
    queryFn: () => fetchRunsTrend(days),
    ...options,
  });
}
