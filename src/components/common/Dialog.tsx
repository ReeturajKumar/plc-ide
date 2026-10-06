import { useEffect, type ReactNode } from "react";

interface DialogProps {
    title: string;
    onClose?: () => void;
    children: ReactNode;
    footer?: ReactNode;
}

/** Modal dialog: overlay, Escape-to-close, centered card. */
const Dialog = ({ title, onClose, children, footer }: DialogProps) => {
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            if (e.key === "Escape") onClose?.();
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [onClose]);

    return (
        <div
            className="fixed inset-0 z-50 flex items-center justify-center bg-black/60"
            onClick={onClose}
        >
            <div
                role="dialog"
                aria-modal="true"
                aria-label={title}
                className="theme-panel border border-[var(--theme-border)] rounded-lg shadow-xl w-[420px] max-w-[90vw]"
                onClick={(e) => e.stopPropagation()}
            >
                <div className="px-4 py-3 border-b border-[var(--theme-border)] text-sm font-semibold text-[var(--theme-text-primary)]">
                    {title}
                </div>
                <div className="px-4 py-4">{children}</div>
                {footer && (
                    <div className="px-4 py-3 border-t border-[var(--theme-border)] flex justify-end gap-2">
                        {footer}
                    </div>
                )}
            </div>
        </div>
    );
};

export default Dialog;
