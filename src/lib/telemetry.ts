import {useEffect, useState, useSyncExternalStore} from "react";
import {useApp} from "./app-context";
import {getHistory, listenTelemetry, type Config, type ControlStatus, type Sample} from "./ipc";

// Module-level store: the buffer survives screens unmounting/mounting.
// The backend also keeps its own ring buffer, so on app startup the chart
// is filled with get_history instead of starting empty.

const MAX_SAMPLES = 60;

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
            buffer = [...buffer.slice(-(MAX_SAMPLES - 1)), sample];
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

function getSnapshot(): Sample[] {
    return buffer;
}

export function useTelemetry() {
    const history = useSyncExternalStore(subscribe, getSnapshot);
    const latest: Sample | undefined = history[history.length - 1];
    const {config, control: confirmedControl} = useApp();
    const [now, setNow] = useState(Date.now);
    useEffect(() => {
        const timer = setInterval(() => setNow(Date.now()), 1000);
        return () => clearInterval(timer);
    }, []);
    return {history, latest, ...evaluateTelemetry(config, confirmedControl, latest, now)};
}

export function evaluateTelemetry(config: Config | null, confirmedControl: ControlStatus | null, latest: Sample | undefined, now: number) {
    const interval = Math.max(1, Math.min(config?.updateIntervalSecs ?? 2, config?.mode === "auto" ? 2 : 60));
    const ageMs = latest ? Math.max(0, now - latest.timestampMs) : Infinity;
    const isStale = ageMs > Math.max(5000, interval * 2500);
    const control = latest?.control?.configRevision === config?.revision ? latest?.control : confirmedControl;
    const healthy = !!config && !!control && !isStale && latest?.errors.length === 0
        && !control.error && !control.suspended && !control.maintenance && control.appliedMode === config.mode;
    return {ageMs, isStale, control, healthy};
}
