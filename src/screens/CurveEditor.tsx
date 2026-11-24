import {useMemo, useState} from "react";
import {Button, Card, PageHeader} from "../components/ui";
import {PermissionBanner} from "../components/PermissionBanner";
import {isThermalError, type CurvePoint} from "../lib/ipc";
import {defaultPoints, evaluate} from "../lib/curve";
import {formatTemp, useConfig, useControlActions, useDevice} from "../lib/app-context";
import {useTelemetry} from "../lib/telemetry";
import {cn} from "../lib/utils";

const TEMP_MIN = 30;
const TEMP_MAX = 100;
const VIEW_W = 600;
const VIEW_H = 280;
const DEFAULT_POINTS = defaultPoints();

const toX = (tempC: number) => ((tempC - TEMP_MIN) / (TEMP_MAX - TEMP_MIN)) * VIEW_W;
const toY = (percent: number) => VIEW_H - (percent / 100) * VIEW_H;

function temperatureLimits(points: CurvePoint[], index: number) {
    return {
        min: Math.max(TEMP_MIN, Math.ceil((points[index - 1]?.tempC ?? TEMP_MIN - 1) + 1)),
        max: Math.min(TEMP_MAX, Math.floor((points[index + 1]?.tempC ?? TEMP_MAX + 1) - 1)),
    };
}

function CurveGraph({points, onChange, disabled,}: {
    points: CurvePoint[];
    onChange: (points: CurvePoint[]) => void;
    disabled: boolean;
}) {
    const [drag, setDrag] = useState<{index: number; pointerId: number} | null>(null);

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

    const updatePoint = (index: number, patch: Partial<CurvePoint>) => {
        const point = sorted[index];
        const {min, max} = temperatureLimits(sorted, index);
        const tempC = patch.tempC === undefined || min > max ? point.tempC
            : Math.min(max, Math.max(min, Math.round(patch.tempC)));
        const percent = patch.percent === undefined ? point.percent
            : Math.min(100, Math.max(0, Math.round(patch.percent)));
        if (tempC === point.tempC && percent === point.percent) return;
        onChange(sorted.map((p, i) => i === index ? {tempC, percent} : p));
    };

    return (
        <div className="relative mb-10 ml-9 h-72 border-b border-l border-border"
             role="group" aria-label="Temperature-to-boost curve">
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
                className="absolute inset-0 size-full overflow-visible"
                viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
                preserveAspectRatio="none"
                onPointerMove={(event) => {
                    if (disabled || drag?.pointerId !== event.pointerId) return;
                    const rect = event.currentTarget.getBoundingClientRect();
                    if (!rect.width || !rect.height) return;
                    updatePoint(drag.index, {
                        tempC: TEMP_MIN + ((event.clientX - rect.left) / rect.width) * (TEMP_MAX - TEMP_MIN),
                        percent: 100 - ((event.clientY - rect.top) / rect.height) * 100,
                    });
                }}
                onLostPointerCapture={(event) => {
                    if (drag?.pointerId === event.pointerId) setDrag(null);
                }}
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
                        tabIndex={disabled ? undefined : 0}
                        role="group"
                        aria-label={`Point ${i + 1}: ${p.tempC} degrees Celsius, ${p.percent}% boost`}
                        aria-keyshortcuts={disabled ? undefined : "ArrowLeft ArrowRight ArrowUp ArrowDown"}
                        className={cn(
                            !disabled && "touch-none cursor-grab focus-visible:stroke-foreground",
                            drag?.index === i && "cursor-grabbing",
                        )}
                        onKeyDown={(event) => {
                            if (disabled) return;
                            switch (event.key) {
                                case "ArrowLeft": updatePoint(i, {tempC: p.tempC - 1}); break;
                                case "ArrowRight": updatePoint(i, {tempC: p.tempC + 1}); break;
                                case "ArrowDown": updatePoint(i, {percent: p.percent - 1}); break;
                                case "ArrowUp": updatePoint(i, {percent: p.percent + 1}); break;
                                default: return;
                            }
                            event.preventDefault();
                        }}
                        onPointerDown={(event) => {
                            if (disabled || drag || event.button !== 0) return;
                            event.preventDefault();
                            event.currentTarget.setPointerCapture(event.pointerId);
                            setDrag({index: i, pointerId: event.pointerId});
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
    const config = useConfig();
    const {device} = useDevice();
    const {setMode, updateConfig} = useControlActions();
    const {latest, healthy, control} = useTelemetry();
    const fans = device?.fans ?? [];
    const [fanId, setFanId] = useState<string | null>(null);
    const fanIndex = Math.max(0, fans.findIndex((fan) => fan.id === fanId));
    const activeFan = fans[fanIndex];
    const activeFanId = activeFan?.id ?? null;
    const savedPoints = activeFanId ? config?.curves[activeFanId] : undefined;
    const [curveDraft, setCurveDraft] = useState<{fanId: string; points: CurvePoint[]} | null>(null);
    const activeDraft = curveDraft?.fanId === activeFanId ? curveDraft : null;
    const points = activeDraft?.points ?? (savedPoints && savedPoints.length >= 2 ? savedPoints : DEFAULT_POINTS);
    const dirty = activeDraft !== null;
    const [saving, setSaving] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [strategyDraft, setStrategyDraft] = useState<"adaptive" | "curve" | null>(null);
    const [targetDraft, setTargetDraft] = useState<number | null>(null);
    const strategy = strategyDraft ?? config?.automaticControl.strategy ?? "adaptive";
    const targetTempC = targetDraft ?? config?.automaticControl.targetTempC ?? 70;
    const settingsDirty = !!config && (strategy !== config.automaticControl.strategy || targetTempC !== config.automaticControl.targetTempC);
    const resetDraft = () => {
        setCurveDraft(null);
        setStrategyDraft(null);
        setTargetDraft(null);
    };

    const refTemp =
        latest && activeFan
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
                ...(strategy === "curve" && activeFanId ? {curves: {[activeFanId]: points}} : {}),
            });
            await setMode("auto");
            resetDraft();
        } catch (e) {
            setError(isThermalError(e) ? e.message : String(e));
        } finally {
            setSaving(false);
        }
    };

    const controlActive = healthy && control?.appliedMode === "auto" && config?.mode === "auto" && config.automaticControl.strategy === strategy;

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
                            onClick={resetDraft}
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
                        {value: "curve", label: "Curves"},
                    ] as const).map((option) => (
                        <label key={option.value} className="cursor-pointer">
                            <input
                                type="radio"
                                name="automatic-strategy"
                                value={option.value}
                                checked={strategy === option.value}
                                onChange={() => setStrategyDraft(option.value === config?.automaticControl.strategy ? null : option.value)}
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
                        onChange={(e) => {
                            const next = e.currentTarget.valueAsNumber;
                            setTargetDraft(next === config?.automaticControl.targetTempC ? null : next);
                        }}
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
                                disabled={saving}
                                aria-pressed={f.id === activeFanId}
                                onClick={() => {
                                    if (f.id === activeFanId) return;
                                    setFanId(f.id);
                                    setCurveDraft(null);
                                }}
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
                                {activeFan?.label ?? "No fan"}
                            </h2>
                            <p className="mt-1 text-xs text-muted-foreground">
                                Drag a point or focus it with Tab. Left and Right change temperature. Up and Down change boost.
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
                    <CurveGraph
                        key={activeFanId}
                        points={points}
                        disabled={saving || !config || !activeFanId || !device?.writeAccess}
                        onChange={(next) => {
                            if (activeFanId) setCurveDraft({fanId: activeFanId, points: next});
                        }}
                    />
                </Card>
            </>}
        </div>
    );
}
