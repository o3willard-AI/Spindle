//! Dex integration for Spindle.
//!
//! # Usage
//! ```ignore
//! use spindle_dex::generate_config;
//!
//! let config = generate_config(&spindle_config);
//! ```
//!
//! ## Dex sidecar
//! `spindle-server` starts Dex as child process, or operator runs separately.
//! OIDC, SAML, LDAP connector stanzas in generated config — mapped from Spindle config sections.
//! Health check: poll Dex `/.well-known/openid-configuration` until ready, then proceed.

use serde::{Deserialize, Serialize};

pub mod health;

/// Dex configuration generated from Spindle config.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DexConfig {
    /// Issuer URL for the OIDC provider.
    pub issuer: String,
    /// Direct URL to the Dex server.
    pub issuer_url: String,
    /// Whether to enable health checks.
    pub health_check: bool,
    /// Connector stanzas for OIDC, SAML, LDAP.
    #[serde(default)]
    pub connectors: Vec<ConnectorConfig>,
    /// Feature flags for the Dex server.
    #[serde(default)]
    pub features: Features,
}

/// Connector configuration for a single authentication method.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectorConfig {
    /// Connector ID (e.g., "github", "saml", "ldap").
    pub id: String,
    /// Connector type (e.g., "oidc", "saml", "ldap").
    #[serde(rename = "type")]
    pub connector_type: String,
    /// Connector-specific configuration.
    #[serde(default)]
    pub config: ConnectorSpecificConfig,
}

/// Connector-specific configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectorSpecificConfig {
    /// Application client ID.
    pub client_id: Option<String>,
    /// Application client secret.
    pub client_secret: Option<String>,
    /// Application callback URL.
    pub redirect_url: Option<String>,
    /// Application scope.
    pub scope: Option<Vec<String>>,
    /// Application group claim.
    pub group_claim: Option<String>,
    /// Application group mapping.
    pub group_mapping: Option<Vec<GroupMapping>>,
}

/// Group mapping configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GroupMapping {
    /// Group name in the identity provider.
    pub group: String,
    /// Spindle group name.
    pub spindle_group: String,
}

/// Feature flags for the Dex server.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Features {
    /// Enable the OIDC connector.
    pub oidc: Option<bool>,
    /// Enable the SAML connector.
    pub saml: Option<bool>,
    /// Enable the LDAP connector.
    pub ldap: Option<bool>,
}

/// Spindle configuration for identity and authentication.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpindleConfig {
    /// Identity provider configuration.
    #[serde(default)]
    pub identity: IdentityConfig,
    /// Feature flags.
    #[serde(default)]
    pub features: Features,
}

/// Identity provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IdentityConfig {
    /// OIDC configuration.
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    /// SAML configuration.
    #[serde(default)]
    pub saml: Option<SamlConfig>,
    /// LDAP configuration.
    #[serde(default)]
    pub ldap: Option<LdapConfig>,
    /// Default redirect URL.
    pub redirect_url: Option<String>,
    /// Default scope.
    pub scope: Option<Vec<String>>,
    /// Default group claim.
    pub group_claim: Option<String>,
}

/// OIDC configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OidcConfig {
    /// Application client ID.
    pub client_id: String,
    /// Application client secret.
    pub client_secret: String,
    /// Application callback URL.
    pub redirect_url: String,
    /// Application scope.
    pub scope: Option<Vec<String>>,
    /// Application group claim.
    pub group_claim: Option<String>,
    /// Application group mapping.
    #[serde(default)]
    pub group_mapping: Vec<GroupMapping>,
}

/// SAML configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SamlConfig {
    /// Application client ID.
    pub client_id: String,
    /// Application callback URL.
    pub redirect_url: String,
    /// Application scope.
    pub scope: Option<Vec<String>>,
    /// Application group claim.
    pub group_claim: Option<String>,
    /// Application group mapping.
    #[serde(default)]
    pub group_mapping: Vec<GroupMapping>,
}

/// LDAP configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LdapConfig {
    /// Application client ID.
    pub client_id: String,
    /// Application callback URL.
    pub redirect_url: String,
    /// Application scope.
    pub scope: Option<Vec<String>>,
    /// Application group claim.
    pub group_claim: Option<String>,
}

/// Generate Dex configuration from Spindle config.
pub fn generate_config(config: &SpindleConfig) -> Result<DexConfig, String> {
    let mut connectors = Vec::new();

    // Add OIDC connector if configured
    if let Some(oidc) = &config.identity.oidc {
        connectors.push(ConnectorConfig {
            id: "oidc".to_string(),
            connector_type: "oidc".to_string(),
            config: ConnectorSpecificConfig {
                client_id: Some(oidc.client_id.clone()),
                client_secret: Some(oidc.client_secret.clone()),
                redirect_url: Some(oidc.redirect_url.clone()),
                scope: oidc.scope.clone(),
                group_claim: oidc.group_claim.clone(),
                group_mapping: Some(oidc.group_mapping.clone()),
            },
        });
    }

    // Add SAML connector if configured
    if let Some(saml) = &config.identity.saml {
        connectors.push(ConnectorConfig {
            id: "saml".to_string(),
            connector_type: "saml".to_string(),
            config: ConnectorSpecificConfig {
                client_id: Some(saml.client_id.clone()),
                client_secret: None,
                redirect_url: Some(saml.redirect_url.clone()),
                scope: saml.scope.clone(),
                group_claim: saml.group_claim.clone(),
                group_mapping: Some(saml.group_mapping.clone()),
            },
        });
    }

    // Add LDAP connector if configured
    if let Some(ldap) = &config.identity.ldap {
        connectors.push(ConnectorConfig {
            id: "ldap".to_string(),
            connector_type: "ldap".to_string(),
            config: ConnectorSpecificConfig {
                client_id: Some(ldap.client_id.clone()),
                client_secret: None,
                redirect_url: Some(ldap.redirect_url.clone()),
                scope: ldap.scope.clone(),
                group_claim: ldap.group_claim.clone(),
                group_mapping: None,
            },
        });
    }

    Ok(DexConfig {
        issuer: "https://spindle.local/dex".to_string(),
        issuer_url: "https://spindle.local/dex".to_string(),
        health_check: true,
        connectors,
        features: config.features.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_config_empty() {
        let config = SpindleConfig::default();

        let dex_config = generate_config(&config).unwrap();
        assert_eq!(dex_config.connectors.len(), 0);
    }

    #[test]
    fn test_generate_config_oidc() {
        let config = SpindleConfig {
            identity: IdentityConfig {
                oidc: Some(OidcConfig {
                    client_id: "test-client".to_string(),
                    client_secret: "test-secret".to_string(),
                    redirect_url: "https://spindle.local/callback".to_string(),
                    scope: Some(vec!["openid".to_string(), "profile".to_string()]),
                    group_claim: Some("groups".to_string()),
                    group_mapping: vec![GroupMapping {
                        group: "admin".to_string(),
                        spindle_group: "spindle-admin".to_string(),
                    }],
                }),
                saml: None,
                ldap: None,
                redirect_url: None,
                scope: None,
                group_claim: None,
            },
            features: Features::default(),
        };

        let dex_config = generate_config(&config).unwrap();
        assert_eq!(dex_config.connectors.len(), 1);
        assert_eq!(dex_config.connectors[0].id, "oidc");
        assert_eq!(dex_config.connectors[0].connector_type, "oidc");
    }

    #[test]
    fn test_generate_config_multiple_connectors() {
        let config = SpindleConfig {
            identity: IdentityConfig {
                oidc: Some(OidcConfig {
                    client_id: "test-client".to_string(),
                    client_secret: "test-secret".to_string(),
                    redirect_url: "https://spindle.local/callback".to_string(),
                    scope: Some(vec!["openid".to_string()]),
                    group_claim: Some("groups".to_string()),
                    group_mapping: vec![],
                }),
                saml: Some(SamlConfig {
                    client_id: "saml-client".to_string(),
                    redirect_url: "https://spindle.local/saml".to_string(),
                    scope: Some(vec!["urn:oid:2.5.4.43".to_string()]),
                    group_claim: Some("groups".to_string()),
                    group_mapping: vec![],
                }),
                ldap: Some(LdapConfig {
                    client_id: "ldap-client".to_string(),
                    redirect_url: "https://spindle.local/ldap".to_string(),
                    scope: None,
                    group_claim: None,
                }),
                redirect_url: None,
                scope: None,
                group_claim: None,
            },
            features: Features::default(),
        };

        let dex_config = generate_config(&config).unwrap();
        assert_eq!(dex_config.connectors.len(), 3);
    }
}
