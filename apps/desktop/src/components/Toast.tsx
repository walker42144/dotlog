export interface ToastAction {
  label: string;
  onClick: () => void;
}

interface ToastProps {
  message: string;
  tone: "info" | "error";
  action?: ToastAction;
}

export function Toast({ message, tone, action }: ToastProps) {
  return (
    <div className={`toast${tone === "error" ? " toast--error" : ""}`} role="status" aria-live="polite">
      <span className="toast__dot" />
      <span>{message}</span>
      {action && (
        <button className="toast__action" type="button" onClick={action.onClick}>
          {action.label}
        </button>
      )}
    </div>
  );
}
