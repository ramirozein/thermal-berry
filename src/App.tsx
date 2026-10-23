import {useState} from "react";
import {AppProvider, useApp} from "./lib/app-context";
import {Sidebar} from "./components/Sidebar";
import {Dashboard} from "./screens/Dashboard";
import {ManualControl} from "./screens/ManualControl";
import {CurveEditor} from "./screens/CurveEditor";
import {Settings} from "./screens/Settings";
import {useTelemetry} from "./lib/telemetry";

export type Screen = "dashboard" | "manual" | "curve" | "settings";

function ControlFeedback() {
    const {actionError, dismissError} = useApp();
    const {latest, ageMs, isStale, control} = useTelemetry();
    const errors = [...new Set([
        actionError,
        control?.error,
        ...(latest?.errors ?? []),
        latest && isStale ? `Telemetry is stale. Last reading ${Math.floor(ageMs / 1000)} seconds ago.` : null,
        control?.maintenance ? "Maintenance in progress. Fan control and telemetry persistence are paused." : null,
        control?.suspended ? "Fan control is suspended. Select a mode to retry." : null,
    ].filter((error): error is string => !!error))];
    if (!errors.length) return null;
    return <div role="alert" className="mb-6 rounded-xl border border-destructive/40 p-4 text-sm text-destructive">
        {errors.map((error) => <p key={error}>{error}</p>)}
        {actionError && <button className="mt-2 underline" onClick={dismissError}>Dismiss action error</button>}
    </div>;
}

export default function App() {
    const [screen, setScreen] = useState<Screen>("dashboard");

    return (
        <AppProvider>
            <main className="flex h-screen bg-background text-foreground">
                <Sidebar screen={screen} onNavigate={setScreen}/>
                <div className="min-w-0 flex-1 overflow-y-auto">
                    <div className="mx-auto max-w-5xl p-5 sm:p-8">
                        <ControlFeedback/>
                        {screen === "dashboard" && <Dashboard/>}
                        {screen === "manual" && <ManualControl/>}
                        {screen === "curve" && <CurveEditor/>}
                        {screen === "settings" && <Settings/>}
                    </div>
                </div>
            </main>
        </AppProvider>
    );
}
