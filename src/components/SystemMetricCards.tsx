import {systemMetrics, type MetricsSnapshot} from "../lib/metrics";
import {Card} from "./ui";

export function SystemMetricCards({snapshot, now}: {snapshot: MetricsSnapshot | null; now: number}) {
    const {cpu, ram, ramDetail} = systemMetrics(snapshot, now);
    return <div className="grid grid-cols-2 gap-4">
        <Card className="p-5">
            <p className="text-sm text-muted-foreground">CPU usage</p>
            <p className="mt-3 font-mono text-2xl">{cpu}</p>
        </Card>
        <Card className="p-5">
            <p className="text-sm text-muted-foreground">RAM usage</p>
            <p className="mt-3 font-mono text-2xl">{ram}</p>
            {ramDetail && <p className="mt-1 text-xs text-muted-foreground">{ramDetail}</p>}
        </Card>
    </div>;
}
