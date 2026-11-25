import {
    createContext,
    useCallback,
    useContext,
    useEffect,
    useMemo,
    useState,
    useRef,
    type ReactNode,
} from "react";
import * as ipc from "./ipc";
import type {Config, DeviceInfo, FanMode, GeneralConfig, UpdateInfo} from "./ipc";
export {formatTemp} from "./utils";

interface DeviceContextValue {
    capabilities: ipc.Capabilities | null;
    device: DeviceInfo | null;
    deviceError: ipc.ThermalError | null;
    refreshDevice: () => Promise<void>;
}

interface ControlActions {
    updateConfig: (patch: ipc.ConfigPatch) => Promise<void>;
    cancelManualAdjustments: () => void;
    setFanBoost: (fanId: string, percent: number, expectedEpoch: number) => Promise<void>;
    setMode: (mode: FanMode) => Promise<void>;
    setAllFansBoost: (enabled: boolean, percent: number) => Promise<void>;
}

const DeviceContext = createContext<DeviceContextValue | null>(null);
const ConfigContext = createContext<Config | null>(null);
const GeneralConfigContext = createContext<GeneralConfig | null>(null);
const ControlContext = createContext<ipc.ControlStatus | null>(null);
const ControlActionsContext = createContext<ControlActions | null>(null);
const ActionFeedbackContext = createContext<{actionError: string | null; dismissError: () => void} | null>(null);
const UpdateInfoContext = createContext<UpdateInfo | null>(null);

export function AppProvider({children}: { children: ReactNode }) {
    const [snapshot, setSnapshot] = useState<ipc.StateSnapshot | null>(null);
    const [preferences, setPreferences] = useState<GeneralConfig | null>(null);
    const [capabilities, setCapabilities] = useState<ipc.Capabilities | null>(null);
    const config = snapshot?.config ?? null;
    const generalConfig = config ?? preferences;
    const isLinux = capabilities?.linuxMaintenance === true;
    const isDesktop = capabilities?.linuxMaintenance === false;
    const [actionError, setActionError] = useState<string | null>(null);
    const dismissError = useCallback(() => setActionError(null), []);
    const manualSession = useRef(0);
    const cancelManualAdjustments = useCallback(() => { manualSession.current += 1; }, []);
    const pending = useRef<Promise<void>>(Promise.resolve());
    const acceptSnapshot = useCallback((next: ipc.StateSnapshot) => {
        setSnapshot((current) => {
            if (current && current.config.revision > next.config.revision) return current;
            return {...next, config: current?.config.revision === next.config.revision ? current.config : next.config};
        });
    }, []);
    const refreshLinuxState = useCallback(async () => acceptSnapshot(await ipc.getState()), [acceptSnapshot]);
    const refreshPreferences = useCallback(async () => setPreferences(await ipc.getPreferences()), []);
    const refreshState = isDesktop ? refreshPreferences : refreshLinuxState;
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
    const device = capabilities?.device ?? null;
    const [deviceError, setDeviceError] = useState<ipc.ThermalError | null>(null);
    const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);

    const refreshDevice = useCallback(async () => {
        try {
            const next = await ipc.getCapabilities();
            setCapabilities(next);
            setDeviceError(next.linuxMaintenance && !next.device
                ? {kind: "device_not_found", message: "No supported thermal device was found on this system"}
                : null);
        } catch (e) {
            setCapabilities((current) => current && {
                ...current,
                device: null,
                temperatures: false,
                fanRpm: false,
                fanControl: false,
                writeAccess: false,
            });
            setDeviceError(ipc.isThermalError(e) ? e : {kind: "io", message: String(e)});
        }
    }, []);

    useEffect(() => {
        void refreshDevice();
    }, [refreshDevice]);

    useEffect(() => {
        if (!isDesktop) return;
        void refreshPreferences().catch((e) => setActionError(ipc.isThermalError(e) ? e.message : String(e)));
    }, [isDesktop, refreshPreferences]);

    useEffect(() => {
        if (!isLinux) return;
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
            if (!disposed) await refreshLinuxState().catch((e) => {
                if (!disposed) setActionError(ipc.isThermalError(e) ? e.message : String(e));
            });
        })();
        ipc.checkForUpdate().then(setUpdateInfo).catch(() => setUpdateInfo(null));
        return () => { disposed = true; unlisten?.(); };
    }, [isLinux, acceptSnapshot, refreshLinuxState]);

    const theme = generalConfig?.theme;
    useEffect(() => {
        if (!theme) return;
        try { localStorage.setItem("thermal-berry-theme", theme); } catch {}
        const media = window.matchMedia("(prefers-color-scheme: dark)");
        const apply = () => {
            const dark =
                theme === "dark" || (theme === "system" && media.matches);
            document.documentElement.classList.toggle("dark", dark);
        };
        apply();
        media.addEventListener("change", apply);
        return () => media.removeEventListener("change", apply);
    }, [theme]);

    const updateConfig = useCallback((patch: ipc.ConfigPatch) => {
        if (patch.automaticControl || patch.curves) cancelManualAdjustments();
        return mutate(() => isDesktop ? ipc.setPreferences(patch) : ipc.setConfig(patch));
    }, [mutate, isDesktop, cancelManualAdjustments]);
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

    const deviceContext = useMemo(
        () => ({capabilities, device, deviceError, refreshDevice}),
        [capabilities, device, deviceError, refreshDevice],
    );
    const actions = useMemo(
        () => ({updateConfig, setMode, setFanBoost, cancelManualAdjustments, setAllFansBoost}),
        [updateConfig, setMode, setFanBoost, cancelManualAdjustments, setAllFansBoost],
    );
    const feedback = useMemo(() => ({actionError, dismissError}), [actionError, dismissError]);

    return <DeviceContext value={deviceContext}>
        <GeneralConfigContext value={generalConfig}>
            <ConfigContext value={config}>
                <ControlContext value={snapshot?.control ?? null}>
                    <ControlActionsContext value={actions}>
                        <ActionFeedbackContext value={feedback}>
                            <UpdateInfoContext value={updateInfo}>{children}</UpdateInfoContext>
                        </ActionFeedbackContext>
                    </ControlActionsContext>
                </ControlContext>
            </ConfigContext>
        </GeneralConfigContext>
    </DeviceContext>;
}

export function useDevice() {
    const context = useContext(DeviceContext);
    if (!context) throw new Error("useDevice must be used within <AppProvider>");
    return context;
}

export function useConfig() {
    return useContext(ConfigContext);
}

export function useGeneralConfig() {
    return useContext(GeneralConfigContext);
}

export function useControlStatus() {
    return useContext(ControlContext);
}

export function useControlActions() {
    const context = useContext(ControlActionsContext);
    if (!context) throw new Error("useControlActions must be used within <AppProvider>");
    return context;
}

export function useActionFeedback() {
    const context = useContext(ActionFeedbackContext);
    if (!context) throw new Error("useActionFeedback must be used within <AppProvider>");
    return context;
}

export function useUpdateInfo() {
    return useContext(UpdateInfoContext);
}
