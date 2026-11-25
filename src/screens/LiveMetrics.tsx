import {Card, PageHeader} from "../components/ui";
import {SystemMetricCards} from "../components/SystemMetricCards";
import {currentFanSpeeds, currentTemperatures, useMetrics} from "../lib/metrics";
import {formatTemp} from "../lib/utils";

const temperatureCards = [
    {id: "cpu", label: "CPU temperature"},
    {id: "gpu", label: "GPU temperature"},
];

export function LiveMetrics() {
    const {snapshot, now} = useMetrics();
    const temperatures = currentTemperatures(snapshot, now);
    const fans = currentFanSpeeds(snapshot, now);
    const hardware = snapshot?.hardwareModel;
    const waitingForTemperatures = !snapshot || snapshot.temperatures.status === "pending";
    return <div className="flex flex-col gap-6">
        <PageHeader title="Overview" subtitle={hardware?.model ?? "Live system readings"}/>
        <SystemMetricCards snapshot={snapshot} now={now}/>
        <div className="grid grid-cols-2 gap-4">
            {temperatureCards.map(({id, label}) => {
                const reading = temperatures.find((temperature) => temperature.id === id);
                return <Card key={id} className="p-5">
                    <p className="text-sm text-muted-foreground">{label}</p>
                    {reading
                        ? <p className="mt-3 font-mono text-2xl">{formatTemp(reading.celsius, snapshot?.tempUnit ?? "celsius")}</p>
                        : <p className="mt-3">{id === "gpu" && waitingForTemperatures ? "Waiting…" : "Not available"}</p>}
                </Card>;
            })}
            {temperatures.filter(({id}) => id !== "cpu" && id !== "gpu").map(({id, label, celsius}) => (
                <Card key={id} className="p-5">
                    <p className="text-sm text-muted-foreground">{label}</p>
                    <p className="mt-3 font-mono text-2xl">{formatTemp(celsius, snapshot?.tempUnit ?? "celsius")}</p>
                </Card>
            ))}
            {fans.map(({id, label, rpm}) => (
                <Card key={id} className="p-5">
                    <p className="text-sm text-muted-foreground">{label}</p>
                    <p className="mt-3 font-mono text-2xl">{rpm.toLocaleString()} RPM</p>
                </Card>
            ))}
            {snapshot?.fanSpeeds && fans.length === 0 && (
                <Card className="p-5">
                    <p className="text-sm text-muted-foreground">Fan speed</p>
                    <p className="mt-3">{snapshot.fanSpeeds.status === "pending" ? "Waiting…" : "Not available"}</p>
                </Card>
            )}
        </div>
    </div>;
}
