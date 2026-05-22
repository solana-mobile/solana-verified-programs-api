//! Validated request inputs. Constructors and `Deserialize` impls do all
//! parsing — once you hold one of these, the value is already valid.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use solana_sdk::pubkey::Pubkey;
use std::{fmt, str::FromStr};

/// A Solana program address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramId(pub Pubkey);

impl ProgramId {
    pub fn as_str(&self) -> String {
        self.0.to_string()
    }
}

impl FromStr for ProgramId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("public key cannot be empty".into());
        }
        Pubkey::from_str(trimmed)
            .map(ProgramId)
            .map_err(|e| format!("invalid public key ({s}): {e}"))
    }
}

impl fmt::Display for ProgramId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for ProgramId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for ProgramId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        ProgramId::from_str(&s).map_err(de::Error::custom)
    }
}

/// A Solana account that signed an Otter Verify PDA entry. Same parsing
/// rules as [`ProgramId`]; the distinct type keeps signers and programs from
/// being mixed up at call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signer(pub Pubkey);

impl FromStr for Signer {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ProgramId::from_str(s).map(|p| Signer(p.0))
    }
}

impl fmt::Display for Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for Signer {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for Signer {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Signer::from_str(&s).map_err(de::Error::custom)
    }
}

/// `https://` URL (or `http://` for loopback hosts only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryUrl(String);

impl FromStr for RepositoryUrl {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err("URL cannot be empty".into());
        }
        let url = url::Url::parse(trimmed).map_err(|e| format!("invalid URL: {e}"))?;
        let host = url
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| "URL must have a valid host".to_string())?;
        match url.scheme() {
            "https" => {}
            "http" => {
                const LOCAL: [&str; 3] = ["localhost", "127.0.0.1", "::1"];
                if !(LOCAL.contains(&host) || host.starts_with("127.")) {
                    return Err("URL must use https except for localhost".into());
                }
            }
            _ => return Err("URL must use http or https scheme".into()),
        }
        Ok(RepositoryUrl(trimmed.to_string()))
    }
}

impl Serialize for RepositoryUrl {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepositoryUrl {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        RepositoryUrl::from_str(&s).map_err(de::Error::custom)
    }
}

/// URL the API will POST verification results to. Same scheme rules as
/// [`RepositoryUrl`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookUrl(String);

impl WebhookUrl {
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl FromStr for WebhookUrl {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        RepositoryUrl::from_str(s).map(|r| WebhookUrl(r.0))
    }
}

impl Serialize for WebhookUrl {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WebhookUrl {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        WebhookUrl::from_str(&s).map_err(de::Error::custom)
    }
}

/// Empty means "no filter"; non-empty must look like a program address or
/// URL — the listing query ILIKEs against both `program_id` and `repository`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery(String);

impl SearchQuery {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SearchQuery {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Ok(SearchQuery(String::new()));
        }
        if ProgramId::from_str(trimmed).is_ok() || RepositoryUrl::from_str(trimmed).is_ok() {
            return Ok(SearchQuery(trimmed.to_string()));
        }
        Err("search must be a valid Solana address or a valid URL".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_id_parses_and_rejects() {
        assert!(ProgramId::from_str("verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC").is_ok());
        assert!(ProgramId::from_str("").is_err());
        assert!(ProgramId::from_str("not-a-pubkey").is_err());
    }

    #[test]
    fn repository_url_rules() {
        assert!(RepositoryUrl::from_str("https://github.com/x/y").is_ok());
        assert!(RepositoryUrl::from_str("http://github.com/x/y").is_err());
        assert!(RepositoryUrl::from_str("http://localhost:3000/cb").is_ok());
        assert!(RepositoryUrl::from_str("http://127.0.0.1/cb").is_ok());
        assert!(RepositoryUrl::from_str("ftp://github.com/x/y").is_err());
        assert!(RepositoryUrl::from_str("github.com/x/y").is_err());
        assert!(RepositoryUrl::from_str("").is_err());
    }

    #[test]
    fn search_accepts_pubkey_or_url() {
        assert!(SearchQuery::from_str("").is_ok());
        assert!(SearchQuery::from_str("verifycLy8mB96wd9wqq3WDXQwM4oU6r42Th37Db9fC").is_ok());
        assert!(SearchQuery::from_str("https://github.com/foo/bar").is_ok());
        assert!(SearchQuery::from_str("not-a-pubkey").is_err());
    }
}
