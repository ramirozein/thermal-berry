import {clsx, type ClassValue} from "clsx";
import {twMerge} from "tailwind-merge";
import type {TempUnit} from "./ipc";

export function cn(...inputs: ClassValue[]) {
    return twMerge(clsx(inputs));
}
export function formatTemp(celsius: number, unit: TempUnit): string {
    if (unit === "fahrenheit") return `${Math.round(celsius * 1.8 + 32)}°F`;
    return `${Math.round(celsius)}°C`;
}
