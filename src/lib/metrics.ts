import {useEffect, useState, useSyncExternalStore} from "react";
import {invoke} from "@tauri-apps/api/core";
import {listen, type UnlistenFn} from "@tauri-apps/api/event";

type Reading<T> = {
    value: T | null;
    status: "pending" | "available" | "unavailable" | "temporaryError";
    timestampMs: number | null;
};

type Memory = {usedBytes: number; totalBytes: number; percent: number};
type Temperature = {id: string; label: string; celsius: number};

export type MetricsSnapshot = {
    sequence: number;
    cpuPercent: Reading<number>;
    memory: Reading<Memory>;
    temperatures: Reading<Temperature[]>;
    temperatureExpiresMs: number | null;
};

export function isNewerSequence(current: number | undefined, next: number) {
    return current == null || next > current;
}

export function createMetricsStore(
    listenForUpdates: (accept: (snapshot: MetricsSnapshot) => void) => Promise<UnlistenFn>,
    readSnapshot: () => Promise<MetricsSnapshot>,
) {
    let snapshot: MetricsSnapshot | null = null;
    // A connected listener keeps the cache current between visits to the dashboard.
    let connectingOrListening = false;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    const subscribers = new Set<() => void>();

    function accept(next: MetricsSnapshot) {
        if (!isNewerSequence(snapshot?.sequence, next.sequence)) return;
        snapshot = next;
        subscribers.forEach((notify) => notify());
    }

    async function connect() {
        if (connectingOrListening || retryTimer !== undefined || !subscribers.size) return;
        connectingOrListening = true;
        try {
            await listenForUpdates(accept);
        } catch (error) {
            console.error("Metrics subscription failed:", error);
            connectingOrListening = false;
            if (subscribers.size) {
                retryTimer = setTimeout(() => {
                    retryTimer = undefined;
                    void connect();
                }, 2000);
            }
            return;
        }
        try {
            accept(await readSnapshot());
        } catch (error) {
            console.error("Initial metrics snapshot failed:", error);
        }
    }

    function subscribe(notify: () => void) {
        subscribers.add(notify);
        void connect();
        return () => {
            subscribers.delete(notify);
            if (!subscribers.size) {
                clearTimeout(retryTimer);
                retryTimer = undefined;
            }
        };
    }

    return {getSnapshot: () => snapshot, subscribe};
}

const {getSnapshot, subscribe} = createMetricsStore(
    (accept) => listen<MetricsSnapshot>("metrics-updated", (event) => accept(event.payload)),
    () => invoke<MetricsSnapshot>("get_metrics_snapshot"),
);
const formatPercent = (percent: number) => `${Math.round(percent)}%`;
const formatGiB = (bytes: number) => `${(bytes / 1024 ** 3).toFixed(1)} GiB`;

export function useMetrics() {
    const snapshot = useSyncExternalStore(subscribe, getSnapshot);
    const [now, setNow] = useState(Date.now);
    useEffect(() => {
        const timer = window.setInterval(() => setNow(Date.now()), 1000);
        return () => window.clearInterval(timer);
    }, []);
    return {snapshot, now};
}

export function useOsdMetrics() {
    const [display, setDisplay] = useState<{now: number; snapshot: MetricsSnapshot | null}>(
        () => ({now: Date.now(), snapshot: null}),
    );
    const [ready, setReady] = useState(false);
    useEffect(() => {
        let active = true;
        let timer: ReturnType<typeof setTimeout>;
        const refresh = async () => {
            try {
                const snapshot = await invoke<MetricsSnapshot>("get_metrics_snapshot");
                if (active) setDisplay({now: Date.now(), snapshot});
            } catch (error) {
                if (active) {
                    console.error(error);
                    setDisplay(previous => ({...previous, now: Date.now()}));
                }
            } finally {
                if (active) {
                    setReady(true);
                    timer = setTimeout(refresh, 2000);
                }
            }
        };
        void refresh();
        return () => {
            active = false;
            clearTimeout(timer);
        };
    }, []);
    return {...display, ready};
}

export function systemMetrics(snapshot: MetricsSnapshot | null, now: number) {
    const cpu = snapshot?.cpuPercent;
    const memory = snapshot?.memory;
    const fresh = <T,>(reading: Reading<T> | undefined) =>
        reading?.status === "available" && reading.timestampMs !== null && now - reading.timestampMs <= 6000;
    const cpuValue = fresh(cpu) ? cpu?.value : null;
    const ramValue = fresh(memory) ? memory?.value : null;
    return {
        cpu: cpuValue == null ? cpu?.status === "pending" || !cpu ? "Waiting…" : "Not available" : formatPercent(cpuValue),
        ram: ramValue ? formatPercent(ramValue.percent) : memory ? "Not available" : "Waiting…",
        ramDetail: ramValue ? `${formatGiB(ramValue.usedBytes)} / ${formatGiB(ramValue.totalBytes)}` : undefined,
    };
}

export function currentTemperatures(snapshot: MetricsSnapshot | null, now: number) {
    if (snapshot?.temperatures.status !== "available" || now > (snapshot.temperatureExpiresMs ?? 0)) return [];
    return snapshot.temperatures.value ?? [];
}
