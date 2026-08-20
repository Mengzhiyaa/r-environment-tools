# JSON-RPC Messages

RET communicates over stdio using JSON-RPC 2.0 messages.

- Requests are sent to the `ret server` process on stdin.
- Responses and notifications are sent on stdout.
- A sample client is available in [sample.js](./sample.js).

## Common Types

```typescript
type Architecture = "arm64" | "x86_64" | "x86";

enum RInstallationKind {
  Chocolatey,
  Conda,
  EnvironmentModule,
  Guix,
  Homebrew,
  LinuxGlobal,
  MacFramework,
  MacPorts,
  Nix,
  Pixi,
  Rig,
  Scoop,
  Spack,
  WindowsHq,
  WindowsRegistry,
  GlobalPaths,
}

enum EnvManagerType {
  Chocolatey,
  Conda,
  EnvironmentModule,
  Guix,
  Mamba,
  Homebrew,
  MacPorts,
  Nix,
  Rig,
  Scoop,
  Spack,
  WindowsRegistry,
}

interface Manager {
  executable: string;
  version?: string;
  tool: EnvManagerType;
}

type DiscoverySource = "locator" | "globalPaths" | "explicitSearch" | "rVersions";

type LocatorMetadata =
  | {
      type: "conda";
      environmentPath: string;
    }
  | {
      type: "pixi";
      environmentPath: string;
      manifestPath?: string;
      environmentName?: string;
    }
  | {
      type: "module";
      moduleName: string;
      startupCommand: string;
    };

interface RVersionsOverlay {
  label?: string;
  script?: string;
  repo?: string;
  library?: string;
  module?: string;
  moduleStartupCommand?: string;
}

interface RInstallation {
  displayName?: string;
  name?: string;
  executable?: string;
  kind?: RInstallationKind;
  version?: string;
  home?: string;
  manager?: Manager;
  arch?: Architecture;
  scriptPath?: string;
  orthogonal?: boolean;
  knownExecutables?: string[];
  symlinks?: string[];
  discoveredBy?: DiscoverySource[];
  locatorMetadata?: LocatorMetadata;
  rversionsOverlay?: RVersionsOverlay;
  startupCommand?: string;
  environmentVariables?: Record<string, string>;
  error?: string;
}
```

`displayName` is reserved for a custom label supplied by an external source
(for example, an r-versions entry). Manager-backed installations expose their
identity through `kind`, `name`, and `version`; clients can therefore format a
Conda installation as `R 4.3.3 (Conda: seurat4)` without parsing a preformatted
label.

## Example Installation Payloads

```json
{
  "executable": "/home/mzy/miniforge3/envs/R/bin/R",
  "home": "/home/mzy/miniforge3/envs/R/lib/R",
  "version": "4.5.3",
  "arch": "x86_64",
  "scriptPath": "/home/mzy/miniforge3/envs/R/bin/Rscript",
  "startupCommand": "conda activate /home/mzy/miniforge3/envs/R",
  "environmentVariables": {
    "R_LIBS": "/opt/R-libs/production"
  },
  "kind": "Conda",
  "discoveredBy": ["locator", "rVersions"],
  "locatorMetadata": {
    "type": "conda",
    "environmentPath": "/home/mzy/miniforge3/envs/R"
  },
  "rversionsOverlay": {
    "label": "Production R",
    "repo": "https://ppm.example.com/cran/latest",
    "library": "/opt/R-libs/production"
  },
  "displayName": "Production R"
}
```

```json
{
  "executable": "/opt/modules/R/4.3.0/bin/R",
  "home": "/opt/modules/R/4.3.0/lib/R",
  "version": "4.3.0",
  "arch": "x86_64",
  "scriptPath": "/opt/modules/R/4.3.0/bin/Rscript",
  "startupCommand": "eval $(/usr/bin/modulecmd sh load R/4.3.0)",
  "kind": "EnvironmentModule",
  "discoveredBy": ["locator"],
  "locatorMetadata": {
    "type": "module",
    "moduleName": "R/4.3.0",
    "startupCommand": "eval $(/usr/bin/modulecmd sh load R/4.3.0)"
  }
}
```

```json
{
  "executable": "/home/user/project/.pixi/envs/default/bin/R",
  "home": "/home/user/project/.pixi/envs/default/lib/R",
  "version": "4.3.3",
  "arch": "x86_64",
  "scriptPath": "/home/user/project/.pixi/envs/default/bin/Rscript",
  "startupCommand": null,
  "environmentVariables": null,
  "kind": "Pixi",
  "discoveredBy": ["locator"],
  "locatorMetadata": {
    "type": "pixi",
    "environmentPath": "/home/user/project/.pixi/envs/default",
    "manifestPath": "/home/user/project/pixi.toml",
    "environmentName": "default"
  }
}
```

```json
{
  "executable": "/opt/R/4.4.1/bin/R",
  "home": "/opt/R/4.4.1/lib/R",
  "version": "4.4.1",
  "arch": "x86_64",
  "scriptPath": "/opt/R/4.4.1/bin/Rscript",
  "startupCommand": null,
  "environmentVariables": null,
  "kind": "LinuxGlobal",
  "discoveredBy": ["locator", "globalPaths"],
  "locatorMetadata": null,
  "rversionsOverlay": null
}
```

## Configuration Request

This should be the first request sent to the server. Send it again only when configuration changes.

All properties are optional.

_Request_:

- method: `configure`
- params: `ConfigureParams`

_Response_:

- result: `null`

```typescript
interface ConfigureParams {
  /**
   * Project or workspace directories that should be searched for R installations.
   * Glob patterns are supported.
   */
  workspaceDirectories?: string[];
  /**
   * Additional global directories that may contain one or more R installations.
   * Glob patterns are supported.
   */
  environmentDirectories?: string[];
  /**
   * Explicit directories to keep in the server configuration.
   * Useful when the client wants persistent one-off search roots.
   * Glob patterns are supported.
   */
  searchDirectories?: string[];
  /**
   * Explicit executable paths to keep in the server configuration.
   * Glob patterns are supported; only files are kept after expansion.
   */
  executables?: string[];
  /**
   * Path to the conda, mamba, or micromamba executable.
   */
  condaExecutable?: string;
  /**
   * Path to the rig executable.
   */
  rigExecutable?: string;
  /**
   * Directory used to cache resolved installation details.
   * This directory may be deleted by `clear` or `clearCache`.
   */
  cacheDirectory?: string;
}
```

## Refresh Request

Performs discovery and streams results via notifications.

_Request_:

- method: `refresh`
- params: `RefreshParams | null`

_Response_:

- result: `RefreshResult`

```typescript
interface RefreshParams {
  /**
   * Limits the search to a specific installation kind.
   *
   * Accepts native RET kind names such as "Rig" and "MacFramework".
   */
  searchKind?: string;
  /**
   * Limits the search to an explicit set of directories or executable paths.
   * When provided, this refresh ignores configured workspaceDirectories and
   * environmentDirectories for this request.
   *
   * Glob patterns are supported.
   */
  searchPaths?: string[];
}

interface RefreshResult {
  /**
   * Total time taken by the refresh, in milliseconds.
   */
  duration: number;
  /** Stable identifier included in RefreshProgress events for this scan. */
  refreshId: number;
}
```

Refreshes use a configuration snapshot. Requests with the same normalized parameters and
configuration generation share one scan and each receive a response. If configuration changes
during a scan, notifications and state synchronization from the older generation are discarded.

Refresh streams notifications as results are discovered:

- `installation` — each discovered R installation
- `manager` — each discovered environment manager
- `telemetry` — timing and diagnostic events

## Find Request

Searches a single path and returns matching installations directly.

Unlike `refresh`, this request does not stream notifications and does not include managers in the result payload.

_Request_:

- method: `find`
- params: `FindParams`

_Response_:

- result: `RInstallation[] | null`

```typescript
interface FindParams {
  /**
   * File or directory to search.
   *
   * If this is a file, RET tries to classify that executable directly.
   * If this is a directory, RET searches it recursively.
   */
  searchPath: string;
}
```

## Resolve Request

Resolves a single installation from an executable path or installation directory.

_Request_:

- method: `resolve`
- params: `ResolveParams`

_Response_:

- result: `RInstallation`

```typescript
interface ResolveParams {
  /**
   * Fully qualified path to `R`, `Rscript`, or an installation home directory.
   */
  executable: string;
}
```

`resolve` may emit the telemetry event `InaccurateEnvironmentInfo` when resolved data differs from the initial locator classification.

## Conda Info Request

Returns Conda telemetry information.

_Request_:

- method: `condaInfo`
- params: `null`

_Response_:

- result: `CondaTelemetryInfo`

```typescript
interface CondaTelemetryInfo {
  canSpawnConda: boolean;
  condaRcs: string[];
  envDirs: string[];
  environmentsTxt?: string;
  environmentsTxtExists?: boolean;
  userProvidedEnvFound?: boolean;
  environmentsFromTxt: string[];
  executable?: string;
  condaVersion?: string;
  rootPrefix?: string;
  condaPrefix?: string;
}
```

## Clear Cache Request

Both `clear` and `clearCache` are accepted.

_Request_:

- method: `clear` or `clearCache`
- params: `null`

_Response_:

- result: `null`

If `cacheDirectory` has not been configured, this is a no-op.

Warning:

- the configured cache directory may be deleted recursively
- use a dedicated cache directory for RET

## Manager Notification

Sent during `refresh` whenever a manager is discovered.

_Notification_:

- method: `manager`
- params: `Manager`

## Installation Notification

Sent during `refresh` for each discovered R installation.

_Notification_:

- method: `installation`
- params: `RInstallation`

## Telemetry Notification

Sent during `refresh`, and sometimes during `resolve`, with timing and diagnostic events.

_Notification_:

- method: `telemetry`
- params: `TelemetryParams`

```typescript
type TelemetryEventName =
  | "GlobalEnvironmentsSearchCompleted"
  | "GlobalPathVariableEnvironmentsSearchCompleted"
  | "AllSearchPathsEnvironmentsSearchCompleted"
  | "SearchCompleted"
  | "InaccurateEnvironmentInfo"
  | "RefreshPerformance"
  | "RefreshProgress";

interface TelemetryParams {
  event: TelemetryEventName;
  data: unknown;
}

interface RefreshPerformance {
  total: number;
  breakdown: Record<string, number>;
  locators: Record<string, number>;
}

interface RefreshProgress {
  refreshId: number;
  generation: number;
  phase: "refresh" | "locators" | "path" | "searchPaths" | "merge";
  status: "started" | "completed";
  elapsedMs: number;
  phaseElapsedMs?: number;
  locator?: string;
}

interface InaccurateEnvironmentInfo {
  kind?: RInstallationKind;
  invalidExecutable?: boolean;
  executableNotInSymlinks?: boolean;
  invalidPrefix?: boolean;
  invalidVersion?: boolean;
  invalidArch?: boolean;
}
```

Notes:

- `RefreshPerformance` carries millisecond timings.
- `RefreshProgress` never includes workspace, home, environment names, or other user paths.
- `InaccurateEnvironmentInfo` is emitted only when resolve-time data contradicts discovery-time data.
- the duration-based search completion events serialize Rust duration payloads; treat them as telemetry-only values rather than user-facing protocol data.
