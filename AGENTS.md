# Project Directives

These directives are mandatory across all Thermal Berry components, dependencies, integrations, installers, and updates.

## Zero Telemetry

- Never collect or transmit user or device information for telemetry purposes, including analytics, tracking, fingerprinting, and automated diagnostic reporting.
- Access only the device information strictly necessary for application functionality. All sensor readings and diagnostic logs must remain local.

## External Services

- Prohibit direct or indirect integration with external services capable of collecting user or device information, including APIs, SDKs, plugins, dependencies, and remote assets.
- Restrict existing network communications to their intended operational purpose, excluding personal information, device identifiers, sensor readings, and diagnostic logs.

## Security First

- Prioritize security over convenience and feature delivery. Report security blockers before proceeding with any unsafe implementation.
- Enforce the principle of least privilege, validate inputs at all trust boundaries, including Tauri commands, and prevent exposure of credentials and signing keys.
- Preserve thermal protection mechanisms, verify affected safeguards, and explicitly report any limitations in security or safety validation.
