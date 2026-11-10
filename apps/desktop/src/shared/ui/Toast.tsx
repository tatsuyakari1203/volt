import { X } from 'lucide-react';
import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { Button } from './Button';

/** What to show in a toast. */
export interface ToastOptions {
  /** Short sentence describing what happened. */
  message: string;
  /** Label of the action button, for example "Undo". Requires `onAction`. */
  actionLabel?: string;
  /** Called when the action button is pressed. The toast is dismissed afterwards. */
  onAction?: () => void;
  /** Auto-dismiss delay in ms. Defaults to 6000, or 9000 when there is an action. */
  durationMs?: number;
}

interface ToastEntry extends ToastOptions {
  id: number;
}

interface ToastApi {
  /** Shows a toast and returns its id. */
  show: (options: ToastOptions) => number;
  /** Removes a toast early. */
  dismiss: (id: number) => void;
}

const ToastContext = createContext<ToastApi | null>(null);

/** Returns the toast API. Must be used under a `ToastProvider`. */
export function useToast(): ToastApi {
  const api = useContext(ToastContext);
  if (!api) {
    throw new Error('useToast must be used inside a ToastProvider');
  }
  return api;
}

const MAX_VISIBLE = 3;

/**
 * Provides `useToast` and renders the toast stack. Toasts announce politely to screen readers,
 * pause their timer while hovered or focused, and can carry an action such as Undo.
 */
export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastEntry[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => {
    setToasts((current) => current.filter((toast) => toast.id !== id));
  }, []);

  const show = useCallback((options: ToastOptions) => {
    const id = nextId.current++;
    setToasts((current) => [...current, { ...options, id }].slice(-MAX_VISIBLE));
    return id;
  }, []);

  const api = useMemo(() => ({ show, dismiss }), [show, dismiss]);

  return (
    <ToastContext.Provider value={api}>
      {children}
      <section
        aria-label="Notifications"
        aria-live="polite"
        // Starts after the navigation rail so a toast never covers it.
        className="pointer-events-none fixed inset-x-0 bottom-4 z-50 pl-rail wide:pl-rail-wide"
      >
        <div className="flex flex-col items-center gap-2 px-4">
          {toasts.map((toast) => (
            <ToastItem key={toast.id} toast={toast} onDismiss={dismiss} />
          ))}
        </div>
      </section>
    </ToastContext.Provider>
  );
}

function ToastItem({ toast, onDismiss }: { toast: ToastEntry; onDismiss: (id: number) => void }) {
  const [paused, setPaused] = useState(false);
  const { id, message, actionLabel, onAction, durationMs } = toast;
  const hasAction = Boolean(actionLabel && onAction);
  const delay = durationMs ?? (hasAction ? 9000 : 6000);

  useEffect(() => {
    if (paused) return;
    const timer = window.setTimeout(() => onDismiss(id), delay);
    return () => window.clearTimeout(timer);
  }, [paused, delay, id, onDismiss]);

  return (
    <div
      role="status"
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={() => setPaused(false)}
      className="pointer-events-auto flex w-full max-w-sm animate-[volt-toast-in_var(--duration-base)_var(--ease-standard)] items-center gap-2 rounded-md bg-toast py-2 pr-2 pl-4 text-toast-fg shadow-pop"
    >
      <p className="min-w-0 flex-1 py-1 text-body">{message}</p>
      {hasAction ? (
        <Button
          variant="ghost"
          size="sm"
          className="text-toast-action! hover:bg-toast-fg/10! active:bg-toast-fg/15!"
          onClick={() => {
            onAction?.();
            onDismiss(id);
          }}
        >
          {actionLabel}
        </Button>
      ) : null}
      <Button
        variant="ghost"
        size="sm"
        aria-label="Dismiss notification"
        className="w-8 px-0! text-toast-fg! hover:bg-toast-fg/10! active:bg-toast-fg/15!"
        onClick={() => onDismiss(id)}
      >
        <X aria-hidden size={16} />
      </Button>
    </div>
  );
}
