import {
    createContext,
    useCallback,
    useContext,
    useEffect,
    useState,
    type ReactNode,
} from "react";
import * as ipc from "./ipc";
import type {Config, DeviceInfo, FanMode, TempUnit} from "./ipc";

interface AppContextValue {
    config: Config | null;
    device: DeviceInfo | null;
    /** null while loading; ThermalError if detection failed. */
    deviceError: ipc.ThermalError | null;
    updateConfig: (patch: Partial<Config>) => Promise<void>;
    setMode: (mode: FanMode) => Promise<void>;
    setAllFansBoost: (enabled: boolean, percent: number) => Promise<void>;
    refreshDevice: () => Promise<void>;
}

const AppContext = createContext<AppContextValue | null>(null);

export function AppProvider({children}: { children: ReactNode }) {
    const [config, setConfigState] = useState<Config | null>(null);
    const [device, setDevice] = useState<DeviceInfo | null>(null);
    const [deviceError, setDeviceError] = useState<ipc.ThermalError | null>(null);

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
        void ipc.getConfig().then(setConfigState);
        void refreshDevice();
    }, [refreshDevice]);

    // Theme: applies the `dark` class on <html> based on config + OS preference.
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

    const updateConfig = useCallback(
        async (patch: Partial<Config>) => {
            if (!config) return;
            const next = {...config, ...patch};
            setConfigState(next); // optimistic; the backend returns the persisted version
            setConfigState(await ipc.setConfig(next));
        },
        [config],
    );

    const setMode = useCallback(
        async (mode: FanMode) => {
            await ipc.setMode(mode);
            setConfigState(await ipc.getConfig());
        },
        [],
    );

    const setAllFansBoost = useCallback(
        async (enabled: boolean, percent: number) => {
            setConfigState(await ipc.setAllFansBoost(enabled, percent));
        },
        [],
    );

    return (
        <AppContext.Provider
            value={{config, device, deviceError, updateConfig, setMode, setAllFansBoost, refreshDevice}}
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
