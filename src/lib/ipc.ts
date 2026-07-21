import {invoke} from "@tauri-apps/api/core";
import {listen, type UnlistenFn} from "@tauri-apps/api/event";

// Mirror types of the Rust structs (serde rename_all = camelCase).

/**
 * Auto always runs the curve engine; Manual applies a fixed % per fan;
 * Disabled hands every fan back to the EC and the app stops touching them
 * (only reachable from the tray menu's "Disable fan control").
 */
export type FanMode = "auto" | "manual" | "disabled";
export type TempUnit = "celsius" | "fahrenheit";
export type Theme = "system" | "light" | "dark";

export interface CurvePoint {
    tempC: number;
    percent: number;
}

/** Switch that sets every fan to the same manual boost, or back to automatic. */
export interface AllFansBoost {
    enabled: boolean;
    percent: number;
}

export interface Config {
    updateIntervalSecs: number;
    tempUnit: TempUnit;
    theme: Theme;
    mode: FanMode;
    manualBoosts: Record<string, number>;
    curves: Record<string, CurvePoint[]>;
    vendorOverride: string | null;
    historyRetentionDays: number;
    allFansBoost: AllFansBoost;
}

export interface TempReading {
    label: string;
    celsius: number;
}

export interface FanReading {
    id: string;
    label: string;
    rpm: number | null;
    boostPercent: number | null;
    maxRpm: number | null;
}

export interface Sample {
    timestampMs: number;
    temps: TempReading[];
    fans: FanReading[];
}

export interface FanInfo {
    id: string;
    label: string;
    maxRpm: number | null;
    writable: boolean;
}

export interface DeviceInfo {
    vendor: string;
    driver: string;
    model: string | null;
    fans: FanInfo[];
    sensors: string[];
    supportsAutoCurve: boolean;
    writeAccess: boolean;
    availableVendors: string[];
}

export interface ThermalError {
    kind:
        | "device_not_found"
        | "unknown_vendor"
        | "unsupported"
        | "permission_denied"
        | "invalid_value"
        | "io";
    message: string;
}

export function isThermalError(e: unknown): e is ThermalError {
    return typeof e === "object" && e !== null && "kind" in e && "message" in e;
}

export const getDeviceInfo = () => invoke<DeviceInfo>("get_device_info");
export const getHistory = () => invoke<Sample[]>("get_history");
/** Historical telemetry from SQLite, beyond the in-RAM ring buffer. */
export const getHistoryRange = (fromMs: number, toMs: number) =>
    invoke<Sample[]>("get_history_range", {fromMs, toMs});
export const getConfig = () => invoke<Config>("get_config");
export const setConfig = (config: Config) => invoke<Config>("set_config", {config});
export const setFanBoost = (fanId: string, percent: number) =>
    invoke<void>("set_fan_boost", {fanId, percent});
export const setAllFansBoost = (enabled: boolean, percent: number) =>
    invoke<Config>("set_all_fans_boost", {enabled, percent});
export const setMode = (mode: FanMode) => invoke<void>("set_mode", {mode});
export const saveCurve = (fanId: string, points: CurvePoint[]) =>
    invoke<void>("save_curve", {fanId, points});
export const selectVendor = (vendor: string) =>
    invoke<DeviceInfo>("select_vendor", {vendor});
export const checkWriteAccess = () => invoke<boolean>("check_write_access");
export const installUdevRule = () => invoke<void>("install_udev_rule");

export const listenTelemetry = (
    handler: (sample: Sample) => void,
): Promise<UnlistenFn> => listen<Sample>("telemetry", (event) => handler(event.payload));
