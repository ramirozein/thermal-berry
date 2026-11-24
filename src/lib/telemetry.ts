import {useEffect, useState, useSyncExternalStore} from "react";
import {useConfig, useControlStatus} from "./app-context";
import {getHistory, listenTelemetry, type Config, type ControlStatus, type Sample} from "./ipc";
import {HISTORY_CAPACITY} from "./constants";

let buffer: Sample[] = [];
let started = false;
const listeners = new Set<() => void>();

function notify() {
    for (const listener of listeners) listener();
}

async function start() {
    if (started) return;
    started = true;
    try {
        buffer = await getHistory();
        notify();
    } catch (e) {
        console.error("get_history failed:", e);
    }
    try {
        await listenTelemetry((sample) => {
            buffer = [...buffer.slice(-(HISTORY_CAPACITY - 1)), sample];
            notify();
        });
    } catch (e) {
        started = false;
        console.error("Telemetry subscription failed:", e);
    }
}

function subscribe(listener: () => void) {
    listeners.add(listener);
    void start();
    return () => {
        listeners.delete(listener);
    };
}

export function useTelemetryHistory() {
    return useSyncExternalStore(subscribe, getHistorySnapshot);
}

const getHistorySnapshot = () => buffer;
const getLatestSnapshot = (): Sample | undefined => buffer[buffer.length - 1];

function useTelemetrySnapshot() {
    const latest = useSyncExternalStore(subscribe, getLatestSnapshot);
    const config = useConfig();
    const confirmedControl = useControlStatus();
    return {config, latest, confirmedControl};
}

function sampleControl(config: Config | null, confirmedControl: ControlStatus | null, latest: Sample | undefined) {
    return latest?.control?.configRevision === config?.revision ? latest?.control : confirmedControl;
}

export function useTelemetryReading() {
    const {config, latest, confirmedControl} = useTelemetrySnapshot();
    return {config, latest, control: sampleControl(config, confirmedControl, latest)};
}

export function useTelemetry() {
    const {config, latest, confirmedControl} = useTelemetrySnapshot();
    const [now, setNow] = useState(Date.now);
    useEffect(() => {
        const timer = setInterval(() => setNow(Date.now()), 1000);
        return () => clearInterval(timer);
    }, []);
    return {latest, ...evaluateTelemetry(config, confirmedControl, latest, now)};
}

export function evaluateTelemetry(config: Config | null, confirmedControl: ControlStatus | null, latest: Sample | undefined, now: number) {
    const interval = Math.max(1, Math.min(config?.updateIntervalSecs ?? 2, config?.mode === "auto" ? 2 : 60));
    const ageMs = latest ? Math.max(0, now - latest.timestampMs) : Infinity;
    const isStale = ageMs > Math.max(5000, interval * 2500);
    const control = sampleControl(config, confirmedControl, latest);
    const healthy = !!config && !!control && !isStale && latest?.errors.length === 0
        && !control.error && !control.suspended && !control.maintenance && control.appliedMode === config.mode;
    return {ageMs, isStale, control, healthy};
}
