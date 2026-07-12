// Based on pixel-agents by pablodelucca (https://github.com/pablodelucca/pixel-agents)
// Licensed under MIT
import { useEffect, useId, useRef, type ReactNode } from 'react';

import { Button } from './Button.js';

interface ModalProps {
  isOpen: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  /** z-index for backdrop (modal gets +1). Default 50 */
  zIndex?: number;
  className?: string;
}

/** Focusable-element selector used for the focus trap. */
const FOCUSABLE =
  'a[href], button:not([disabled]), textarea, input, select, [tabindex]:not([tabindex="-1"])';

export function Modal({
  isOpen,
  onClose,
  title,
  children,
  zIndex = 50,
  className = '',
}: ModalProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  // Remember what had focus before the dialog opened, so we can restore it on
  // close — keyboard/screen-reader users are not stranded on the canvas.
  const previouslyFocused = useRef<HTMLElement | null>(null);

  // Move focus into the dialog on open, restore it on close.
  useEffect(() => {
    if (!isOpen) return;
    previouslyFocused.current = document.activeElement as HTMLElement | null;
    const panel = panelRef.current;
    const first = panel?.querySelector<HTMLElement>(FOCUSABLE);
    (first ?? panel)?.focus();
    return () => {
      previouslyFocused.current?.focus?.();
    };
  }, [isOpen]);

  // Escape closes; Tab is trapped within the dialog (a11y — WCAG 2.1.1 / 2.4.3).
  useEffect(() => {
    if (!isOpen) return;
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== 'Tab') return;
      const panel = panelRef.current;
      if (!panel) return;
      const focusables = Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
        (el) => el.offsetParent !== null || el === document.activeElement,
      );
      if (focusables.length === 0) {
        e.preventDefault();
        panel.focus();
        return;
      }
      const firstEl = focusables[0];
      const lastEl = focusables[focusables.length - 1];
      const active = document.activeElement;
      if (e.shiftKey && (active === firstEl || active === panel)) {
        e.preventDefault();
        lastEl.focus();
      } else if (!e.shiftKey && active === lastEl) {
        e.preventDefault();
        firstEl.focus();
      }
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  return (
    <>
      <div className="fixed inset-0 bg-black/50" style={{ zIndex }} onClick={onClose} />
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className={`fixed top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 flex flex-col max-h-[85vh] bg-bg border-2 border-border rounded-none shadow-pixel p-4 min-w-xs focus:outline-none ${className}`}
        style={{ zIndex: zIndex + 1 }}
      >
        <div className="flex items-center justify-between py-4 px-10 border-b border-border mb-4 shrink-0">
          <span id={titleId} className="text-accent-bright text-2xl">
            {title}
          </span>
          <Button variant="ghost" size="icon" onClick={onClose} aria-label="Close">
            ×
          </Button>
        </div>
        <div className="pixel-scroll overflow-y-auto">{children}</div>
      </div>
    </>
  );
}
