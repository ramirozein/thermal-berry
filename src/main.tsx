import {StrictMode} from "react";
import {createRoot} from "react-dom/client";
import {isTauri} from "@tauri-apps/api/core";
import {getCurrentWindow} from "@tauri-apps/api/window";
import "./styles/globals.css";

const isOsd = isTauri() && getCurrentWindow().label === "osd";
if (isOsd) document.documentElement.classList.add("osd");

const root = createRoot(document.getElementById("root") as HTMLElement);
const windowContent = isOsd
    ? import("./screens/Osd").then(({Osd}) => Osd)
    : import("./App").then(({default: App}) => App);

void windowContent.then((WindowContent) => {
    root.render(<StrictMode><WindowContent/></StrictMode>);
}).catch((error) => {
    console.error("Window failed to load:", error);
    root.render(<p role="alert" className="p-5 text-destructive">Unable to load Thermal Berry.</p>);
});
