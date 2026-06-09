use crate::secrets_store;
use airstack_config::ServiceConfig;
use anyhow::{Context, Result};

pub fn resolve_service_env(
    project: &str,
    service_name: &str,
    service: &ServiceConfig,
) -> Result<ServiceConfig> {
    let mut resolved = service.clone();
    let Some(env) = &service.env else {
        return Ok(resolved);
    };

    let mut resolved_env = env.clone();
    for (key, value) in env {
        let resolved_value = resolve_value(project, service_name, key, value)?;
        resolved_env.insert(key.clone(), resolved_value);
    }
    resolved.env = Some(resolved_env);
    Ok(resolved)
}

fn resolve_value(project: &str, service_name: &str, env_key: &str, raw_value: &str) -> Result<String> {
    resolve_placeholders(raw_value, |placeholder| {
        lookup_placeholder(project, placeholder)
            .with_context(|| {
                format!(
                    "Failed resolving env placeholder '{}' for service '{}' key '{}'",
                    placeholder, service_name, env_key
                )
            })
            .and_then(|value| {
                value.with_context(|| {
                    format!(
                        "Unresolved env placeholder '{}' for service '{}' key '{}'. Set it in the process environment, the project .env, or `airstack secret set {}`.",
                        placeholder, service_name, env_key, placeholder
                    )
                })
            })
    })
}

fn lookup_placeholder(project: &str, key: &str) -> Result<Option<String>> {
    if let Some(value) = std::env::var_os(key) {
        return value
            .into_string()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("Environment variable '{}' is not valid UTF-8", key));
    }

    secrets_store::get(project, key)
}

fn resolve_placeholders<F>(raw: &str, mut lookup: F) -> Result<String>
where
    F: FnMut(&str) -> Result<String>,
{
    let mut resolved = String::with_capacity(raw.len());
    let mut cursor = 0usize;

    while let Some(rel_start) = raw[cursor..].find("${") {
        let start = cursor + rel_start;
        resolved.push_str(&raw[cursor..start]);

        let key_start = start + 2;
        let Some(rel_end) = raw[key_start..].find('}') else {
            anyhow::bail!("Malformed placeholder in '{}': missing closing '}}'", raw);
        };
        let end = key_start + rel_end;
        let key = &raw[key_start..end];
        if key.is_empty() {
            anyhow::bail!("Malformed placeholder in '{}': empty variable name", raw);
        }

        let replacement = lookup(key)?;
        resolved.push_str(&replacement);
        cursor = end + 1;
    }

    resolved.push_str(&raw[cursor..]);
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::resolve_placeholders;

    #[test]
    fn resolves_multiple_placeholders() {
        let value = resolve_placeholders("token=${A};url=${B}", |key| {
            Ok(match key {
                "A" => "alpha".to_string(),
                "B" => "beta".to_string(),
                _ => unreachable!("unexpected key"),
            })
        })
        .expect("placeholders should resolve");

        assert_eq!(value, "token=alpha;url=beta");
    }

    #[test]
    fn leaves_plain_strings_unchanged() {
        let value = resolve_placeholders("plain-value", |_| {
            unreachable!("plain string should not need lookup")
        })
            .expect("plain string should not need lookup");
        assert_eq!(value, "plain-value");
    }

    #[test]
    fn errors_on_unresolved_placeholders() {
        let err = resolve_placeholders("${MISSING}", |_| {
            anyhow::bail!("No value found for placeholder 'MISSING'")
        })
        .expect_err("missing should fail");
        let text = err.to_string();
        assert!(text.contains("No value found for placeholder 'MISSING'"));
    }
}
