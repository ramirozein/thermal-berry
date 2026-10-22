import {
    createContext,
    useCallback,
    useContext,
    useEffect,
    useState,
    useRef,
    type ReactNode,
} from "react";
import * as ipc from "./ipc";
import type {Config, DeviceInfo, FanMode, TempUnit, UpdateInfo} from "./ipc";

interface AppContextValue {
    config: Config | null;
    control: ipc.ControlStatus | null;
    actionError: string | null;
    dismissError: () => void;
    device: DeviceInfo | null;
    deviceError: ipc.ThermalError | null;
    updateInfo: UpdateInfo | null;
    updateConfig: (patch: ipc.ConfigPatch) => Promise<void>;
    cancelManualAdjustments: () => void;
    setFanBoost: (fanId: string, percent: number, expectedEpoch: number) => Promise<void>;
    setMode: (mode: FanMode) => Promise<void>;
    setAllFansBoost: (enabled: boolean, percent: number) => Promise<void>;
    refreshDevice: () => Promise<void>;
}

const AppContext = createContext<AppContextValue | null>(null);

export function AppProvider({children}: { children: ReactNode }) {
    const [snapshot, setSnapshot] = useState<ipc.StateSnapshot | null>(null);
    const config = snapshot?.config ?? null;
    const [actionError, setActionError] = useState<string | null>(null);
    const manualSession = useRef(0);
    const cancelManualAdjustments = useCallback(() => { manualSession.current += 1; }, []);
    const pending = useRef<Promise<void>>(Promise.resolve());
    const acceptSnapshot = useCallback((next: ipc.StateSnapshot) => {
        setSnapshot((current) => {
            if (current && current.config.revision > next.config.revision) return current;
            return {...next, config: current?.config.revision === next.config.revision ? current.config : next.config};
        });
    }, []);
    const refreshState = useCallback(async () => acceptSnapshot(await ipc.getState()), [acceptSnapshot]);
    const mutate = useCallback((operation: () => Promise<void>) => {
        const task = pending.current.then(async () => {
            try {
                await operation();
                await refreshState();
                setActionError(null);
            } catch (e) {
                setActionError(ipc.isThermalError(e) ? e.message : String(e));
                await refreshState().catch(() => {});
                throw e;
            }
        });
        pending.current = task.catch(() => {});
        return task;
    }, [refreshState]);
    const [device, setDevice] = useState<DeviceInfo | null>(null);
    const [deviceError, setDeviceError] = useState<ipc.ThermalError | null>(null);
    const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);

    const refreshDevice = useCallback(async () => {
        try {
            setDevice(await ipc.getDeviceInfo());
            setDeviceError(null);
        } catch (e) {
            setDevice(null);
            if (ipc.isThermalError(e)) setDeviceError(e);
            else setDeviceError({kind: "io", message: String(e)});
        }
    }, []);

    useEffect(() => {
        let disposed = false;
        let unlisten: (() => void) | undefined;
        void (async () => {
            try {
                const stop = await ipc.listenState(acceptSnapshot);
                if (disposed) { stop(); return; }
                unlisten = stop;
            } catch (e) {
                if (!disposed) setActionError(ipc.isThermalError(e) ? e.message : String(e));
            }
            if (!disposed) await refreshState().catch((e) => {
                if (!disposed) setActionError(ipc.isThermalError(e) ? e.message : String(e));
            });
        })();
        void refreshDevice();
        ipc.checkForUpdate().then(setUpdateInfo).catch(() => setUpdateInfo(null));
        return () => { disposed = true; unlisten?.(); };
    }, [refreshDevice, acceptSnapshot, refreshState]);

    useEffect(() => {
        if (!config) return;
        const media = window.matchMedia("(prefers-color-scheme: dark)");
        const apply = () => {
            const dark =
                config.theme === "dark" || (config.theme === "system" && media.matches);
            document.documentElement.classList.toggle("dark", dark);
        };
        apply();
        media.addEventListener("change", apply);
        return () => media.removeEventListener("change", apply);
    }, [config]);

    const updateConfig = useCallback((patch: ipc.ConfigPatch) => {
        if (patch.automaticControl || patch.curves) cancelManualAdjustments();
        return mutate(() => ipc.setConfig(patch));
    }, [mutate, cancelManualAdjustments]);
    const setMode = useCallback((mode: FanMode) => {
        cancelManualAdjustments();
        return mutate(() => ipc.setMode(mode));
    }, [mutate, cancelManualAdjustments]);
    const setFanBoost = useCallback((fanId: string, percent: number, expectedEpoch: number) => {
        const session = manualSession.current;
        return mutate(() => session === manualSession.current
            ? ipc.setFanBoost(fanId, percent, expectedEpoch) : Promise.resolve());
    }, [mutate]);
    const setAllFansBoost = useCallback((enabled: boolean, percent: number) => {
        cancelManualAdjustments();
        return mutate(() => ipc.setAllFansBoost(enabled, percent));
    }, [mutate, cancelManualAdjustments]);

    return (
        <AppContext.Provider
            value={{
                config,
                control: snapshot?.control ?? null,
                actionError,
                dismissError: () => setActionError(null),
                device,
                deviceError,
                updateInfo,
                updateConfig,
                setMode,
                setFanBoost,
                cancelManualAdjustments,
                setAllFansBoost,
                refreshDevice,
            }}
        >
            {children}
        </AppContext.Provider>
    );
}

export function useApp() {
    const ctx = useContext(AppContext);
    if (!ctx) throw new Error("useApp must be used within <AppProvider>");
    return ctx;
}

export function formatTemp(celsius: number, unit: TempUnit): string {
    if (unit === "fahrenheit") return `${Math.round(celsius * 1.8 + 32)}°F`;
    return `${Math.round(celsius)}°C`;
}
