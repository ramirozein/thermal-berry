import {useMemo} from "react";
import {buildHistoryPath, HISTORY_CHART_HEIGHT, HISTORY_CHART_WIDTH, type HistoryPoint} from "../lib/chart";

export function LineChart({label, series, endMs, min, max, spanSecs, formatValue}: {
    label: string;
    series: HistoryPoint[];
    endMs: number;
    min: number;
    max: number;
    spanSecs: number;
    formatValue?: (v: number) => string;
}) {
    const {path, lastValue} = useMemo(
        () => buildHistoryPath(series, endMs, spanSecs, min, max),
        [series, endMs, spanSecs, min, max],
    );
    const lastLabel = lastValue === null ? null : formatValue?.(lastValue) ?? String(lastValue);
    const axisLabels = [`${spanSecs}s`, `${Math.round(spanSecs * 2 / 3)}s`, `${Math.round(spanSecs / 3)}s`, "Latest"];

    return (
        <div className="relative h-32 w-full overflow-hidden" role="img"
             aria-label={`${label}. ${lastLabel === null ? "Waiting for data" : `Latest available value: ${lastLabel}. Window: ${spanSecs} seconds`}`}>
            <div className="absolute inset-0 flex flex-col justify-between py-2" aria-hidden="true">
                {[0, 1, 2, 3].map((line) => (
                    <div key={line} className="border-t border-border/70"/>
                ))}
            </div>
            <svg
                className="absolute inset-0 size-full overflow-visible"
                viewBox={`0 0 ${HISTORY_CHART_WIDTH} ${HISTORY_CHART_HEIGHT}`}
                preserveAspectRatio="none"
                aria-hidden="true"
            >
                {path && (
                    <path
                        d={path}
                        fill="none"
                        stroke="var(--primary)"
                        strokeWidth="2"
                        strokeLinecap="round"
                        vectorEffect="non-scaling-stroke"
                    />
                )}
            </svg>
            {lastValue === null && (
                <p className="absolute inset-0 flex items-center justify-center text-xs text-muted-foreground">
                    Waiting for data…
                </p>
            )}
            {lastLabel !== null && (
                <span className="absolute right-0 top-0 font-mono text-xs text-primary">
                    {lastLabel}
                </span>
            )}
            <div className="absolute bottom-0 left-0 right-0 flex justify-between text-[10px] text-muted-foreground">
                {axisLabels.map((label, i) => (
                    <span key={i}>{label}</span>
                ))}
            </div>
        </div>
    );
}
