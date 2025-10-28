/**
 * Hanzod - Hanzo Daemon Configuration
 * Local daemon for managing Hanzo instances with optional cloud sync
 */

export interface HanzodConfig {
  // Local settings
  local: {
    port: number;
    host: string;
    dataDir: string;
    enableMetrics: boolean;
    enableLogging: boolean;
  };

  // Cloud sync settings (opt-in)
  cloud: {
    enabled: boolean;
    endpoint?: string;
    apiKey?: string;
    syncInterval?: number;
    shareAgentActivity?: boolean;
    shareScreenshots?: boolean;
    allowRemoteControl?: boolean;
  };

  // Instance tracking
  tracking: {
    trackBrowserExtensions: boolean;
    trackIDEExtensions: boolean;
    trackCLIAgents: boolean;
    trackDesktopApps: boolean;
    trackGPUNodes: boolean;
  };

  // Remote assistance
  assist: {
    enabled: boolean;
    requireConsent: boolean;
    allowedDomains: string[];
    sessionTimeout: number;
  };

  // Security
  security: {
    encryptLocalData: boolean;
    requireAuthentication: boolean;
    allowedOrigins: string[];
    rateLimiting: {
      enabled: boolean;
      maxRequests: number;
      windowMs: number;
    };
  };

  // Resource management
  resources: {
    maxCPUPercent: number;
    maxMemoryMB: number;
    maxStorageMB: number;
    gpuScheduling: {
      enabled: boolean;
      strategy: 'fifo' | 'priority' | 'fair';
    };
  };
}

export const defaultConfig: HanzodConfig = {
  local: {
    port: 9326,
    host: 'localhost',
    dataDir: '~/.hanzo/data',
    enableMetrics: true,
    enableLogging: true
  },

  cloud: {
    enabled: false, // Opt-in only
    endpoint: 'https://cloud.hanzo.ai',
    syncInterval: 60000, // 1 minute
    shareAgentActivity: false,
    shareScreenshots: false,
    allowRemoteControl: false
  },

  tracking: {
    trackBrowserExtensions: true,
    trackIDEExtensions: true,
    trackCLIAgents: true,
    trackDesktopApps: true,
    trackGPUNodes: true
  },

  assist: {
    enabled: true,
    requireConsent: true,
    allowedDomains: ['localhost', '127.0.0.1'],
    sessionTimeout: 3600000 // 1 hour
  },

  security: {
    encryptLocalData: true,
    requireAuthentication: false,
    allowedOrigins: ['http://localhost:*', 'chrome-extension://*', 'moz-extension://*'],
    rateLimiting: {
      enabled: true,
      maxRequests: 100,
      windowMs: 60000
    }
  },

  resources: {
    maxCPUPercent: 80,
    maxMemoryMB: 4096,
    maxStorageMB: 10240,
    gpuScheduling: {
      enabled: true,
      strategy: 'fair'
    }
  }
};