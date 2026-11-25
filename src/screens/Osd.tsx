import {useLayoutEffect, useRef} from "react";
import {invoke} from "@tauri-apps/api/core";
import {currentTemperatures, systemMetrics, useOsdMetrics} from "../lib/metrics";
import {formatTemp} from "../lib/utils";

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
    const {cpu, ram, ramDetail} = systemMetrics(snapshot, now);
    const temperatures = currentTemperatures(snapshot, now);
    return (
        <main ref={panel} aria-label="Thermal Berry readings"
              onMouseDown={(event) => {
                  if (event.button === 0) {
                      event.preventDefault();
                      void invoke("drag_osd").catch(console.error);
                  }
              }}
              className="osd-glass w-max cursor-move select-none rounded-xl px-2.5 py-1.5 font-mono text-xs leading-4 text-white">
            <div className="grid grid-cols-[max-content_max-content] gap-x-3 gap-y-0.5">
                <span>CPU <strong>{cpu}</strong></span>
                <span>RAM <strong>{ram}</strong></span>
                {ramDetail && <span className="col-span-2 text-[11px] text-white/85">{ramDetail}</span>}
                {temperatures.map(({id, label, celsius}) => (
                    <span key={id} className="max-w-40 truncate" title={label}>{label}{" "}
                        <strong>{formatTemp(celsius, snapshot?.tempUnit ?? "celsius")}</strong></span>
                ))}
            </div>
            {!temperatures.length && <p className="mt-1 text-[11px] text-white/85">Temperatures not available</p>}
        </main>
    );
}
