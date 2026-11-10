import {
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
  useEffect,
  useId,
  useRef,
} from 'react';
import { createPortal } from 'react-dom';

export interface DialogProps {
  /** The dialog renders nothing while closed. */
  open: boolean;
  /** Called on Escape or a backdrop click. Set state to `open={false}` here. */
  onClose: () => void;
  /** Dialog title. Also its accessible name. */
  title: string;
  /** Optional supporting text, linked as the accessible description. */
  description?: string;
  /** Body content. */
  children?: ReactNode;
  /** Action buttons, right-aligned. */
  footer?: ReactNode;
  /** Element to focus on open. Defaults to the first focusable element in the dialog. */
  initialFocusRef?: RefObject<HTMLElement | null>;
}

const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

/**
 * Modal dialog. Moves focus in on open, keeps Tab inside, closes on Escape and returns focus to
 * the element that opened it. The page behind is not interactive until it closes.
 */
export function Dialog({ open, ...rest }: DialogProps) {
  if (!open) return null;
  return createPortal(<DialogPanel {...rest} />, document.body);
}

function DialogPanel({
  onClose,
  title,
  description,
  children,
  footer,
  initialFocusRef,
}: Omit<DialogProps, 'open'>) {
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const descriptionId = useId();
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  // Focus in on mount, restore on unmount (which is when `open` turns false).
  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const panel = panelRef.current;
    const target =
      initialFocusRef?.current ?? panel?.querySelector<HTMLElement>(FOCUSABLE) ?? panel;
    target?.focus();
    return () => {
      if (previous?.isConnected) previous.focus();
    };
  }, [initialFocusRef]);

  // Escape closes. Listening on the document covers focus resting on the backdrop.
  useEffect(() => {
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  // If focus ever lands outside the dialog (for example after a click on the backdrop), pull it back.
  useEffect(() => {
    const onFocusIn = (event: FocusEvent) => {
      const panel = panelRef.current;
      if (panel && event.target instanceof Node && !panel.contains(event.target)) {
        const first = panel.querySelector<HTMLElement>(FOCUSABLE);
        (first ?? panel).focus();
      }
    };
    document.addEventListener('focusin', onFocusIn);
    return () => document.removeEventListener('focusin', onFocusIn);
  }, []);

  const trapTab = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Tab') return;
    const panel = panelRef.current;
    if (!panel) return;
    const items = Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE));
    const first = items[0];
    const last = items[items.length - 1];
    if (!first || !last) {
      event.preventDefault();
      panel.focus();
      return;
    }
    const active = document.activeElement;
    if (event.shiftKey && (active === first || active === panel)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && active === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: clicking the backdrop is a pointer-only shortcut; Escape and the footer buttons are the accessible ways to close.
    <div
      role="presentation"
      className="fixed inset-0 z-40 grid animate-[volt-fade-in_var(--duration-base)_var(--ease-standard)] place-items-center bg-overlay p-4"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onCloseRef.current();
      }}
    >
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        tabIndex={-1}
        onKeyDown={trapTab}
        className="w-full max-w-sm animate-[volt-dialog-in_var(--duration-base)_var(--ease-standard)] rounded-lg border border-line bg-surface p-6 text-fg shadow-pop outline-none"
      >
        <h2 id={titleId} className="text-subtitle font-semibold">
          {title}
        </h2>
        {description ? (
          <p id={descriptionId} className="mt-2 text-body text-muted">
            {description}
          </p>
        ) : null}
        {children ? <div className="mt-4">{children}</div> : null}
        {footer ? <div className="mt-6 flex justify-end gap-2">{footer}</div> : null}
      </div>
    </div>
  );
}
