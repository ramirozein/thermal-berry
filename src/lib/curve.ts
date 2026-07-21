import type {CurvePoint} from "./ipc";

/** Mirror of default_points() in src-tauri/src/curve.rs */
export function defaultPoints(): CurvePoint[] {
    return [
        {tempC: 40, percent: 0},
        {tempC: 60, percent: 25},
        {tempC: 75, percent: 55},
        {tempC: 85, percent: 80},
        {tempC: 95, percent: 100},
    ];
}

/** Mirror of evaluate() in Rust: linear interpolation with saturation. */
export function evaluate(points: CurvePoint[], tempC: number): number {
    if (points.length === 0) return 0;
    const sorted = [...points].sort((a, b) => a.tempC - b.tempC);
    const first = sorted[0];
    const last = sorted[sorted.length - 1];
    if (tempC <= first.tempC) return first.percent;
    if (tempC >= last.tempC) return last.percent;
    for (let i = 1; i < sorted.length; i++) {
        const a = sorted[i - 1];
        const b = sorted[i];
        if (tempC <= b.tempC) {
            const span = b.tempC - a.tempC;
            if (span === 0) return b.percent;
            const t = (tempC - a.tempC) / span;
            return Math.round(a.percent + t * (b.percent - a.percent));
        }
    }
    return last.percent;
}
