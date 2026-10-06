import { useEffect, useRef, useState } from "react";

export interface MenuItem {
    label: string;
    shortcut?: string;
    onClick?: () => void;
    disabled?: boolean;
    separator?: boolean;
}

/** A single top-bar menu (File, Edit, …) that opens a dropdown of items. */
const Menu = ({ label, items }: { label: string; items: MenuItem[] }) => {
    const [open, setOpen] = useState(false);
    const ref = useRef<HTMLDivElement>(null);

    useEffect(() => {
        if (!open) return;
        const onDown = (e: MouseEvent) => {
            if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
        };
        window.addEventListener("mousedown", onDown);
        return () => window.removeEventListener("mousedown", onDown);
    }, [open]);

    return (
        <div ref={ref} className="relative">
            <button
                onClick={() => setOpen((o) => !o)}
                className={`px-2 py-0.5 rounded transition-colors cursor-pointer hover:bg-[var(--theme-bg-hover)] ${
                    open ? "bg-[var(--theme-bg-hover)] text-[var(--theme-text-primary)]" : "hover:text-[var(--theme-text-primary)]"
                }`}
            >
                {label}
            </button>

            {open && (
                <div className="absolute left-0 top-full mt-1 z-50 min-w-[200px] theme-panel border border-[var(--theme-border)] rounded-md shadow-xl py-1">
                    {items.map((item, i) =>
                        item.separator ? (
                            <div key={i} className="my-1 border-t border-[var(--theme-border)]" />
                        ) : (
                            <button
                                key={i}
                                disabled={item.disabled}
                                onClick={() => {
                                    setOpen(false);
                                    item.onClick?.();
                                }}
                                className="w-full flex items-center justify-between px-3 py-1.5 text-left text-[12px] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer"
                            >
                                <span>{item.label}</span>
                                {item.shortcut && (
                                    <span className="text-[var(--theme-text-muted)] ml-6">{item.shortcut}</span>
                                )}
                            </button>
                        )
                    )}
                </div>
            )}
        </div>
    );
};

export default Menu;
