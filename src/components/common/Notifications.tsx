import { useEffect } from "react";
import { useUIStore } from "../../store/uiStore";

const colors = {
    error: "border-red-500/50 text-red-300",
    info: "border-[var(--theme-border)] text-[var(--theme-text-secondary)]",
    success: "border-emerald-500/50 text-emerald-300",
} as const;

/** Toast stack in the bottom-right; auto-dismisses each after a few seconds. */
const Notifications = () => {
    const notifications = useUIStore((s) => s.notifications);
    const dismiss = useUIStore((s) => s.dismiss);

    useEffect(() => {
        if (!notifications.length) return;
        const timers = notifications.map((n) => setTimeout(() => dismiss(n.id), 4000));
        return () => timers.forEach(clearTimeout);
    }, [notifications, dismiss]);

    if (!notifications.length) return null;

    return (
        <div className="fixed bottom-8 right-4 z-50 flex flex-col gap-2 w-72">
            {notifications.map((n) => (
                <div
                    key={n.id}
                    role="status"
                    onClick={() => dismiss(n.id)}
                    className={`theme-panel border rounded-md px-3 py-2 text-xs shadow-lg cursor-pointer ${colors[n.kind]}`}
                >
                    {n.message}
                </div>
            ))}
        </div>
    );
};

export default Notifications;
