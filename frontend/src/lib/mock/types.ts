export type StatusKind = "ok" | "fail" | "warn" | "unknown";

export type NodeStatus = "success" | "failed" | "missing";
export type RunStatus = "success" | "failed" | "missing";
export type ControlStatus = "passed" | "failed" | "skipped" | "waived";
export type ComplianceStatus = "compliant" | "non-compliant" | "skipped" | "unknown";

export interface AttributeEntry {
  key: string;
  value: string;
  category: "default" | "normal" | "override" | "automatic";
  group: string;
}

/** Node entity — machine managed by Spindle.
 *  Fields are mapped from the API's snake_case response:
 *  chef_environment→environment, policy_group→policyGroup,
 *  policy_name→policyName, last_seen→lastSeen, run_list→runList,
 *  node_type→nodeType.
 *  Parse-out fields (waived, skipped, tags, ip, fqdn, kernel,
 *  cpuCores, memoryGb, cloud, region, uptimeDays) have been removed;
 *  warning_count maps to `warnings`.
 */
export interface FleetNode {
  id: string;
  name: string;
  platform: string;
  platformVersion: string;
  environment: string;
  policyGroup: string;
  policyName: string;
  nodeType: string;
  status: NodeStatus;
  compliance: ComplianceStatus;
  lastSeen: string;
  runList: string[];
  attributes: AttributeEntry[];
  /** Passed control count from latest compliance report (passed_count). */
  passed: number;
  /** Failed control count from latest compliance report (failed_count). */
  failed: number;
  /** Warning count from latest compliance report (warning_count). */
  warnings: number;
  complianceTrend: number[];
  flipped: boolean;
}

export interface ResourceEvent {
  id: string;
  type: string;
  name: string;
  action: string;
  status: "updated" | "up-to-date" | "skipped" | "failed";
  durationSec: number;
  cookbook: string;
  delta?: string | undefined;
}

export interface Run {
  id: string;
  nodeId: string;
  nodeName: string;
  environment: string;
  status: RunStatus;
  startedAt: string;
  /** Duration in seconds (duration_ms / 1000). */
  durationSec: number;
  totalResources: number;
  updatedResources: number;
  failedResources: number;
  cookbook: string;
  runList: string[];
  resources: ResourceEvent[];
  errorLog?: string | undefined;
  errorSummary?: string | undefined;
}

export interface ControlResult {
  codeDesc: string;
  status: "passed" | "failed" | "skipped";
  message?: string | undefined;
  runTimeMs: number;
}

export interface Control {
  id: string;
  title: string;
  impact: number;
  severity: "critical" | "high" | "medium" | "low";
  status: ControlStatus;
  profileId: string;
  profileName: string;
  desc: string;
  tags: string[];
  results: ControlResult[];
}

export interface NodeProfileResult {
  profileId: string;
  profileName: string;
  profileTitle: string;
  version: string;
  status: ComplianceStatus;
  controls: Control[];
}

/** Compliance report (scan) summary mapped from the API's snake_case fields.
 *  passed_count→passed, failed_count→failed, warning_count→warnings.
 */
export interface Scan {
  id: string;
  nodeId: string;
  nodeName: string;
  startedAt: string;
  durationSec: number;
  status: ComplianceStatus;
  /** Passed control count (passed_count). */
  passed: number;
  /** Failed control count (failed_count). */
  failed: number;
  /** Warning count (warning_count). */
  warnings: number;
  profiles: NodeProfileResult[];
}

export interface Profile {
  id: string;
  name: string;
  title: string;
  version: string;
  vendor: string;
  installed: boolean;
  summary: string;
  platforms: string[];
  controlCount: number;
  testCount: number;
  nodes: number;
  passRate: number;
  updatedAt: string;
}

export interface CookbookFile {
  path: string;
  content: string;
}

export interface CookbookVersion {
  version: string;
  nodes: number;
  updatedAt: string;
  files: CookbookFile[];
}

export interface Cookbook {
  name: string;
  maintainer: string;
  description: string;
  nodes: number;
  lastSeen: string;
  versions: CookbookVersion[];
}

export type ActivityType = "converge" | "scan" | "node";

export interface ActivityEvent {
  id: string;
  type: ActivityType;
  status: StatusKind;
  nodeId: string;
  nodeName: string;
  title: string;
  detail: string;
  at: string;
  href: string;
}

export interface User {
  id: string;
  name: string;
  email: string;
  role: "Owner" | "Admin" | "Operator" | "Viewer";
  teams: string[];
  lastActive: string;
  status: "active" | "invited" | "suspended";
}

export interface Team {
  id: string;
  name: string;
  description: string;
  members: number;
  environments: string[];
}

export interface ApiToken {
  id: string;
  name: string;
  prefix: string;
  scope: "read" | "write" | "admin";
  createdAt: string;
  lastUsed: string | null;
  expiresAt: string | null;
  status: "active" | "revoked";
}

export interface NotificationRule {
  id: string;
  name: string;
  channel: "Slack" | "PagerDuty" | "Email" | "Webhook";
  target: string;
  trigger: "converge-failure" | "compliance-failure" | "node-missing" | "profile-drift";
  enabled: boolean;
  lastFired: string | null;
}

export interface RetentionPolicy {
  id: string;
  dataset: string;
  description: string;
  retainDays: number;
  archive: boolean;
  estimatedSize: string;
  enabled: boolean;
}

export interface ControlRollup {
  id: string;
  title: string;
  profileId: string;
  profileTitle: string;
  severity: Control["severity"];
  impact: number;
  failing: number;
  passing: number;
  warnings: number;
  nodes: string[];
}

/** A flipped node reference: just enough to render the alert pill. */
export interface FlippedNode {
  id: string;
  name: string;
}

/** Fleet summary from GET /v1/summary. */
export interface FleetSummary {
  total: number;
  online: number;
  offline: number;
  convergeSuccess: number;
  convergeFailed: number;
  compliant: number;
  nonCompliant: number;
  unknownCompliance: number;
  flipped: FlippedNode[];
}

/** One day in a compliance pass-rate trend. */
export interface ComplianceTrendItem {
  date: string;
  passRate: number;
  passed: number;
  failed: number;
}

/** Response shape for GET /v1/compliance/trend. */
export interface ComplianceTrendResponse {
  data: {
    items: ComplianceTrendItem[];
  };
}

/** One day in a converge outcomes trend. */
export interface RunsTrendItem {
  date: string;
  success: number;
  failed: number;
}

/** Response shape for GET /v1/runs/trend. */
export interface RunsTrendResponse {
  data: {
    items: RunsTrendItem[];
  };
}
