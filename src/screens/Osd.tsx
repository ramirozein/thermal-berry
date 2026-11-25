import {useLayoutEffect, useRef} from "react";
import {invoke} from "@tauri-apps/api/core";
import {Cpu, Gpu} from "lucide-react";
import {currentTemperatures, systemMetrics, useOsdMetrics} from "../lib/metrics";
import {formatTemp} from "../lib/utils";

const temperatureSlots = [
    {id: "cpu", label: "CPU", Icon: Cpu},
    {id: "gpu", label: "GPU", Icon: Gpu},
];

function Usage({label, value, percent, detail}: {
    label: string;
    value: string;
    percent: number | null;
    detail?: string;
}) {
    return (
        <div className="grid grid-cols-1 grid-rows-[22px_15px_4px] gap-y-0.5">
            <div className="flex items-baseline justify-between gap-1 whitespace-nowrap text-[17px] leading-[22px]">
                <span>{label}</span>
                <strong className={percent === null ? "text-[10px] font-medium text-white/70" : "font-semibold"}>{value}</strong>
            </div>
            <span className="self-end justify-self-end whitespace-nowrap text-[10px] leading-[15px] text-white/70">{detail}</span>
            <div aria-hidden="true" className="overflow-hidden rounded-full bg-white/20">
                <div className="h-full rounded-full bg-primary" style={{width: `${percent ?? 0}%`}}/>
            </div>
        </div>
    );
}

export function Osd() {
    const {snapshot, now, ready} = useOsdMetrics();
    const panel = useRef<HTMLElement>(null);
    useLayoutEffect(() => {
        const element = panel.current;
        if (!element) return;
        const resize = () => {
            const bounds = element.getBoundingClientRect();
            void invoke("resize_osd", {
                width: Math.ceil(bounds.width),
                height: Math.ceil(bounds.height),
            }).catch(console.error);
        };
        resize();
        const observer = new ResizeObserver(resize);
        observer.observe(element, {box: "border-box"});
        return () => observer.disconnect();
    }, [ready]);
    if (!ready) return null;
    const {cpu, cpuPercent, ram, ramPercent, ramDetail} = systemMetrics(snapshot, now);
    const temperatures = currentTemperatures(snapshot, now);
    const primaryTemperatures = temperatureSlots.map((slot) => ({
        ...slot,
        reading: temperatures.find(({id}) => id === slot.id) ?? temperatures.find(({label}) => {
            const sensorLabel = label.trim().toLowerCase();
            return sensorLabel === slot.id || sensorLabel.endsWith(` · ${slot.id}`);
        }),
    }));
    const otherTemperatures = temperatures.filter((reading) =>
        !primaryTemperatures.some((slot) => slot.reading === reading),
    );
    const unit = snapshot?.tempUnit ?? "celsius";
    return (
        <main ref={panel} aria-label="Thermal Berry readings"
              onMouseDown={(event) => {
                  if (event.button === 0) {
                      event.preventDefault();
                      void invoke("drag_osd").catch(console.error);
                  }
              }}
              className="dark osd-glass w-[240px] cursor-move select-none overflow-hidden rounded-[12px] px-3 py-2.5 tabular-nums text-white">
            <div className="grid grid-cols-2 gap-x-4">
                <Usage label="CPU" value={cpu} percent={cpuPercent}/>
                <Usage label="RAM" value={ram} percent={ramPercent} detail={ramDetail}/>
            </div>
            <section aria-label="Temperatures" className="mt-3 grid grid-cols-2 gap-x-4 gap-y-1">
                {primaryTemperatures.map(({id, label, Icon, reading}) => (
                    <div key={id} className="flex min-w-0 items-center gap-1 whitespace-nowrap text-sm leading-5"
                         title={reading?.label ?? "Temperature not available"}>
                        <Icon aria-hidden="true" className="size-3.5 shrink-0 text-primary"/>
                        <span>{label}</span>
                        {reading
                            ? <strong className="font-medium text-primary">{formatTemp(reading.celsius, unit).replace("°", " °")}</strong>
                            : <span className="text-white/70"><span aria-hidden="true">—</span><span className="sr-only">Temperature not available</span></span>}
                    </div>
                ))}
                {otherTemperatures.map(({id, label, celsius}) => (
                    <div key={id} className="flex min-w-0 items-baseline gap-1 text-[10px] leading-4" title={label}>
                        <span className="truncate">{label}</span>
                        <strong className="shrink-0 font-medium text-primary">{formatTemp(celsius, unit)}</strong>
                    </div>
                ))}
            </section>
        </main>
    );
}
