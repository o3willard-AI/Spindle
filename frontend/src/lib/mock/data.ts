import type {
  ActivityEvent,
  ApiToken,
  AttributeEntry,
  Control,
  Cookbook,
  FleetNode,
  NotificationRule,
  Profile,
  ResourceEvent,
  RetentionPolicy,
  Run,
  Scan,
  Team,
  User,
} from "./types";

/* ------------------------------------------------------------------ *
 * Deterministic pseudo-random generator so SSR and client agree.
 * ------------------------------------------------------------------ */
function makeRng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s * 1664525 + 1013904223) >>> 0;
    return s / 4294967296;
  };
}
const rng = makeRng(20260822);
const pick = <T,>(arr: readonly T[], r = rng()) => arr[Math.floor(r * arr.length) % arr.length];
const int = (min: number, max: number) => min + Math.floor(rng() * (max - min + 1));

/** Fixed "now" so the demo fleet is stable between renders. */
export const NOW = new Date("2026-08-22T18:40:00.000Z");
const minutesAgo = (m: number) => new Date(NOW.getTime() - m * 60_000).toISOString();
const hoursAgo = (h: number) => minutesAgo(h * 60);
const daysAgo = (d: number) => minutesAgo(d * 1440);

/* ------------------------------------------------------------------ *
 * Nodes
 * ------------------------------------------------------------------ */

const ENVIRONMENTS = ["production", "staging", "development"] as const;

function attrs(node: {
  fqdn: string;
  ip: string;
  platform: string;
  platformVersion: string;
  platformFamily: string;
  kernel: string;
  environment: string;
  cpuCores: number;
  memoryGb: number;
  cloud: string;
  region: string;
}): AttributeEntry[] {
  return [
    { key: "fqdn", value: node.fqdn, category: "automatic", group: "system" },
    { key: "hostname", value: node.fqdn.split(".")[0]!, category: "automatic", group: "system" },
    { key: "ipaddress", value: node.ip, category: "automatic", group: "network" },
    { key: "macaddress", value: "0a:1f:" + node.ip.split(".").slice(1).join(":") + ":c4", category: "automatic", group: "network" },
    { key: "platform", value: node.platform, category: "automatic", group: "system" },
    { key: "platform_version", value: node.platformVersion, category: "automatic", group: "system" },
    { key: "platform_family", value: node.platformFamily, category: "automatic", group: "system" },
    { key: "kernel.release", value: node.kernel, category: "automatic", group: "kernel" },
    { key: "kernel.machine", value: "x86_64", category: "automatic", group: "kernel" },
    { key: "cpu.total", value: String(node.cpuCores), category: "automatic", group: "hardware" },
    { key: "memory.total", value: `${node.memoryGb * 1024 * 1024} kB`, category: "automatic", group: "hardware" },
    { key: "cloud.provider", value: node.cloud, category: "automatic", group: "cloud" },
    { key: "cloud.region", value: node.region, category: "automatic", group: "cloud" },
    { key: "chef_environment", value: node.environment, category: "normal", group: "spindle" },
    { key: "spindle.owner_team", value: "platform-infra", category: "normal", group: "spindle" },
    { key: "spindle.patch_window", value: "sun 02:00-04:00 UTC", category: "normal", group: "spindle" },
    { key: "nginx.worker_processes", value: "auto", category: "default", group: "nginx" },
    { key: "nginx.worker_connections", value: "2048", category: "default", group: "nginx" },
    { key: "nginx.keepalive_timeout", value: "65", category: "override", group: "nginx" },
    { key: "openssh.server.permit_root_login", value: "no", category: "override", group: "openssh" },
    { key: "openssh.server.password_authentication", value: "no", category: "override", group: "openssh" },
    { key: "auditd.max_log_file", value: "64", category: "default", group: "auditd" },
    { key: "auditd.space_left_action", value: "email", category: "override", group: "auditd" },
    { key: "postgresql.version", value: "16.3", category: "normal", group: "postgresql" },
    { key: "postgresql.max_connections", value: "300", category: "override", group: "postgresql" },
    { key: "monitoring.agent_version", value: "4.12.0", category: "normal", group: "monitoring" },
    { key: "monitoring.scrape_interval", value: "15s", category: "default", group: "monitoring" },
  ];
}

interface NodeSeed {
  name: string;
  platform: string;
  platformVersion: string;
  platformFamily: string;
  kernel: string;
  environment: string;
  policyGroup: string;
  policyName: string;
  status: FleetNode["status"];
  compliance: FleetNode["compliance"];
  lastSeenMin: number;
  cloud: string;
  region: string;
  cpuCores: number;
  memoryGb: number;
  runList: string[];
  tags: string[];
  flipped?: boolean;
  failed?: number;
}

const NODE_SEEDS: NodeSeed[] = [
  {
    name: "web-edge-01.prod.iad", platform: "ubuntu", platformVersion: "24.04", platformFamily: "debian",
    kernel: "6.8.0-45-generic", environment: "production", policyGroup: "prod-edge", policyName: "edge-web",
    status: "success", compliance: "compliant", lastSeenMin: 4, cloud: "aws", region: "us-east-1",
    cpuCores: 8, memoryGb: 16, runList: ["policy[edge-web]", "recipe[nginx::default]", "recipe[hardening::cis]"],
    tags: ["edge", "public", "tier-1"], failed: 0,
  },
  {
    name: "web-edge-02.prod.iad", platform: "ubuntu", platformVersion: "24.04", platformFamily: "debian",
    kernel: "6.8.0-45-generic", environment: "production", policyGroup: "prod-edge", policyName: "edge-web",
    status: "failed", compliance: "non-compliant", lastSeenMin: 12, cloud: "aws", region: "us-east-1",
    cpuCores: 8, memoryGb: 16, runList: ["policy[edge-web]", "recipe[nginx::default]", "recipe[hardening::cis]"],
    tags: ["edge", "public", "tier-1"], flipped: true, failed: 7,
  },
  {
    name: "api-core-01.prod.iad", platform: "rhel", platformVersion: "9.4", platformFamily: "rhel",
    kernel: "5.14.0-427.el9", environment: "production", policyGroup: "prod-api", policyName: "core-api",
    status: "success", compliance: "compliant", lastSeenMin: 7, cloud: "aws", region: "us-east-1",
    cpuCores: 16, memoryGb: 64, runList: ["policy[core-api]", "recipe[java::openjdk]", "recipe[hardening::cis]"],
    tags: ["api", "tier-1"], failed: 0,
  },
  {
    name: "api-core-02.prod.dub", platform: "rhel", platformVersion: "9.4", platformFamily: "rhel",
    kernel: "5.14.0-427.el9", environment: "production", policyGroup: "prod-api", policyName: "core-api",
    status: "success", compliance: "non-compliant", lastSeenMin: 9, cloud: "aws", region: "eu-west-1",
    cpuCores: 16, memoryGb: 64, runList: ["policy[core-api]", "recipe[java::openjdk]", "recipe[hardening::cis]"],
    tags: ["api", "tier-1", "eu"], flipped: true, failed: 4,
  },
  {
    name: "db-primary-01.prod.iad", platform: "debian", platformVersion: "12", platformFamily: "debian",
    kernel: "6.1.0-23-amd64", environment: "production", policyGroup: "prod-data", policyName: "postgres-primary",
    status: "success", compliance: "compliant", lastSeenMin: 3, cloud: "aws", region: "us-east-1",
    cpuCores: 32, memoryGb: 128, runList: ["policy[postgres-primary]", "recipe[postgresql::server]", "recipe[backup::pgbackrest]"],
    tags: ["data", "stateful", "tier-0"], failed: 1,
  },
  {
    name: "db-replica-02.prod.dub", platform: "debian", platformVersion: "12", platformFamily: "debian",
    kernel: "6.1.0-23-amd64", environment: "production", policyGroup: "prod-data", policyName: "postgres-replica",
    status: "missing", compliance: "unknown", lastSeenMin: 2180, cloud: "aws", region: "eu-west-1",
    cpuCores: 32, memoryGb: 128, runList: ["policy[postgres-replica]", "recipe[postgresql::replica]"],
    tags: ["data", "stateful", "eu"], failed: 0,
  },
  {
    name: "cache-redis-01.prod.iad", platform: "amazon", platformVersion: "2023", platformFamily: "amazon",
    kernel: "6.1.94-99.176.amzn2023", environment: "production", policyGroup: "prod-cache", policyName: "redis-cluster",
    status: "success", compliance: "compliant", lastSeenMin: 6, cloud: "aws", region: "us-east-1",
    cpuCores: 8, memoryGb: 32, runList: ["policy[redis-cluster]", "recipe[redis::server]"],
    tags: ["cache", "tier-1"], failed: 0,
  },
  {
    name: "build-runner-04.stg.iad", platform: "ubuntu", platformVersion: "22.04", platformFamily: "debian",
    kernel: "5.15.0-119-generic", environment: "staging", policyGroup: "stg-ci", policyName: "ci-runner",
    status: "failed", compliance: "non-compliant", lastSeenMin: 21, cloud: "gcp", region: "us-central1",
    cpuCores: 16, memoryGb: 32, runList: ["policy[ci-runner]", "recipe[docker::default]", "recipe[buildkit::default]"],
    tags: ["ci", "ephemeral"], failed: 9,
  },
  {
    name: "log-collector-01.stg.iad", platform: "sles", platformVersion: "15.5", platformFamily: "suse",
    kernel: "5.14.21-150500", environment: "staging", policyGroup: "stg-observability", policyName: "log-collector",
    status: "success", compliance: "compliant", lastSeenMin: 11, cloud: "gcp", region: "us-central1",
    cpuCores: 4, memoryGb: 16, runList: ["policy[log-collector]", "recipe[vector::agent]"],
    tags: ["observability"], failed: 0,
  },
  {
    name: "sandbox-dev-07.dev.iad", platform: "ubuntu", platformVersion: "24.04", platformFamily: "debian",
    kernel: "6.8.0-45-generic", environment: "development", policyGroup: "dev-sandbox", policyName: "sandbox",
    status: "success", compliance: "skipped", lastSeenMin: 47, cloud: "aws", region: "us-east-1",
    cpuCores: 4, memoryGb: 8, runList: ["policy[sandbox]", "recipe[devtools::default]"],
    tags: ["sandbox"], failed: 0,
  },
];

/* --------------------------- Compliance ---------------------------- */

interface ControlSeed {
  id: string;
  title: string;
  impact: number;
  desc: string;
  tags: string[];
}

const CIS_CONTROLS: ControlSeed[] = [
  { id: "cis-1.1.1", title: "Ensure mounting of cramfs filesystems is disabled", impact: 0.5, desc: "The cramfs filesystem type is a compressed read-only Linux filesystem. Removing support reduces the attack surface.", tags: ["level-1", "filesystem"] },
  { id: "cis-1.4.1", title: "Ensure bootloader password is set", impact: 0.7, desc: "Setting the boot loader password prevents unauthenticated users from changing boot parameters.", tags: ["level-1", "boot"] },
  { id: "cis-3.2.1", title: "Ensure IP forwarding is disabled", impact: 0.6, desc: "Hosts that are not routers must not forward IPv4/IPv6 packets.", tags: ["level-1", "network"] },
  { id: "cis-4.1.3", title: "Ensure auditing for processes that start prior to auditd is enabled", impact: 0.5, desc: "Configure grub so processes started before auditd are audited.", tags: ["level-2", "audit"] },
  { id: "cis-5.2.4", title: "Ensure SSH X11 forwarding is disabled", impact: 0.6, desc: "X11 forwarding exposes the display server to the remote host.", tags: ["level-1", "ssh"] },
  { id: "cis-5.2.10", title: "Ensure SSH root login is disabled", impact: 0.9, desc: "Direct root login over SSH removes accountability and enables brute-force attacks.", tags: ["level-1", "ssh"] },
  { id: "cis-5.3.1", title: "Ensure password creation requirements are configured", impact: 0.7, desc: "pam_pwquality must enforce minimum length and complexity.", tags: ["level-1", "pam"] },
  { id: "cis-5.4.2", title: "Ensure system accounts are secured", impact: 0.6, desc: "System accounts must not be usable for interactive login.", tags: ["level-1", "accounts"] },
  { id: "cis-6.1.2", title: "Ensure permissions on /etc/passwd are configured", impact: 0.4, desc: "/etc/passwd must be 0644 and owned by root:root.", tags: ["level-1", "permissions"] },
  { id: "cis-6.2.7", title: "Ensure no users have .netrc files", impact: 0.5, desc: ".netrc files may store unencrypted credentials.", tags: ["level-1", "accounts"] },
];

const APP_CONTROLS: ControlSeed[] = [
  { id: "nginx-01", title: "Ensure NGINX runs as an unprivileged user", impact: 0.8, desc: "The worker processes must not run as root.", tags: ["nginx", "app"] },
  { id: "nginx-02", title: "Ensure TLS 1.0/1.1 are disabled", impact: 0.9, desc: "Only TLS 1.2 and TLS 1.3 may be negotiated.", tags: ["nginx", "tls"] },
  { id: "nginx-03", title: "Ensure server tokens are off", impact: 0.3, desc: "Version disclosure aids fingerprinting.", tags: ["nginx", "app"] },
  { id: "docker-01", title: "Ensure the Docker daemon socket is not exposed over TCP", impact: 1.0, desc: "Remote unauthenticated daemon access is equivalent to root on the host.", tags: ["docker", "app"] },
  { id: "docker-02", title: "Ensure containers do not run with the privileged flag", impact: 0.9, desc: "Privileged containers bypass namespace isolation.", tags: ["docker", "app"] },
  { id: "pg-01", title: "Ensure PostgreSQL logs connections", impact: 0.4, desc: "log_connections must be on for audit traceability.", tags: ["postgresql", "app"] },
  { id: "pg-02", title: "Ensure SSL is enforced for client connections", impact: 0.8, desc: "hostssl entries must cover all non-local connections.", tags: ["postgresql", "tls"] },
];

function severityOf(impact: number): Control["severity"] {
  if (impact >= 0.9) return "critical";
  if (impact >= 0.7) return "high";
  if (impact >= 0.4) return "medium";
  return "low";
}

const FAIL_MESSAGES: Record<string, string> = {
  "cis-5.2.10": "expected sshd_config PermitRootLogin to eq \"no\"\n     got: \"yes\"",
  "cis-5.2.4": "expected sshd_config X11Forwarding to eq \"no\"\n     got: \"yes\"",
  "cis-3.2.1": "expected net.ipv4.ip_forward to eq 0\n     got: 1",
  "cis-1.4.1": "File /boot/grub/grub.cfg is expected to contain \"password_pbkdf2\"",
  "cis-6.1.2": "File /etc/passwd mode is expected to cmp == \"0644\"\n     got: \"0664\"",
  "nginx-02": "expected ssl_protocols to eq \"TLSv1.2 TLSv1.3\"\n     got: \"TLSv1 TLSv1.1 TLSv1.2\"",
  "docker-01": "Port 2375 is expected not to be listening",
  "docker-02": "Container ci-builder-3 is expected not to be privileged",
  "pg-02": "expected pg_hba.conf to contain \"hostssl all all 0.0.0.0/0 scram-sha-256\"",
};

function buildControls(
  seeds: ControlSeed[],
  profileId: string,
  profileName: string,
  failCount: number,
  offset: number,
): Control[] {
  return seeds.map((seed, i) => {
    const isFail = (i + offset) % seeds.length < failCount;
    const isSkip = !isFail && (i + offset) % 7 === 3;
    const status: Control["status"] = isFail ? "failed" : isSkip ? "skipped" : "passed";
    const results = Array.from({ length: isFail ? 2 : 3 }).map((_, k) => ({
      codeDesc:
        status === "failed" && k === 0
          ? `${seed.title} is expected to be compliant`
          : `${seed.id} check ${k + 1} is expected to be enforced`,
      status: (status === "failed" && k === 0 ? "failed" : status === "skipped" ? "skipped" : "passed") as ControlResult["status"],
      message: status === "failed" && k === 0 ? (FAIL_MESSAGES[seed.id] ?? "expected value to be compliant\n     got: drifted") : undefined,
      runTimeMs: 3 + ((i * 7 + k * 13) % 40),
    }));
    return {
      id: seed.id,
      title: seed.title,
      impact: seed.impact,
      severity: severityOf(seed.impact),
      status,
      profileId,
      profileName,
      desc: seed.desc,
      tags: seed.tags,
      results,
    };
  });
}
type ControlResult = Control["results"][number];

export const profiles: Profile[] = [
  {
    id: "cis-linux-benchmark", name: "cis-linux-benchmark", title: "CIS Distribution Independent Linux Benchmark",
    version: "2.4.1", vendor: "Center for Internet Security", installed: true,
    summary: "Baseline hardening controls for filesystem, boot, network, SSH, PAM and account policy across all Linux distributions.",
    platforms: ["ubuntu", "debian", "rhel", "amazon", "sles"], controlCount: CIS_CONTROLS.length, testCount: 128,
    nodes: 9, passRate: 0.87, updatedAt: daysAgo(14),
  },
  {
    id: "nginx-baseline", name: "nginx-baseline", title: "NGINX Server Baseline",
    version: "1.9.0", vendor: "Spindle Security", installed: true,
    summary: "TLS configuration, privilege separation and information-disclosure checks for NGINX edge servers.",
    platforms: ["ubuntu", "debian"], controlCount: 3, testCount: 24, nodes: 2, passRate: 0.72, updatedAt: daysAgo(6),
  },
  {
    id: "docker-baseline", name: "docker-baseline", title: "Docker Daemon & Container Baseline",
    version: "2.2.0", vendor: "Spindle Security", installed: true,
    summary: "Daemon exposure, container privilege and image provenance checks for CI and container hosts.",
    platforms: ["ubuntu", "amazon"], controlCount: 2, testCount: 31, nodes: 1, passRate: 0.41, updatedAt: daysAgo(3),
  },
  {
    id: "postgres-baseline", name: "postgres-baseline", title: "PostgreSQL 16 Security Baseline",
    version: "1.3.2", vendor: "Spindle Security", installed: true,
    summary: "Connection auditing, TLS enforcement and role privilege checks for PostgreSQL clusters.",
    platforms: ["debian", "rhel"], controlCount: 2, testCount: 19, nodes: 2, passRate: 0.79, updatedAt: daysAgo(21),
  },
  {
    id: "pci-dss-4", name: "pci-dss-4", title: "PCI DSS 4.0 Technical Requirements",
    version: "4.0.1", vendor: "Center for Internet Security", installed: false,
    summary: "Cardholder-data environment controls mapped to PCI DSS 4.0 requirement families 1 through 10.",
    platforms: ["ubuntu", "rhel"], controlCount: 64, testCount: 210, nodes: 0, passRate: 0, updatedAt: daysAgo(40),
  },
  {
    id: "stig-rhel9", name: "stig-rhel9", title: "DISA STIG for RHEL 9",
    version: "1.2.0", vendor: "DISA", installed: false,
    summary: "Defense Information Systems Agency Security Technical Implementation Guide for Red Hat Enterprise Linux 9.",
    platforms: ["rhel"], controlCount: 312, testCount: 480, nodes: 0, passRate: 0, updatedAt: daysAgo(9),
  },
  {
    id: "ssh-hardening", name: "ssh-hardening", title: "OpenSSH Hardening Profile",
    version: "3.1.0", vendor: "Spindle Security", installed: false,
    summary: "Cipher suites, key exchange algorithms and login policy for OpenSSH servers.",
    platforms: ["ubuntu", "debian", "rhel", "sles"], controlCount: 28, testCount: 61, nodes: 0, passRate: 0, updatedAt: daysAgo(30),
  },
];

const PROFILE_CONTROL_SEEDS: Record<string, ControlSeed[]> = {
  "cis-linux-benchmark": CIS_CONTROLS,
  "nginx-baseline": APP_CONTROLS.filter((c) => c.id.startsWith("nginx")),
  "docker-baseline": APP_CONTROLS.filter((c) => c.id.startsWith("docker")),
  "postgres-baseline": APP_CONTROLS.filter((c) => c.id.startsWith("pg")),
};

function profilesForNode(seed: NodeSeed): string[] {
  const list = ["cis-linux-benchmark"];
  if (seed.name.startsWith("web-edge")) list.push("nginx-baseline");
  if (seed.name.startsWith("build-runner")) list.push("docker-baseline");
  if (seed.name.startsWith("db-")) list.push("postgres-baseline");
  return list;
}

/* ------------------------------ Build ------------------------------ */

function trend(base: number, drop: boolean, i: number): number[] {
  return Array.from({ length: 30 }, (_, d) => {
    const wobble = ((i * 13 + d * 7) % 9) - 4;
    if (drop && d > 24) return Math.max(38, base - 44 + wobble);
    return Math.min(100, Math.max(0, base + wobble));
  });
}

export const nodes: FleetNode[] = NODE_SEEDS.map((seed, i) => {
  const ip = `10.${seed.environment === "production" ? 20 : seed.environment === "staging" ? 30 : 40}.${i + 1}.${11 + i}`;
  const base = {
    fqdn: seed.name + ".spindle.internal",
    ip,
    platform: seed.platform,
    platformVersion: seed.platformVersion,
    platformFamily: seed.platformFamily,
    kernel: seed.kernel,
    environment: seed.environment,
    cpuCores: seed.cpuCores,
    memoryGb: seed.memoryGb,
    cloud: seed.cloud,
    region: seed.region,
  };
  const nodeProfiles = profilesForNode(seed);
  let passed = 0;
  let failed = 0;
  let skipped = 0;
  nodeProfiles.forEach((pid, pi) => {
    const ctrls = buildControls(PROFILE_CONTROL_SEEDS[pid]!, pid, pid, pi === 0 ? Math.min(seed.failed ?? 0, 4) : Math.max(0, (seed.failed ?? 0) - 4), i + pi);
    ctrls.forEach((c) => {
      if (c.status === "failed") failed += 1;
      else if (c.status === "skipped") skipped += 1;
      else passed += 1;
    });
  });
  return {
    id: `node-${String(i + 1).padStart(2, "0")}`,
    name: seed.name,
    ...base,
    policyGroup: seed.policyGroup,
    policyName: seed.policyName,
    status: seed.status,
    compliance: seed.compliance,
    lastSeen: minutesAgo(seed.lastSeenMin),
    uptimeDays: 3 + ((i * 17) % 210),
    runList: seed.runList,
    tags: seed.tags,
    attributes: attrs(base),
    controlsPassed: passed,
    controlsFailed: failed,
    controlsSkipped: skipped,
    controlsWaived: i === 2 ? 1 : 0,
    complianceTrend: trend(seed.compliance === "compliant" ? 96 : 78, !!seed.flipped, i),
    flipped: !!seed.flipped,
  };
});

export const nodeById = (id: string) => nodes.find((n) => n.id === id);

/* ------------------------------- Runs ------------------------------- */

const RESOURCE_TEMPLATES: Array<[string, string, string, string]> = [
  ["package", "nginx", "install", "nginx"],
  ["template", "/etc/nginx/nginx.conf", "create", "nginx"],
  ["service", "nginx", "enable", "nginx"],
  ["service", "nginx", "restart", "nginx"],
  ["directory", "/var/log/spindle", "create", "base"],
  ["user", "svc-deploy", "create", "base"],
  ["group", "operators", "manage", "base"],
  ["file", "/etc/motd", "create", "base"],
  ["cookbook_file", "/usr/local/bin/health-probe", "create", "base"],
  ["execute", "sysctl -p", "run", "hardening"],
  ["template", "/etc/ssh/sshd_config", "create", "hardening"],
  ["service", "sshd", "reload", "hardening"],
  ["package", "auditd", "install", "hardening"],
  ["template", "/etc/audit/auditd.conf", "create", "hardening"],
  ["remote_file", "/opt/agent/agent.tar.gz", "create", "monitoring"],
  ["execute", "systemctl daemon-reload", "run", "monitoring"],
  ["package", "postgresql-16", "install", "postgresql"],
  ["template", "/etc/postgresql/16/main/pg_hba.conf", "create", "postgresql"],
  ["service", "postgresql", "start", "postgresql"],
  ["package", "docker-ce", "install", "docker"],
  ["template", "/etc/docker/daemon.json", "create", "docker"],
  ["service", "docker", "restart", "docker"],
];

const ERROR_LOG = `================================================================================
Error executing action \`restart\` on resource 'service[nginx]'
================================================================================

Mixlib::ShellOut::ShellCommandFailed
------------------------------------
Expected process to exit with [0], but received '1'
---- Begin output of /usr/sbin/service nginx restart ----
STDOUT:
STDERR: Job for nginx.service failed because the control process exited with error code.
See "systemctl status nginx.service" and "journalctl -xeu nginx.service" for details.
---- End output of /usr/sbin/service nginx restart ----
Ran /usr/sbin/service nginx restart returned 1

Resource Declaration:
---------------------
# In /var/cinc/cache/cookbooks/nginx/recipes/default.rb

 84: service 'nginx' do
 85:   supports status: true, restart: true, reload: true
 86:   action [:enable, :start]
 87:   subscribes :restart, 'template[/etc/nginx/nginx.conf]', :delayed
 88: end

Compiled Resource:
------------------
# Declared in /var/cinc/cache/cookbooks/nginx/recipes/default.rb:84:in \`from_file'

service("nginx") do
  action [:enable, :start]
  service_name "nginx"
  enabled true
  running false
  declared_type :service
end

System Info:
------------
cinc_version=18.4.12
platform=ubuntu
platform_version=24.04
ruby=ruby 3.1.4p223
program_name=/usr/bin/cinc-client
executable=/usr/bin/cinc-client

Running handlers:
[2026-08-22T18:28:11+00:00] ERROR: Running exception handlers
Running handlers complete
[2026-08-22T18:28:11+00:00] ERROR: Exception handlers complete
Cinc Client failed. 41 resources updated in 01 minutes 12 seconds`;

function buildResources(count: number, fail: boolean, offset: number): ResourceEvent[] {
  const out: ResourceEvent[] = [];
  for (let i = 0; i < count; i += 1) {
    const t = RESOURCE_TEMPLATES[(i + offset) % RESOURCE_TEMPLATES.length]!;
    const mod = (i + offset) % 6;
    const status: ResourceEvent["status"] = mod === 0 ? "updated" : mod === 4 ? "skipped" : "up-to-date";
    out.push({
      id: `res-${offset}-${i}`,
      type: t[0],
      name: t[1],
      action: t[2],
      cookbook: t[3],
      status,
      durationMs: 4 + ((i * 37 + offset * 11) % 900),
      delta: status === "updated" && t[0] === "template" ? "3 lines changed" : undefined,
    });
  }
  if (fail) {
    out[out.length - 1] = {
      id: `res-${offset}-fail`,
      type: "service",
      name: "nginx",
      action: "restart",
      cookbook: "nginx",
      status: "failed",
      durationMs: 12048,
      delta: "exit status 1",
    };
  }
  return out;
}

export const runs: Run[] = (() => {
  const out: Run[] = [];
  let counter = 0;
  for (let cycle = 0; cycle < 8; cycle += 1) {
    nodes.forEach((n, ni) => {
      counter += 1;
      const isLatest = cycle === 0;
      let status: Run["status"] = "success";
      if (n.status === "missing") status = cycle < 2 ? "missing" : "success";
      else if (n.status === "failed" && (isLatest || (cycle === 3 && ni % 2 === 0))) status = "failed";
      else if (!isLatest && (ni + cycle) % 11 === 0) status = "failed";
      const resourceCount = 28 + ((ni * 5 + cycle * 3) % 40);
      const resources = buildResources(status === "missing" ? 0 : resourceCount, status === "failed", counter);
      out.push({
        id: `run-${String(counter).padStart(4, "0")}`,
        nodeId: n.id,
        nodeName: n.name,
        environment: n.environment,
        status,
        startedAt: minutesAgo(cycle * 240 + ni * 7 + 5),
        durationSec: status === "missing" ? 0 : 24 + ((ni * 13 + cycle * 17) % 210),
        totalResources: resources.length,
        updatedResources: resources.filter((r) => r.status === "updated").length,
        failedResources: resources.filter((r) => r.status === "failed").length,
        cookbook: n.policyName,
        runList: n.runList,
        resources,
        errorLog: status === "failed" ? ERROR_LOG : undefined,
        errorSummary: status === "failed" ? "service[nginx] (nginx::default line 84) had an error: Mixlib::ShellOut::ShellCommandFailed" : undefined,
      });
    });
  }
  return out.sort((a, b) => b.startedAt.localeCompare(a.startedAt));
})();

export const runById = (id: string) => runs.find((r) => r.id === id);
export const runsForNode = (nodeId: string) => runs.filter((r) => r.nodeId === nodeId);

/* ------------------------------ Scans ------------------------------- */

export const scans: Scan[] = nodes.map((n, i) => {
  const seed = NODE_SEEDS[i]!;
  const pids = profilesForNode(seed);
  const profileResults = pids.map((pid, pi) => {
    const controls = buildControls(
      PROFILE_CONTROL_SEEDS[pid]!,
      pid,
      profiles.find((p) => p.id === pid)!.title,
      pi === 0 ? Math.min(seed.failed ?? 0, 4) : Math.max(0, (seed.failed ?? 0) - 4),
      i + pi,
    );
    const anyFail = controls.some((c) => c.status === "failed");
    return {
      profileId: pid,
      profileName: pid,
      profileTitle: profiles.find((p) => p.id === pid)!.title,
      version: profiles.find((p) => p.id === pid)!.version,
      status: (n.compliance === "unknown"
        ? "unknown"
        : n.compliance === "skipped"
          ? "skipped"
          : anyFail
            ? "non-compliant"
            : "compliant") as Scan["status"],
      controls,
    };
  });
  return {
    id: `scan-${String(i + 1).padStart(3, "0")}`,
    nodeId: n.id,
    nodeName: n.name,
    startedAt: minutesAgo(seed.lastSeenMin + 3),
    durationSec: 18 + i * 4,
    status: n.compliance,
    profiles: profileResults,
  };
});

export const scanForNode = (nodeId: string) => scans.find((s) => s.nodeId === nodeId);

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

export const controlRollups: ControlRollup[] = (() => {
  const map = new Map<string, ControlRollup>();
  scans.forEach((s) =>
    s.profiles.forEach((p) =>
      p.controls.forEach((c) => {
        const key = `${p.profileId}:${c.id}`;
        const entry = map.get(key) ?? {
          id: c.id,
          title: c.title,
          profileId: p.profileId,
          profileTitle: p.profileTitle,
          severity: c.severity,
          impact: c.impact,
          failing: 0,
          passing: 0,
          skipped: 0,
          nodes: [],
        };
        if (c.status === "failed") {
          entry.failing += 1;
          entry.nodes.push(s.nodeName);
        } else if (c.status === "skipped") entry.skipped += 1;
        else entry.passing += 1;
        map.set(key, entry);
      }),
    ),
  );
  return [...map.values()].sort((a, b) => b.failing - a.failing || b.impact - a.impact);
})();

/* --------------------------- Cookbooks ------------------------------ */

const NGINX_RECIPE = `#
# Cookbook:: nginx
# Recipe:: default
#

package 'nginx' do
  action :install
end

template '/etc/nginx/nginx.conf' do
  source 'nginx.conf.erb'
  owner 'root'
  group 'root'
  mode '0644'
  variables(
    worker_processes: node['nginx']['worker_processes'],
    worker_connections: node['nginx']['worker_connections']
  )
  notifies :reload, 'service[nginx]', :delayed
end

service 'nginx' do
  supports status: true, restart: true, reload: true
  action [:enable, :start]
end
`;

const HARDENING_RECIPE = `#
# Cookbook:: hardening
# Recipe:: cis
#

template '/etc/ssh/sshd_config' do
  source 'sshd_config.erb'
  mode '0600'
  variables(
    permit_root_login: node['openssh']['server']['permit_root_login'],
    password_authentication: node['openssh']['server']['password_authentication']
  )
  notifies :reload, 'service[sshd]', :delayed
end

%w(net.ipv4.ip_forward net.ipv4.conf.all.send_redirects).each do |key|
  sysctl key do
    value 0
    action :apply
  end
end

package 'auditd'

service 'sshd' do
  action [:enable, :start]
end
`;

const METADATA = (name: string, version: string) => `name '${name}'
maintainer 'Platform Infrastructure'
maintainer_email 'infra@spindle.io'
license 'Apache-2.0'
version '${version}'
chef_version '>= 17.0'
supports 'ubuntu', '>= 22.04'
supports 'debian', '>= 12.0'
supports 'rhel', '>= 9.0'
`;

function cookbook(name: string, maintainer: string, description: string, versions: string[], nodesCount: number, lastSeenMin: number, recipe: string): Cookbook {
  return {
    name,
    maintainer,
    description,
    nodes: nodesCount,
    lastSeen: minutesAgo(lastSeenMin),
    versions: versions.map((v, i) => ({
      version: v,
      nodes: i === 0 ? nodesCount : Math.max(0, nodesCount - i * 2),
      updatedAt: daysAgo(i * 12 + 2),
      files: [
        { path: "metadata.rb", content: METADATA(name, v) },
        { path: "recipes/default.rb", content: recipe },
        { path: "attributes/default.rb", content: `default['${name}']['enabled'] = true\ndefault['${name}']['version'] = '${v}'\n` },
      ],
    })),
  };
}

export const cookbooks: Cookbook[] = [
  cookbook("nginx", "Platform Infrastructure", "Installs and configures NGINX for edge web tiers.", ["4.2.1", "4.1.0", "3.9.4"], 2, 6, NGINX_RECIPE),
  cookbook("hardening", "Security Engineering", "Applies CIS baseline hardening: SSH, sysctl, auditd, PAM.", ["7.0.3", "6.8.1", "6.5.0"], 8, 4, HARDENING_RECIPE),
  cookbook("postgresql", "Data Platform", "PostgreSQL 16 server, replica and backup configuration.", ["9.1.0", "9.0.2"], 2, 11, NGINX_RECIPE.replace(/nginx/g, "postgresql")),
  cookbook("docker", "CI Platform", "Docker Engine and buildkit configuration for CI runners.", ["6.3.0", "6.2.0", "6.0.1"], 1, 21, NGINX_RECIPE.replace(/nginx/g, "docker")),
  cookbook("monitoring", "Observability", "Deploys the metrics and log shipping agents.", ["3.4.0", "3.3.2"], 10, 3, NGINX_RECIPE.replace(/nginx/g, "vector")),
  cookbook("base", "Platform Infrastructure", "Common users, groups, MOTD and filesystem layout for every node.", ["12.6.0", "12.5.1", "12.4.0"], 10, 3, HARDENING_RECIPE),
  cookbook("redis", "Data Platform", "Redis cluster provisioning and persistence tuning.", ["5.2.0"], 1, 6, NGINX_RECIPE.replace(/nginx/g, "redis")),
];

export const cookbookByName = (name: string) => cookbooks.find((c) => c.name === name);

/* ---------------------------- Activity ------------------------------ */

export const activity: ActivityEvent[] = (() => {
  const out: ActivityEvent[] = [];
  runs.slice(0, 34).forEach((r) => {
    out.push({
      id: `act-run-${r.id}`,
      type: "converge",
      status: r.status === "success" ? "ok" : r.status === "failed" ? "fail" : "warn",
      nodeId: r.nodeId,
      nodeName: r.nodeName,
      title:
        r.status === "success"
          ? `Converge succeeded — ${r.updatedResources} resources updated`
          : r.status === "failed"
            ? "Converge failed — service[nginx] restart error"
            : "Converge missing — node did not check in",
      detail: `${r.cookbook} · ${r.totalResources} resources · ${r.durationSec}s`,
      at: r.startedAt,
      href: `/runs/${r.id}`,
    });
  });
  scans.forEach((s) => {
    const failing = s.profiles.reduce((acc, p) => acc + p.controls.filter((c) => c.status === "failed").length, 0);
    out.push({
      id: `act-scan-${s.id}`,
      type: "scan",
      status: s.status === "compliant" ? "ok" : s.status === "non-compliant" ? "fail" : s.status === "unknown" ? "warn" : "unknown",
      nodeId: s.nodeId,
      nodeName: s.nodeName,
      title: failing > 0 ? `Compliance scan failed — ${failing} control${failing === 1 ? "" : "s"} failing` : "Compliance scan passed",
      detail: `${s.profiles.map((p) => p.profileName).join(", ")} · ${s.durationSec}s`,
      at: s.startedAt,
      href: `/nodes/${s.nodeId}`,
    });
  });
  const nodeEvents: Array<[string, string, string, ActivityEvent["status"], number]> = [
    ["node-06", "db-replica-02.prod.dub", "Node stopped reporting — 36h since last check-in", "warn", 2180],
    ["node-08", "build-runner-04.stg.iad", "Policy group changed: stg-ci → stg-ci-canary", "unknown", 320],
    ["node-10", "sandbox-dev-07.dev.iad", "Node registered with Spindle", "ok", 4300],
    ["node-04", "api-core-02.prod.dub", "Compliance status flipped: compliant → non-compliant", "fail", 62],
    ["node-02", "web-edge-02.prod.iad", "Compliance status flipped: compliant → non-compliant", "fail", 40],
  ];
  nodeEvents.forEach(([id, name, title, status, min], i) => {
    out.push({
      id: `act-node-${i}`,
      type: "node",
      status,
      nodeId: id,
      nodeName: name,
      title,
      detail: "Inventory change",
      at: minutesAgo(min),
      href: `/nodes/${id}`,
    });
  });
  return out.sort((a, b) => b.at.localeCompare(a.at));
})();

/* ----------------------------- Metrics ------------------------------ */

export const complianceTrend30d = Array.from({ length: 30 }, (_, i) => {
  const day = new Date(NOW.getTime() - (29 - i) * 86_400_000);
  const wave = Math.sin(i / 3.4) * 3;
  const compliant = i > 26 ? 6 : 8 + (i % 2 === 0 ? 0 : 1) - (i > 20 ? 1 : 0);
  return {
    date: day.toISOString().slice(0, 10),
    label: day.toISOString().slice(5, 10),
    passRate: Math.round(Math.min(99, Math.max(62, 92 + wave - (i > 26 ? 22 : 0)))),
    compliant,
    nonCompliant: 10 - compliant - (i > 26 ? 1 : 1),
    skipped: 1,
  };
});

export const convergeSuccess14d = Array.from({ length: 14 }, (_, i) => {
  const day = new Date(NOW.getTime() - (13 - i) * 86_400_000);
  const failed = i >= 12 ? 3 + (i % 2) : (i % 4 === 0 ? 2 : 1);
  const total = 10;
  return {
    date: day.toISOString().slice(0, 10),
    label: day.toISOString().slice(5, 10),
    success: total - failed,
    failed,
    rate: Math.round(((total - failed) / total) * 100),
  };
});

/* ----------------------------- Settings ------------------------------ */

export const users: User[] = [
  { id: "u1", name: "Dana Whitfield", email: "dana@spindle.io", role: "Owner", teams: ["Platform Infra"], lastActive: minutesAgo(3), status: "active" },
  { id: "u2", name: "Marcus Oyelaran", email: "marcus@spindle.io", role: "Admin", teams: ["Platform Infra", "Security"], lastActive: minutesAgo(48), status: "active" },
  { id: "u3", name: "Priya Raghunathan", email: "priya@spindle.io", role: "Operator", teams: ["Security"], lastActive: hoursAgo(5), status: "active" },
  { id: "u4", name: "Tomas Lindqvist", email: "tomas@spindle.io", role: "Operator", teams: ["Data Platform"], lastActive: hoursAgo(19), status: "active" },
  { id: "u5", name: "Rowan Alvarez", email: "rowan@contractor.io", role: "Viewer", teams: ["CI Platform"], lastActive: daysAgo(6), status: "invited" },
  { id: "u6", name: "Kenji Watanabe", email: "kenji@spindle.io", role: "Operator", teams: ["Observability"], lastActive: daysAgo(41), status: "suspended" },
];

export const teams: Team[] = [
  { id: "t1", name: "Platform Infra", description: "Owns edge, API and base cookbooks across all regions.", members: 8, environments: ["production", "staging"] },
  { id: "t2", name: "Security", description: "Owns CIS baselines, waivers and compliance reporting.", members: 5, environments: ["production", "staging", "development"] },
  { id: "t3", name: "Data Platform", description: "PostgreSQL, Redis and backup infrastructure.", members: 6, environments: ["production"] },
  { id: "t4", name: "CI Platform", description: "Build runners and container hosts.", members: 4, environments: ["staging", "development"] },
  { id: "t5", name: "Observability", description: "Metrics, logs and alert routing.", members: 3, environments: ["production", "staging"] },
];

export const apiTokens: ApiToken[] = [
  { id: "tk1", name: "terraform-provisioner", prefix: "spn_live_7fA2", scope: "write", createdAt: daysAgo(220), lastUsed: minutesAgo(14), expiresAt: daysAgo(-145), status: "active" },
  { id: "tk2", name: "grafana-readonly", prefix: "spn_live_c31K", scope: "read", createdAt: daysAgo(96), lastUsed: minutesAgo(2), expiresAt: null, status: "active" },
  { id: "tk3", name: "compliance-export", prefix: "spn_live_9dQm", scope: "read", createdAt: daysAgo(31), lastUsed: hoursAgo(30), expiresAt: daysAgo(-60), status: "active" },
  { id: "tk4", name: "legacy-ci", prefix: "spn_live_44xZ", scope: "admin", createdAt: daysAgo(420), lastUsed: daysAgo(190), expiresAt: null, status: "revoked" },
];

export const notificationRules: NotificationRule[] = [
  { id: "n1", name: "Prod converge failures", channel: "Slack", target: "#infra-alerts", trigger: "converge-failure", enabled: true, lastFired: minutesAgo(12) },
  { id: "n2", name: "Critical control regressions", channel: "PagerDuty", target: "Infra — Primary", trigger: "compliance-failure", enabled: true, lastFired: minutesAgo(62) },
  { id: "n3", name: "Node missing > 2h", channel: "Slack", target: "#infra-oncall", trigger: "node-missing", enabled: true, lastFired: hoursAgo(36) },
  { id: "n4", name: "Weekly profile drift digest", channel: "Email", target: "security@spindle.io", trigger: "profile-drift", enabled: false, lastFired: daysAgo(9) },
  { id: "n5", name: "SIEM forwarder", channel: "Webhook", target: "https://siem.spindle.io/hooks/spindle", trigger: "compliance-failure", enabled: true, lastFired: minutesAgo(62) },
];

export const retentionPolicies: RetentionPolicy[] = [
  { id: "r1", dataset: "Converge run reports", description: "Full resource-event payloads for every converge.", retainDays: 90, archive: true, estimatedSize: "412 GB", enabled: true },
  { id: "r2", dataset: "Compliance scan results", description: "Per-control results including evidence output.", retainDays: 365, archive: true, estimatedSize: "196 GB", enabled: true },
  { id: "r3", dataset: "Node attribute snapshots", description: "Automatic, default, normal and override attributes per check-in.", retainDays: 30, archive: false, estimatedSize: "58 GB", enabled: true },
  { id: "r4", dataset: "Audit log", description: "Console and API actions taken by users and tokens.", retainDays: 730, archive: true, estimatedSize: "12 GB", enabled: true },
  { id: "r5", dataset: "Raw agent logs", description: "Unparsed cinc-client stdout captured on failure.", retainDays: 14, archive: false, estimatedSize: "89 GB", enabled: false },
];

/* ------------------------------ Summary ------------------------------ */

export const fleetSummary = {
  total: nodes.length,
  online: nodes.filter((n) => n.status !== "missing").length,
  offline: nodes.filter((n) => n.status === "missing").length,
  convergeSuccess: nodes.filter((n) => n.status === "success").length,
  convergeFailed: nodes.filter((n) => n.status === "failed").length,
  compliant: nodes.filter((n) => n.compliance === "compliant").length,
  nonCompliant: nodes.filter((n) => n.compliance === "non-compliant").length,
  unknownCompliance: nodes.filter((n) => n.compliance === "unknown" || n.compliance === "skipped").length,
  flipped: nodes.filter((n) => n.flipped),
};

export const uniq = <T,>(arr: T[]) => [...new Set(arr)];
export const environments = uniq(nodes.map((n) => n.environment));
export const platforms = uniq(nodes.map((n) => n.platform));
export const policyGroups = uniq(nodes.map((n) => n.policyGroup));
export { ENVIRONMENTS, pick, int };
