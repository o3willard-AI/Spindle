import type {
  ActivityEvent,
  Cookbook,
  ControlRollup,
  FleetNode,
  Profile,
  Run,
  Scan,
  Team,
  ApiToken,
  NotificationRule,
  RetentionPolicy,
  User,
} from "@/lib/mock/types";

const BASE_URL = import.meta.env.VITE_API_URL || "";

function getToken(): string | null {
  return localStorage.getItem("spindle_token");
}

export function setToken(token: string) {
  localStorage.setItem("spindle_token", token);
}

export function clearToken() {
  localStorage.removeItem("spindle_token");
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

/** Fetch wrapper that returns the decoded JSON envelope. */
async function apiFetch<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getToken();
  const headers = new Headers(init?.headers);
  if (token) {
    headers.set("Authorization", `Bearer ${token}`);
  }
  headers.set("Accept", "application/json");

  const url = path.startsWith("http") ? path : `${BASE_URL}${path}`;
  const res = await fetch(url, { ...init, headers });

  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    const err = body as ApiError;
    throw new Error(err.error?.message || `HTTP ${res.status}`);
  }

  return (body as ApiResponse<T>).data as T;
}

// --- Nodes ---
export async function fetchNodes(params?: { limit?: number; platform?: string; status?: string }): Promise<FleetNode[]> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set("limit", String(params.limit));
  if (params?.platform) qs.set("platform", params.platform);
  if (params?.status) qs.set("status", params.status);
  return apiFetch<FleetNode[]>(`/v1/nodes?${qs.toString()}`);
}

export async function fetchNode(id: string): Promise<FleetNode> {
  return apiFetch<FleetNode>(`/v1/nodes/${encodeURIComponent(id)}`);
}

// --- Runs ---
export async function fetchRuns(params?: { limit?: number; nodeId?: string }): Promise<Run[]> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set("limit", String(params.limit));
  if (params?.nodeId) qs.set("node_id", params.nodeId);
  return apiFetch<Run[]>(`/v1/runs?${qs.toString()}`);
}

export async function fetchRun(id: string): Promise<Run> {
  return apiFetch<Run>(`/v1/runs/${encodeURIComponent(id)}`);
}

// --- Compliance ---
export async function fetchComplianceReports(params?: { limit?: number; node?: string; profile?: string }): Promise<Scan[]> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set("limit", String(params.limit));
  if (params?.node) qs.set("node", params.node);
  if (params?.profile) qs.set("profile", params.profile);
  return apiFetch<Scan[]>(`/v1/compliance/reports?${qs.toString()}`);
}

export async function fetchComplianceProfiles(): Promise<Profile[]> {
  return apiFetch<Profile[]>("/v1/compliance/profiles");
}

export async function fetchControlRollups(): Promise<ControlRollup[]> {
  return apiFetch<ControlRollup[]>("/v1/compliance/controls");
}

// --- Cookbooks ---
export async function fetchCookbooks(): Promise<Cookbook[]> {
  return apiFetch<Cookbook[]>("/v1/cookbooks");
}

export async function fetchCookbook(name: string): Promise<Cookbook> {
  return apiFetch<Cookbook>(`/v1/cookbooks/${encodeURIComponent(name)}`);
}

// --- Activity ---
export async function fetchActivity(params?: { limit?: number; types?: string }): Promise<ActivityEvent[]> {
  const qs = new URLSearchParams();
  if (params?.limit) qs.set("limit", String(params.limit));
  if (params?.types) qs.set("types", params.types);
  return apiFetch<ActivityEvent[]>(`/v1/activity?${qs.toString()}`);
}

// --- Settings (admin endpoints) ---
export async function fetchUsers(): Promise<User[]> {
  return apiFetch<User[]>("/v1/admin/users");
}

export async function fetchTeams(): Promise<Team[]> {
  return apiFetch<Team[]>("/v1/admin/teams");
}

export async function fetchApiTokens(): Promise<ApiToken[]> {
  return apiFetch<ApiToken[]>("/v1/admin/tokens");
}

export async function fetchNotificationRules(): Promise<NotificationRule[]> {
  return apiFetch<NotificationRule[]>("/v1/admin/notifications");
}

export async function fetchRetentionPolicies(): Promise<RetentionPolicy[]> {
  return apiFetch<RetentionPolicy[]>("/v1/admin/retention");
}
