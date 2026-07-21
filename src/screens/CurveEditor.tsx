import {useEffect, useMemo, useRef, useState} from "react";
import {Button, Card, PageHeader} from "../components/ui";
import {PermissionBanner} from "../components/PermissionBanner";
import {saveCurve, type CurvePoint} from "../lib/ipc";
import {defaultPoints, evaluate} from "../lib/curve";
import {formatTemp, useApp} from "../lib/app-context";
import {useTelemetry} from "../lib/telemetry";
import {cn} from "../lib/utils";

const TEMP_MIN = 30;
const TEMP_MAX = 100;
const VIEW_W = 600;
const VIEW_H = 280;

const toX = (tempC: number) =>
    ((tempC - TEMP_MIN) / (TEMP_MAX - TEMP_MIN)) * VIEW_W;
const toY = (percent: number) => VIEW_H - (percent / 100) * VIEW_H;

// Keeps a dragged point within its neighbors (so the curve stays
// monotonic on X) and within [0, 100] on Y.
function clampPoint(sorted: CurvePoint[], index: number, tempC: number, percent: number): CurvePoint {
    const lo = index > 0 ? sorted[index - 1].tempC + 1 : TEMP_MIN;
    const hi = index < sorted.length - 1 ? sorted[index + 1].tempC - 1 : TEMP_MAX;
    return {
        tempC: Math.round(Math.min(hi, Math.max(lo, tempC))),
        percent: Math.round(Math.min(100, Math.max(0, percent))),
    };
}

function CurveGraph({
                        points,
                        onChange,
                        disabled,
                    }: {
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
        // Extends the flat curve to the edges of the domain.
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
        const tempC =
            TEMP_MIN + ((clientX - rect.left) / rect.width) * (TEMP_MAX - TEMP_MIN);
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
        // eslint-disable-next-line react-hooks/exhaustive-deps
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
    const {config, device, setMode} = useApp();
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

    // When the fan changes (or config arrives), reload the saved points.
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
        if (!activeFanId) return;
        setSaving(true);
        try {
            await saveCurve(activeFanId, points);
            await setMode("auto");
            setDirty(false);
        } catch (e) {
            console.error(e);
        } finally {
            setSaving(false);
        }
    };

    const curveActive = config?.mode === "auto";

    return (
        <div className="flex flex-col gap-6">
            <PageHeader
                title="Curves"
                subtitle="Define what Auto mode does at each temperature"
                action={
                    <div className="flex gap-2">
                        <Button
                            variant="outline"
                            disabled={!dirty}
                            onClick={() => {
                                setPoints(savedPoints);
                                setDirty(false);
                            }}
                        >
                            Revert
                        </Button>
                        <Button onClick={() => void apply()} disabled={saving || !activeFanId}>
                            {saving
                                ? "Applying…"
                                : curveActive && !dirty
                                    ? "Auto mode active"
                                    : "Apply to Auto mode"}
                        </Button>
                    </div>
                }
            />
            <PermissionBanner/>
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
        </div>
    );
}
