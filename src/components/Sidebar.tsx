import {
    Activity,
    CircleGauge,
    Download,
    Laptop,
    LoaderCircle,
    Moon,
    Settings,
    SlidersHorizontal,
    Sun,
} from "lucide-react";
import {useState} from "react";
import {installUpdate, isThermalError} from "../lib/ipc";
import {cn} from "../lib/utils";
import {useConfig, useControlActions, useDevice, useGeneralConfig, useUpdateInfo} from "../lib/app-context";
import type {Screen} from "../App";

const navItems = [
    {id: "dashboard" as const, label: "Overview", icon: Activity},
    {id: "manual" as const, label: "Manual Control", icon: SlidersHorizontal, requiresFanControl: true},
    {id: "curve" as const, label: "Automatic Control", icon: CircleGauge, requiresFanControl: true},
    {id: "osd" as const, label: "On-Screen Display", icon: Download},
    {id: "settings" as const, label: "Settings", icon: Settings},
];

function BerryMark() {
    return (
        <img
            src="/favicon.png"
            alt=""
            className="size-8 shrink-0 rounded-xl object-cover"
        />
    );
}

export function Sidebar({screen, onNavigate,}: {
    screen: Screen;
    onNavigate: (screen: Screen) => void;
}) {
    const {capabilities, device} = useDevice();
    const config = useConfig();
    const generalConfig = useGeneralConfig();
    const {updateConfig} = useControlActions();
    const updateInfo = useUpdateInfo();
    const [updating, setUpdating] = useState(false);
    const [updateError, setUpdateError] = useState<string | null>(null);
    const handleUpdate = async () => {
        if (updating) return;
        setUpdating(true);
        setUpdateError(null);
        try {
            await installUpdate();
        } catch (error) {
            setUpdateError(isThermalError(error) ? error.message : String(error));
            setUpdating(false);
        }
    };
    const hostName = capabilities?.hostName;
    const deviceName = device?.model ?? hostName ?? "Computer name unavailable";

    return (
        <aside
            className="flex w-16 shrink-0 flex-col border-r border-sidebar-border bg-sidebar px-3 py-5 md:w-60 md:px-4">
            <div className="flex items-center gap-3 px-2">
                <BerryMark/>
                <div className="hidden md:block">
                    <p className="text-sm font-semibold tracking-tight">Thermal Berry</p>
                    <p className="mt-0.5 text-[10px] text-muted-foreground">by rzein</p>
                </div>
            </div>
            <nav className="mt-10 flex flex-1 flex-col gap-1" aria-label="Main navigation">
                {navItems.filter((item) => !item.requiresFanControl || capabilities?.fanControl).map(({id, label, icon: Icon}) => (
                    <button
                        key={id}
                        type="button"
                        onClick={() => onNavigate(id)}
                        aria-label={label}
                        aria-current={screen === id ? "page" : undefined}
                        className={cn(
                            "flex h-10 items-center gap-3 rounded-lg px-3 text-sm transition-colors",
                            screen === id
                                ? "bg-sidebar-accent font-medium text-sidebar-accent-foreground"
                                : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-foreground",
                        )}
                    >
                        <Icon className={cn("size-4 shrink-0", screen === id && "text-primary")}/>
                        <span className="hidden md:inline">{label}</span>
                    </button>
                ))}
            </nav>
            <div className="border-t border-sidebar-border pt-4">
                {updateInfo?.available && (
                    <button
                        type="button"
                        onClick={() => void handleUpdate()}
                        disabled={updating}
                        aria-label={updating ? "Downloading and installing update" : `Install v${updateInfo.latestVersion} and restart`}
                        aria-busy={updating}
                        title={updateError ?? (updating ? "Downloading and installing update…" : `Install v${updateInfo.latestVersion} and restart`)}
                        className="mb-1 flex h-10 w-full items-center gap-3 rounded-lg bg-primary/10 px-3 text-sm font-medium text-primary transition-colors hover:bg-primary/20 disabled:cursor-wait disabled:opacity-60"
                    >
                        {updating ? <LoaderCircle className="size-4 shrink-0 animate-spin"/> :
                            <Download className="size-4 shrink-0"/>}
                        <span className="hidden md:inline">
                            {updating ? "Updating…" : `Update to v${updateInfo.latestVersion}`}
                        </span>
                    </button>
                )}
                {updateError && (
                    <p role="alert" className="mb-2 break-words text-xs text-destructive md:px-3">
                        {updateError}
                    </p>
                )}
                {generalConfig && <button
                    type="button"
                    aria-label="Toggle light or dark theme"
                    onClick={() => void updateConfig({
                        theme: document.documentElement.classList.contains("dark") ? "light" : "dark",
                    }).catch(() => {
                    })}
                    className="flex h-10 w-full items-center gap-3 rounded-lg px-3 text-sm text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground"
                >
                    <Sun className="hidden size-4 shrink-0 dark:block"/>
                    <Moon className="size-4 shrink-0 dark:hidden"/>
                    <span className="hidden md:dark:inline">Light theme</span>
                    <span className="hidden md:inline dark:hidden">Dark theme</span>
                </button>}
                <div className="mt-2 flex items-center gap-3 px-1 py-2 md:px-3" title={deviceName}>
                    <div className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-secondary">
                        <Laptop className="size-3.5"/>
                    </div>
                    <div className="hidden min-w-0 md:block">
                        <p className="truncate text-[11px] font-medium">
                            {deviceName}
                        </p>
                        {device && config && <p className="text-[10px] text-muted-foreground">
                            {config.mode === "manual" ? "Manual" : config.mode === "disabled" ? "Disabled" : "Automatic"}
                        </p>}
                    </div>
                </div>
            </div>
        </aside>
    );
}
