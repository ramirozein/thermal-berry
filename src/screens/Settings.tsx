import {useEffect, useId, useRef, useState} from "react";
import {Laptop, ShieldCheck} from "lucide-react";
import {Button, Card, PageHeader, Select, SettingRow, TextInput, Toggle} from "../components/ui";
import {PermissionBanner} from "../components/PermissionBanner";
import {getAutostart, setAutostart, selectVendor, uninstallApp, isThermalError, type AllFansBoost} from "../lib/ipc";
import {useConfig, useControlActions, useDevice, useGeneralConfig} from "../lib/app-context";
import {cn} from "../lib/utils";
import {getVersion} from "@tauri-apps/api/app";

function AllFansBoostCard({allFansBoost, onLiveReapply, onPercentChange,}: {
    allFansBoost: AllFansBoost;
    onLiveReapply: (percent: number) => Promise<void>;
    onPercentChange: (patch: Partial<AllFansBoost>) => Promise<void>;
}) {
    const [percentDraft, setPercentDraft] = useState(String(allFansBoost.percent));
    const [error, setError] = useState<string | null>(null);

    useEffect(() => setPercentDraft(String(allFansBoost.percent)), [allFansBoost.percent]);

    return (
        <Card className="overflow-hidden">
            <div className="px-5 py-3 text-xs font-semibold text-muted-foreground">
                BOOST ALL FANS
            </div>
            <SettingRow
                title="Boost percent"
                description="Applied to every fan by the tray menu's Boost all fans switch"
            >
                <TextInput
                    label="Boost percent"
                    type="number"
                    value={percentDraft}
                    onChange={setPercentDraft}
                    onBlur={() => {
                        const percent = Math.min(100, Math.max(0, Math.round(Number(percentDraft) || 0)));
                        setPercentDraft(String(percent));
                        if (percent === allFansBoost.percent) return;
                        setError(null);
                        const commit = allFansBoost.enabled
                            ? onLiveReapply(percent)
                            : onPercentChange({percent});
                        commit.catch((e) => {
                            setError(isThermalError(e) ? e.message : String(e));
                            setPercentDraft(String(allFansBoost.percent));
                        });
                    }}
                />
            </SettingRow>
            {error && (
                <div className="border-t border-border px-5 py-3 text-xs text-destructive">{error}</div>
            )}
        </Card>
    );
}

const UNINSTALL_CONFIRM_PHRASE = "DELETE";

function UninstallCard({platform}: {platform: string | undefined}) {
    const windows = platform === "windows";
    const macOS = platform === "macos";
    const description = windows
        ? "Opens the Windows uninstaller, where you can choose to delete app data."
        : macOS
            ? "Moves Thermal Berry to the Trash, removes its login item and closes the app."
            : "Removes the udev rule, the installed package, all saved settings and history, then closes the app. This cannot be undone.";
    const dialog = useRef<HTMLDialogElement>(null);
    const opener = useRef<HTMLButtonElement>(null);
    const cancelButton = useRef<HTMLButtonElement>(null);
    const titleId = useId();
    const descriptionId = useId();
    const [confirmText, setConfirmText] = useState("");
    const [pending, setPending] = useState(false);
    const [error, setError] = useState<string | null>(null);

    const resetConfirmation = () => {
        setConfirmText("");
        setError(null);
        opener.current?.focus();
    };

    const runUninstall = async () => {
        setPending(true);
        setError(null);
        try {
            await uninstallApp();
        } catch (e) {
            setPending(false);
            setError(isThermalError(e) ? e.message : String(e));
        }
    };

    return (
        <Card className="overflow-hidden border-destructive/40">
            <div className="px-5 py-3 text-xs font-semibold text-destructive">DANGER ZONE</div>
            <SettingRow
                title="Uninstall Thermal Berry"
                description={description}
            >
                <Button variant="destructive" disabled={pending} onClick={(event) => {
                    if (windows) {
                        void runUninstall();
                        return;
                    }
                    opener.current = event.currentTarget;
                    dialog.current?.showModal();
                    cancelButton.current?.focus();
                }}>
                    {windows && pending ? "Opening uninstaller…" : "Uninstall…"}
                </Button>
            </SettingRow>
            {windows && error && <p role="alert" className="px-5 py-3 text-xs text-destructive">{error}</p>}

            {!windows && <dialog ref={dialog} aria-labelledby={titleId} aria-describedby={descriptionId}
                    className="m-auto w-11/12 max-w-sm rounded-xl border border-border bg-card p-5 text-foreground backdrop:bg-black/50"
                    onCancel={(event) => { if (pending) event.preventDefault(); }}
                    onClose={resetConfirmation}>
                <h2 id={titleId} className="text-sm font-semibold text-destructive">Uninstall Thermal Berry?</h2>
                <p id={descriptionId} className="mt-2 text-xs leading-relaxed text-muted-foreground">
                    {macOS ? description : "Permanently uninstalls the app and deletes all data and configurations."}
                </p>
                <p className="mt-3 text-xs text-muted-foreground">
                    Type <span className="font-mono font-semibold text-foreground">{UNINSTALL_CONFIRM_PHRASE}</span> to confirm.
                </p>
                <TextInput
                    label="Confirm uninstall"
                    value={confirmText}
                    onChange={setConfirmText}
                    placeholder={UNINSTALL_CONFIRM_PHRASE}
                    readOnly={pending}
                    className="mt-2 w-full text-left"
                />
                {error && <p role="alert" className="mt-2 text-xs text-destructive">{error}</p>}
                <div className="mt-4 flex justify-end gap-2">
                    <Button ref={cancelButton} variant="ghost" disabled={pending} onClick={() => dialog.current?.close()}>
                        Cancel
                    </Button>
                    <Button
                        variant="destructive"
                        disabled={confirmText !== UNINSTALL_CONFIRM_PHRASE || pending}
                        onClick={() => void runUninstall()}
                    >
                        {pending ? "Uninstalling…" : macOS ? "Move to Trash" : "Uninstall permanently"}
                    </Button>
                </div>
            </dialog>}
        </Card>
    );
}

export function Settings() {
    const config = useConfig();
    const generalConfig = useGeneralConfig();
    const {capabilities, device, deviceError, refreshDevice} = useDevice();
    const linux = capabilities?.linuxMaintenance === true;
    const {updateConfig, setAllFansBoost} = useControlActions();
    const [vendorError, setVendorError] = useState<string | null>(null);
    const [autostart, setAutostartState] = useState<boolean | null>(null);
    const [autostartPending, setAutostartPending] = useState(false);
    const [autostartError, setAutostartError] = useState<string | null>(null);
    const [version, setVersion] = useState("");

    useEffect(() => {
        getAutostart()
            .then(setAutostartState)
            .catch((e) => setAutostartError(isThermalError(e) ? e.message : String(e)));
        void getVersion().then(setVersion);
    }, []);

    const toggleAutostart = async (on: boolean) => {
        setAutostartError(null);
        setAutostartState(on);
        setAutostartPending(true);
        try {
            await setAutostart(on);
        } catch (e) {
            setAutostartState(!on);
            setAutostartError(isThermalError(e) ? e.message : String(e));
        } finally {
            setAutostartPending(false);
        }
    };

    const pickVendor = async (vendor: string) => {
        setVendorError(null);
        try {
            await selectVendor(vendor);
            await refreshDevice();
        } catch (e) {
            setVendorError(isThermalError(e) ? e.message : String(e));
        }
    };

    return (
        <div className="flex flex-col gap-6">
            <PageHeader title="Settings" subtitle="Device and application preferences"/>

            {linux && <Card>
                <div className="flex items-center gap-4 p-5">
                    <div className="flex size-11 items-center justify-center rounded-xl bg-secondary">
                        <Laptop className="size-5"/>
                    </div>
                    <div className="flex-1">
                        <h2 className="text-sm font-semibold">
                            {device?.model ?? "Device not detected"}
                        </h2>
                        <p className="mt-1 text-xs text-muted-foreground">
                            {device
                                ? `Vendor: ${device.vendor} · Driver: ${device.driver}`
                                : (deviceError?.message ?? "Looking for compatible hardware…")}
                        </p>
                    </div>
                    <span className="flex items-center gap-2 text-xs text-muted-foreground">
                        <span className=
                                  {cn("size-1.5 rounded-full", device ? "bg-primary" : "bg-destructive")}/>
                        {device ? "Connected" : "Disconnected"}
                    </span>
                </div>
                {device && (
                    <div className="grid border-t border-border sm:grid-cols-3">
                        {[
                            ["Fans", String(device.fans.length)],
                            ["Sensors", device.sensors.join(", ") || "0"],
                            [
                                "Write permissions",
                                device.fanControl ? device.writeAccess ? "Active" : "Pending" : "Read only",
                            ],
                        ].map(([key, value], i) => (
                            <div
                                key={key}
                                className={cn("p-4", i > 0 && "border-t border-border sm:border-l sm:border-t-0")}
                            >
                                <p className="text-xs text-muted-foreground">{key}</p>
                                <p className="mt-1 font-mono text-xs font-medium">{value}</p>
                            </div>
                        ))}
                    </div>
                )}
            </Card>}

            <PermissionBanner/>

            {linux && !device && (
                <Card className="overflow-hidden">
                    <div className="px-5 py-3 text-xs font-semibold text-muted-foreground">
                        MANUAL DETECTION
                    </div>
                    <SettingRow
                        title="Choose vendor"
                        description="If automatic detection failed, force a supported vendor"
                    >
                        <div className="flex flex-col items-end gap-1">
                            <div className="flex gap-2">
                                {(deviceError ? ["alienware"] : []).map((v) => (
                                    <Button key={v} variant="outline" onClick={() => void pickVendor(v)}>
                                        {v}
                                    </Button>
                                ))}
                            </div>
                            {vendorError && (
                                <p className="text-xs text-destructive">{vendorError}</p>
                            )}
                        </div>
                    </SettingRow>
                </Card>
            )}

            <Card className="overflow-hidden">
                <div className="px-5 py-3 text-xs font-semibold text-muted-foreground">GENERAL</div>
                {generalConfig && <>
                    <SettingRow
                        title="Telemetry interval"
                        description={linux
                            ? "How often readings refresh. Automatic control uses at most 2 seconds for a prompt response."
                            : "How often live system readings refresh"}
                    >
                        <Select
                            label="Telemetry interval"
                            value={String(generalConfig.updateIntervalSecs)}
                            options={[
                                {value: "1", label: "1 second"},
                                {value: "2", label: "2 seconds"},
                                {value: "5", label: "5 seconds"},
                                {value: "10", label: "10 seconds"},
                            ]}
                            onChange={(v) => void updateConfig({updateIntervalSecs: Number(v)}).catch(() => {})}
                        />
                    </SettingRow>
                    <SettingRow
                        title="Temperature unit"
                        description="Unit used throughout the application"
                    >
                        <Select
                            label="Temperature unit"
                            value={generalConfig.tempUnit}
                            options={[
                                {value: "celsius", label: "Celsius"},
                                {value: "fahrenheit", label: "Fahrenheit"},
                            ]}
                            onChange={(v) => void updateConfig({tempUnit: v}).catch(() => {})}
                        />
                    </SettingRow>
                    <SettingRow title="Theme" description="Interface appearance">
                        <Select
                            label="Theme"
                            value={generalConfig.theme}
                            options={[
                                {value: "system", label: "System"},
                                {value: "light", label: "Light"},
                                {value: "dark", label: "Dark"},
                            ]}
                            onChange={(v) => void updateConfig({theme: v}).catch(() => {})}
                        />
                    </SettingRow>
                </>}
                {config && <SettingRow
                    title="History retention"
                    description="How long telemetry history is kept in the local database"
                >
                    <Select
                        label="History retention"
                        value={String(config.historyRetentionDays)}
                        options={[
                            {value: "1", label: "1 day"},
                            {value: "7", label: "7 days"},
                            {value: "30", label: "30 days"},
                            {value: "90", label: "90 days"},
                        ]}
                        onChange={(v) => void updateConfig({historyRetentionDays: Number(v)}).catch(() => {})}
                    />
                </SettingRow>}
                <SettingRow
                    title="Launch on startup"
                    description="Start Thermal Berry automatically when you log in"
                >
                    <div className="flex flex-col items-end gap-1">
                        <Toggle
                            label="Launch on startup"
                            on={autostart ?? false}
                            disabled={autostart === null || autostartPending}
                            onChange={(on) => void toggleAutostart(on)}
                        />
                        {autostartError && (
                            <p className="text-xs text-destructive">{autostartError}</p>
                        )}
                    </div>
                </SettingRow>
            </Card>

            {config && (
                <AllFansBoostCard
                    allFansBoost={config.allFansBoost}
                    onLiveReapply={(percent) => setAllFansBoost(true, percent)}
                    onPercentChange={(patch) =>
                        updateConfig({allFansBoostPercent: patch.percent})
                    }
                />
            )}

            <UninstallCard platform={capabilities?.platform}/>

            <SettingRow title="Thermal Berry" description={`Version ${version} · Open source`}>
                  <span className="flex items-center gap-2 text-xs text-muted-foreground">
                    <ShieldCheck className="size-4 text-primary"/>
                    rzein
                  </span>
            </SettingRow>
        </div>
    );
}
