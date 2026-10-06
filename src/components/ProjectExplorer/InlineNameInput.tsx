import { useLayoutEffect, useRef, useState } from "react";

interface InlineNameInputProps {
    initialValue: string;
    /** Error message for a value, or null if it's acceptable. */
    validate: (value: string) => string | null;
    onSubmit: (value: string) => void;
    onCancel: () => void;
    /** Reports each edit, e.g. to update the row icon as the name is typed. */
    onValueChange?: (value: string) => void;
    placeholder?: string;
}

/**
 * VS Code-style name box inside the Explorer tree. Enter confirms, Escape cancels, and
 * clicking away confirms a valid change or cancels otherwise. Problems show as you type.
 */
const InlineNameInput = ({ initialValue, validate, onSubmit, onCancel, onValueChange, placeholder }: InlineNameInputProps) => {
    const [value, setValue] = useState(initialValue);
    const input = useRef<HTMLInputElement>(null);
    // Enter and the blur that follows it must not finish the edit twice.
    const finished = useRef(false);
    const error = value.trim() ? validate(value) : null;

    useLayoutEffect(() => {
        const el = input.current;
        if (!el) return;
        el.focus();
        // Select the name but not the extension, like VS Code: "main" in "main.st".
        const dot = initialValue.lastIndexOf(".");
        el.setSelectionRange(0, dot > 0 ? dot : initialValue.length);
    }, [initialValue]);

    const finish = (commit: boolean) => {
        if (finished.current) return;
        const trimmed = value.trim();
        const nothingToDo = !commit || !trimmed || trimmed === initialValue;
        if (!nothingToDo && validate(value)) return; // Enter on an invalid name: keep editing
        finished.current = true;
        if (nothingToDo) onCancel();
        else onSubmit(value);
    };

    return (
        <div className="relative flex-1 min-w-0">
            <input
                ref={input}
                value={value}
                spellCheck={false}
                aria-label="File name"
                aria-invalid={!!error}
                placeholder={placeholder}
                onChange={(e) => {
                    setValue(e.target.value);
                    onValueChange?.(e.target.value);
                }}
                onBlur={() => (validate(value) ? finish(false) : finish(true))}
                onKeyDown={(e) => {
                    // Keys typed here belong to the box, not to the tree row (Enter opens,
                    // Delete deletes) or the global shortcuts.
                    e.stopPropagation();
                    if (e.key === "F5") e.preventDefault(); // don't reload the webview
                    if (e.key === "Enter") finish(true);
                    else if (e.key === "Escape") finish(false);
                }}
                onClick={(e) => e.stopPropagation()}
                onContextMenu={(e) => e.stopPropagation()}
                className={`w-full bg-[var(--theme-bg-input)] text-[var(--theme-text-primary)] text-[13px] px-1 py-0 border outline-none ${
                    error ? "border-red-500" : "border-[var(--theme-border-focus)]"
                }`}
            />
            {error && (
                <div
                    role="alert"
                    className="absolute left-0 right-0 top-full z-20 px-2 py-1 text-[12px] leading-snug text-[var(--theme-text-primary)] bg-[#5a1d1d] border border-red-500 border-t-0"
                >
                    {error}
                </div>
            )}
        </div>
    );
};

export default InlineNameInput;
