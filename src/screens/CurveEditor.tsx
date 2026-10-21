import {useEffect, useMemo, useRef, useState} from "react";
import {Button, Card, PageHeader} from "../components/ui";
import {PermissionBanner} from "../components/PermissionBanner";
import {isThermalError, type CurvePoint} from "../lib/ipc";
import {defaultPoints, evaluate} from "../lib/curve";
import {formatTemp, useApp} from "../lib/app-context";
import {useTelemetry} from "../lib/telemetry";
import {cn} from "../lib/utils";

const TEMP_MIN = 30;
const TEMP_MAX = 100;
const VIEW_W = 600;
const VIEW_H = 280;

const toX = (tempC: number) => ((tempC - TEMP_MIN) / (TEMP_MAX - TEMP_MIN)) * VIEW_W;
const toY = (percent: number) => VIEW_H - (percent / 100) * VIEW_H;

// Keeps a dragged point within its neighbors (so the curve stays monotonic on X) and within [0, 100] on Y.
function clampPoint(sorted: CurvePoint[], index: number, tempC: number, percent: number): CurvePoint {
    const lo = index > 0 ? sorted[index - 1].tempC + 1 : TEMP_MIN;
    const hi = index < sorted.length - 1 ? sorted[index + 1].tempC - 1 : TEMP_MAX;
    return {
        tempC: Math.round(Math.min(hi, Math.max(lo, tempC))),
        percent: Math.round(Math.min(100, Math.max(0, percent))),
    };
}

function CurveGraph({points, onChange, disabled,}: {
    points: CurvePoint[];
    onChange: (points: CurvePoint[]) => void;
    disabled: boolean;
}) {
    const svgRef = useRef<SVGSVGElement>(null);
    const [dragIndex, setDragIndex] = useState<number | null>(null);

    const sorted = useMemo(
        () => [...points].sort((a, b) => a.tempC - b.tempC),
        [points],
    );

    const polyline = useMemo(() => {
        const inner = sorted.map((p) => `${toX(p.tempC)},${toY(p.percent)}`);
        const first = sorted[0];
        const last = sorted[sorted.length - 1];
        return [
            `${toX(TEMP_MIN)},${toY(first.percent)}`,
            ...inner,
            `${toX(TEMP_MAX)},${toY(last.percent)}`,
        ].join(" ");
    }, [sorted]);

    const moveTo = (index: number, clientX: number, clientY: number) => {
        const svg = svgRef.current;
        if (!svg) return;
        const rect = svg.getBoundingClientRect();
        const tempC = TEMP_MIN + ((clientX - rect.left) / rect.width) * (TEMP_MAX - TEMP_MIN);
        const percent = 100 - ((clientY - rect.top) / rect.height) * 100;
        const clamped = clampPoint(sorted, index, tempC, percent);
        onChange(sorted.map((p, i) => (i === index ? clamped : p)));
    };

    useEffect(() => {
        if (dragIndex === null) return;
        const onMove = (e: PointerEvent) => moveTo(dragIndex, e.clientX, e.clientY);
        const onUp = () => setDragIndex(null);
        window.addEventListener("pointermove", onMove);
        window.addEventListener("pointerup", onUp);
        return () => {
            window.removeEventListener("pointermove", onMove);
            window.removeEventListener("pointerup", onUp);
        };
    }, [dragIndex, sorted]);

    return (
        <div
            className="relative h-72 border-b border-l border-border"
            role="img"
            aria-label="Editable temperature-to-boost curve"
        >
            <div className="absolute inset-0 flex flex-col justify-between" aria-hidden="true">
                {[0, 1, 2, 3, 4].map((i) => (
                    <div key={i} className="border-t border-border/70"/>
                ))}
            </div>
            <div className="absolute inset-0 flex justify-between" aria-hidden="true">
                {[0, 1, 2, 3, 4, 5, 6].map((i) => (
                    <div key={i} className="border-l border-border/60"/>
                ))}
            </div>
            <svg
                ref={svgRef}
                className="absolute inset-0 size-full overflow-visible"
                viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
                preserveAspectRatio="none"
            >
                <polyline
                    points={polyline}
                    fill="none"
                    stroke="var(--primary)"
                    strokeWidth="2.5"
                    vectorEffect="non-scaling-stroke"
                />
                {sorted.map((p, i) => (
                    <circle
                        key={i}
                        cx={toX(p.tempC)}
                        cy={toY(p.percent)}
                        r="7"
                        fill="var(--background)"
                        stroke="var(--primary)"
                        strokeWidth="3"
                        vectorEffect="non-scaling-stroke"
                        className={cn(!disabled && "cursor-grab", dragIndex === i && "cursor-grabbing")}
                        onPointerDown={(e) => {
                            if (disabled) return;
                            e.preventDefault();
                            setDragIndex(i);
                        }}
                    />
                ))}
            </svg>
            <div
                className="absolute -bottom-7 left-0 right-0 flex justify-between font-mono text-[10px] text-muted-foreground">
                {[30, 45, 60, 75, 90, 100].map((t) => (
                    <span key={t}>{t}°</span>
                ))}
            </div>
            <div
                className="absolute -left-10 inset-y-0 flex flex-col justify-between font-mono text-[10px] text-muted-foreground">
                {["100%", "75%", "50%", "25%", "0%"].map((l) => (
                    <span key={l}>{l}</span>
                ))}
            </div>
        </div>
    );
}

export function CurveEditor() {
    const {config, device, setMode, updateConfig} = useApp();
    const {latest} = useTelemetry();
    const fans = device?.fans ?? [];
    const [fanId, setFanId] = useState<string | null>(null);
    const activeFanId = fanId ?? fans[0]?.id ?? null;

    const savedPoints = useMemo(() => {
        if (!activeFanId) return defaultPoints();
        const saved = config?.curves[activeFanId];
        return saved && saved.length >= 2 ? saved : defaultPoints();
    }, [config, activeFanId]);

    const [points, setPoints] = useState<CurvePoint[]>(savedPoints);
    const [dirty, setDirty] = useState(false);
    const [saving, setSaving] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [strategy, setStrategy] = useState<"adaptive" | "curve">("adaptive");
    const [targetTempC, setTargetTempC] = useState(70);
    useEffect(() => {
        if (!config) return;
        setStrategy(config.automaticControl.strategy);
        setTargetTempC(config.automaticControl.targetTempC);
    }, [config?.automaticControl]);
    const settingsDirty = !!config && (strategy !== config.automaticControl.strategy || targetTempC !== config.automaticControl.targetTempC);

    useEffect(() => {
        setPoints(savedPoints);
        setDirty(false);
    }, [savedPoints]);

    const fanIndex = fans.findIndex((f) => f.id === activeFanId);
    const refTemp =
        latest && fanIndex >= 0
            ? (latest.temps[fanIndex]?.celsius ??
                (latest.temps.length
                    ? Math.max(...latest.temps.map((t) => t.celsius))
                    : null))
            : null;

    const apply = async () => {
        if (!config || (strategy === "curve" && !activeFanId)) return;
        setSaving(true);
        setError(null);
        try {
            await updateConfig({
                automaticControl: {strategy, targetTempC},
                ...(strategy === "curve" && activeFanId ? {curves: {...config.curves, [activeFanId]: points}} : {}),
            });
            await setMode("auto");
            setDirty(false);
        } catch (e) {
            setError(isThermalError(e) ? e.message : String(e));
        } finally {
            setSaving(false);
        }
    };

    const controlActive = config?.mode === "auto" && config.automaticControl.strategy === strategy;

    return (
        <div className="flex flex-col gap-6">
            <PageHeader
                title="Automatic Control"
                subtitle="Choose how automatic cooling responds to heat"
                action={
                    <div className="flex gap-2">
                        <Button
                            variant="outline"
                            disabled={saving || (!dirty && !settingsDirty)}
                            onClick={() => {
                                setPoints(savedPoints);
                                setDirty(false);
                                if (config) {
                                    setStrategy(config.automaticControl.strategy);
                                    setTargetTempC(config.automaticControl.targetTempC);
                                }
                            }}
                        >
                            Revert
                        </Button>
                        <Button onClick={() => void apply()} disabled={saving || !config || !activeFanId || !device?.writeAccess}>
                            {saving
                                ? "Applying…"
                                : controlActive && !dirty && !settingsDirty
                                    ? "Auto mode active"
                                    : "Apply to Auto mode"}
                        </Button>
                    </div>
                }
            />
            <PermissionBanner/>
            {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
            <Card className="flex flex-wrap items-center justify-between gap-4 p-5">
                <div>
                    <h2 className="text-sm font-semibold">Regulation method</h2>
                    <p className="mt-1 text-xs text-muted-foreground">Target temperature is recommended. Saved curves remain available.</p>
                </div>
                <fieldset aria-label="Automatic regulation method" className="flex flex-wrap gap-1 rounded-lg bg-secondary p-1">
                    {([
                        {value: "adaptive", label: "Target temperature"},
                        {value: "curve", label: "Advanced curves"},
                    ] as const).map((option) => (
                        <label key={option.value} className="cursor-pointer">
                            <input
                                type="radio"
                                name="automatic-strategy"
                                value={option.value}
                                checked={strategy === option.value}
                                onChange={() => setStrategy(option.value)}
                                disabled={saving || !config}
                                className="peer sr-only"
                            />
                            <span className="block rounded-md px-3 py-1.5 text-xs font-medium text-muted-foreground transition-colors peer-checked:bg-background peer-checked:text-foreground peer-focus-visible:ring-2 peer-focus-visible:ring-ring/40 peer-disabled:opacity-50">
                                {option.label}
                            </span>
                        </label>
                    ))}
                </fieldset>
            </Card>
            {strategy === "adaptive" ? (
                <Card className="p-6">
                    <div className="flex items-start justify-between gap-4">
                        <div>
                            <h2 className="text-sm font-semibold">Target temperature</h2>
                            <p className="mt-1 max-w-lg text-xs leading-relaxed text-muted-foreground">
                                Automatically adjusts fan boost to keep the system near your chosen temperature.
                            </p>
                        </div>
                        <span className="shrink-0 font-mono text-lg text-primary">{formatTemp(targetTempC, config?.tempUnit ?? "celsius")}</span>
                    </div>
                    <input
                        aria-label="Target temperature in Celsius"
                        type="range"
                        min="50"
                        max="85"
                        step="1"
                        value={targetTempC}
                        disabled={saving || !config}
                        onChange={(e) => setTargetTempC(Number(e.target.value))}
                        className="range-control mt-6 w-full"
                    />
                    <div className="mt-2 flex justify-between text-xs text-muted-foreground">
                        <span>{formatTemp(50, config?.tempUnit ?? "celsius")} · More cooling</span>
                        <span>{formatTemp(85, config?.tempUnit ?? "celsius")} · Less boost</span>
                    </div>
                </Card>
            ) : <>
                {fans.length > 1 && (
                    <div className="flex items-center gap-1 self-start rounded-lg bg-secondary p-1">
                        {fans.map((f) => (
                            <button
                                key={f.id}
                                type="button"
                                onClick={() => setFanId(f.id)}
                                className={cn(
                                    "rounded-md px-3 py-1.5 text-xs font-medium",
                                    f.id === activeFanId ? "bg-background" : "text-muted-foreground",
                                )}
                            >
                                {f.label}
                            </button>
                        ))}
                    </div>
                )}
                <Card className="p-6">
                    <div className="mb-8 flex items-start justify-between">
                        <div>
                            <h2 className="text-sm font-semibold">
                                {fans.find((f) => f.id === activeFanId)?.label ?? "No fan"}
                            </h2>
                            <p className="mt-1 text-xs text-muted-foreground">
                                Drag the points to adjust the response
                            </p>
                        </div>
                        <div className="text-right">
                            <p className="font-mono text-sm font-medium text-primary">
                                {refTemp !== null && config
                                    ? `${formatTemp(refTemp, config.tempUnit)} → ${evaluate(points, refTemp)}%`
                                    : "—"}
                            </p>
                            <p className="mt-1 text-xs text-muted-foreground">Current response</p>
                        </div>
                    </div>
                    <div className="ml-9 pb-7">
                        <CurveGraph
                            points={points}
                            disabled={!device?.writeAccess}
                            onChange={(next) => {
                                setPoints(next);
                                setDirty(true);
                            }}
                        />
                    </div>
                </Card>
            </>}
        </div>
    );
}
