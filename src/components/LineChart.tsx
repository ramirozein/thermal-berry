import {useMemo} from "react";

/**
 * Minimalist SVG chart fed by the real telemetry history.
 * `series` is a list of values (oldest → most recent); gaps
 * (null) are skipped from the stroke.
 */
export function LineChart({
                              series,
                              min,
                              max,
                              spanSecs,
                              formatValue,
                          }: {
    series: (number | null)[];
    /** Floor/ceiling of the Y axis; expand if the data exceeds them. */
    min: number;
    max: number;
    /** Seconds covered by the full buffer, for the X axis labels. */
    spanSecs: number;
    formatValue?: (v: number) => string;
}) {
    const W = 360;
    const H = 100;
    const PAD_Y = 6;

    const {path, lastValue} = useMemo(() => {
        const values = series.filter((v): v is number => v !== null);
        if (values.length === 0) return {path: "", lastValue: null};
        const lo = Math.min(min, ...values);
        const hi = Math.max(max, ...values);
        const range = hi - lo || 1;
        // Partial buffer: the line grows from the left until it fills up.
        const n = series.length;
        const step = n > 1 ? W / (n - 1) : 0;
        let d = "";
        series.forEach((v, i) => {
            if (v === null) return;
            const x = n > 1 ? i * step : W;
            const y = H - PAD_Y - ((v - lo) / range) * (H - PAD_Y * 2);
            d += d === "" ? `M${x.toFixed(1)} ${y.toFixed(1)}` : ` L${x.toFixed(1)} ${y.toFixed(1)}`;
        });
        return {path: d, lastValue: values[values.length - 1]};
    }, [series, min, max]);

    const axisLabels = useMemo(() => {
        const marks = [1, 2 / 3, 1 / 3, 0];
        return marks.map((m) => {
            const secs = Math.round(spanSecs * m);
            return secs === 0 ? "now" : `${secs}s`;
        });
    }, [spanSecs]);

    return (
        <div className="relative h-32 w-full overflow-hidden" role="img" aria-label="history">
            <div className="absolute inset-0 flex flex-col justify-between py-2" aria-hidden="true">
                {[0, 1, 2, 3].map((line) => (
                    <div key={line} className="border-t border-border/70"/>
                ))}
            </div>
            <svg
                className="absolute inset-0 size-full overflow-visible"
                viewBox={`0 0 ${W} ${H}`}
                preserveAspectRatio="none"
                aria-hidden="true"
            >
                {path && (
                    <path
                        d={path}
                        fill="none"
                        stroke="var(--primary)"
                        strokeWidth="2"
                        vectorEffect="non-scaling-stroke"
                    />
                )}
            </svg>
            {lastValue === null && (
                <p className="absolute inset-0 flex items-center justify-center text-xs text-muted-foreground">
                    Waiting for data…
                </p>
            )}
            {lastValue !== null && formatValue && (
                <span className="absolute right-0 top-0 font-mono text-xs text-primary">
          {formatValue(lastValue)}
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
