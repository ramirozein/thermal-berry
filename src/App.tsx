import {lazy, Suspense, useState} from "react";
import {AppProvider, useActionFeedback, useDevice} from "./lib/app-context";
import {Button, Card, PageHeader} from "./components/ui";
import {Sidebar} from "./components/Sidebar";
import {Dashboard} from "./screens/Dashboard";
import {LiveMetrics} from "./screens/LiveMetrics";
import {useTelemetry} from "./lib/telemetry";

const ManualControl = lazy(() => import("./screens/ManualControl").then(({ManualControl}) => ({default: ManualControl})));
const CurveEditor = lazy(() => import("./screens/CurveEditor").then(({CurveEditor}) => ({default: CurveEditor})));
const Settings = lazy(() => import("./screens/Settings").then(({Settings}) => ({default: Settings})));
const OsdSettings = lazy(() => import("./components/OsdSettings").then(({OsdSettings}) => ({default: OsdSettings})));

export type Screen = "dashboard" | "manual" | "curve" | "osd" | "settings";

function ControlFeedback() {
    const {actionError, dismissError} = useActionFeedback();
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

function AppContent() {
    const [screen, setScreen] = useState<Screen>("dashboard");
    const {capabilities, deviceError, refreshDevice} = useDevice();

    if (!capabilities) {
        return (
            <main className="min-h-screen bg-background p-8 text-foreground">
                <div className="mx-auto flex max-w-5xl flex-col gap-6">
                    <PageHeader title="Thermal Berry" subtitle="Hardware monitoring"/>
                    <Card className="p-5 text-sm text-muted-foreground">
                        {deviceError ? <div role="alert" className="flex items-center justify-between gap-4">
                                <span>{deviceError.message}</span>
                                <Button onClick={() => void refreshDevice()}>Retry</Button>
                            </div> : "Checking hardware capabilities…"}
                    </Card>
                </div>
            </main>
        );
    }

    const activeScreen = !capabilities.fanControl && (screen === "manual" || screen === "curve")
        ? "dashboard" : screen;

    return (
        <main className="flex h-screen bg-background text-foreground">
            <Sidebar screen={activeScreen} onNavigate={setScreen}/>
            <div className="min-w-0 flex-1 overflow-y-auto">
                <div className="mx-auto max-w-5xl p-5 sm:p-8">
                    {capabilities.linuxMaintenance && <ControlFeedback/>}
                    <Suspense fallback={<p role="status" className="text-sm text-muted-foreground">Loading…</p>}>
                        {activeScreen === "dashboard" && (capabilities.linuxMaintenance ? <Dashboard/> : <LiveMetrics/>)}
                        {activeScreen === "manual" && <ManualControl/>}
                        {activeScreen === "curve" && <CurveEditor/>}
                        {activeScreen === "osd" && <div className="flex flex-col gap-6">
                            <PageHeader title="On-Screen Display" subtitle="Configure the live readings overlay"/>
                            <OsdSettings/>
                        </div>}
                        {activeScreen === "settings" && capabilities.linuxMaintenance && <Settings/>}
                    </Suspense>
                </div>
            </div>
        </main>
    );
}

export default function App() {
    return <AppProvider><AppContent/></AppProvider>;
}
