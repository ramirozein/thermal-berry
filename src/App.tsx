import {useState} from "react";
import {AppProvider} from "./lib/app-context";
import {Sidebar} from "./components/Sidebar";
import {Dashboard} from "./screens/Dashboard";
import {ManualControl} from "./screens/ManualControl";
import {CurveEditor} from "./screens/CurveEditor";
import {Settings} from "./screens/Settings";

export type Screen = "dashboard" | "manual" | "curve" | "settings";

export default function App() {
    const [screen, setScreen] = useState<Screen>("dashboard");

    return (
        <AppProvider>
            <main className="flex h-screen bg-background text-foreground">
                <Sidebar screen={screen} onNavigate={setScreen}/>
                <div className="min-w-0 flex-1 overflow-y-auto">
                    <div className="mx-auto max-w-5xl p-5 sm:p-8">
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
