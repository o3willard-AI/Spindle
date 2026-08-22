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

export interface FleetNode {
  id: string;
  name: string;
  fqdn: string;
  ip: string;
  platform: string;
  platformVersion: string;
  platformFamily: string;
  kernel: string;
  environment: string;
  policyGroup: string;
  policyName: string;
  status: NodeStatus;
  compliance: ComplianceStatus;
  lastSeen: string;
  uptimeDays: number;
  cpuCores: number;
  memoryGb: number;
  cloud: string;
  region: string;
  runList: string[];
  tags: string[];
  attributes: AttributeEntry[];
  controlsPassed: number;
  controlsFailed: number;
  controlsSkipped: number;
  controlsWaived: number;
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

export interface Scan {
  id: string;
  nodeId: string;
  nodeName: string;
  startedAt: string;
  durationSec: number;
  status: ComplianceStatus;
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
  skipped: number;
  nodes: string[];
}

export interface FleetSummary {
  total: number;
  online: number;
  offline: number;
  convergeSuccess: number;
  convergeFailed: number;
  compliant: number;
  nonCompliant: number;
  unknownCompliance: number;
  flipped: FleetNode[];
}
