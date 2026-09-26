import type { ReactNode } from "react";

/**
 * Label + hint + error wrapper for a single form control. The control itself is passed as a
 * child so the same field can hold an `input`, a `select` or a `textarea` (styled by the
 * `.input` / `.select` / `.textarea` classes from `styles.css`).
 */

export interface FormFieldProps {
  label: string;
  /** `id` of the wrapped control, so clicking the label focuses it. */
  htmlFor?: string | undefined;
  hint?: string | undefined;
  error?: string | null | undefined;
  required?: boolean | undefined;
  /** Extra content rendered under the hint (counters, previews). */
  footer?: ReactNode | undefined;
  children: ReactNode;
}

export function FormField({
  label,
  htmlFor,
  hint,
  error,
  required = false,
  footer,
  children,
}: FormFieldProps) {
  const hasError = error !== undefined && error !== null && error.length > 0;

  return (
    <div className="w-full">
      <label className="field-label" htmlFor={htmlFor}>
        {label}
        {required ? <span className="ml-1 text-danger">*</span> : null}
      </label>

      {children}

      {hasError ? (
        <p className="field-error" role="alert">
          {error}
        </p>
      ) : hint !== undefined ? (
        <p className="field-hint">{hint}</p>
      ) : null}

      {footer !== undefined ? <div className="mt-1">{footer}</div> : null}
    </div>
  );
}
