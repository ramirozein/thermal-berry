export type HistoryPoint = {timestampMs: number; value: number | null};

export const HISTORY_CHART_WIDTH = 360;
export const HISTORY_CHART_HEIGHT = 100;
const VERTICAL_PADDING = 6;

export function buildHistoryPath(
    series: HistoryPoint[],
    endMs: number,
    spanSecs: number,
    min: number,
    max: number,
) {
    const startMs = endMs - spanSecs * 1000;
    const visible = series.filter(({timestampMs}) => timestampMs >= startMs && timestampMs <= endMs);
    for (const {value} of visible) {
        if (value === null) continue;
        min = Math.min(min, value);
        max = Math.max(max, value);
    }

    const range = max - min || 1;
    const duration = endMs - startMs || 1;
    let path = "";
    let lastValue: number | null = null;
    let inSegment = false;
    for (const {timestampMs, value} of visible) {
        if (value === null) {
            inSegment = false;
            continue;
        }
        const x = (timestampMs - startMs) / duration * HISTORY_CHART_WIDTH;
        const y = HISTORY_CHART_HEIGHT - VERTICAL_PADDING
            - (value - min) / range * (HISTORY_CHART_HEIGHT - VERTICAL_PADDING * 2);
        const point = `${x.toFixed(1)} ${y.toFixed(1)}`;
        // A round cap on the zero-length segment makes an isolated reading visible.
        path += inSegment ? ` L${point}` : ` M${point} l0 0`;
        inSegment = true;
        lastValue = value;
    }
    return {path: path.trimStart(), lastValue};
}
