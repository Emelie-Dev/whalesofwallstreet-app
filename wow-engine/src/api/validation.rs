use std::collections::HashSet;
use url::{Host, Url};

pub fn validate_stellar_address(addr: &str) -> Result<(), String> {
    if addr.len() != 56 {
        return Err("Stellar address must be exactly 56 characters long".to_string());
    }
    if !addr.starts_with('G') {
        return Err("Stellar address must start with 'G'".to_string());
    }
    for c in addr.chars() {
        if !c.is_ascii_uppercase() && !('2'..='7').contains(&c) {
            return Err("Stellar address contains invalid characters (must be uppercase alphanumeric base32)".to_string());
        }
    }
    Ok(())
}

pub fn validate_asset_code(asset: &str) -> Result<(), String> {
    if asset.is_empty() {
        return Err("Asset code cannot be empty".to_string());
    }

    if asset.starts_with("stellar:") {
        let parts: Vec<&str> = asset.split(':').collect();
        if parts.len() != 3 {
            return Err(
                "Invalid fully qualified Stellar asset format. Must be stellar:CODE:ISSUER"
                    .to_string(),
            );
        }
        let code = parts[1];
        let issuer = parts[2];
        validate_stellar_address(issuer)?;
        if code.is_empty() || code.len() > 12 {
            return Err("Asset code must be between 1 and 12 characters".to_string());
        }
        return Ok(());
    }

    if asset.starts_with("iso4217:") {
        let parts: Vec<&str> = asset.split(':').collect();
        if parts.len() != 2 || parts[1].len() != 3 {
            return Err(
                "Invalid ISO-4217 asset format. Must be iso4217:CURRENCY (e.g. iso4217:USD)"
                    .to_string(),
            );
        }
        return Ok(());
    }

    if asset.len() > 12 {
        return Err("Asset code must be 12 characters or fewer".to_string());
    }

    for c in asset.chars() {
        if !c.is_ascii_alphanumeric() {
            return Err("Asset code must be alphanumeric".to_string());
        }
    }

    Ok(())
}

pub fn validate_anchor_domain(
    domain: &str,
    allowlist: &HashSet<String>,
) -> Result<String, String> {
    if domain.is_empty() {
        return Err("Anchor domain cannot be empty".to_string());
    }

    if !allowlist.contains(domain) {
        return Err("Anchor domain is not allowlisted".to_string());
    }

    // Forzar parseo simulando un scheme para poder analizar el host
    let url_str = format!("https://{}", domain);
    let parsed = Url::parse(&url_str).map_err(|_| "Malformed domain".to_string())?;

    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Userinfo components are not allowed".to_string());
    }

    match parsed.host() {
        Some(Host::Domain(d)) => {
            if d == "localhost" || d.ends_with(".local") {
                return Err("Local/private domains are not allowed".to_string());
            }
        }
        Some(Host::Ipv4(_)) | Some(Host::Ipv6(_)) => {
            return Err("Direct IP addresses are not allowed, must be a domain".to_string());
        }
        None => return Err("Invalid hostname".to_string()),
    }

    Ok(domain.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_anchor_domain_happy_path() {
        let mut allowlist = HashSet::new();
        allowlist.insert("example.com".to_string());
        allowlist.insert("testanchor.stellar.org".to_string());

        assert_eq!(
            validate_anchor_domain("example.com", &allowlist),
            Ok("example.com".to_string())
        );
        assert_eq!(
            validate_anchor_domain("testanchor.stellar.org", &allowlist),
            Ok("testanchor.stellar.org".to_string())
        );
    }

    #[test]
    fn test_validate_anchor_domain_rejections() {
        let mut allowlist = HashSet::new();
        allowlist.insert("127.0.0.1".to_string());
        allowlist.insert("169.254.169.254".to_string());
        allowlist.insert("localhost".to_string());
        allowlist.insert("admin:pass@dominio.com".to_string());
        allowlist.insert("app.local".to_string());

        // Empty domain
        assert_eq!(
            validate_anchor_domain("", &allowlist),
            Err("Anchor domain cannot be empty".to_string())
        );

        // Not in allowlist
        assert_eq!(
            validate_anchor_domain("unauthorized-domain.com", &allowlist),
            Err("Anchor domain is not allowlisted".to_string())
        );

        // Direct IP addresses
        assert_eq!(
            validate_anchor_domain("127.0.0.1", &allowlist),
            Err("Direct IP addresses are not allowed, must be a domain".to_string())
        );
        assert_eq!(
            validate_anchor_domain("169.254.169.254", &allowlist),
            Err("Direct IP addresses are not allowed, must be a domain".to_string())
        );

        // Localhost and .local domains
        assert_eq!(
            validate_anchor_domain("localhost", &allowlist),
            Err("Local/private domains are not allowed".to_string())
        );
        assert_eq!(
            validate_anchor_domain("app.local", &allowlist),
            Err("Local/private domains are not allowed".to_string())
        );

        // Userinfo embedded
        assert_eq!(
            validate_anchor_domain("admin:pass@dominio.com", &allowlist),
            Err("Userinfo components are not allowed".to_string())
        );
    }
}
