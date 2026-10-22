import {useEffect, useRef, useState} from "react";
import {Fan, Info, RotateCcw} from "lucide-react";
import {Button, Card, PageHeader} from "../components/ui";
import {PermissionBanner} from "../components/PermissionBanner";
import {type FanInfo} from "../lib/ipc";
import {formatTemp, useApp} from "../lib/app-context";
import {useTelemetry} from "../lib/telemetry";

function FanControl({fan, disabled, timers}: { fan: FanInfo; disabled: boolean; timers: Map<string, ReturnType<typeof setTimeout>> }) {
    const {config, setFanBoost} = useApp();
    const {latest, control} = useTelemetry();
    const [value, setValue] = useState(config?.manualBoosts[fan.id] ?? 0);
    useEffect(() => () => {
        clearTimeout(timers.get(fan.id));
        timers.delete(fan.id);
    }, [fan.id, timers, config?.controlEpoch]);

    const configPct = config?.mode === "manual" ? (config.manualBoosts[fan.id] ?? 0) : 0;
    const lastSynced = useRef(configPct);
    useEffect(() => {
        lastSynced.current = configPct;
        setValue(configPct);
    }, [configPct, config?.controlEpoch]);

    const reading = latest?.fans.find((f) => f.id === fan.id);
    const sensor = latest?.temps.find((t) => fan.label.includes(t.label));

    const apply = (pct: number) => {
        setValue(pct);
        clearTimeout(timers.get(fan.id));
        if (!config) return;
        const epoch = config.controlEpoch;
        timers.set(fan.id, setTimeout(() => {
            timers.delete(fan.id);
            void setFanBoost(fan.id, pct, epoch).catch(() => setValue(lastSynced.current));
        }, 150));
    };

    return (
        <Card className="p-5">
            <div className="flex items-start justify-between">
                <div className="flex items-center gap-3">
                    <div className="flex size-9 items-center justify-center rounded-lg bg-secondary">
                        <Fan className="size-4"/>
                    </div>
                    <div>
                        <h2 className="text-sm font-semibold">{fan.label}</h2>
                        <p className="mt-0.5 text-xs text-muted-foreground">
                            {sensor && config
                                ? `${sensor.label} · ${formatTemp(sensor.celsius, config.tempUnit)}`
                                : "—"}
                        </p>
                    </div>
                </div>
                <div className="text-right">
                    <p className="font-mono text-sm font-medium">
                        {reading?.rpm !== null && reading?.rpm !== undefined
                            ? `${reading.rpm.toLocaleString()} RPM`
                            : "—"}
                    </p>
                    <p className="mt-0.5 text-xs text-muted-foreground">Current</p>
                </div>
            </div>
            <div className="mt-8 flex items-center gap-4">
                <span className="w-8 text-xs text-muted-foreground">0%</span>
                <input
                    aria-label={`Boost for ${fan.label}`}
                    className="range-control min-w-0 flex-1"
                    type="range"
                    min="0"
                    max="100"
                    value={value}
                    disabled={disabled}
                    onChange={(e) => apply(Number(e.target.value))}
                />
                <span className="w-10 text-right font-mono text-sm font-medium text-primary">
          {value}%
        </span>
            </div>
            <p className="mt-4 text-xs text-muted-foreground">
                Last successful command: {control?.appliedBoosts[fan.id] ?? "—"}%
            </p>
            <div
                className="mt-5 flex items-center justify-between border-t border-border pt-4 text-xs text-muted-foreground">
                <span>Observed boost</span>
                <span className="font-mono text-foreground">
          {reading?.boostPercent !== null && reading?.boostPercent !== undefined
              ? `${reading.boostPercent}%`
              : "—"}
        </span>
            </div>
        </Card>
    );
}

export function ManualControl() {
    const {device, setMode, control, cancelManualAdjustments} = useApp();
    const timers = useRef(new Map<string, ReturnType<typeof setTimeout>>()).current;
    const cancelPending = () => {
        timers.forEach(clearTimeout);
        timers.clear();
    };
    useEffect(() => () => {
        cancelManualAdjustments();
        timers.forEach(clearTimeout);
        timers.clear();
    }, [timers, cancelManualAdjustments]);
    const writable = !!device?.writeAccess && !control?.maintenance;

    return (
        <div className="flex flex-col gap-6">
            <PageHeader
                title="Manual Control"
                subtitle="Set a constant boost per fan"
                action={
                    <Button variant="outline" onClick={() => {
                        cancelPending();
                        void setMode("auto").catch(() => {});
                    }}>
                        <RotateCcw className="size-3.5"/>
                        Back to automatic
                    </Button>
                }
            />
            <PermissionBanner/>
            <div className="rounded-xl border border-border bg-secondary/60 p-4 text-sm text-muted-foreground">
                <div className="flex items-start gap-3">
                    <Info className="mt-0.5 size-4 shrink-0 text-primary"/>
                    <p className="leading-relaxed">
                        Boost adds on top of firmware cooling: 0% is full EC control, 100% is max.
                    </p>
                </div>
            </div>
            <div className="grid gap-4 lg:grid-cols-2">
                {device?.fans.map((fan) => (
                    <FanControl key={fan.id} fan={fan} disabled={!writable} timers={timers}/>
                ))}
                {!device && (
                    <Card className="p-5 text-sm text-muted-foreground lg:col-span-2">
                        No device detected. Check Settings to choose a vendor
                        manually.
                    </Card>
                )}
            </div>
        </div>
    );
}
