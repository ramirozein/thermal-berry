import {useEffect, useState} from "react";
import {Laptop, ShieldCheck} from "lucide-react";
import {Button, Card, PageHeader, Select, SettingRow, TextInput} from "../components/ui";
import {PermissionBanner} from "../components/PermissionBanner";
import {selectVendor, isThermalError, type AllFansBoost} from "../lib/ipc";
import {useApp} from "../lib/app-context";
import {cn} from "../lib/utils";
import {version} from '../../package.json';

/**
 * The on/off switch itself lives in the tray menu (next to the fan mode
 * items), not here — this card only holds the percent it applies.
 */
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

export function Settings() {
    const {config, device, deviceError, updateConfig, setAllFansBoost, refreshDevice} = useApp();
    const [vendorError, setVendorError] = useState<string | null>(null);

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

            <Card>
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
            <span
                className={cn(
                    "size-1.5 rounded-full",
                    device ? "bg-primary" : "bg-destructive",
                )}
            />
                        {device ? "Connected" : "Disconnected"}
          </span>
                </div>
                {device && (
                    <div className="grid border-t border-border sm:grid-cols-3">
                        {[
                            ["Fans detected", String(device.fans.length)],
                            ["Sensors", device.sensors.join(", ") || "0"],
                            [
                                "Write permissions",
                                device.writeAccess ? "Active" : "Pending",
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
            </Card>

            <PermissionBanner/>

            {!device && (
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

            {config && (
                <Card className="overflow-hidden">
                    <div className="px-5 py-3 text-xs font-semibold text-muted-foreground">GENERAL</div>
                    <SettingRow
                        title="Telemetry interval"
                        description="How often sensors and fans are refreshed"
                    >
                        <Select
                            label="Telemetry interval"
                            value={String(config.updateIntervalSecs)}
                            options={[
                                {value: "1", label: "1 second"},
                                {value: "2", label: "2 seconds"},
                                {value: "5", label: "5 seconds"},
                                {value: "10", label: "10 seconds"},
                            ]}
                            onChange={(v) => void updateConfig({updateIntervalSecs: Number(v)})}
                        />
                    </SettingRow>
                    <SettingRow
                        title="Temperature unit"
                        description="Unit used throughout the application"
                    >
                        <Select
                            label="Temperature unit"
                            value={config.tempUnit}
                            options={[
                                {value: "celsius", label: "Celsius"},
                                {value: "fahrenheit", label: "Fahrenheit"},
                            ]}
                            onChange={(v) => void updateConfig({tempUnit: v})}
                        />
                    </SettingRow>
                    <SettingRow title="Theme" description="Interface appearance">
                        <Select
                            label="Theme"
                            value={config.theme}
                            options={[
                                {value: "system", label: "System"},
                                {value: "light", label: "Light"},
                                {value: "dark", label: "Dark"},
                            ]}
                            onChange={(v) => void updateConfig({theme: v})}
                        />
                    </SettingRow>
                    <SettingRow
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
                            onChange={(v) => void updateConfig({historyRetentionDays: Number(v)})}
                        />
                    </SettingRow>
                </Card>
            )}

            {config && (
                <AllFansBoostCard
                    allFansBoost={config.allFansBoost}
                    onLiveReapply={(percent) => setAllFansBoost(true, percent)}
                    onPercentChange={(patch) =>
                        updateConfig({allFansBoost: {...config.allFansBoost, ...patch}})
                    }
                />
            )}

            <Card className="overflow-hidden">
                <SettingRow
                    title="Thermal Berry"
                    description={`Version ${version} · Open source`}
                >
          <span className="flex items-center gap-2 text-xs text-muted-foreground">
            <ShieldCheck className="size-4 text-primary"/>
            rzein
          </span>
                </SettingRow>
            </Card>
        </div>
    );
}
