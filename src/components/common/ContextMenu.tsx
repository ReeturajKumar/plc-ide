import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { MenuItem } from "./Menu";

interface ContextMenuProps {
    /** Pointer position (viewport coordinates) where the menu opens. */
    x: number;
    y: number;
    items: MenuItem[];
    onClose: () => void;
}

/** Right-click menu: opens at the pointer, stays on screen, keyboard navigable. */
const ContextMenu = ({ x, y, items, onClose }: ContextMenuProps) => {
    const ref = useRef<HTMLDivElement>(null);
    const [position, setPosition] = useState({ left: x, top: y });

    // Flip/clamp so the menu never runs off the window edge.
    useLayoutEffect(() => {
        const menu = ref.current;
        if (!menu) return;
        const { width, height } = menu.getBoundingClientRect();
        const margin = 4;
        setPosition({
            left: Math.max(margin, Math.min(x, window.innerWidth - width - margin)),
            top: y + height > window.innerHeight - margin ? Math.max(margin, y - height) : y,
        });
        buttons(menu)[0]?.focus();
    }, [x, y]);

    useEffect(() => {
        const onPointerDown = (e: MouseEvent) => {
            if (!ref.current?.contains(e.target as Node)) onClose();
        };
        window.addEventListener("mousedown", onPointerDown);
        window.addEventListener("blur", onClose);
        window.addEventListener("resize", onClose);
        window.addEventListener("scroll", onClose, true);
        return () => {
            window.removeEventListener("mousedown", onPointerDown);
            window.removeEventListener("blur", onClose);
            window.removeEventListener("resize", onClose);
            window.removeEventListener("scroll", onClose, true);
        };
    }, [onClose]);

    const onKeyDown = (e: React.KeyboardEvent) => {
        const list = ref.current ? buttons(ref.current) : [];
        const current = list.indexOf(document.activeElement as HTMLButtonElement);
        if (e.key === "Escape" || e.key === "Tab") {
            e.preventDefault();
            onClose();
        } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            const step = e.key === "ArrowDown" ? 1 : -1;
            list[(current + step + list.length) % list.length]?.focus();
        }
    };

    return (
        <div
            ref={ref}
            role="menu"
            onKeyDown={onKeyDown}
            onContextMenu={(e) => e.preventDefault()}
            style={position}
            className="fixed z-50 min-w-[230px] theme-panel border border-[var(--theme-border)] rounded-md shadow-xl py-1 text-[12px]"
        >
            {items.map((item, i) =>
                item.separator ? (
                    <div key={i} role="separator" className="my-1 border-t border-[var(--theme-border)]" />
                ) : (
                    <button
                        key={i}
                        role="menuitem"
                        disabled={item.disabled}
                        onClick={() => {
                            onClose();
                            item.onClick?.();
                        }}
                        className="w-full flex items-center justify-between gap-6 px-3 py-1.5 text-left text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)] hover:text-[var(--theme-text-primary)] focus:bg-[var(--theme-bg-hover)] focus:text-[var(--theme-text-primary)] focus:outline-none disabled:opacity-40 disabled:cursor-not-allowed cursor-pointer"
                    >
                        <span>{item.label}</span>
                        {item.shortcut && <span className="text-[var(--theme-text-muted)]">{item.shortcut}</span>}
                    </button>
                )
            )}
        </div>
    );
};

function buttons(menu: HTMLElement): HTMLButtonElement[] {
    return Array.from(menu.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"));
}

export default ContextMenu;
