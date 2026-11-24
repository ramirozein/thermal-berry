import {memo, useMemo} from "react";
import {Check, Cpu, Fan, Thermometer} from "lucide-react";
import {Card, PageHeader, StatusPill} from "../components/ui";
import {LineChart} from "../components/LineChart";
import {useTelemetry, useTelemetryHistory} from "../lib/telemetry";
import {formatTemp, useConfig, useDevice} from "../lib/app-context";
import {cn} from "../lib/utils";
import {SystemMetricCards} from "../components/SystemMetricCards";
import {currentTemperatures, useMetrics} from "../lib/metrics";
import type {Sample, TempUnit} from "../lib/ipc";
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

const DashboardReadings = memo(function DashboardReadings({latest, isStale, unit}: {
    latest: Sample | undefined;
    isStale: boolean;
    unit: TempUnit;
}) {
    const {snapshot, now} = useMetrics();
    const temperatures = currentTemperatures(snapshot, now);
    const tempValue = (celsius: number) => unit === "fahrenheit" ? Math.round(celsius * 1.8 + 32) : Math.round(celsius);
    const unitSuffix = unit === "fahrenheit" ? "°F" : "°C";

    return (
        <>
            <SystemMetricCards snapshot={snapshot} now={now}/>
            <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
                {temperatures.map((t) => (
                    <MetricCard
                        key={t.id}
                        label={t.label}
                        value={String(tempValue(t.celsius))}
                        unit={unitSuffix}
                        detail={isStale ? "Last reading · stale" : "Current temperature"}
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
                {latest && !temperatures.length && (
                    <Card className="p-5 text-sm text-muted-foreground">Temperatures not available</Card>
                )}
                {!latest && (
                    <Card className="col-span-2 p-5 text-sm text-muted-foreground lg:col-span-4">
                        Waiting for the first reading from the monitor…
                    </Card>
                )}
            </div>
        </>
    );
});

const DashboardCharts = memo(function DashboardCharts({spanSecs, unit}: {spanSecs: number; unit: TempUnit}) {
    const history = useTelemetryHistory();
    const endMs = history[history.length - 1]?.timestampMs ?? 0;
    const {temperatureSeries, fanSeries} = useMemo(() => ({
        temperatureSeries: history.map(({timestampMs, temps}) => ({
            timestampMs,
            value: temps.length ? Math.max(...temps.map(({celsius}) => celsius)) : null,
        })),
        fanSeries: history.map(({timestampMs, fans}) => {
            const speeds = fans.flatMap(({rpm}) => rpm === null ? [] : [rpm]);
            return {
                timestampMs,
                value: speeds.length ? speeds.reduce((sum, rpm) => sum + rpm, 0) / speeds.length : null,
            };
        }),
    }), [history]);

    return (
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
                    label="Temperature history"
                    series={temperatureSeries}
                    endMs={endMs}
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
                    label="Fan speed history"
                    series={fanSeries}
                    endMs={endMs}
                    min={0}
                    max={6000}
                    spanSecs={spanSecs}
                    formatValue={(v) => `${Math.round(v).toLocaleString()} RPM`}
                />
            </Card>
        </div>
    );
});

export function Dashboard() {
    const {latest, isStale, healthy, control} = useTelemetry();
    const config = useConfig();
    const {device} = useDevice();
    const unit = config?.tempUnit ?? "celsius";
    const configuredInterval = config?.updateIntervalSecs ?? 2;
    const intervalSecs = config?.mode === "auto" ? Math.min(configuredInterval, 2) : configuredInterval;
    const spanSecs = HISTORY_CAPACITY * intervalSecs;

    const modeLabel = config?.mode === "manual" ? "Manual control"
        : config?.mode === "disabled" ? "Fan control disabled" : config?.automaticControl.strategy === "curve" ? "Automatic curves" : "Target temperature control";

    const statusItems = [
        `${modeLabel} · ${control?.appliedMode === config?.mode && healthy ? "confirmed" : "unconfirmed"}`,
        device ? `${device.fans.length} fans` : "No fans",
        device ? `${isStale ? 0 : latest?.temps.length ?? 0}/${device.sensors.length} sensors reporting` : "No sensors"
    ];

    return (
        <div className="flex flex-col gap-6">
            <PageHeader title="Overview" subtitle="Live system telemetry"
                        action={<StatusPill label={!latest ? "Waiting" : isStale ? "Stale" : healthy ? "Live" : "Degraded"} live={healthy}/>}/>
            <DashboardReadings latest={latest} isStale={isStale} unit={unit}/>
            <DashboardCharts spanSecs={spanSecs} unit={unit}/>
            <Card>
                <div className="flex items-center justify-between p-5">
                    <div>
                        <h2 className="text-sm font-semibold">System status</h2>
                        <p className="mt-1 text-xs text-muted-foreground">
                            {device
                                ? healthy ? "Readings current; fan control confirmed" : control?.suspended ? "Fan control suspended; select a mode to retry" : "Control or telemetry needs attention"
                                : "No compatible device detected"}
                        </p>
                    </div>
                    <span className="flex items-center gap-2 text-xs font-medium">
            <span className={cn("size-1.5 rounded-full", healthy ? "bg-primary" : "bg-destructive")}/>
                        {!device ? "No device" : healthy ? "Healthy" : isStale ? "Stale" : "Degraded"}
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
