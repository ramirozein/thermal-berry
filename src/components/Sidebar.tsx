import {
    Activity,
    CircleGauge,
    Laptop,
    Moon,
    Settings,
    SlidersHorizontal,
    Sun,
} from "lucide-react";
import {cn} from "../lib/utils";
import {useApp} from "../lib/app-context";
import type {Screen} from "../App";
import logo from "../assets/logo.jpeg";

const navItems = [
    {id: "dashboard" as const, label: "Overview", icon: Activity},
    {id: "manual" as const, label: "Manual Control", icon: SlidersHorizontal},
    {id: "curve" as const, label: "Curves", icon: CircleGauge},
    {id: "settings" as const, label: "Settings", icon: Settings},
];

function BerryMark() {
    return (
        <img
            src={logo}
            alt=""
            className="size-8 shrink-0 rounded-xl object-cover dark:invert"
        />
    );
}

export function Sidebar({
                            screen,
                            onNavigate,
                        }: {
    screen: Screen;
    onNavigate: (screen: Screen) => void;
}) {
    const {config, device, updateConfig} = useApp();
    const isDark =
        config?.theme === "dark" ||
        (config?.theme === "system" &&
            window.matchMedia("(prefers-color-scheme: dark)").matches);

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
                {navItems.map(({id, label, icon: Icon}) => (
                    <button
                        key={id}
                        type="button"
                        onClick={() => onNavigate(id)}
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
                <button
                    type="button"
                    onClick={() => void updateConfig({theme: isDark ? "light" : "dark"})}
                    className="flex h-10 w-full items-center gap-3 rounded-lg px-3 text-sm text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground"
                >
                    {isDark ? <Sun className="size-4"/> : <Moon className="size-4"/>}
                    <span className="hidden md:inline">
            {isDark ? "Light theme" : "Dark theme"}
          </span>
                </button>
                <div className="mt-2 hidden items-center gap-3 px-3 py-2 md:flex">
                    <div className="flex size-7 shrink-0 items-center justify-center rounded-lg bg-secondary">
                        <Laptop className="size-3.5"/>
                    </div>
                    <div className="min-w-0">
                        <p className="truncate text-[11px] font-medium">
                            {device?.model ?? "No device"}
                        </p>
                        <p className="text-[10px] text-muted-foreground">
                            {device
                                ? config?.mode === "manual"
                                    ? "Manual"
                                    : config?.mode === "disabled"
                                        ? "Disabled"
                                        : "Automatic"
                                : "Not detected"}
                        </p>
                    </div>
                </div>
            </div>
        </aside>
    );
}
