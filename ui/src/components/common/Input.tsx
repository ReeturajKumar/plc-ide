import type { InputHTMLAttributes } from "react";

interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
    label?: string;
    error?: string | null;
}

const Input = ({ label, error, id, className = "", ...props }: InputProps) => (
    <div className="flex flex-col gap-1">
        {label && (
            <label htmlFor={id} className="text-[11px] font-medium text-[var(--theme-text-secondary)]">
                {label}
            </label>
        )}
        <input
            id={id}
            aria-invalid={!!error}
            {...props}
            className={`bg-[var(--theme-bg-input)] border rounded-md px-3 py-1.5 text-xs text-[var(--theme-text-primary)] placeholder:text-[var(--theme-text-muted)] focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-border-focus)] ${
                error ? "border-red-500" : "border-[var(--theme-border)]"
            } ${className}`}
        />
        {error && <span className="text-[11px] text-red-400">{error}</span>}
    </div>
);

export default Input;
