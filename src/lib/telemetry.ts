import {useSyncExternalStore} from "react";
import {getHistory, listenTelemetry, type Sample} from "./ipc";

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
    await listenTelemetry((sample) => {
        buffer = [...buffer.slice(-(MAX_SAMPLES - 1)), sample];
        notify();
    });
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
    return {history, latest};
}
