import type { AirstackConfig } from './types';

export interface ValidationResult {
  valid: boolean;
  errors: string[];
  warnings: string[];
}

export function validateConfig(config: AirstackConfig): ValidationResult {
  const errors: string[] = [];
  const warnings: string[] = [];

  // Validate project
  if (!config.project.name.trim()) {
    errors.push('Project name cannot be empty');
  }

  if (config.project.name.includes(' ')) {
    warnings.push('Project name contains spaces, consider using hyphens or underscores');
  }

  // Validate infrastructure
  if (config.infra?.servers) {
    const serverNames = new Set<string>();
    
    for (const server of config.infra.servers) {
      // Check for duplicate names
      if (serverNames.has(server.name)) {
        errors.push(`Duplicate server name: ${server.name}`);
      }
      serverNames.add(server.name);

      // Validate server configuration
      if (!server.name.trim()) {
        errors.push('Server name cannot be empty');
      }

      if (!['hetzner'].includes(server.provider)) {
        warnings.push(`Unknown provider '${server.provider}' for server '${server.name}'`);
      }

      if (server.ssh_key && !server.ssh_key.includes('/') && !server.ssh_key.startsWith('~')) {
        warnings.push(`SSH key for server '${server.name}' should be a file path or key ID`);
      }
    }
  }

  // Validate services
  if (config.services) {
    const serviceNames = new Set<string>();
    
    for (const [serviceName, service] of Object.entries(config.services)) {
      // Check for duplicate names
      if (serviceNames.has(serviceName)) {
        errors.push(`Duplicate service name: ${serviceName}`);
      }
      serviceNames.add(serviceName);

      // Validate service configuration
      if (!service.image.trim()) {
        errors.push(`Service '${serviceName}' must have an image`);
      }

      if (service.ports.length === 0) {
        warnings.push(`Service '${serviceName}' has no exposed ports`);
      }

      // Check for common port conflicts
      for (const port of service.ports) {
        if (port < 1 || port > 65535) {
          errors.push(`Service '${serviceName}' has invalid port: ${port}`);
        }

        if (port < 1024) {
          warnings.push(`Service '${serviceName}' uses privileged port ${port}`);
        }
      }

      // Validate dependencies
      if (service.depends_on) {
        for (const dep of service.depends_on) {
          if (!config.services[dep]) {
            errors.push(`Service '${serviceName}' depends on '${dep}' which is not defined`);
          }
        }
      }

      // Validate volumes
      if (service.volumes) {
        for (const volume of service.volumes) {
          if (!volume.includes(':')) {
            warnings.push(`Volume '${volume}' in service '${serviceName}' should use host:container format`);
          }
        }
      }
    }

    // Check for circular dependencies
    const visited = new Set<string>();
    const visiting = new Set<string>();

    function checkCircularDeps(serviceName: string): boolean {
      if (visiting.has(serviceName)) {
        return true; // Circular dependency found
      }
      if (visited.has(serviceName)) {
        return false;
      }

      visiting.add(serviceName);
      const service = config.services![serviceName];
      
      if (service.depends_on) {
        for (const dep of service.depends_on) {
          if (checkCircularDeps(dep)) {
            errors.push(`Circular dependency detected involving service '${serviceName}'`);
            return true;
          }
        }
      }

      visiting.delete(serviceName);
      visited.add(serviceName);
      return false;
    }

    for (const serviceName of Object.keys(config.services)) {
      checkCircularDeps(serviceName);
    }
  }

  return {
    valid: errors.length === 0,
    errors,
    warnings,
  };
}