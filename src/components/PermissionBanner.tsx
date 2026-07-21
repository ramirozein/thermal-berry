import {useState} from "react";
import {ShieldAlert} from "lucide-react";
import {Button} from "./ui";
import {installUdevRule, isThermalError} from "../lib/ipc";
import {useApp} from "../lib/app-context";

/**
 * Notice shown when fan_boost isn't writable: offers to install the udev
 * rule (a single password prompt via pkexec). Disappears once access is granted.
 */
export function PermissionBanner() {
    const {device, refreshDevice} = useApp();
    const [installing, setInstalling] = useState(false);
    const [error, setError] = useState<string | null>(null);

    if (!device || device.writeAccess) return null;

    const install = async () => {
        setInstalling(true);
        setError(null);
        try {
            await installUdevRule();
            await refreshDevice();
        } catch (e) {
            setError(isThermalError(e) ? e.message : String(e));
        } finally {
            setInstalling(false);
        }
    };

    return (
        <div className="rounded-xl border border-border bg-secondary/60 p-4 text-sm">
            <div className="flex items-start gap-3">
                <ShieldAlert className="mt-0.5 size-4 shrink-0 text-primary"/>
                <div className="flex-1">
                    <p className="leading-relaxed text-muted-foreground">
                        Fan control requires write permissions on{" "}
                        <code className="font-mono text-xs">/sys</code>. Install the udev rule once
                        (it will ask for your password) and the app will work without privileges.
                    </p>
                    {error && <p className="mt-2 text-xs text-destructive">{error}</p>}
                </div>
                <Button onClick={() => void install()} disabled={installing}>
                    {installing ? "Installing…" : "Enable control"}
                </Button>
            </div>
        </div>
    );
}
