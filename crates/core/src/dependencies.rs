use airstack_config::ServiceConfig;
use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashMap, HashSet};

pub fn deployment_order(
    services: &HashMap<String, ServiceConfig>,
    root: Option<&str>,
) -> Result<Vec<String>> {
    let mut ordered = Vec::new();
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();

    if let Some(root_service) = root {
        if !services.contains_key(root_service) {
            anyhow::bail!("Service '{}' not found in configuration", root_service);
        }
        visit(
            root_service,
            services,
            &mut visiting,
            &mut visited,
            &mut ordered,
        )?;
    } else {
        let all_services: BTreeSet<String> = services.keys().cloned().collect();
        for service in all_services {
            visit(
                &service,
                services,
                &mut visiting,
                &mut visited,
                &mut ordered,
            )?;
        }
    }

    Ok(ordered)
}

fn visit(
    service: &str,
    services: &HashMap<String, ServiceConfig>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    ordered: &mut Vec<String>,
) -> Result<()> {
    if visited.contains(service) {
        return Ok(());
    }

    if !visiting.insert(service.to_string()) {
        anyhow::bail!(
            "Circular service dependency detected while resolving '{}'",
            service
        );
    }

    let service_cfg = services
        .get(service)
        .with_context(|| format!("Service '{}' not found in configuration", service))?;

    let deps: BTreeSet<String> = service_cfg
        .depends_on
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();

    for dep in deps {
        if !services.contains_key(&dep) {
            anyhow::bail!("Service '{}' depends on missing service '{}'", service, dep);
        }
        visit(&dep, services, visiting, visited, ordered)?;
    }

    visiting.remove(service);
    visited.insert(service.to_string());
    ordered.push(service.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::deployment_order;
    use airstack_config::ServiceConfig;
    use std::collections::HashMap;

    fn svc(depends_on: Option<Vec<&str>>) -> ServiceConfig {
        ServiceConfig {
            image: "nginx:latest".to_string(),
            ports: vec![80],
            env: None,
            volumes: None,
            depends_on: depends_on.map(|deps| deps.into_iter().map(|d| d.to_string()).collect()),
            target_server: None,
            healthcheck: None,
            profile: None,
        }
    }

    #[test]
    fn resolves_nested_dependencies() {
        let mut services = HashMap::new();
        services.insert("db".to_string(), svc(None));
        services.insert("api".to_string(), svc(Some(vec!["db"])));
        services.insert("web".to_string(), svc(Some(vec!["api"])));

        let order = deployment_order(&services, Some("web")).unwrap();
        assert_eq!(order, vec!["db", "api", "web"]);
    }

    #[test]
    fn detects_cycles() {
        let mut services = HashMap::new();
        services.insert("a".to_string(), svc(Some(vec!["b"])));
        services.insert("b".to_string(), svc(Some(vec!["a"])));

        let err = deployment_order(&services, Some("a")).unwrap_err();
        assert!(err.to_string().contains("Circular service dependency"));
    }
}
