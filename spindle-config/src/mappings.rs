//! Group/claim mapping rules for identity-to-role resolution (M3-08).
//!
//! See [`MappingRule`], [`MappingEvaluator`], and [`validate_mappings`].

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::{ConfigError, IdentityConfig};

/// How a mapping rule matches a principal's identity attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatchType {
    /// Match against a group name from the principal's group list.
    Group,
    /// Match against a claim key/value from the principal's claims map.
    Claim,
}

impl std::fmt::Display for MatchType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchType::Group => write!(f, "group"),
            MatchType::Claim => write!(f, "claim"),
        }
    }
}

/// A single group/claim mapping rule.
///
/// Each rule specifies a connector, a match type (group or claim),
/// a regex pattern to match against, and the roles and scopes to
/// assign when the pattern matches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub struct MappingRule {
    /// Connector this rule applies to (e.g. "oidc", "ldap", "saml", "local").
    /// An empty string means "all connectors".
    #[serde(default)]
    pub connector: String,

    /// Match type: group or claim.
    pub match_type: MatchType,

    /// Regex pattern for matching.
    /// - For `group`: tested against each group name.
    /// - For `claim`: tested against the value of the claim key specified
    ///   by `claim_key`.
    #[serde(rename = "match_value")]
    pub match_value: String,

    /// For `claim` match_type, the claim key to look up in the principal's
    /// claims map. Ignored for `group` match_type.
    #[serde(default)]
    pub claim_key: String,

    /// Roles to assign when this rule matches.
    #[serde(default)]
    pub assign_roles: Vec<String>,

    /// Scopes to assign when this rule matches.
    #[serde(default)]
    pub assign_scope: Vec<String>,
}

impl MappingRule {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.match_value.is_empty() {
            return Err(ConfigError::InvalidValue {
                section: "identity.mappings",
                field: "match_value",
                reason: "match_value must not be empty".into(),
            });
        }
        if self.match_type == MatchType::Claim && self.claim_key.is_empty() {
            return Err(ConfigError::InvalidValue {
                section: "identity.mappings",
                field: "claim_key",
                reason: "claim_key is required when match_type = \"claim\"".into(),
            });
        }
        // Validate regex compiles
        regex::Regex::new(&self.match_value).map_err(|e| ConfigError::InvalidValue {
            section: "identity.mappings",
            field: "match_value",
            reason: format!("invalid regex: {e}"),
        })?;
        Ok(())
    }
}

impl Default for MappingRule {
    fn default() -> Self {
        Self {
            connector: String::new(),
            match_type: MatchType::Group,
            match_value: String::new(),
            claim_key: String::new(),
            assign_roles: vec![],
            assign_scope: vec![],
        }
    }
}

/// Result of applying mapping rules to a principal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MappingResult {
    /// Roles assigned by the first matching rule.
    pub roles: Vec<String>,
    /// Scopes assigned by the first matching rule.
    pub scope: Vec<String>,
}

/// Validates mapping rules for ambiguity and circular references.
///
/// Ambiguity: two rules where one's match_value regex is a superset of
/// another's and they would always match the same set of principals
/// (e.g. `.*` and `admin` for the same connector + match_type).
///
/// Circular group references: when a rule assigns a group name as a role/scope,
/// and that group name is also the match target of another rule that forms a cycle.
pub fn validate_mappings(rules: &[MappingRule]) -> Result<(), ConfigError> {
    // 1. Validate each rule individually
    for (i, rule) in rules.iter().enumerate() {
        if let Err(e) = rule.validate() {
            return Err(ConfigError::MappingRuleInvalid {
                index: i,
                reason: e.to_string(),
            });
        }
    }

    // 2. Detect ambiguous rules (regex superset conflicts)
    for i in 0..rules.len() {
        for j in (i + 1)..rules.len() {
            let (a, b) = (&rules[i], &rules[j]);
            if a.connector == b.connector && a.match_type == b.match_type {
                if let Some(conflict) = detect_regex_subset_conflict(a, b) {
                    return Err(ConfigError::AmbiguousMappingRule {
                        rule_a_index: i,
                        rule_b_index: j,
                        reason: conflict,
                    });
                }
            }
        }
    }

    // 3. Detect circular group references
    if has_circular_group_refs(rules) {
        return Err(ConfigError::CircularGroupReference {
            reason: "circular group reference detected in mapping rules".into(),
        });
    }

    Ok(())
}

/// Detect if two regexes for the same connector+match_type would cause
/// ambiguity — e.g., `.*` matches everything, making `admin` unreachable
/// when the tautological rule appears first in config order.
fn detect_regex_subset_conflict(a: &MappingRule, b: &MappingRule) -> Option<String> {
    let regex_a = regex::Regex::new(&a.match_value).ok()?;
    let regex_b = regex::Regex::new(&b.match_value).ok()?;

    let samples = generate_sample_strings(a, b);

    let mut a_matches_all_b = true;
    let mut b_matches_all_a = true;

    for s in &samples {
        let a_matches = regex_a.is_match(s);
        let b_matches = regex_b.is_match(s);

        if !a_matches && b_matches {
            a_matches_all_b = false;
        }
        if !b_matches && a_matches {
            b_matches_all_a = false;
        }
    }

    let a_is_subset = a_matches_all_b;
    let b_is_subset = b_matches_all_a;

    if a_is_subset && b_is_subset {
        Some(format!(
            "rules are equivalent (both match the same set of strings): '{}' vs '{}'",
            a.match_value, b.match_value
        ))
    } else if b_is_subset {
        Some(format!(
            "rule with pattern '{}' is a superset of rule with pattern '{}' — \
             the other rule will never match when this one appears first in config order",
            a.match_value, b.match_value
        ))
    } else if a_is_subset {
        Some(format!(
            "rule with pattern '{}' is a subset of rule with pattern '{}' — \
             it will never match when the other rule appears first in config order",
            a.match_value, b.match_value
        ))
    } else {
        None
    }
}

/// Generate sample strings for regex overlap testing.
fn generate_sample_strings(a: &MappingRule, b: &MappingRule) -> Vec<String> {
    let mut samples = Vec::new();

    let extract_literals = |pattern: &str| -> Vec<String> {
        let re = regex::Regex::new(r"[a-zA-Z_][a-zA-Z0-9_-]*").expect("valid static regex");
        re.captures_iter(pattern)
            .filter_map(|m| m.get(0).map(|m| m.as_str().to_string()))
            .collect()
    };

    let literals_a = extract_literals(&a.match_value);
    let literals_b = extract_literals(&b.match_value);

    for la in &literals_a {
        for lb in &literals_b {
            samples.push(format!("{la}.{lb}"));
            samples.push(format!("{lb}.{la}"));
            samples.push(la.clone());
            samples.push(lb.clone());
        }
    }

    samples.extend([
        "".to_string(),
        "admin".to_string(),
        "engineering".to_string(),
        "us-east".to_string(),
        "test".to_string(),
        "root".to_string(),
        "0".to_string(),
        "12345".to_string(),
    ]);

    samples.extend(literals_a);
    samples.extend(literals_b);

    samples
}

/// Detect circular group references among mapping rules.
///
/// A circular reference occurs when:
/// - Rule A matches group "X" and assigns group "Y"  
/// - Rule B matches group "Y" and assigns group "X"
///
/// Or longer chains: A→B→C→A.
///
/// Self-reference (a rule matching group "G" and assigning "G" as a role/scope)
/// is only flagged when "G" is also the match target of another group rule.
fn has_circular_group_refs(rules: &[MappingRule]) -> bool {
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    // Collect all literal group names that any rule matches on.
    // We strip leading/trailing anchors (^, $) to extract the literal.
    let matched_groups: HashSet<String> = rules
        .iter()
        .filter(|r| r.match_type == MatchType::Group)
        .filter_map(|r| {
            let pattern = r.match_value.trim_matches(|c| c == '^' || c == '$');
            // Only treat as literal if it contains no regex metacharacters
            if pattern.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') {
                Some(pattern.to_string())
            } else {
                None
            }
        })
        .collect();

    // Build adjacency graph from rules.
    // For each rule that matches a literal group, if it assigns a role/scope that
    // is also a matched group, create an edge.
    // Only rules whose match_value is a literal group name contribute edges —
    // broad regexes like .* are not group-specific and don't create edges.
    for rule in rules {
        if rule.match_type != MatchType::Group {
            continue;
        }
        let pattern = rule.match_value.trim_matches(|c| c == '^' || c == '$');
        // Only consider rules with literal group name patterns for graph edges
        let is_literal = pattern
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.');
        if !is_literal {
            continue;
        }
        if !matched_groups.contains(pattern) {
            continue;
        }
        // This rule specifically matches a literal group
        for assigned in &rule.assign_roles {
            if matched_groups.contains(assigned) {
                graph
                    .entry(pattern.to_string())
                    .or_default()
                    .push(assigned.clone());
            }
        }
        for assigned in &rule.assign_scope {
            if matched_groups.contains(assigned) {
                graph
                    .entry(pattern.to_string())
                    .or_default()
                    .push(assigned.clone());
            }
        }
    }

    // DFS-based cycle detection
    let mut visited: HashSet<String> = HashSet::new();
    let mut stack: HashSet<String> = HashSet::new();

    fn dfs(
        node: &str,
        graph: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        stack: &mut HashSet<String>,
    ) -> bool {
        if stack.contains(node) {
            return true;
        }
        if visited.contains(node) {
            return false;
        }
        visited.insert(node.to_string());
        stack.insert(node.to_string());

        if let Some(neighbors) = graph.get(node) {
            for neighbor in neighbors {
                if dfs(neighbor, graph, visited, stack) {
                    return true;
                }
            }
        }

        stack.remove(node);
        false
    }

    for node in graph.keys() {
        if dfs(node, &graph, &mut visited, &mut stack) {
            return true;
        }
    }

    false
}

/// Evaluates mapping rules against a principal, producing internal roles.
///
/// Rules are evaluated in config order; the first matching rule wins.
/// Results are cached per principal (keyed by subject + connector).
#[derive(Debug, Clone)]
pub struct MappingEvaluator {
    rules: Vec<MappingRule>,
    cache: HashMap<String, MappingResult>,
}

impl MappingEvaluator {
    /// Create a new evaluator from a list of mapping rules.
    pub fn new(rules: Vec<MappingRule>) -> Self {
        Self {
            rules,
            cache: HashMap::new(),
        }
    }

    /// Create an evaluator from an IdentityConfig, validating rules first.
    pub fn from_identity_config(config: &IdentityConfig) -> Result<Self, ConfigError> {
        Self::try_new(config.mappings.clone())
    }

    /// Create a new evaluator, validating rules first.
    pub fn try_new(rules: Vec<MappingRule>) -> Result<Self, ConfigError> {
        validate_mappings(&rules)?;
        Ok(Self::new(rules))
    }

    /// Evaluate rules for a principal, using cached result if available.
    ///
    /// `groups` is the principal's resolved group list.
    /// Claims are a `HashMap<String, String>` as defined in `spindle-identity`'s
    /// `Principal` struct.
    pub fn evaluate(
        &mut self,
        connector: &str,
        subject: &str,
        groups: &[String],
        claims: &HashMap<String, String>,
    ) -> MappingResult {
        let cache_key = format!("{connector}:{subject}");
        if let Some(cached) = self.cache.get(&cache_key) {
            return cached.clone();
        }

        let result = self.evaluate_uncached(connector, groups, claims);
        self.cache.insert(cache_key, result.clone());
        result
    }

    /// Evaluate rules without caching.
    fn evaluate_uncached(
        &self,
        connector: &str,
        groups: &[String],
        claims: &HashMap<String, String>,
    ) -> MappingResult {
        for rule in &self.rules {
            // Check connector match (empty = all connectors)
            if !rule.connector.is_empty() && rule.connector != connector {
                continue;
            }

            if self.rule_matches(rule, groups, claims) {
                return MappingResult {
                    roles: rule.assign_roles.clone(),
                    scope: rule.assign_scope.clone(),
                };
            }
        }

        // No rule matched — return empty result
        MappingResult::default()
    }

    /// Check if a single rule matches the principal's groups/claims.
    fn rule_matches(
        &self,
        rule: &MappingRule,
        groups: &[String],
        claims: &HashMap<String, String>,
    ) -> bool {
        let regex = match regex::Regex::new(&rule.match_value) {
            Ok(re) => re,
            Err(_) => return false, // Invalid regex — should have been caught in validation
        };

        match rule.match_type {
            MatchType::Group => groups.iter().any(|g| regex.is_match(g)),
            MatchType::Claim => claims
                .get(&rule.claim_key)
                .map(|val| regex.is_match(val))
                .unwrap_or(false),
        }
    }

    /// Clear the evaluation cache.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Returns the number of cached entries.
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_group_rule(connector: &str, pattern: &str, roles: &[&str], scope: &[&str]) -> MappingRule {
        MappingRule {
            connector: connector.to_string(),
            match_type: MatchType::Group,
            match_value: pattern.to_string(),
            claim_key: String::new(),
            assign_roles: roles.iter().map(|s| s.to_string()).collect(),
            assign_scope: scope.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn make_claim_rule(connector: &str, key: &str, pattern: &str, roles: &[&str], scope: &[&str]) -> MappingRule {
        MappingRule {
            connector: connector.to_string(),
            match_type: MatchType::Claim,
            match_value: pattern.to_string(),
            claim_key: key.to_string(),
            assign_roles: roles.iter().map(|s| s.to_string()).collect(),
            assign_scope: scope.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_match_type_display() {
        assert_eq!(format!("{}", MatchType::Group), "group");
        assert_eq!(format!("{}", MatchType::Claim), "claim");
    }

    #[test]
    fn test_mapping_rule_default() {
        let rule = MappingRule::default();
        assert!(rule.connector.is_empty());
        assert_eq!(rule.match_type, MatchType::Group);
        assert!(rule.match_value.is_empty());
        assert!(rule.assign_roles.is_empty());
        assert!(rule.assign_scope.is_empty());
    }

    #[test]
    fn test_mapping_rule_serde_group() {
        use figment::providers::{Format, Toml};

        let toml_str = r#"
connector = "ldap"
match-type = "group"
match_value = "^admin$"
assign-roles = ["viewer"]
assign-scope = ["project-admin"]
"#;
        let fig = figment::Figment::from(Toml::string(toml_str));
        let rule: MappingRule = fig.extract().unwrap();
        assert_eq!(rule.connector, "ldap");
        assert_eq!(rule.match_type, MatchType::Group);
        assert_eq!(rule.match_value, "^admin$");
        assert_eq!(rule.assign_roles, vec!["viewer"]);
        assert_eq!(rule.assign_scope, vec!["project-admin"]);
    }

    #[test]
    fn test_mapping_rule_serde_claim() {
        use figment::providers::{Format, Toml};

        let toml_str = r#"
connector = "oidc"
match-type = "claim"
claim-key = "department"
match_value = "^engineering$"
assign-roles = ["viewer", "compliance-auditor"]
assign-scope = ["project-engineering"]
"#;
        let fig = figment::Figment::from(Toml::string(toml_str));
        let rule: MappingRule = fig.extract().unwrap();
        assert_eq!(rule.connector, "oidc");
        assert_eq!(rule.match_type, MatchType::Claim);
        assert_eq!(rule.claim_key, "department");
        assert_eq!(rule.match_value, "^engineering$");
    }

    #[test]
    fn test_validate_empty_mappings_ok() {
        assert!(validate_mappings(&[]).is_ok());
    }

    #[test]
    fn test_validate_single_rule_ok() {
        let rules = vec![make_group_rule("ldap", "^admin$", &["viewer"], &["project-admin"])];
        assert!(validate_mappings(&rules).is_ok());
    }

    #[test]
    fn test_validate_claim_rule_with_claim_key_ok() {
        let rules = vec![make_claim_rule("oidc", "department", "engineering", &["viewer"], &[])];
        assert!(validate_mappings(&rules).is_ok());
    }

    #[test]
    fn test_validate_claim_rule_missing_claim_key_fails() {
        let rules = vec![MappingRule {
            connector: "oidc".to_string(),
            match_type: MatchType::Claim,
            match_value: "engineering".to_string(),
            claim_key: String::new(),
            assign_roles: vec![],
            assign_scope: vec![],
        }];
        assert!(validate_mappings(&rules).is_err());
    }

    #[test]
    fn test_validate_empty_match_value_fails() {
        let rules = vec![make_group_rule("ldap", "", &["viewer"], &[])];
        let err = validate_mappings(&rules).unwrap_err();
        assert!(matches!(err, ConfigError::MappingRuleInvalid { .. }));
    }

    #[test]
    fn test_validate_invalid_regex_fails() {
        let rules = vec![make_group_rule("ldap", "[invalid", &["viewer"], &[])];
        let err = validate_mappings(&rules).unwrap_err();
        assert!(matches!(err, ConfigError::MappingRuleInvalid { .. }));
    }

    #[test]
    fn test_validate_ambiguous_equivalent_regex_fails() {
        // Two equivalent patterns for same connector + match_type
        let rules = vec![
            make_group_rule("ldap", "admin", &["viewer"], &[]),
            make_group_rule("ldap", "admin", &["editor"], &[]),
        ];
        let err = validate_mappings(&rules).unwrap_err();
        assert!(matches!(err, ConfigError::AmbiguousMappingRule { .. }));
    }

    #[test]
    fn test_validate_ambiguous_superset_fails() {
        // `.*` matches everything, `admin` is a subset
        let rules = vec![
            make_group_rule("ldap", ".*", &["editor"], &[]),
            make_group_rule("ldap", "admin", &["viewer"], &[]),
        ];
        let err = validate_mappings(&rules).unwrap_err();
        assert!(matches!(err, ConfigError::AmbiguousMappingRule { .. }));
    }

    #[test]
    fn test_validate_different_connectors_not_ambiguous() {
        let rules = vec![
            make_group_rule("ldap", ".*", &["admin"], &[]),
            make_group_rule("oidc", "admin", &["viewer"], &[]),
        ];
        assert!(validate_mappings(&rules).is_ok());
    }

    #[test]
    fn test_validate_different_match_types_not_ambiguous() {
        let rules = vec![
            make_group_rule("ldap", ".*", &["admin"], &[]),
            make_claim_rule("ldap", "dept", ".*", &["viewer"], &[]),
        ];
        assert!(validate_mappings(&rules).is_ok());
    }

    #[test]
    fn test_validate_non_overlapping_regex_ok() {
        let rules = vec![
            make_group_rule("ldap", "^admin$", &["viewer"], &[]),
            make_group_rule("ldap", "^engineering$", &["editor"], &[]),
        ];
        assert!(validate_mappings(&rules).is_ok());
    }

    #[test]
    fn test_validate_circular_group_ref_self_fails() {
        // Rule matches group "admin" and assigns group "admin" as a role
        let rules = vec![make_group_rule("ldap", "admin", &["admin"], &[])];
        assert!(validate_mappings(&rules).is_err());
    }

    #[test]
    fn test_validate_circular_group_ref_two_node_fails() {
        // Rule A matches "group_a", assigns "group_b" as role
        // Rule B matches "group_b", assigns "group_a" as role
        let rules = vec![
            make_group_rule("ldap", "^group_a$", &["group_b"], &[]),
            make_group_rule("ldap", "^group_b$", &["group_a"], &[]),
        ];
        let err = validate_mappings(&rules).unwrap_err();
        assert!(matches!(err, ConfigError::CircularGroupReference { .. }));
    }

    #[test]
    fn test_validate_no_circular_when_assigned_not_a_group() {
        // "admin" is assigned as a role but "admin" is not a matched group name
        let rules = vec![make_group_rule("ldap", "^engineering$", &["admin"], &[])];
        assert!(validate_mappings(&rules).is_ok());
    }

    #[test]
    fn test_validate_empty_connector_matches_all() {
        let rules = vec![make_group_rule("", "^admin$", &["viewer"], &[])];
        assert!(validate_mappings(&rules).is_ok());
    }

    #[test]
    fn test_validate_two_equivalent_full_match_fails() {
        // Both match all strings
        let rules = vec![
            make_group_rule("ldap", ".*", &["viewer"], &[]),
            make_group_rule("ldap", ".*", &["editor"], &[]),
        ];
        assert!(validate_mappings(&rules).is_err());
    }

    // ── MappingEvaluator tests ─────────────────────────────────────

    #[test]
    fn test_evaluator_first_match_wins() {
        let rules = vec![
            make_group_rule("ldap", "^admin$", &["editor"], &["scope-a"]),
            make_group_rule("ldap", ".*", &["viewer"], &["scope-b"]),
        ];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec!["admin".to_string(), "engineering".to_string()];
        let claims = HashMap::new();

        let result = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert_eq!(result.roles, vec!["editor"]);
        assert_eq!(result.scope, vec!["scope-a"]);
    }

    #[test]
    fn test_evaluator_no_match_returns_empty() {
        let rules = vec![make_group_rule("ldap", "^admin$", &["viewer"], &[])];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec!["engineering".to_string()];
        let claims = HashMap::new();

        let result = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert!(result.roles.is_empty());
        assert!(result.scope.is_empty());
    }

    #[test]
    fn test_evaluator_config_order_matters() {
        // Two non-overlapping group patterns — first match wins by config order
        let rules_first_specific: Vec<MappingRule> = vec![
            make_group_rule("ldap", "^admin$", &["editor"], &[]),
            make_group_rule("ldap", "^engineering$", &["viewer"], &[]),
        ];
        let mut evaluator = MappingEvaluator::new(rules_first_specific);
        let groups = vec!["admin".to_string()];
        let claims = HashMap::new();

        let result = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert_eq!(result.roles, vec!["editor"]);
    }

    #[test]
    fn test_evaluator_connector_specific() {
        let rules = vec![
            make_group_rule("ldap", "^admin$", &["viewer"], &[]),
            make_group_rule("oidc", "^admin$", &["editor"], &[]),
        ];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec!["admin".to_string()];
        let claims = HashMap::new();

        // LDAP connector gets "viewer"
        let result = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert_eq!(result.roles, vec!["viewer"]);

        // OIDC connector gets "editor"
        let result = evaluator.evaluate("oidc", "user1", &groups, &claims);
        assert_eq!(result.roles, vec!["editor"]);
    }

    #[test]
    fn test_evaluator_claim_match() {
        let rules = vec![make_claim_rule(
            "oidc",
            "department",
            "^engineering$",
            &["viewer", "compliance-auditor"],
            &["project-engineering"],
        )];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec![];
        let claims: HashMap<String, String> = vec![
            ("department".to_string(), "engineering".to_string()),
            ("email".to_string(), "user@example.com".to_string()),
        ]
        .into_iter()
        .collect();

        let result = evaluator.evaluate("oidc", "user1", &groups, &claims);
        assert_eq!(result.roles, vec!["viewer", "compliance-auditor"]);
        assert_eq!(result.scope, vec!["project-engineering"]);
    }

    #[test]
    fn test_evaluator_claim_no_match() {
        let rules = vec![make_claim_rule(
            "oidc",
            "department",
            "^engineering$",
            &["viewer"],
            &[],
        )];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec![];
        let claims: HashMap<String, String> = vec![
            ("department".to_string(), "marketing".to_string()),
        ]
        .into_iter()
        .collect();

        let result = evaluator.evaluate("oidc", "user1", &groups, &claims);
        assert!(result.roles.is_empty());
    }

    #[test]
    fn test_evaluator_regex_group_match() {
        let rules = vec![make_group_rule(
            "ldap",
            "^project-(\\w+)$",
            &["viewer"],
            &["project-$1"],
        )];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec!["project-engineering".to_string()];
        let claims = HashMap::new();

        let result = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert_eq!(result.roles, vec!["viewer"]);
        // Note: assign_scope is literal, regex expansion in assignments is not supported
        assert_eq!(result.scope, vec!["project-$1"]);
    }

    #[test]
    fn test_evaluator_cache_works() {
        let rules = vec![make_group_rule("ldap", "^admin$", &["viewer"], &[])];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec!["admin".to_string()];
        let claims = HashMap::new();

        // First evaluation — not cached
        let result1 = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert_eq!(evaluator.cache_size(), 1);

        // Second evaluation of same principal — should use cache
        let result2 = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert_eq!(result1, result2);
        assert_eq!(evaluator.cache_size(), 1); // still 1, not 2
    }

    #[test]
    fn test_evaluator_different_principals_cached_separately() {
        let rules = vec![make_group_rule("ldap", ".*", &["viewer"], &[])];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec!["anygroup".to_string()];
        let claims = HashMap::new();

        evaluator.evaluate("ldap", "user1", &groups, &claims);
        evaluator.evaluate("ldap", "user2", &groups, &claims);
        assert_eq!(evaluator.cache_size(), 2);
    }

    #[test]
    fn test_evaluator_clear_cache() {
        let rules = vec![make_group_rule("ldap", ".*", &["viewer"], &[])];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec!["anygroup".to_string()];
        let claims = HashMap::new();

        evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert_eq!(evaluator.cache_size(), 1);
        evaluator.clear_cache();
        assert_eq!(evaluator.cache_size(), 0);
    }

    #[test]
    fn test_evaluator_empty_connector_matches_all() {
        let rules = vec![make_group_rule("", "^admin$", &["viewer"], &[])];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec!["admin".to_string()];
        let claims = HashMap::new();

        let result = evaluator.evaluate("any-connector", "user1", &groups, &claims);
        assert_eq!(result.roles, vec!["viewer"]);
    }

    #[test]
    fn test_evaluator_try_new_validates_rules() {
        let rules = vec![make_group_rule("ldap", "[invalid", &["viewer"], &[])];
        let result = MappingEvaluator::try_new(rules);
        assert!(result.is_err());
    }

    #[test]
    fn test_evaluator_try_new_accepts_valid() {
        let rules = vec![make_group_rule("ldap", "^admin$", &["viewer"], &[])];
        let result = MappingEvaluator::try_new(rules);
        assert!(result.is_ok());
    }

    #[test]
    fn test_evaluator_multiple_groups_one_match() {
        let rules = vec![make_group_rule("ldap", "^devops$", &["editor"], &[])];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups = vec!["engineering".to_string(), "devops".to_string(), "admin".to_string()];
        let claims = HashMap::new();

        let result = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert_eq!(result.roles, vec!["editor"]);
    }

    #[test]
    fn test_evaluator_empty_groups_no_match() {
        let rules = vec![make_group_rule("ldap", "^admin$", &["viewer"], &[])];
        let mut evaluator = MappingEvaluator::new(rules);
        let groups: Vec<String> = vec![];
        let claims = HashMap::new();

        let result = evaluator.evaluate("ldap", "user1", &groups, &claims);
        assert!(result.roles.is_empty());
    }

    #[test]
    fn test_mapping_result_default() {
        let result = MappingResult::default();
        assert!(result.roles.is_empty());
        assert!(result.scope.is_empty());
    }
}
