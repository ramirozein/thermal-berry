import {Check, Cpu, Fan, Thermometer} from "lucide-react";
import {Card, PageHeader, StatusPill} from "../components/ui";
import {LineChart} from "../components/LineChart";
import {useTelemetry} from "../lib/telemetry";
import {formatTemp, useApp} from "../lib/app-context";
import {cn} from "../lib/utils";
import {HISTORY_CAPACITY} from "../lib/constants";

function MetricCard({label, value, unit, detail, icon: Icon}: {
    label: string;
    value: string;
    unit: string;
    detail: string;
    icon: typeof Cpu;
}) {
    return (
        <Card className="p-5">
            <div className="flex items-center justify-between">
                <p className="text-sm font-medium text-muted-foreground">{label}</p>
                <Icon className="size-4 text-muted-foreground" aria-hidden="true"/>
            </div>
            <div className="mt-5 flex items-end gap-1">
                <span className="font-mono text-4xl font-medium tracking-tight">{value}</span>
                <span className="pb-1 text-sm text-muted-foreground">{unit}</span>
            </div>
            <p className="mt-2 text-xs text-muted-foreground">{detail}</p>
        </Card>
    );
}

export function Dashboard() {
    const {history, latest} = useTelemetry();
    const {config, device} = useApp();
    const unit = config?.tempUnit ?? "celsius";
    const intervalSecs = config?.updateIntervalSecs ?? 2;
    const spanSecs = HISTORY_CAPACITY * intervalSecs;

    const tempValue = (celsius: number) => unit === "fahrenheit" ? Math.round(celsius * 1.8 + 32) : Math.round(celsius);
    const unitSuffix = unit === "fahrenheit" ? "°F" : "°C";

    const modeLabel = config?.mode === "manual" ? "Manual control"
        : config?.mode === "disabled" ? "Fan control disabled" : "Automatic control";

    const statusItems = [
        modeLabel,
        device ? `${device.fans.length} fans` : "No fans",
        device ? `${device.sensors.length} active sensors` : "No sensors"
    ];

    return (
        <div className="flex flex-col gap-6">
            <PageHeader title="Overview" subtitle="Live system telemetry"
                        action={<StatusPill label={latest ? "Live" : "Waiting"} live={!!latest}/>}/>
            <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
                {latest?.temps.map((t) => (
                    <MetricCard
                        key={t.label}
                        label={t.label}
                        value={String(tempValue(t.celsius))}
                        unit={unitSuffix}
                        detail="Current temperature"
                        icon={Thermometer}
                    />
                ))}
                {latest?.fans.map((f) => (
                    <MetricCard
                        key={f.id}
                        label={f.label}
                        value={f.rpm !== null ? f.rpm.toLocaleString() : "—"}
                        unit="RPM"
                        detail={f.rpm !== null && f.maxRpm ? `${Math.round((f.rpm / f.maxRpm) * 100)}% of max` : "Current speed"}
                        icon={Fan}
                    />
                ))}
                {!latest && (
                    <Card className="col-span-2 p-5 text-sm text-muted-foreground lg:col-span-4">
                        Waiting for the first reading from the monitor…
                    </Card>
                )}
            </div>
            <div className="grid gap-4 lg:grid-cols-2">
                <Card className="p-5">
                    <div className="mb-5 flex items-start justify-between">
                        <div>
                            <h2 className="text-sm font-semibold">Temperature</h2>
                            <p className="mt-1 text-xs text-muted-foreground">
                                Max across sensors · last {Math.round(spanSecs / 60)} min
                            </p>
                        </div>
                    </div>
                    <LineChart
                        series={history.map((s) =>
                            s.temps.length ? Math.max(...s.temps.map((t) => t.celsius)) : null,
                        )}
                        min={30}
                        max={90}
                        spanSecs={spanSecs}
                        formatValue={(v) => formatTemp(v, unit)}
                    />
                </Card>
                <Card className="p-5">
                    <div className="mb-5 flex items-start justify-between">
                        <div>
                            <h2 className="text-sm font-semibold">Fans</h2>
                            <p className="mt-1 text-xs text-muted-foreground">Combined average</p>
                        </div>
                    </div>
                    <LineChart
                        series={history.map((s) => {
                            const rpms = s.fans
                                .map((f) => f.rpm)
                                .filter((r): r is number => r !== null);
                            return rpms.length
                                ? rpms.reduce((a, b) => a + b, 0) / rpms.length
                                : null;
                        })}
                        min={0}
                        max={6000}
                        spanSecs={spanSecs}
                        formatValue={(v) => `${Math.round(v).toLocaleString()} RPM`}
                    />
                </Card>
            </div>
            <Card>
                <div className="flex items-center justify-between p-5">
                    <div>
                        <h2 className="text-sm font-semibold">System status</h2>
                        <p className="mt-1 text-xs text-muted-foreground">
                            {device
                                ? "All sensors are reporting normally"
                                : "No compatible device detected"}
                        </p>
                    </div>
                    <span className="flex items-center gap-2 text-xs font-medium">
            <span className={cn("size-1.5 rounded-full", device ? "bg-primary" : "bg-destructive")}/>
                        {device ? "Healthy" : "No device"}
          </span>
                </div>
                <div className="grid border-t border-border sm:grid-cols-3">
                    {statusItems.map((item, i) => (
                        <div
                            key={item}
                            className={cn(
                                "flex items-center gap-3 p-4 text-sm",
                                i > 0 && "border-t border-border sm:border-l sm:border-t-0",
                            )}
                        >
                            <Check className="size-4 text-primary" aria-hidden="true"/>
                            {item}
                        </div>
                    ))}
                </div>
            </Card>
        </div>
    );
}
