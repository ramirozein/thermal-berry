import {Card, PageHeader} from "../components/ui";
import {SystemMetricCards} from "../components/SystemMetricCards";
import {currentTemperatures, useMetrics} from "../lib/metrics";

export function LiveMetrics() {
    const {snapshot, now} = useMetrics();
    const temperatures = currentTemperatures(snapshot, now);
    return <div className="flex flex-col gap-6">
        <PageHeader title="Overview" subtitle="Live system readings"/>
        <SystemMetricCards snapshot={snapshot} now={now}/>
        <div className="grid grid-cols-2 gap-4">
            {temperatures.length ? temperatures.map(({id, label, celsius}) => (
                <Card key={id} className="p-5"><p className="text-sm text-muted-foreground">{label}</p><p className="mt-3 font-mono text-2xl">{Math.round(celsius)} °C</p></Card>
            )) : ["CPU temperature", "GPU temperature"].map((label) => (
                <Card key={label} className="p-5"><p className="text-sm text-muted-foreground">{label}</p><p className="mt-3">Not available</p></Card>
            ))}
        </div>
    </div>;
}
