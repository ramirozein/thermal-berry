import type {ButtonHTMLAttributes, KeyboardEventHandler, ReactNode} from "react";
import {cn} from "../lib/utils";

type ButtonVariant = "primary" | "outline" | "ghost";

export function Button({
                           variant = "primary",
                           className,
                           ...props
                       }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant }) {
    return (
        <button
            type="button"
            className={cn(
                "inline-flex h-8 items-center justify-center gap-2 rounded-lg px-3 text-xs font-medium transition-colors disabled:pointer-events-none disabled:opacity-50",
                variant === "primary" && "bg-primary text-primary-foreground hover:bg-primary/90",
                variant === "outline" && "border border-border bg-transparent hover:bg-secondary",
                variant === "ghost" && "hover:bg-secondary",
                className,
            )}
            {...props}
        />
    );
}

export function Toggle({
                           on,
                           onChange,
                           disabled,
                           label,
                       }: {
    on: boolean;
    onChange: (on: boolean) => void;
    disabled?: boolean;
    label?: string;
}) {
    return (
        <button
            type="button"
            role="switch"
            aria-checked={on}
            aria-label={label}
            disabled={disabled}
            onClick={() => onChange(!on)}
            className={cn(
                "flex h-6 w-11 shrink-0 items-center rounded-full p-0.5 transition-colors disabled:opacity-50",
                on ? "bg-primary" : "bg-muted-foreground/30",
            )}
        >
      <span
          className={cn(
              "size-5 rounded-full bg-background transition-transform",
              on && "translate-x-5",
          )}
      />
        </button>
    );
}

export function PageHeader({
                               title,
                               subtitle,
                               action,
                           }: {
    title: string;
    subtitle: string;
    action?: ReactNode;
}) {
    return (
        <header className="flex min-h-12 items-start justify-between gap-4">
            <div>
                <h1 className="text-balance text-2xl font-semibold tracking-tight">{title}</h1>
                <p className="mt-1 text-sm text-muted-foreground">{subtitle}</p>
            </div>
            {action}
        </header>
    );
}

export function StatusPill({label, live = true}: { label: string; live?: boolean }) {
    return (
        <div className="flex items-center gap-2 rounded-full bg-secondary px-3 py-1.5 text-xs font-medium">
      <span
          className={cn("size-1.5 rounded-full", live ? "bg-primary" : "bg-muted-foreground/50")}
      />
            {label}
        </div>
    );
}

export function Card({className, children}: { className?: string; children: ReactNode }) {
    return (
        <section className={cn("rounded-xl border border-border bg-card", className)}>
            {children}
        </section>
    );
}

export function SettingRow({
                               title,
                               description,
                               children,
                           }: {
    title: string;
    description: string;
    children: ReactNode;
}) {
    return (
        <div
            className="flex min-h-16 items-center justify-between gap-6 border-t border-border px-5 py-4 first:border-t-0">
            <div>
                <p className="text-sm font-medium">{title}</p>
                <p className="mt-1 text-xs leading-relaxed text-muted-foreground">{description}</p>
            </div>
            {children}
        </div>
    );
}

export function TextInput({
                              value,
                              onChange,
                              onBlur,
                              onFocus,
                              onKeyDown,
                              placeholder,
                              label,
                              type = "text",
                              className,
                              readOnly,
                          }: {
    value: string;
    onChange: (value: string) => void;
    onBlur?: () => void;
    onFocus?: () => void;
    onKeyDown?: KeyboardEventHandler<HTMLInputElement>;
    placeholder?: string;
    label?: string;
    type?: "text" | "number";
    className?: string;
    readOnly?: boolean;
}) {
    return (
        <input
            type={type}
            aria-label={label}
            value={value}
            placeholder={placeholder}
            readOnly={readOnly}
            onChange={(e) => onChange(e.target.value)}
            onBlur={onBlur}
            onFocus={onFocus}
            onKeyDown={onKeyDown}
            className={cn(
                "w-24 rounded-lg bg-secondary px-3 py-2 text-right text-xs font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40",
                className,
            )}
        />
    );
}

export function Select<T extends string>({
                                             value,
                                             options,
                                             onChange,
                                             label,
                                         }: {
    value: T;
    options: { value: T; label: string }[];
    onChange: (value: T) => void;
    label?: string;
}) {
    return (
        <select
            aria-label={label}
            value={value}
            onChange={(e) => onChange(e.target.value as T)}
            className="rounded-lg bg-secondary px-3 py-2 text-xs font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
        >
            {options.map((o) => (
                <option key={o.value} value={o.value}>
                    {o.label}
                </option>
            ))}
        </select>
    );
}
