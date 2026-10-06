import type { ButtonHTMLAttributes } from "react";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
    variant?: "primary" | "secondary" | "ghost";
}

const styles: Record<NonNullable<ButtonProps["variant"]>, string> = {
    primary:
        "theme-btn-primary text-[var(--theme-btn-primary-text)] hover:bg-[var(--theme-btn-primary-hover)] border border-[var(--theme-border)]",
    secondary:
        "bg-[var(--theme-bg-input)] text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)] border border-[var(--theme-border)]",
    ghost:
        "bg-transparent text-[var(--theme-text-secondary)] hover:text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)]",
};

const Button = ({ variant = "primary", className = "", ...props }: ButtonProps) => (
    <button
        {...props}
        className={`px-3 py-1.5 rounded-md text-xs font-medium transition-colors cursor-pointer focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-border-focus)] disabled:opacity-50 disabled:cursor-not-allowed ${styles[variant]} ${className}`}
    />
);

export default Button;
