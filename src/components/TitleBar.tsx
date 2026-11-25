import {useEffect, useState} from "react";
import {getCurrentWindow} from "@tauri-apps/api/window";
import {Maximize2, Minimize2, Minus, X} from "lucide-react";

const controlClass = "grid size-6 place-items-center rounded-full bg-neutral-200 hover:bg-neutral-300 focus-visible:ring-inset dark:bg-[#303030] dark:hover:bg-neutral-600";

export function TitleBar() {
    const [maximized, setMaximized] = useState(false);
    const maximizeLabel = maximized ? "Restore window" : "Maximize window";
    const MaximizeIcon = maximized ? Minimize2 : Maximize2;

    useEffect(() => {
        const appWindow = getCurrentWindow();
        let active = true;
        let timer: ReturnType<typeof setTimeout>;
        const refresh = () => {
            void appWindow.isMaximized().then((value) => {
                if (active) setMaximized(value);
            }).catch(console.error);
        };
        const onResize = () => {
            clearTimeout(timer);
            timer = setTimeout(refresh, 100);
        };

        refresh();
        window.addEventListener("resize", onResize);
        return () => {
            active = false;
            clearTimeout(timer);
            window.removeEventListener("resize", onResize);
        };
    }, []);

    return (
        <header className="relative flex h-[34px] shrink-0 select-none items-center border-b border-black/5 bg-neutral-100 text-neutral-900 dark:border-white/5 dark:bg-[#1b1b1b] dark:text-[#e3e3e3]">
            <div data-tauri-drag-region className="absolute inset-0 grid place-items-center">
                <span className="pointer-events-none text-sm font-semibold">Thermal Berry</span>
            </div>
            <div role="group" aria-label="Window controls" className="relative ml-auto flex gap-4 pr-2.5">
                <button type="button" aria-label="Minimize window" title="Minimize window"
                        className={controlClass}
                        onClick={() => void getCurrentWindow().minimize().catch(console.error)}>
                    <Minus className="size-3" strokeWidth={3} aria-hidden="true"/>
                </button>
                <button type="button" aria-label={maximizeLabel} title={maximizeLabel}
                        className={controlClass}
                        onClick={() => void getCurrentWindow().toggleMaximize().catch(console.error)}>
                    <MaximizeIcon className="size-3" strokeWidth={2.5} aria-hidden="true"/>
                </button>
                <button type="button" aria-label="Close window" title="Close window"
                        className={controlClass}
                        onClick={() => void getCurrentWindow().close().catch(console.error)}>
                    <X className="size-3.5" strokeWidth={3} aria-hidden="true"/>
                </button>
            </div>
        </header>
    );
}
