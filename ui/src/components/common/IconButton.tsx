import { useState, type ReactNode } from "react";
import ContextMenu from "./ContextMenu";
import type { MenuItem } from "./Menu";

interface IconButtonProps {
    title: string;
    onClick: () => void;
    disabled?: boolean;
    /** Highlighted, e.g. a layout toggle whose area is visible. */
    active?: boolean;
    children: ReactNode;
}

/** A small VS Code-style toolbar icon button. `children` are the SVG's shapes. */
const IconButton = ({ title, onClick, disabled, active, children }: IconButtonProps) => (
    <button
        onClick={onClick}
        disabled={disabled}
        title={title}
        aria-label={title}
        aria-pressed={active}
        className={`w-6 h-6 shrink-0 flex items-center justify-center rounded cursor-pointer transition-colors focus:outline-none focus-visible:ring-1 focus-visible:ring-[var(--theme-border-focus)] disabled:opacity-35 disabled:cursor-not-allowed ${
            active ? "text-[var(--theme-text-primary)]" : "text-[var(--theme-text-muted)]"
        } hover:text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)] disabled:hover:bg-transparent`}
    >
        <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            {children}
        </svg>
    </button>
);

/** "…" button that opens a menu of further actions below it. */
export const MoreButton = ({ title, items }: { title: string; items: () => MenuItem[] }) => {
    const [at, setAt] = useState<{ x: number; y: number } | null>(null);
    return (
        <>
            <button
                onClick={(e) => {
                    const rect = e.currentTarget.getBoundingClientRect();
                    setAt(at ? null : { x: rect.right - 200, y: rect.bottom + 2 });
                }}
                title={title}
                aria-label={title}
                aria-haspopup="menu"
                aria-expanded={!!at}
                className="w-6 h-6 shrink-0 flex items-center justify-center rounded cursor-pointer text-[var(--theme-text-muted)] hover:text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)] focus:outline-none focus-visible:ring-1 focus-visible:ring-[var(--theme-border-focus)]"
            >
                <svg className="w-4 h-4" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
                    <circle cx="5" cy="12" r="1.6" />
                    <circle cx="12" cy="12" r="1.6" />
                    <circle cx="19" cy="12" r="1.6" />
                </svg>
            </button>
            {at && <ContextMenu x={at.x} y={at.y} items={items()} onClose={() => setAt(null)} />}
        </>
    );
};

export default IconButton;
