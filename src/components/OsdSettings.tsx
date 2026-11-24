import {useEffect, useState} from "react";
import {invoke} from "@tauri-apps/api/core";
import {listen} from "@tauri-apps/api/event";
import {Button, Card, Select, SettingRow, Toggle} from "./ui";

type Corner = "topLeft" | "topRight" | "bottomLeft" | "bottomRight";
type Preferences = {
    enabled: boolean;
    monitor: string | null;
    corner: Corner;
    alwaysOnTop: boolean;
    clickThrough: boolean;
};

type Settings = {
    preferences: Preferences;
    monitors: {id: string; name: string}[];
    trayAvailable: boolean;
    positionSupported: boolean;
    alwaysOnTopSupported: boolean;
    clickThroughSupported: boolean;
};

export function OsdSettings() {
    const [settings, setSettings] = useState<Settings | null>(null);
    const [pending, setPending] = useState(false);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        let mounted = true;
        const changes = listen<Settings>("osd-settings-changed", ({payload}) => {
            if (mounted) setSettings(payload);
        });
        const errors = listen<string>("osd-error", ({payload}) => {
            if (mounted) setError(payload);
        });
        void changes.then(() => invoke<Settings>("get_osd_settings"))
            .then((result) => { if (mounted) setSettings(result); })
            .catch((reason) => { if (mounted) setError(String(reason)); });
        return () => {
            mounted = false;
            void changes.then((stop) => stop(), () => {});
            void errors.then((stop) => stop(), () => {});
        };
    }, []);
    const apply = async (patch: Partial<Preferences>) => {
        setError(null);
        setPending(true);
        try {
            setSettings(await invoke<Settings>("set_osd_settings", {patch}));
        } catch (reason) {
            setError(String(reason));
        } finally {
            setPending(false);
        }
    };
    const preferences = settings?.preferences;
    const monitor = preferences?.monitor ?? "";
    const monitorOptions = [{value: "", label: "Primary display"}, ...(settings?.monitors.map(({id, name}) => ({value: id, label: name})) ?? [])];
    if (monitor && !monitorOptions.some(({value}) => value === monitor)) monitorOptions.push({value: monitor, label: "Preferred display disconnected"});

    return <Card className="overflow-hidden">
        <div className="px-5 py-3 text-xs font-semibold text-muted-foreground">GENERAL</div>
        <SettingRow title="On-screen display" description="Show live readings in a separate window">
            <Toggle label="On-screen display" on={preferences?.enabled ?? false} disabled={!preferences || pending} onChange={(enabled) => void apply({enabled})}/>
        </SettingRow>
        {preferences && <fieldset disabled={pending}>
            {settings.positionSupported ? <>
                <SettingRow title="Display">
                    <Select label="Display" value={monitor} options={monitorOptions} onChange={(value) => void apply({monitor: value})}/>
                </SettingRow>
                <SettingRow title="Position" description="Pick a corner or drag the overlay to place it freely.">
                    <Select<Corner> label="Position" value={preferences.corner} options={[
                        {value: "topRight", label: "Top right"}, {value: "topLeft", label: "Top left"},
                        {value: "bottomRight", label: "Bottom right"}, {value: "bottomLeft", label: "Bottom left"},
                    ]} onChange={(corner) => void apply({corner})}/>
                </SettingRow>
            </> : null}
            {settings.alwaysOnTopSupported && <SettingRow title="Always on top">
                <Toggle label="Always on top" on={settings.alwaysOnTopSupported && preferences.alwaysOnTop}
                        disabled={pending || !settings.alwaysOnTopSupported} onChange={(alwaysOnTop) => void apply({alwaysOnTop})}/>
            </SettingRow>}
            {settings.clickThroughSupported && <SettingRow title="Click-through" description="Turn this off here to drag the overlay.">
                <Toggle label="Click-through" on={settings.clickThroughSupported && preferences.clickThrough}
                        disabled={pending || !settings.clickThroughSupported} onChange={(clickThrough) => void apply({clickThrough})}/>
            </SettingRow>}
        </fieldset>}
        {settings && !settings.trayAvailable && <SettingRow title="Quit Thermal Berry"><Button onClick={() => void invoke("quit_app").catch((reason) => setError(String(reason)))}>Quit</Button></SettingRow>}
        {error && <p role="alert" className="px-5 py-3 text-sm text-destructive">{error}</p>}
    </Card>;
}
