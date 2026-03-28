# JSON-RPC Messages

RET communicates over stdio using JSON-RPC 2.0 messages.

- Requests are sent to the `ret server` process on stdin.
- Responses and notifications are sent on stdout.
- A sample client is available in [sample.js](./sample.js).

This document focuses on the JSON-RPC server. The CLI exposes the same discovery and resolution features, but its default JSON schema is different:

- JSON-RPC server default: `pet`
- CLI default: `ret`

## Output Schemas

RET supports three output schemas on JSON and JSON-RPC surfaces.

```typescript
type OutputSchema = "ret" | "pet" | "dual";
```

- `ret`: native R installation objects
- `pet`: PET-compatible environment objects
- `dual`: both shapes at once

The JSON-RPC server starts in `pet` mode by default. Set `outputSchema` in `configure` to switch modes.

## Common Types

```typescript
type Architecture = "arm64" | "x64" | "x86";

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

interface RInstallation {
  displayName?: string;
  name?: string;
  executable?: string;
  kind?: RInstallationKind;
  version?: string;
  home?: string;
  manager?: Manager;
  arch?: Architecture;
  symlinks?: string[];
  error?: string;
}
```

The PET-compatible environment shape is lossy by design:

- `home` becomes `prefix`
- `MacFramework` becomes `MacPythonOrg`
- several R-specific kinds collapse to `GlobalPaths`

```typescript
type PetEnvironmentKind =
  | "Conda"
  | "Homebrew"
  | "LinuxGlobal"
  | "MacPythonOrg"
  | "GlobalPaths"
  | "WindowsRegistry";

interface Environment {
  displayName?: string;
  name?: string;
  executable?: string;
  kind?: PetEnvironmentKind;
  version?: string;
  prefix?: string;
  manager?: Manager;
  arch?: Architecture;
  symlinks?: string[];
  error?: string;
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
   * Accepted for PET-compatible configure payloads, but ignored by RET.
   */
  pipenvExecutable?: string;
  /**
   * Accepted for PET-compatible configure payloads, but ignored by RET.
   */
  poetryExecutable?: string;
  /**
   * Directory used to cache resolved installation details.
   * This directory may be deleted by `clear` or `clearCache`.
   */
  cacheDirectory?: string;
  /**
   * Output schema for JSON-RPC requests and notifications.
   * Defaults to "pet" for the server.
   */
  outputSchema?: OutputSchema;
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
   * Also accepts PET-compatible aliases "MacPythonOrg" and "GlobalPaths".
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
}
```

Refresh notifications depend on the configured output schema:

- `ret`: `installation`, `manager`, `telemetry`
- `pet`: `environment`, `manager`, `telemetry`
- `dual`: `installation`, `environment`, `manager`, `telemetry`

## Find Request

Searches a single path and returns matching installations directly.

Unlike `refresh`, this request does not stream notifications and does not include managers in the result payload.

_Request_:

- method: `find`
- params: `FindParams`

_Response_:

- result: `FindResult | null`

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

type FindResult =
  | RInstallation[]
  | Environment[]
  | {
      installations: RInstallation[];
      environments: Environment[];
    };
```

The response shape is selected by `outputSchema`:

- `ret`: `RInstallation[]`
- `pet`: `Environment[]`
- `dual`: `{ installations, environments }`

## Resolve Request

Resolves a single installation from an executable path or installation directory.

_Request_:

- method: `resolve`
- params: `ResolveParams`

_Response_:

- result: `ResolveResult`

```typescript
interface ResolveParams {
  /**
   * Fully qualified path to `R`, `Rscript`, or an installation home directory.
   */
  executable: string;
}

type ResolveResult =
  | RInstallation
  | Environment
  | {
      installation: RInstallation;
      environment: Environment;
    };
```

The response shape is selected by `outputSchema`:

- `ret`: `RInstallation`
- `pet`: `Environment`
- `dual`: `{ installation, environment }`

In PET-compatible modes, `resolve` may emit the telemetry event `InaccuratePythonEnvironmentInfo` when resolved data differs from the initial locator classification. The wire name is intentionally kept for PET compatibility.

## Conda Info Request

Returns Conda telemetry information in a PET-compatible shape.

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

RET always includes the PET telemetry fields. It also includes additional optional fields such as `executable`, `condaVersion`, `rootPrefix`, and `condaPrefix` when that data is available.

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

Sent during `refresh` when the output schema includes native RET installations.

_Notification_:

- method: `installation`
- params: `RInstallation`

This notification is emitted in `ret` and `dual` modes.

## Environment Notification

Sent during `refresh` when the output schema includes PET-compatible environments.

_Notification_:

- method: `environment`
- params: `Environment`

This notification is emitted in `pet` and `dual` modes.

## Telemetry Notification

Sent during `refresh`, and sometimes during `resolve`, with PET-compatible event names.

_Notification_:

- method: `telemetry`
- params: `TelemetryParams`

```typescript
type TelemetryEventName =
  | "GlobalEnvironmentsSearchCompleted"
  | "GlobalPathVariableEnvironmentsSearchCompleted"
  | "AllSearchPathsEnvironmentsSearchCompleted"
  | "SearchCompleted"
  | "InaccuratePythonEnvironmentInfo"
  | "RefreshPerformance";

interface TelemetryParams {
  event: TelemetryEventName;
  data: unknown;
}

interface RefreshPerformance {
  total: number;
  breakdown: Record<string, number>;
  locators: Record<string, number>;
}

interface InaccuratePythonEnvironmentInfo {
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
- `InaccuratePythonEnvironmentInfo` is emitted only when resolve-time data contradicts discovery-time data.
- the duration-based search completion events serialize Rust duration payloads; treat them as telemetry-only values rather than user-facing protocol data.
